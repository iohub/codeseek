//! Hybrid search service combining Dense (Vector) and Sparse (BM25) retrieval channels.
//!
//! Uses Reciprocal Rank Fusion (RRF) to merge results from both channels,
//! providing better recall and precision than either channel alone.

use crate::services::embedding_service::{EmbeddingService, SearchResult};
use crate::services::reranker_service::RerankerService;
use crate::storage::traits_bm25::{CandidateSource, CodeChunk, FusedCandidate, TextSearchProvider, TextSearchResult};
use anyhow::{Result, anyhow};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tracing::{debug, warn};

/// Configuration for hybrid search behavior.
#[derive(Debug, Clone)]
pub struct HybridSearchConfig {
    /// RRF fusion constant. Higher = ranks matter less.
    /// Industry standard is 60. See: https://plg.uwaterloo.ca/~gvcormac/cormacksigir09-rrf.pdf
    pub rrf_k: f64,
    /// Number of dense (vector) results to fetch per fusion query.
    /// Should be >= top_k to ensure enough candidates for fusion.
    pub dense_limit: usize,
    /// Number of sparse (BM25) results to fetch per fusion query.
    /// Should be >= top_k to ensure enough candidates for fusion.
    pub sparse_limit: usize,
    /// Whether to enable the sparse (BM25) channel.
    /// When false, falls back to dense-only search.
    pub enable_sparse: bool,
    /// Timeout for the entire hybrid search operation (milliseconds).
    /// 0 means no timeout.
    pub timeout_ms: u64,
    /// Threshold (in chars) for short code penalty. 
    /// Code blocks shorter than this get their score reduced.
    pub short_code_threshold: usize,
    /// Penalty strength for short code blocks (0.0 = no penalty, 1.0 = full penalty).
    /// Applied as: penalty = 1.0 - max(0, 1 - len/threshold) * strength
    pub short_code_penalty: f64,
}

impl Default for HybridSearchConfig {
    fn default() -> Self {
        Self {
            rrf_k: 60.0,
            dense_limit: 100,
            sparse_limit: 100,
            enable_sparse: true,
            timeout_ms: 0,
            short_code_threshold: 30,
            short_code_penalty: 0.5,
        }
    }
}

/// The hybrid search service that orchestrates dual-channel retrieval.
///
/// Architecture:
/// ```text
///    Query
///      │
///      ├──→ EmbeddingService.search() ──→ DenseResults
///      │
///      └──→ TextSearchProvider.search() ──→ SparseResults (BM25)
///      │
///      └──→ RRF Fusion ──→ FusedCandidates (sorted by fused score)
///      │
///      └──→ (Optional) Cross-Encoder Reranker ──→ Final Results
/// ```
pub struct HybridSearchService {
    dense: Arc<EmbeddingService>,
    sparse: Arc<dyn TextSearchProvider>,
    config: HybridSearchConfig,
    /// 可选的 Cross-Encoder Reranker 服务（RRF 融合后执行精排）
    reranker: Option<RerankerService>,
}

/// Internal metadata structure used to track line info during RRF fusion.
///
/// Filled from either channel: the dense channel carries the real `code_block` /
/// line range / language, the sparse (BM25) channel only carries path, symbol,
/// language and (when available) `line_start`. Missing pieces are merged from the
/// other channel via [`FusionMeta::merge_missing`].
#[derive(Clone, Default)]
struct FusionMeta {
    file_path: String,
    symbol_name: String,
    language: String,
    code_block: String,
    line_start: usize,
    line_end: usize,
}

impl From<&SearchResult> for FusionMeta {
    fn from(sr: &SearchResult) -> Self {
        Self {
            file_path: sr.file_path.clone(),
            symbol_name: sr.symbol_name.clone(),
            language: sr.language.clone(),
            code_block: sr.code_block.clone(),
            line_start: sr.line_start,
            line_end: sr.line_end,
        }
    }
}

impl From<&TextSearchResult> for FusionMeta {
    fn from(sr: &TextSearchResult) -> Self {
        Self {
            file_path: sr.file_path.clone(),
            symbol_name: sr.symbol_name.clone(),
            language: sr.language.clone(),
            // BM25 通道不存储正文（content 非 STORED），保持为空串
            code_block: String::new(),
            line_start: sr.line_start,
            // BM25 通道不存储结束行
            line_end: 0,
        }
    }
}

impl FusionMeta {
    /// Fill only the fields this side is missing from `other`.
    ///
    /// "Missing" means empty string for text fields and `0` for line numbers,
    /// so real data already present is never overwritten by a placeholder.
    fn merge_missing(&mut self, other: &Self) {
        if self.file_path.is_empty() {
            self.file_path = other.file_path.clone();
        }
        if self.symbol_name.is_empty() {
            self.symbol_name = other.symbol_name.clone();
        }
        if self.language.is_empty() {
            self.language = other.language.clone();
        }
        if self.code_block.is_empty() {
            self.code_block = other.code_block.clone();
        }
        if self.line_start == 0 {
            self.line_start = other.line_start;
        }
        if self.line_end == 0 {
            self.line_end = other.line_end;
        }
    }
}

/// Build the snippet id of a dense result using the very same scheme as the
/// Tantivy index (`CodeChunk::generate_id`), so both ranked lists can be merged
/// by id. Format: `{file_path}#{line_start}#{symbol_name}`.
fn dense_snippet_id(sr: &SearchResult) -> String {
    CodeChunk::generate_id(&sr.file_path, sr.line_start, &sr.symbol_name)
}

impl HybridSearchService {
    /// Create a new hybrid search service without reranker.
    pub fn new(
        dense: Arc<EmbeddingService>,
        sparse: Arc<dyn TextSearchProvider>,
        config: HybridSearchConfig,
    ) -> Self {
        Self { dense, sparse, config, reranker: None }
    }

    /// Create a new hybrid search service with optional reranker.
    pub fn with_reranker(
        dense: Arc<EmbeddingService>,
        sparse: Arc<dyn TextSearchProvider>,
        config: HybridSearchConfig,
        reranker: Option<RerankerService>,
    ) -> Self {
        Self { dense, sparse, config, reranker }
    }

    /// Execute hybrid search: query both channels, fuse results via RRF.
    ///
    /// # Arguments
    /// * `query` - The search query string
    /// * `limit` - Number of final results to return
    ///
    /// # Returns
    /// A vector of `FusedCandidate` sorted by fused RRF score (descending).
    /// If sparse channel is disabled or unavailable, falls back to dense-only.
    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<FusedCandidate>> {
        if query.is_empty() {
            return Ok(Vec::new());
        }

        let dense_limit = self.config.dense_limit.max(limit);
        let sparse_limit = if self.config.enable_sparse {
            self.config.sparse_limit.max(limit)
        } else {
            0
        };

        // Dispatch both channels concurrently: `tokio::join!` polls both futures in
        // the same task, so the sparse (BM25) query no longer waits for the dense
        // (vector) query to finish before it even starts.
        let (dense_res, sparse_res_opt) = if self.config.enable_sparse {
            let (dense_res, sparse_res) = tokio::join!(
                self.dense.search(query, dense_limit),
                self.sparse.search(query, sparse_limit)
            );
            (dense_res, Some(sparse_res))
        } else {
            debug!("Sparse channel disabled in config");
            (self.dense.search(query, dense_limit).await, None)
        };

        // Dense results are required — if they fail, the whole search fails
        let dense_raw = dense_res.map_err(|e| {
            anyhow!("Dense (vector) search failed: {}", e)
        })?;

        // Handle sparse results with graceful degradation
        let sparse_results: Option<Vec<TextSearchResult>> = match sparse_res_opt {
            None => None,
            Some(Ok(results)) => {
                debug!("Sparse channel returned {} results", results.len());
                Some(results)
            }
            Some(Err(e)) => {
                warn!(
                    "Sparse (BM25) channel failed, falling back to dense-only: {}",
                    e
                );
                // Return dense results as dense-only candidates
                return Ok(
                    dense_raw
                        .into_iter()
                        .map(|sr| FusedCandidate {
                            snippet_id: dense_snippet_id(&sr),
                            final_score: sr.score as f64,
                            file_path: sr.file_path,
                            symbol_name: sr.symbol_name,
                            language: sr.language,
                            line_start: sr.line_start,
                            line_end: sr.line_end,
                            code_block: sr.code_block,
                            source: CandidateSource::DenseOnly,
                        })
                        .take(limit)
                        .collect(),
                );
            }
        };

        // Perform RRF fusion — 返回足够的候选供后续 rerank 使用
        let rrf_limit = self.config.dense_limit.max(self.config.sparse_limit);
        let fused = self.reciprocal_rank_fusion(dense_raw, sparse_results.unwrap_or_default(), rrf_limit);
        debug!("Hybrid search returned {} fused candidates (before rerank)", fused.len());

        // Phase 2: Optional Cross-Encoder Rerank
        if let Some(ref reranker) = self.reranker {
            if reranker.config().enabled {
                return self.apply_rerank(reranker, query, fused, limit).await;
            }
        }

        // 无 rerank 时截断到 limit
        let mut result = fused;
        result.truncate(limit);
        Ok(result)
    }

    /// Reciprocal Rank Fusion (RRF) for combining ranked lists.
    ///
    /// Formula: score(d) = Σ 1 / (k + rank(d))
    /// where k is the RRF constant (default 60), rank(d) is the 0-based position of document d
    /// in a ranked list, and the sum is over all channels that returned d.
    ///
    /// Documents appearing in both channels get a higher fused score.
    ///
    /// 两个通道的 id 统一由 [`CodeChunk::generate_id`] 生成（三段式
    /// `{file_path}#{line_start}#{symbol_name}`），因此「只被 BM25 命中」的片段
    /// 也会作为 `CandidateSource::SparseOnly` 候选保留，不再被静默丢弃。
    fn reciprocal_rank_fusion(
        &self,
        dense: Vec<SearchResult>,
        sparse: Vec<TextSearchResult>,
        limit: usize,
    ) -> Vec<FusedCandidate> {
        Self::fuse(dense, sparse, &self.config, limit)
    }

    /// Pure RRF fusion core.
    ///
    /// 只依赖入参与 `config`（不触碰 `self.dense` / `self.sparse`），因此无需构造
    /// EmbeddingService 或 Tantivy 索引即可单测。
    fn fuse(
        dense: Vec<SearchResult>,
        sparse: Vec<TextSearchResult>,
        config: &HybridSearchConfig,
        limit: usize,
    ) -> Vec<FusedCandidate> {
        let k = config.rrf_k;

        // Accumulate scores per snippet_id, along with metadata from whichever
        // channel(s) produced it. ids 记录在 set 里，供后面判定候选来源。
        let mut score_map: HashMap<String, f64> = HashMap::new();
        let mut meta_map: HashMap<String, FusionMeta> = HashMap::new();
        let mut dense_ids: HashSet<String> = HashSet::new();
        let mut sparse_ids: HashSet<String> = HashSet::new();

        // Process dense channel
        for (rank, dr) in dense.iter().enumerate() {
            let snippet_id = dense_snippet_id(dr);
            let rrf_score = 1.0 / (k + rank as f64 + 1.0);
            *score_map.entry(snippet_id.clone()).or_insert(0.0) += rrf_score;

            let dense_meta = FusionMeta::from(dr);
            let meta = meta_map
                .entry(snippet_id.clone())
                .or_insert_with(|| dense_meta.clone());
            meta.merge_missing(&dense_meta);

            dense_ids.insert(snippet_id);
        }

        // Process sparse channel — 纯 BM25 命中本身就是合法候选
        for (rank, sr) in sparse.iter().enumerate() {
            let rrf_score = 1.0 / (k + rank as f64 + 1.0);
            *score_map.entry(sr.snippet_id.clone()).or_insert(0.0) += rrf_score;

            let sparse_meta = FusionMeta::from(sr);
            let meta = meta_map
                .entry(sr.snippet_id.clone())
                .or_insert_with(|| sparse_meta.clone());
            meta.merge_missing(&sparse_meta);

            sparse_ids.insert(sr.snippet_id.clone());
        }

        // Sort by fused score descending
        let mut ranked: Vec<(String, f64)> = score_map.into_iter().collect();
        ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // Take top-K and build FusedCandidate structs
        ranked
            .into_iter()
            .take(limit)
            .filter_map(|(snippet_id, fused_score)| {
                // Determine source from the ids recorded while walking each channel
                let in_dense = dense_ids.contains(&snippet_id);
                let in_sparse = sparse_ids.contains(&snippet_id);
                let source = match (in_dense, in_sparse) {
                    (true, true) => CandidateSource::Fused,
                    (true, false) => CandidateSource::DenseOnly,
                    (false, true) => CandidateSource::SparseOnly,
                    (false, false) => return None,
                };

                // Both channels insert metadata, so every candidate has one here
                let meta = meta_map.get(&snippet_id)?;

                // P1: Apply short code penalty — 仅在真的有正文时计算，
                // 否则纯 BM25 候选（content 未存储）会被当成 0 长度代码而腰斩分数
                let final_score = if meta.code_block.trim().is_empty() {
                    fused_score
                } else {
                    let code_len = meta.code_block.trim().chars().count();
                    if code_len < config.short_code_threshold {
                        let ratio = code_len as f64 / config.short_code_threshold as f64;
                        let penalty_factor = 1.0 - (1.0 - ratio) * config.short_code_penalty;
                        let penalty_factor = penalty_factor.max(0.0); // clamp to [0, 1]
                        fused_score * penalty_factor
                    } else {
                        fused_score
                    }
                };
                
                Some(FusedCandidate {
                    snippet_id,
                    final_score,
                    file_path: meta.file_path.clone(),
                    symbol_name: meta.symbol_name.clone(),
                    language: meta.language.clone(),
                    line_start: meta.line_start,
                    line_end: meta.line_end,
                    code_block: meta.code_block.clone(),
                    source,
                })
            })
            .collect()
    }

    /// Get the current config.
    pub fn config(&self) -> &HybridSearchConfig {
        &self.config
    }

    /// 对 RRF 融合结果执行 Cross-Encoder 重排
    async fn apply_rerank(
        &self,
        reranker: &RerankerService,
        query: &str,
        fused: Vec<FusedCandidate>,
        limit: usize,
    ) -> Result<Vec<FusedCandidate>> {
        // 取 RRF 候选池（limit * candidate_multiplier），但不超过 fused 总长度
        let pool_size = (limit * reranker.config().candidate_multiplier).min(fused.len());
        let (candidate_pool, remaining) = fused.split_at(pool_size);

        debug!("Reranker: sending {} candidates (limit={}, multiplier={})", 
            pool_size, limit, reranker.config().candidate_multiplier);

        match reranker.rerank(query, candidate_pool.to_vec()).await {
            Ok(mut reranked) => {
                // 重排结果只取 top limit
                reranked.truncate(limit);
                
                // 如果重排结果不足 limit，用剩余的 RRF 候选填充
                if reranked.len() < limit {
                    let need = limit - reranked.len();
                    reranked.extend(
                        remaining.iter()
                            .take(need)
                            .cloned()
                    );
                }
                
                debug!("Reranker: final {} results after rerank", reranked.len());
                Ok(reranked)
            }
            Err(e) => {
                // 降级：返回 RRF 结果
                warn!("Reranker failed, falling back to RRF results: {}", e);
                let mut fallback = fused;
                fallback.truncate(limit);
                Ok(fallback)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 60+ chars — 超过 default 的 short_code_threshold(30)，避免惩罚干扰断言
    const CODE_BLOCK: &str = "fn foo() {\n    let mut v = Vec::new();\n    v.push(1);\n    v\n}\n";

    fn make_dense(file_path: &str, symbol: &str, line_start: usize, line_end: usize) -> SearchResult {
        SearchResult {
            file_path: file_path.to_string(),
            symbol_name: symbol.to_string(),
            code_block: CODE_BLOCK.to_string(),
            score: 0.9,
            symbol_type: "function".to_string(),
            language: "rust".to_string(),
            line_start,
            line_end,
        }
    }

    fn make_sparse(snippet_id: &str, file_path: &str, symbol: &str, line_start: usize) -> TextSearchResult {
        TextSearchResult {
            snippet_id: snippet_id.to_string(),
            score: 3.5,
            file_path: file_path.to_string(),
            symbol_name: symbol.to_string(),
            language: "rust".to_string(),
            line_start,
        }
    }

    #[test]
    fn sparse_only_candidate_survives_fusion() {
        let id = CodeChunk::generate_id("/tmp/a.rs", 10, "foo");
        let sparse = vec![make_sparse(&id, "/tmp/a.rs", "foo", 0)];
        let config = HybridSearchConfig::default();

        let fused = HybridSearchService::fuse(Vec::new(), sparse, &config, 10);

        assert_eq!(fused.len(), 1, "纯 BM25 命中不应被丢弃");
        let c = &fused[0];
        assert_eq!(c.snippet_id, id);
        assert_eq!(c.source, CandidateSource::SparseOnly);
        assert_eq!(c.file_path, "/tmp/a.rs");
        assert_eq!(c.symbol_name, "foo");
        assert_eq!(c.language, "rust");
        assert_eq!(c.line_start, 0);
        assert_eq!(c.line_end, 0);
        assert!(c.code_block.is_empty());
        // 没有正文 ⇒ 不施加短代码惩罚，分数就是未惩罚的 1/(k+rank+1)
        let expected = 1.0 / (config.rrf_k + 0.0 + 1.0);
        assert!((c.final_score - expected).abs() < 1e-9);
    }

    #[test]
    fn fused_ids_align_and_scores_sum() {
        let dense = vec![make_dense("/tmp/a.rs", "foo", 10, 20)];
        let id = CodeChunk::generate_id("/tmp/a.rs", 10, "foo");
        let sparse = vec![make_sparse(&id, "/tmp/a.rs", "foo", 10)];
        let config = HybridSearchConfig::default();

        let fused = HybridSearchService::fuse(dense, sparse, &config, 10);

        assert_eq!(fused.len(), 1, "两个通道的 id 必须对齐为同一条候选");
        let c = &fused[0];
        assert_eq!(c.snippet_id, id);
        assert_eq!(c.source, CandidateSource::Fused);
        let expected = 1.0 / (config.rrf_k + 1.0) + 1.0 / (config.rrf_k + 1.0);
        assert!((c.final_score - expected).abs() < 1e-9);
        assert_eq!(c.line_start, 10);
        assert_eq!(c.line_end, 20);
        assert_eq!(c.language, "rust");
        assert!(!c.code_block.is_empty());
    }

    #[test]
    fn dense_only_candidate_keeps_real_line_info() {
        let dense = vec![make_dense("/tmp/a.rs", "foo", 5, 9)];
        let config = HybridSearchConfig::default();

        let fused = HybridSearchService::fuse(dense, Vec::new(), &config, 10);

        assert_eq!(fused.len(), 1);
        let c = &fused[0];
        assert_eq!(c.source, CandidateSource::DenseOnly);
        assert_eq!(c.snippet_id, CodeChunk::generate_id("/tmp/a.rs", 5, "foo"));
        assert_eq!(c.line_start, 5);
        assert_eq!(c.line_end, 9);
        assert_eq!(c.language, "rust");
    }
}

//! Hybrid search for knowledge entries: vector (LanceDB) + BM25 (Tantivy) + RRF fusion.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::knowledge::record::KnowledgeRecord;

/// A single search result combining scores from multiple retrieval channels.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeSearchResult {
    #[serde(flatten)]
    pub record: KnowledgeRecord,
    /// Vector search score: 1 / (1 + distance)
    pub vector_score: Option<f32>,
    /// BM25 score from Tantivy
    pub bm25_score: Option<f32>,
    /// Final fused score (higher = more relevant)
    pub final_score: f32,
    /// Reranker score (if reranking was applied)
    pub rerank_score: Option<f32>,
}

/// Reciprocal Rank Fusion (RRF) constant.
const RRF_K: f64 = 60.0;

/// RRF fusion of two ranked lists.
///
/// Both `vector_results` and `bm25_results` are expected to be sorted
/// by relevance (best first). Each entry is identified by its record ID.
pub fn rrf_fuse(
    vector_results: Vec<(String, f32)>,
    bm25_results: Vec<(String, f32)>,
    limit: usize,
) -> Vec<(String, f64)> {
    let mut score_map: HashMap<String, f64> = HashMap::new();

    for (rank, (id, _)) in vector_results.iter().enumerate() {
        let rrf = 1.0 / (RRF_K + rank as f64 + 1.0);
        *score_map.entry(id.clone()).or_insert(0.0) += rrf;
    }
    for (rank, (id, _)) in bm25_results.iter().enumerate() {
        let rrf = 1.0 / (RRF_K + rank as f64 + 1.0);
        *score_map.entry(id.clone()).or_insert(0.0) += rrf;
    }

    let mut ranked: Vec<(String, f64)> = score_map.into_iter().collect();
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    ranked.into_iter().take(limit).collect()
}

/// Build a KnowledgeSearchResult from a record and scores.
pub fn build_result(
    record: KnowledgeRecord,
    vector_score: Option<f32>,
    bm25_score: Option<f32>,
    final_score: f32,
    rerank_score: Option<f32>,
) -> KnowledgeSearchResult {
    KnowledgeSearchResult {
        record,
        vector_score,
        bm25_score,
        final_score,
        rerank_score,
    }
}

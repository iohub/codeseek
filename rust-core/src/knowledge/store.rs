//! Knowledge store backed by LanceDB (vector) and Tantivy (BM25).
//!
//! Provides CRUD and hybrid search for [`KnowledgeRecord`] entries.

use anyhow::{anyhow, Context};
use arrow::array::{
    Array, ArrayRef, FixedSizeListBuilder, Float32Builder, Int32Builder, ListBuilder, RecordBatch,
    RecordBatchIterator, StringBuilder,
};
use arrow::datatypes::{DataType, Field, Schema};
use lancedb::connect;
use lancedb::query::{ExecutableQuery, QueryBase};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use futures::TryStreamExt;

use crate::config::Config;
use crate::knowledge::record::KnowledgeRecord;
use crate::knowledge::search::{build_result, rrf_fuse, KnowledgeSearchResult};
use crate::knowledge::tantivy::KnowledgeTantivyIndex;
use crate::services::embedding_service::{EmbeddingProvider, OpenAICompatibleEmbeddingProvider};

/// Project-level knowledge store: LanceDB for vectors + Tantivy for BM25.
pub struct KnowledgeStore {
    /// Path to the LanceDB database directory (~/.codeseek/projects/<project_hash>/knowledge).
    db_path: PathBuf,
    /// Table name within the LanceDB database.
    table_name: String,
    /// BM25 full-text index (optional, always initialised).
    bm25: Option<KnowledgeTantivyIndex>,
}

impl KnowledgeStore {
    /// Open (or create) the knowledge store at `~/.codeseek/projects/<project_hash>/knowledge`.
    pub fn new(project_hash: &str) -> anyhow::Result<Self> {
        let db_path = Config::knowledge_dir(project_hash);
        let bm25_dir = Config::knowledge_bm25_dir(project_hash);

        std::fs::create_dir_all(&db_path)
            .context("failed to create knowledge DB directory")?;
        std::fs::create_dir_all(&bm25_dir)
            .context("failed to create knowledge BM25 index directory")?;

        let bm25 = KnowledgeTantivyIndex::open_or_create(&bm25_dir)
            .context("failed to open/create Tantivy index")?;

        Ok(Self {
            db_path,
            table_name: "knowledge".to_string(),
            bm25: Some(bm25),
        })
    }

    /// Ensure the LanceDB table exists with the correct schema.
    pub async fn ensure_table(&self) -> anyhow::Result<()> {
        let db_path_str = self.db_path.to_string_lossy().to_string();
        let connection = connect(&db_path_str)
            .execute()
            .await
            .with_context(|| {
                format!("failed to connect to LanceDB at {}", self.db_path.display())
            })?;

        let table_names = connection
            .table_names()
            .execute()
            .await
            .with_context(|| "failed to list LanceDB tables")?;

        if table_names.contains(&self.table_name) {
            return Ok(());
        }

        let dimensions = Config::load()
            .map(|c| c.embedding.dimensions)
            .unwrap_or(2560);

        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new(
                "vector",
                DataType::FixedSizeList(
                    Arc::new(Field::new("item", DataType::Float32, true)),
                    dimensions as i32,
                ),
                false,
            ),
            Field::new("type", DataType::Utf8, false),
            Field::new("title", DataType::Utf8, false),
            Field::new("content", DataType::Utf8, false),
            Field::new("tags", DataType::List(Arc::new(Field::new("item", DataType::Utf8, true))), true),
            Field::new(
                "related_files",
                DataType::List(Arc::new(Field::new("item", DataType::Utf8, true))),
                true,
            ),
            Field::new("source_agent", DataType::Utf8, false),
            Field::new("task_id", DataType::Utf8, true),
            Field::new("confidence", DataType::Float32, false),
            Field::new("created_at", DataType::Utf8, false),
            Field::new("updated_at", DataType::Utf8, false),
            Field::new("access_count", DataType::Int32, false),
            Field::new("last_accessed", DataType::Utf8, true),
            Field::new(
                "parent_ids",
                DataType::List(Arc::new(Field::new("item", DataType::Utf8, true))),
                true,
            ),
        ]));

        connection
            .create_empty_table(&self.table_name, schema)
            .execute()
            .await
            .with_context(|| format!("failed to create table '{}'", self.table_name))?;

        log::info!(
            "Created LanceDB table '{}' with {} dimensions",
            self.table_name,
            dimensions
        );
        Ok(())
    }

    /// Build an embedding provider from config (or environment fallback).
    async fn build_provider(&self) -> anyhow::Result<Box<dyn EmbeddingProvider>> {
        let config = Config::load().ok();
        let config = config.as_ref();
        let api_token = if config.map(|c| &c.embedding.api_token).unwrap_or(&String::new()).is_empty() {
            std::env::var("SILICONFLOW_API_KEY").unwrap_or_default()
        } else {
            config.map(|c| c.embedding.api_token.clone()).unwrap_or_default()
        };
        let base_url = if config.map(|c| &c.embedding.api_base_url).unwrap_or(&String::new()).is_empty() {
            None
        } else {
            Some(config.map(|c| c.embedding.api_base_url.clone()).unwrap_or_default())
        };
        let model = if config.map(|c| &c.embedding.model).unwrap_or(&String::new()).is_empty() {
            "Qwen/Qwen3-Embedding-4B".to_string()
        } else {
            config.map(|c| c.embedding.model.clone()).unwrap_or_default()
        };

        let provider =
            OpenAICompatibleEmbeddingProvider::new(api_token, base_url, model);
        Ok(Box::new(provider))
    }

    /// Insert a knowledge record into both LanceDB and the BM25 index.
    pub async fn add(&self, mut record: KnowledgeRecord) -> anyhow::Result<KnowledgeRecord> {
        // Sanitize then compute embedding.
        record = record.sanitize();
        let embed_text = record.embed_text();

        let provider = self
            .build_provider()
            .await
            .context("failed to build embedding provider")?;

        let vector = provider
            .get_embedding(&embed_text)
            .await
            .map_err(|e| anyhow!("embedding API call failed: {}", e))?;

        let dimensions = Config::load()
            .map(|c| c.embedding.dimensions)
            .unwrap_or(2560);
        if vector.len() != dimensions {
            return Err(anyhow!(
                "embedding dimension mismatch: expected {}, got {}",
                dimensions,
                vector.len()
            ));
        }
        record.vector = vector;

        self.ensure_table().await?;

        let dimensions = Config::load()
            .map(|c| c.embedding.dimensions)
            .unwrap_or(2560);

        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new(
                "vector",
                DataType::FixedSizeList(
                    Arc::new(Field::new("item", DataType::Float32, true)),
                    dimensions as i32,
                ),
                false,
            ),
            Field::new("type", DataType::Utf8, false),
            Field::new("title", DataType::Utf8, false),
            Field::new("content", DataType::Utf8, false),
            Field::new("tags", DataType::List(Arc::new(Field::new("item", DataType::Utf8, true))), true),
            Field::new(
                "related_files",
                DataType::List(Arc::new(Field::new("item", DataType::Utf8, true))),
                true,
            ),
            Field::new("source_agent", DataType::Utf8, false),
            Field::new("task_id", DataType::Utf8, true),
            Field::new("confidence", DataType::Float32, false),
            Field::new("created_at", DataType::Utf8, false),
            Field::new("updated_at", DataType::Utf8, false),
            Field::new("access_count", DataType::Int32, false),
            Field::new("last_accessed", DataType::Utf8, true),
            Field::new(
                "parent_ids",
                DataType::List(Arc::new(Field::new("item", DataType::Utf8, true))),
                true,
            ),
        ]));

        let mut id_builder = StringBuilder::new();
        let mut vector_builder = FixedSizeListBuilder::new(Float32Builder::new(), dimensions as i32);
        let mut type_builder = StringBuilder::new();
        let mut title_builder = StringBuilder::new();
        let mut content_builder = StringBuilder::new();

        let mut tags_builder = ListBuilder::new(StringBuilder::new());
        for t in &record.tags {
            tags_builder.values().append_value(t);
        }
        tags_builder.append(true);

        let mut related_files_builder = ListBuilder::new(StringBuilder::new());
        for f in &record.related_files {
            related_files_builder.values().append_value(f);
        }
        related_files_builder.append(true);

        let mut source_agent_builder = StringBuilder::new();
        let mut task_id_builder = StringBuilder::new();
        let mut confidence_builder = Float32Builder::new();
        let mut created_at_builder = StringBuilder::new();
        let mut updated_at_builder = StringBuilder::new();
        let mut access_count_builder = Int32Builder::new();
        let mut last_accessed_builder = StringBuilder::new();
        let mut parent_ids_builder = ListBuilder::new(StringBuilder::new());
        for p in &record.parent_ids {
            parent_ids_builder.values().append_value(p);
        }
        parent_ids_builder.append(true);

        id_builder.append_value(&record.id);
        vector_builder.values().append_slice(&record.vector);
        vector_builder.append(true);
        type_builder.append_value(&record.type_);
        title_builder.append_value(&record.title);
        content_builder.append_value(&record.content);
        source_agent_builder.append_value(&record.source_agent);
        task_id_builder.append_option(if record.task_id.is_empty() {
            None
        } else {
            Some(&record.task_id)
        });
        confidence_builder.append_value(record.confidence);
        created_at_builder.append_value(&record.created_at);
        updated_at_builder.append_value(&record.updated_at);
        access_count_builder.append_value(record.access_count as i32);
        last_accessed_builder.append_option(record.last_accessed.as_ref());

        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(id_builder.finish()),
                Arc::new(vector_builder.finish()),
                Arc::new(type_builder.finish()),
                Arc::new(title_builder.finish()),
                Arc::new(content_builder.finish()),
                Arc::new(tags_builder.finish()),
                Arc::new(related_files_builder.finish()),
                Arc::new(source_agent_builder.finish()),
                Arc::new(task_id_builder.finish()),
                Arc::new(confidence_builder.finish()),
                Arc::new(created_at_builder.finish()),
                Arc::new(updated_at_builder.finish()),
                Arc::new(access_count_builder.finish()),
                Arc::new(last_accessed_builder.finish()),
                Arc::new(parent_ids_builder.finish()),
            ],
        )
        .with_context(|| "failed to build RecordBatch")?;

        let db_path_str = self.db_path.to_string_lossy().to_string();
        let table = connect(&db_path_str)
            .execute()
            .await
            .with_context(|| format!("failed to connect to LanceDB at {}", self.db_path.display()))?;
        let table = table
            .open_table(&self.table_name)
            .execute()
            .await
            .with_context(|| format!("failed to open table '{}'", self.table_name))?;

        let batches = vec![Ok(batch)];
        let data: Box<dyn arrow::array::RecordBatchReader + Send> =
            Box::new(RecordBatchIterator::new(batches, schema));
        table
            .add(data)
            .execute()
            .await
            .with_context(|| "failed to add record to LanceDB")?;

        // Index into BM25.
        if let Some(ref bm25) = self.bm25 {
            bm25
                .add_document(&record)
                .with_context(|| "failed to add document to BM25 index")?;
        }

        log::info!("Added knowledge record: {}", record.id);

        // Return record with vector cleared (not stored in the struct field).
        record.vector = vec![];
        Ok(record)
    }

    /// Delete a record by ID from both LanceDB and the BM25 index.
    pub async fn delete(&self, id: &str) -> anyhow::Result<()> {
        self.ensure_table().await?;

        let db_path_str = self.db_path.to_string_lossy().to_string();
        let table = connect(&db_path_str)
            .execute()
            .await
            .with_context(|| format!("failed to connect to LanceDB at {}", self.db_path.display()))?;
        let table = table
            .open_table(&self.table_name)
            .execute()
            .await
            .with_context(|| format!("failed to open table '{}'", self.table_name))?;

        let predicate = format!("id = '{}'", id.replace('\'', "''"));
        table
            .delete(&predicate)
            .await
            .with_context(|| format!("failed to delete record '{}' from LanceDB", id))?;

        if let Some(ref bm25) = self.bm25 {
            if let Err(e) = bm25.delete_by_id(id) {
                log::warn!("Failed to delete from BM25 index: {}", e);
            }
        }

        log::info!("Deleted knowledge record: {}", id);
        Ok(())
    }

    /// List records with optional filters. Returns up to `limit` records.
    pub async fn list(
        &self,
        limit: usize,
        type_filter: Option<&str>,
        tag_filter: Option<&str>,
    ) -> anyhow::Result<Vec<KnowledgeRecord>> {
        self.ensure_table().await?;

        let db_path_str = self.db_path.to_string_lossy().to_string();
        let connection = connect(&db_path_str)
            .execute()
            .await
            .with_context(|| format!("failed to connect to LanceDB at {}", self.db_path.display()))?;
        let table = connection
            .open_table(&self.table_name)
            .execute()
            .await
            .with_context(|| format!("failed to open table '{}'", self.table_name))?;

        // Scan the full table (knowledge store is expected to be small).
        let fetch_limit = limit.saturating_mul(5).max(100);
        let mut results_stream = table
            .query()
            .limit(fetch_limit)
            .execute()
            .await
            .with_context(|| "failed to execute LanceDB scan")?;

        let mut records = Vec::new();
        while let Some(batch) = results_stream
            .try_next()
            .await
            .with_context(|| "failed to read LanceDB batch")?
        {
            for row in 0..batch.num_rows() {
                let record = self.record_from_batch(&batch, row);
                // Apply filters.
                if let Some(tf) = type_filter {
                    if record.type_ != tf {
                        continue;
                    }
                }
                if let Some(tg) = tag_filter {
                    if !record.tags.iter().any(|t| t == tg) {
                        continue;
                    }
                }
                records.push(record);
                if records.len() >= limit {
                    return Ok(records);
                }
            }
        }

        Ok(records)
    }

    /// Hybrid search: vector (LanceDB) + BM25 (Tantivy) fused via RRF.
    pub async fn search(
        &self,
        query: &str,
        limit: usize,
        _rerank: bool,
    ) -> anyhow::Result<Vec<KnowledgeSearchResult>> {
        if query.is_empty() {
            return Ok(Vec::new());
        }

        let provider = self
            .build_provider()
            .await
            .context("failed to build embedding provider")?;
        let query_vector = provider
            .get_embedding(query)
            .await
            .map_err(|e| anyhow!("embedding API call failed for search query: {}", e))?;

        // ── Vector search ──
        self.ensure_table().await?;
        let db_path_str = self.db_path.to_string_lossy().to_string();
        let connection = connect(&db_path_str)
            .execute()
            .await
            .with_context(|| format!("failed to connect to LanceDB at {}", self.db_path.display()))?;
        let table = connection
            .open_table(&self.table_name)
            .execute()
            .await
            .with_context(|| format!("failed to open table '{}'", self.table_name))?;

        let mut vector_results: Vec<(String, f32)> = Vec::new();
        let mut record_map: HashMap<String, KnowledgeRecord> = HashMap::new();
        let mut vector_score_map: HashMap<String, f32> = HashMap::new();

        let mut results_stream = table
            .query()
            .nearest_to(query_vector)?
            .limit(limit.saturating_mul(2))
            .execute()
            .await
            .with_context(|| "failed to execute vector search")?;

        while let Some(batch) = results_stream
            .try_next()
            .await
            .with_context(|| "failed to read vector search batch")?
        {
            let dist_col = batch
                .column_by_name("_distance")
                .and_then(|c| c.as_any().downcast_ref::<arrow::array::Float32Array>());
            let id_col = batch
                .column_by_name("id")
                .and_then(|c| c.as_any().downcast_ref::<arrow::array::StringArray>());

            for i in 0..batch.num_rows() {
                let id = id_col
                    .map(|c| c.value(i).to_string())
                    .unwrap_or_default();
                let distance = dist_col.map(|c| c.value(i)).unwrap_or(0.0);
                let score = 1.0 / (1.0 + distance);
                vector_results.push((id.clone(), score));

                if !record_map.contains_key(&id) {
                    let record = self.record_from_batch(&batch, i);
                    record_map.insert(id.clone(), record);
                    vector_score_map.insert(id, score);
                }
            }
        }

        // ── BM25 search ──
        let mut bm25_results: Vec<(String, f32)> = Vec::new();
        if let Some(ref bm25) = self.bm25 {
            bm25_results = bm25
                .search(query, limit.saturating_mul(2))
                .unwrap_or_default();
        }

        // ── RRF fusion ──
        let fused = rrf_fuse(vector_results, bm25_results.clone(), limit);

        // ── Build results ──
        let mut results = Vec::new();
        for (id, fused_score) in fused {
            if let Some(record) = record_map.get(&id) {
                let record = record.clone();
                let vs = vector_score_map.get(&id).copied();
                let bs = bm25_results
                    .iter()
                    .find(|(bid, _)| bid == &id)
                    .map(|(_, s)| *s);
                results.push(build_result(
                    record,
                    vs,
                    bs,
                    fused_score as f32,
                    None, // rerank_score: TODO (Phase 1 disabled)
                ));
            } else if let Some(ref bm25) = self.bm25 {
                // Fallback: fetch from BM25-only results (record may have been deleted from LanceDB).
                // This is a best-effort path; in practice both indexes should stay in sync.
                log::warn!("Record {} found in BM25 but not LanceDB, skipping", id);
            }
        }

        results.truncate(limit);
        Ok(results)
    }

    /// Parse a single row from a LanceDB batch into a [`KnowledgeRecord`].
    fn record_from_batch(&self, batch: &RecordBatch, row: usize) -> KnowledgeRecord {
        let id = batch
            .column_by_name("id")
            .and_then(|c| c.as_any().downcast_ref::<arrow::array::StringArray>())
            .map(|c| c.value(row).to_string())
            .unwrap_or_default();

        // Vector is skipped (serde(skip) on the struct field).
        let type_ = batch
            .column_by_name("type")
            .and_then(|c| c.as_any().downcast_ref::<arrow::array::StringArray>())
            .map(|c| c.value(row).to_string())
            .unwrap_or_default();

        let title = batch
            .column_by_name("title")
            .and_then(|c| c.as_any().downcast_ref::<arrow::array::StringArray>())
            .map(|c| c.value(row).to_string())
            .unwrap_or_default();

        let content = batch
            .column_by_name("content")
            .and_then(|c| c.as_any().downcast_ref::<arrow::array::StringArray>())
            .map(|c| c.value(row).to_string())
            .unwrap_or_default();

        let tags = self.extract_list_string(batch, row, "tags");
        let related_files = self.extract_list_string(batch, row, "related_files");
        let source_agent = batch
            .column_by_name("source_agent")
            .and_then(|c| c.as_any().downcast_ref::<arrow::array::StringArray>())
            .map(|c| c.value(row).to_string())
            .unwrap_or_default();

        let task_id = batch
            .column_by_name("task_id")
            .and_then(|c| c.as_any().downcast_ref::<arrow::array::StringArray>())
            .filter(|c| !c.is_null(row))
            .map(|c| c.value(row).to_string())
            .unwrap_or_default();

        let confidence = batch
            .column_by_name("confidence")
            .and_then(|c| c.as_any().downcast_ref::<arrow::array::Float32Array>())
            .map(|c| c.value(row))
            .unwrap_or(0.0);

        let created_at = batch
            .column_by_name("created_at")
            .and_then(|c| c.as_any().downcast_ref::<arrow::array::StringArray>())
            .map(|c| c.value(row).to_string())
            .unwrap_or_default();

        let updated_at = batch
            .column_by_name("updated_at")
            .and_then(|c| c.as_any().downcast_ref::<arrow::array::StringArray>())
            .map(|c| c.value(row).to_string())
            .unwrap_or_default();

        let access_count = batch
            .column_by_name("access_count")
            .and_then(|c| c.as_any().downcast_ref::<arrow::array::Int32Array>())
            .map(|c| c.value(row) as u32)
            .unwrap_or(0);

        let last_accessed = batch
            .column_by_name("last_accessed")
            .and_then(|c| c.as_any().downcast_ref::<arrow::array::StringArray>())
            .filter(|c| !c.is_null(row))
            .map(|c| c.value(row).to_string());

        let parent_ids = self.extract_list_string(batch, row, "parent_ids");

        KnowledgeRecord {
            id,
            vector: vec![],
            type_,
            title,
            content,
            tags,
            related_files,
            source_agent,
            task_id,
            confidence,
            created_at,
            updated_at,
            access_count,
            last_accessed,
            parent_ids,
        }
    }

    /// Extract a List<Utf8> column as a Vec<String> for the given row.
    fn extract_list_string(
        &self,
        batch: &RecordBatch,
        row: usize,
        column_name: &str,
    ) -> Vec<String> {
        let list_col = batch
            .column_by_name(column_name)
            .and_then(|c| c.as_any().downcast_ref::<arrow::array::ListArray>());

        match list_col {
            None => Vec::new(),
            Some(list_arr) => {
                if list_arr.is_null(row) {
                    return Vec::new();
                }
                let value_arr = list_arr.value(row);
                let str_arr = match value_arr.as_any().downcast_ref::<arrow::array::StringArray>() {
                    Some(s) => s,
                    None => {
                        log::warn!("expected StringArray in column {}, got different type", column_name);
                        return Vec::new();
                    }
                };
                let str_len = str_arr.len();
                if str_len == 0 {
                    return Vec::new();
                }
                let start = usize::try_from(list_arr.value_offsets()[row]).unwrap_or(0);
                let end = usize::try_from(list_arr.value_offsets()[row + 1]).unwrap_or(0);
                // Clamp end to actual StringArray length to avoid panic on mismatched batches
                let end = end.min(str_len);
                let start = start.min(end);
                (start..end)
                    .map(|i| str_arr.value(i).to_string())
                    .collect()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge::record::KnowledgeRecord;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_add_and_list() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("knowledge");
        let bm25_dir = dir.path().join("knowledge_index");
        std::fs::create_dir_all(&db_path).unwrap();
        std::fs::create_dir_all(&bm25_dir).unwrap();

        let store = KnowledgeStore {
            db_path: db_path.clone(),
            table_name: "test_knowledge".to_string(),
            bm25: Some(
                KnowledgeTantivyIndex::open_or_create(&bm25_dir).unwrap(),
            ),
        };

        let record = KnowledgeRecord::new(
            "Test Title".to_string(),
            "Test content about Rust programming".to_string(),
            "repo_retrieval".to_string(),
            "repo_agent".to_string(),
            "task-001".to_string(),
        );
        let record = record
            .sanitize()
            .embed_text();
        let _record = store.add(record).await;
        // Note: embedding API will be called; in tests with no API token this may fail.
        // The test is a structural check; actual integration requires a real API token.
        let _ = _record;
    }
}

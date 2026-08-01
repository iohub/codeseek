//! Knowledge-specific BM25 index backed by Tantivy.
//!
//! Indexes `KnowledgeRecord` fields (`title`, `content`, `type_`) for full-text
//! search. Serves as the sparse channel in the hybrid RRF fusion pipeline.

use anyhow::{Result, anyhow};
use std::path::Path;
use std::sync::Arc;
use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::{Schema, STORED, STRING, TEXT};
use tantivy::{Index, IndexWriter, TantivyDocument, Term};
use std::sync::Mutex;

use super::record::KnowledgeRecord;

/// Tantivy-backed BM25 index for knowledge records.
pub struct KnowledgeTantivyIndex {
    index: Index,
    writer: Arc<Mutex<Option<IndexWriter>>>,
    schema: Schema,
    id_field: tantivy::schema::Field,
    title_field: tantivy::schema::Field,
    content_field: tantivy::schema::Field,
    type_field: tantivy::schema::Field,
}

impl KnowledgeTantivyIndex {
    /// Open an existing index or create a new one at `dir`.
    pub fn open_or_create<P: AsRef<Path>>(dir: P) -> Result<Self> {
        let mut schema_builder = Schema::builder();

        let id_field = schema_builder.add_text_field("id", STRING | STORED);
        let title_field = schema_builder.add_text_field("title", TEXT);
        let content_field = schema_builder.add_text_field("content", TEXT);
        let type_field = schema_builder.add_text_field("type", STRING | STORED);

        let schema = schema_builder.build();
        let dir_path = dir.as_ref();
        std::fs::create_dir_all(dir_path)
            .map_err(|e| anyhow!("Failed to create knowledge index dir {:?}: {}", dir_path, e))?;

        let index = if dir_path.read_dir().map_or(false, |mut dir| dir.next().is_some()) {
            Index::open_in_dir(dir_path).unwrap_or_else(|_| {
                Index::create_in_dir(dir_path, schema.clone())
                    .expect("Failed to create fresh Tantivy index")
            })
        } else {
            Index::create_in_dir(dir_path, schema.clone())
                .map_err(|e| anyhow!("Failed to create Tantivy index at {:?}: {}", dir_path, e))?
        };

        let writer = index
            .writer(50_000_000)
            .map_err(|e| anyhow!("Failed to create Tantivy IndexWriter: {}", e))?;

        Ok(Self {
            index,
            schema,
            writer: Arc::new(Mutex::new(Some(writer))),
            id_field,
            title_field,
            content_field,
            type_field,
        })
    }

    fn get_stored_string(
        doc: &TantivyDocument,
        field: tantivy::schema::Field,
        schema: &Schema,
    ) -> String {
        schema
            .get_field_entry(field)
            .is_stored()
            .then(|| {
                doc.get_first(field)
                    .and_then(|val| match val {
                        tantivy::schema::OwnedValue::Str(s) => Some(s.clone()),
                        _ => None,
                    })
            })
            .flatten()
            .unwrap_or_default()
    }

    /// Add a knowledge record to the index.
    pub fn add_document(&self, record: &KnowledgeRecord) -> Result<()> {
        let mut doc = TantivyDocument::default();
        doc.add_text(self.id_field, &record.id);
        doc.add_text(self.title_field, &record.title);
        doc.add_text(self.content_field, &record.content);
        doc.add_text(self.type_field, &record.type_);

        let mut w = self.writer.lock().map_err(|e| anyhow!("Tantivy writer lock poisoned: {}", e))?;
        let writer = w.as_mut().ok_or_else(|| anyhow!("Tantivy writer is dropped"))?;
        writer
            .add_document(doc)
            .map_err(|e| anyhow!("Failed to add document: {}", e))?;
        writer
            .commit()
            .map_err(|e| anyhow!("Tantivy commit failed: {}", e))?;
        Ok(())
    }

    /// Delete a record by its ID.
    pub fn delete_by_id(&self, id: &str) -> Result<()> {
        let term = Term::from_field_text(self.id_field, id);
        let mut w = self.writer.lock().map_err(|e| anyhow!("Tantivy writer lock poisoned: {}", e))?;
        let writer = w.as_mut().ok_or_else(|| anyhow!("Tantivy writer is dropped"))?;
        writer.delete_term(term);
        writer
            .commit()
            .map_err(|e| anyhow!("Tantivy commit after delete failed: {}", e))?;
        Ok(())
    }

    /// Search the index and return sorted (id, bm25_score) pairs.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<(String, f32)>> {
        if query.is_empty() {
            return Ok(Vec::new());
        }

        let reader = self.index.reader().map_err(|e| anyhow!("Tantivy reader error: {}", e))?;
        let searcher = reader.searcher();

        let query_parser = QueryParser::for_index(
            &self.index,
            vec![self.title_field, self.content_field],
        );

        let parsed_query = query_parser.parse_query(query).unwrap_or_else(|_| {
            let sanitized = query.replace(['"', '*', '?'], "");
            if sanitized.contains(' ') {
                query_parser
                    .parse_query(&format!("\"{}\"", sanitized))
                    .unwrap_or_else(|_| query_parser.parse_query(&sanitized).unwrap())
            } else {
                query_parser.parse_query(&sanitized).unwrap()
            }
        });

        let top_docs = searcher
            .search(&parsed_query, &TopDocs::with_limit(limit.max(1)))
            .map_err(|e| anyhow!("Tantivy search error: {}", e))?;

        let mut results = Vec::new();
        for (score, doc_address) in top_docs {
            let doc = searcher
                .doc::<TantivyDocument>(doc_address)
                .map_err(|e| anyhow!("Tantivy doc fetch error: {}", e))?;
            let id = Self::get_stored_string(&doc, self.id_field, &self.schema);
            results.push((id, score));
        }

        Ok(results)
    }

    /// Return the number of documents in the index.
    pub fn document_count(&self) -> Result<usize> {
        let reader = self.index.reader().map_err(|e| anyhow!("Tantivy reader error: {}", e))?;
        Ok(reader.searcher().num_docs() as usize)
    }
}

use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::*;
use tantivy::{Index, IndexWriter, ReloadPolicy, doc};
use tantivy::schema::{TantivyDocument, Value};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use crate::error::AppError;
use axum::http::StatusCode;

pub struct SearchService {
    index: Index,
    writer: Arc<Mutex<IndexWriter>>,
    schema: Schema,
}

impl SearchService {
    pub fn new(storage_path: &PathBuf) -> Result<Self, AppError> {
        let index_path = storage_path.join(".search_index");
        if !index_path.exists() {
            std::fs::create_dir_all(&index_path).map_err(AppError::from)?;
        }

        let mut schema_builder = Schema::builder();
        schema_builder.add_text_field("path", STRING | STORED);
        schema_builder.add_text_field("content", TEXT);
        schema_builder.add_text_field("name", TEXT | STORED);
        let schema = schema_builder.build();

        let index = Index::open_or_create(tantivy::directory::MmapDirectory::open(&index_path).map_err(|e| AppError::Anyhow(anyhow::anyhow!(e)))?, schema.clone())
            .map_err(|e| AppError::Anyhow(anyhow::anyhow!(e)))?;

        let writer = index.writer(50_000_000).map_err(|e| AppError::Anyhow(anyhow::anyhow!(e)))?;

        Ok(Self {
            index,
            writer: Arc::new(Mutex::new(writer)),
            schema,
        })
    }

    pub fn index_file(&self, path: &str, name: &str, content: &str) -> Result<(), AppError> {
        let mut writer = self.writer.lock().map_err(|_| AppError::Status(StatusCode::INTERNAL_SERVER_ERROR))?;
        
        let path_field = self.schema.get_field("path").unwrap();
        let name_field = self.schema.get_field("name").unwrap();
        let content_field = self.schema.get_field("content").unwrap();

        // Remove existing document with same path to avoid duplicates (simple update strategy)
        let term = Term::from_field_text(path_field, path);
        writer.delete_term(term);

        let doc = doc!(
            path_field => path,
            name_field => name,
            content_field => content
        );

        writer.add_document(doc).map_err(|e| AppError::Anyhow(anyhow::anyhow!(e)))?;
        writer.commit().map_err(|e| AppError::Anyhow(anyhow::anyhow!(e)))?;

        Ok(())
    }

    pub fn search(&self, query_str: &str) -> Result<Vec<SearchResult>, AppError> {
        let reader = self.index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()
            .map_err(|e| AppError::Anyhow(anyhow::anyhow!(e)))?;

        let searcher = reader.searcher();

        let path_field = self.schema.get_field("path").unwrap();
        let name_field = self.schema.get_field("name").unwrap();
        let content_field = self.schema.get_field("content").unwrap();

        let query_parser = QueryParser::for_index(&self.index, vec![name_field, content_field]);
        let query = query_parser.parse_query(query_str).map_err(|e| AppError::Anyhow(anyhow::anyhow!(e)))?;

        let top_docs: Vec<(f32, tantivy::DocAddress)> = searcher.search(&query, &TopDocs::with_limit(20))
            .map_err(|e| AppError::Anyhow(anyhow::anyhow!(e)))?;

        let mut results = Vec::new();
        for (_score, doc_address) in top_docs {
            let retrieved_doc: TantivyDocument = searcher.doc(doc_address).map_err(|e| AppError::Anyhow(anyhow::anyhow!(e)))?;
            
            let path = retrieved_doc.get_first(path_field).and_then(|v| v.as_str()).unwrap_or_default().to_string();
            let name = retrieved_doc.get_first(name_field).and_then(|v| v.as_str()).unwrap_or_default().to_string();

            results.push(SearchResult {
                path,
                name,
                score: _score,
            });
        }

        Ok(results)
    }
}

#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct SearchResult {
    pub path: String,
    pub name: String,
    pub score: f32,
}
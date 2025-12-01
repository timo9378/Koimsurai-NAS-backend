use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::*;
use tantivy::{Index, IndexWriter, ReloadPolicy, doc};
use tantivy::schema::{TantivyDocument, Value};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, atomic::{AtomicUsize, Ordering}};
use std::time::{Duration, Instant};
use std::env;
use crate::error::AppError;
use axum::http::StatusCode;
use tracing::{debug, info};

/// 批次提交配置 - 從環境變數讀取
/// Batch commit configuration - read from env
fn get_batch_size() -> usize {
    env::var("SEARCH_BATCH_SIZE")
        .unwrap_or_else(|_| "100".to_string())
        .parse()
        .unwrap_or(100)
}

fn get_commit_interval_secs() -> u64 {
    env::var("SEARCH_COMMIT_INTERVAL_SECS")
        .unwrap_or_else(|_| "5".to_string())
        .parse()
        .unwrap_or(5)
}

pub struct SearchService {
    index: Index,
    writer: Arc<Mutex<IndexWriter>>,
    schema: Schema,
    /// 追蹤待 commit 的文件數量
    pending_count: AtomicUsize,
    /// 上次 commit 的時間
    last_commit: Arc<Mutex<Instant>>,
    /// 批次大小
    batch_size: usize,
    /// commit 間隔 (秒)
    commit_interval: Duration,
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

        // 從環境變數讀取搜尋索引緩衝區大小 (MB)
        // Read search index buffer size from env (MB)
        // 開發機: 50MB, Server: 500MB+
        let buffer_size_mb = env::var("SEARCH_INDEX_BUFFER_MB")
            .unwrap_or_else(|_| "50".to_string())
            .parse::<usize>()
            .unwrap_or(50);
        
        let buffer_size = buffer_size_mb * 1_000_000;
        info!("Search index buffer size: {}MB", buffer_size_mb);
        
        let writer = index.writer(buffer_size).map_err(|e| AppError::Anyhow(anyhow::anyhow!(e)))?;
        
        let batch_size = get_batch_size();
        let commit_interval = Duration::from_secs(get_commit_interval_secs());
        
        info!("Search batch_size: {}, commit_interval: {:?}", batch_size, commit_interval);

        Ok(Self {
            index,
            writer: Arc::new(Mutex::new(writer)),
            schema,
            pending_count: AtomicUsize::new(0),
            last_commit: Arc::new(Mutex::new(Instant::now())),
            batch_size,
            commit_interval,
        })
    }

    /// 索引單一檔案 (不立即 commit，使用批次策略)
    /// Index a single file (doesn't commit immediately, uses batch strategy)
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
        
        // 增加待處理計數
        let pending = self.pending_count.fetch_add(1, Ordering::SeqCst) + 1;
        
        // 檢查是否需要 commit
        let should_commit = {
            let last_commit = self.last_commit.lock().map_err(|_| AppError::Status(StatusCode::INTERNAL_SERVER_ERROR))?;
            pending >= self.batch_size || last_commit.elapsed() >= self.commit_interval
        };
        
        if should_commit {
            debug!("Batch committing {} indexed files", pending);
            writer.commit().map_err(|e| AppError::Anyhow(anyhow::anyhow!(e)))?;
            self.pending_count.store(0, Ordering::SeqCst);
            *self.last_commit.lock().map_err(|_| AppError::Status(StatusCode::INTERNAL_SERVER_ERROR))? = Instant::now();
        }

        Ok(())
    }

    /// 強制 commit 所有待處理的索引變更
    /// Force commit all pending index changes
    pub fn flush(&self) -> Result<(), AppError> {
        let pending = self.pending_count.load(Ordering::SeqCst);
        if pending > 0 {
            info!("Flushing {} pending index entries", pending);
            let mut writer = self.writer.lock().map_err(|_| AppError::Status(StatusCode::INTERNAL_SERVER_ERROR))?;
            writer.commit().map_err(|e| AppError::Anyhow(anyhow::anyhow!(e)))?;
            self.pending_count.store(0, Ordering::SeqCst);
            *self.last_commit.lock().map_err(|_| AppError::Status(StatusCode::INTERNAL_SERVER_ERROR))? = Instant::now();
        }
        Ok(())
    }

    pub fn search(&self, query_str: &str) -> Result<Vec<SearchResult>, AppError> {
        // 搜尋前先 flush 確保結果最新 (可選)
        // Optionally flush before search to ensure up-to-date results
        // self.flush()?;
        
        let reader = self.index
            .reader_builder()
            .reload_policy(ReloadPolicy::OnCommitWithDelay) // 改用 OnCommitWithDelay 來自動更新
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
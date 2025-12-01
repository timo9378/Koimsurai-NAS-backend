use sqlx::{Pool, Sqlite};
use std::path::PathBuf;
use std::sync::Arc;
use std::env;
use dav_server::DavHandler;
use tokio::sync::{broadcast, Semaphore};
use crate::utils::queue::JobQueue;
use crate::models::job::JobUpdate;
use crate::services::audit::AuditService;
use crate::services::search::SearchService;

/// 從環境變數取得即時轉碼並發限制
/// Get transcode concurrency limit from env
/// 開發機: 2, Server (有 GPU): 4-8
pub fn get_max_concurrent_transcodes() -> usize {
    env::var("MAX_CONCURRENT_TRANSCODES")
        .unwrap_or_else(|_| "2".to_string())
        .parse()
        .unwrap_or(2)
}

#[derive(Clone)]
pub struct AppState {
    pub pool: Pool<Sqlite>,
    pub storage_path: PathBuf,
    pub queue: Arc<JobQueue>,
    pub webdav: DavHandler,
    pub tx: broadcast::Sender<JobUpdate>,
    pub audit: Arc<AuditService>,
    pub search: Arc<SearchService>,
    /// Semaphore 用於限制同時進行的 FFmpeg 轉碼數量
    /// Semaphore to limit concurrent FFmpeg transcodes
    pub transcode_semaphore: Arc<Semaphore>,
}


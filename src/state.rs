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
use crate::services::docker::DockerService;
use crate::services::ai::AiService;

/// 從環境變數取得即時轉碼並發限制
/// Get transcode concurrency limit from env
/// 開發機: 2, Server (有 GPU): 4-8
pub fn get_max_concurrent_transcodes() -> usize {
    env::var("MAX_CONCURRENT_TRANSCODES")
        .unwrap_or_else(|_| "2".to_string())
        .parse()
        .unwrap_or(2)
}

/// 從環境變數取得是否啟用 Docker 管理功能
/// Get whether Docker management is enabled from env
pub fn get_docker_enabled() -> bool {
    env::var("ENABLE_DOCKER_MANAGER")
        .map(|v| v.to_lowercase() == "true" || v == "1")
        .unwrap_or(false)
}

/// 從環境變數取得是否啟用 AI 圖片標籤功能
/// Get whether AI image labelling is enabled from env
pub fn get_ai_enabled() -> bool {
    env::var("ENABLE_AI_LABELLING")
        .map(|v| v.to_lowercase() == "true" || v == "1")
        .unwrap_or(false)
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
    /// JWT 簽名密鑰（啟動時從環境變數讀取，避免每次請求都讀取 env var）
    /// JWT signing secret (loaded once at startup from env var)
    pub jwt_secret: Arc<String>,
    /// Semaphore 用於限制同時進行的 FFmpeg 轉碼數量
    /// Semaphore to limit concurrent FFmpeg transcodes
    pub transcode_semaphore: Arc<Semaphore>,
    /// Docker 容器管理服務（可選）
    /// Docker container management service (optional)
    pub docker_service: Option<Arc<DockerService>>,
    /// AI 圖片標籤服務（可選）
    /// AI image tagging service (optional)
    pub ai_service: Option<Arc<AiService>>,
}


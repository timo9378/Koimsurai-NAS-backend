pub mod db;
pub mod handlers;
pub mod middleware;
pub mod models;
pub mod routes;
pub mod state;
pub mod utils;
pub mod error;
pub mod services;

use std::sync::Arc;
use std::path::PathBuf;
use dav_server::{localfs::LocalFs, DavHandler};
use tokio::sync::Semaphore;
use crate::state::{AppState, get_max_concurrent_transcodes, get_docker_enabled, get_ai_enabled};
use crate::utils::queue::{JobQueue, worker};
use crate::services::indexer::Indexer;
use crate::services::audit::AuditService;
use crate::services::search::SearchService;
use crate::services::docker::DockerService;
use crate::services::ai::AiService;
use sqlx::SqlitePool;

pub async fn create_app(pool: SqlitePool, storage_path: PathBuf) -> axum::Router {
    // Initialize Indexer
    let indexer = Arc::new(Indexer::new(pool.clone(), storage_path.clone()));
    
    // Run initial scan (non-blocking for tests might be better, but keeping consistent)
    if let Err(e) = indexer.initial_scan().await {
        tracing::error!("Initial scan failed: {}", e);
    }

    // Spawn file watcher
    let indexer_clone = indexer.clone();
    tokio::spawn(async move {
        if let Err(e) = indexer_clone.run_watcher().await {
            tracing::error!("File watcher failed: {}", e);
        }
    });

    // Initialize Broadcast Channel
    let (tx, _rx) = tokio::sync::broadcast::channel(100);

    // Initialize Job Queue
    let (queue, receiver) = JobQueue::new(100, pool.clone());
    let queue = Arc::new(queue);
    
    // Initialize Search Service
    let search = Arc::new(SearchService::new(&storage_path).expect("Failed to initialize search service"));

    // Initialize AI Service (可選)
    let ai_service = if get_ai_enabled() {
        tracing::info!("🤖 AI Image Labelling ENABLED");
        let config = AiService::config_from_env();
        tracing::info!(
            "   Model: {}, Min confidence: {}, GPU: {}, Max concurrent: {}",
            config.model_name, config.min_confidence, config.use_gpu, config.max_concurrent_inferences
        );
        Some(Arc::new(AiService::new(pool.clone(), Some(config))))
    } else {
        tracing::info!("🤖 AI Image Labelling DISABLED (set ENABLE_AI_LABELLING=true to enable)");
        None
    };

    // Spawn worker (傳遞 ai_service)
    let search_clone = search.clone();
    let ai_clone = ai_service.clone();
    tokio::spawn(worker(receiver, pool.clone(), tx.clone(), search_clone, ai_clone));

    // Initialize WebDAV
    let webdav = DavHandler::builder()
        .filesystem(LocalFs::new(storage_path.clone(), false, false, false))
        .locksystem(dav_server::memls::MemLs::new())
        .build_handler();

    // Initialize Audit Service
    let audit = Arc::new(AuditService::new(pool.clone()));

    // Initialize Transcode Semaphore (限制同時轉碼數量)
    let max_transcodes = get_max_concurrent_transcodes();
    tracing::info!("Max concurrent transcodes: {}", max_transcodes);
    let transcode_semaphore = Arc::new(Semaphore::new(max_transcodes));

    // Initialize Docker Service (可選)
    let docker_service = if get_docker_enabled() {
        tracing::info!("🐳 Docker management ENABLED");
        let service = Arc::new(DockerService::new());
        // 嘗試連接到 Docker daemon
        if let Err(e) = service.connect().await {
            tracing::warn!("Failed to connect to Docker daemon: {}. Docker features may not work until manually connected.", e);
        } else {
            tracing::info!("   Successfully connected to Docker daemon");
        }
        Some(service)
    } else {
        tracing::info!("🐳 Docker management DISABLED (set ENABLE_DOCKER_MANAGER=true to enable)");
        None
    };

    // Initialize HLS Cleanup Task
    // 清理 1 小時以上的暫存檔，每 30 分鐘執行一次
    let hls_cache_dir = storage_path.join(".hls_cache");
    let hls_max_age = std::time::Duration::from_secs(
        std::env::var("HLS_CACHE_MAX_AGE_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(3600) // 預設 1 小時
    );
    let hls_cleanup_interval = std::time::Duration::from_secs(
        std::env::var("HLS_CLEANUP_INTERVAL_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(1800) // 預設 30 分鐘
    );
    
    tracing::info!(
        "🧹 HLS cleanup task: max_age={}s, interval={}s",
        hls_max_age.as_secs(),
        hls_cleanup_interval.as_secs()
    );
    crate::utils::cleanup::spawn_hls_cleanup_task(hls_cache_dir, hls_max_age, hls_cleanup_interval);

    let state = AppState {
        pool,
        storage_path,
        queue,
        webdav,
        tx,
        audit,
        search,
        transcode_semaphore,
        docker_service,
        ai_service,
    };

    routes::create_router(state).await
}
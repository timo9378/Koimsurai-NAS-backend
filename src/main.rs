use std::{env, path::PathBuf};
use tokio::fs;
use dotenvy::dotenv;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod db;
mod handlers;
mod middleware;
mod models;
mod routes;
mod state;
mod utils;
mod error;
mod services;

use crate::state::AppState;
use std::sync::Arc;
use dav_server::{localfs::LocalFs, DavHandler};
use crate::utils::queue::{JobQueue, worker};
use crate::services::indexer::Indexer;
use crate::services::audit::AuditService;
use crate::services::search::SearchService;

#[tokio::main]
async fn main() {
    dotenv().ok();
    
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with(tracing_subscriber::fmt::layer())
        .init();

    // 初始化資料庫
    // Initialize DB
    let pool = db::init_db().await.expect("Failed to initialize database");

    // 初始化儲存目錄
    // Initialize storage
    let storage_path_str = env::var("STORAGE_PATH").unwrap_or_else(|_| "storage".to_string());
    let storage_path = PathBuf::from(storage_path_str);
    if !storage_path.exists() {
        fs::create_dir_all(&storage_path).await.expect("Failed to create storage directory");
    }

    // Initialize Indexer
    let indexer = Arc::new(Indexer::new(pool.clone(), storage_path.clone()));
    
    // Run initial scan
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

    // Spawn worker
    let search_clone = search.clone();
    tokio::spawn(worker(receiver, pool.clone(), tx.clone(), search_clone));

    // Initialize WebDAV
    let webdav = DavHandler::builder()
        .filesystem(LocalFs::new(storage_path.clone(), false, false, false))
        .locksystem(dav_server::memls::MemLs::new())
        .build_handler();

    // Initialize Audit Service
    let audit = Arc::new(AuditService::new(pool.clone()));

    let state = AppState {
        pool,
        storage_path,
        queue,
        webdav,
        tx,
        audit,
        search,
    };

    let app = routes::create_router(state).await;

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    tracing::info!("RustNAS Server running on http://0.0.0.0:3000");
    axum::serve(listener, app).await.unwrap();
}


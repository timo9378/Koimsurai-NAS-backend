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
use crate::state::AppState;
use crate::utils::queue::{JobQueue, worker};
use crate::services::indexer::Indexer;
use crate::services::audit::AuditService;
use crate::services::search::SearchService;
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

    routes::create_router(state).await
}
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

use crate::state::AppState;
use std::sync::Arc;
use dav_server::{localfs::LocalFs, DavHandler};
use crate::utils::queue::{JobQueue, worker};

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

    // Initialize Job Queue
    let (queue, receiver) = JobQueue::new(100);
    let queue = Arc::new(queue);
    
    // Spawn worker
    tokio::spawn(worker(receiver));

    // Initialize WebDAV
    let webdav = DavHandler::builder()
        .filesystem(LocalFs::new(storage_path.clone(), false, false, false))
        .locksystem(dav_server::memls::MemLs::new())
        .build_handler();

    let state = AppState {
        pool,
        storage_path,
        queue,
        webdav,
    };

    let app = routes::create_router(state).await;

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    tracing::info!("RustNAS Server running on http://0.0.0.0:3000");
    axum::serve(listener, app).await.unwrap();
}


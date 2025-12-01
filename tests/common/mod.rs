use std::path::PathBuf;
use std::sync::Arc;
use sqlx::{SqlitePool, sqlite::SqlitePoolOptions};
use tokio::net::TcpListener;
use Koimsurai_NAS::{create_app, db};
use tempfile::TempDir;

pub struct TestApp {
    pub address: String,
    pub pool: SqlitePool,
    pub storage_dir: TempDir,
}

pub async fn spawn_app() -> TestApp {
    // 使用記憶體資料庫進行測試，或者使用暫存檔案
    // 為了確保隔離性，這裡使用暫存檔案資料庫
    let db_dir = TempDir::new().expect("Failed to create temp dir for db");
    let db_path = db_dir.path().join("test.db");
    let database_url = format!("sqlite://{}", db_path.to_str().unwrap());
    
    // 初始化資料庫
    let pool = db::init_db(Some(database_url)).await.expect("Failed to initialize database");

    // 建立暫存儲存目錄
    let storage_dir = TempDir::new().expect("Failed to create temp dir for storage");
    let storage_path = storage_dir.path().to_path_buf();

    let app = create_app(pool.clone(), storage_path).await;

    // 綁定到隨機埠口
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("Failed to bind random port");
    let port = listener.local_addr().unwrap().port();
    let address = format!("http://127.0.0.1:{}", port);

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    TestApp {
        address,
        pool,
        storage_dir, // 保持 TempDir 存活直到 TestApp 被釋放
    }
}
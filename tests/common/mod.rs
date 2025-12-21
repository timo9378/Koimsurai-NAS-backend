use sqlx::SqlitePool;
use tokio::net::TcpListener;
use Koimsurai_NAS::{create_app, db};
use tempfile::TempDir;

/// 測試用的邀請碼
pub const TEST_INVITE_CODE: &str = "test_invite_code_12345";

pub struct TestApp {
    pub address: String,
    pub _pool: SqlitePool,
    pub _storage_dir: TempDir,
}

pub async fn spawn_app() -> TestApp {
    // 設定測試用的邀請碼環境變數
    std::env::set_var("REGISTRATION_INVITE_CODE", TEST_INVITE_CODE);
    // 設定 JWT secret，避免在測試中呼叫產生或驗證 token 時 panic
    std::env::set_var("JWT_SECRET", "test_jwt_secret_for_tests");
    // 為測試環境關閉 secure cookie 標記，讓 HTTP 測試能讀取 cookie
    std::env::set_var("COOKIE_SECURE", "false");
    
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

    TestApp { address, _pool: pool, _storage_dir: storage_dir }
}
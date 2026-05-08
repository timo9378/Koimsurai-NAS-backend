use sqlx::{migrate::MigrateDatabase, sqlite::SqlitePoolOptions, Pool, Sqlite};
use std::env;
use anyhow::Result;
use tracing::info;

// 初始化資料庫連線與表格
// Initialize database connection and tables
pub async fn init_db(database_url: Option<String>) -> Result<Pool<Sqlite>> {
    let database_url = database_url.unwrap_or_else(|| env::var("DATABASE_URL").expect("DATABASE_URL must be set"));
    
    // 從環境變數讀取資料庫連線數 (開發機: 5, Server: 50+)
    // Read max connections from env (Dev: 5, Server: 50+)
    let max_connections = env::var("DATABASE_MAX_CONNECTIONS")
        .unwrap_or_else(|_| "5".to_string())
        .parse::<u32>()
        .unwrap_or(5);
    
    info!("Database max connections: {}", max_connections);
    
    // 如果資料庫檔案不存在，則建立它
    // Create database file if it doesn't exist
    if !Sqlite::database_exists(&database_url).await.unwrap_or(false) {
        println!("Creating database {}", database_url);
        Sqlite::create_database(&database_url).await?;
    }

    let pool = SqlitePoolOptions::new()
        .max_connections(max_connections)
        .connect(&database_url)
        .await?;

    // 啟用 WAL 模式以提升並發效能 (對 Litestream 也是推薦的)
    // Enable WAL mode for better concurrency (also recommended for Litestream)
    sqlx::query("PRAGMA journal_mode=WAL")
        .execute(&pool)
        .await?;
    
    // 設定 synchronous 為 NORMAL (WAL 模式下的推薦設定)
    // Set synchronous to NORMAL (recommended for WAL mode)
    sqlx::query("PRAGMA synchronous=NORMAL")
        .execute(&pool)
        .await?;
    
    // 從環境變數讀取 mmap_size (MB)，讓常用資料駐留 RAM
    // Read mmap_size from env (MB), keeps frequently accessed data in RAM
    let mmap_size_mb = env::var("DATABASE_MMAP_SIZE_MB")
        .unwrap_or_else(|_| "256".to_string())
        .parse::<u64>()
        .unwrap_or(256);
    
    let mmap_size_bytes = mmap_size_mb * 1024 * 1024;
    sqlx::query(&format!("PRAGMA mmap_size={}", mmap_size_bytes))
        .execute(&pool)
        .await?;
    
    info!("Database mmap_size: {}MB", mmap_size_mb);

    // 建立使用者表格
    // Create users table
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS users (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            username TEXT NOT NULL UNIQUE,
            password_hash TEXT NOT NULL,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP
        );
        "#
    )
    .execute(&pool)
    .await?;

    // 建立分享連結表格
    // Create share_links table
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS share_links (
            id TEXT PRIMARY KEY,
            file_path TEXT NOT NULL,
            password_hash TEXT,
            expires_at DATETIME,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            creator_id INTEGER,
            FOREIGN KEY(creator_id) REFERENCES users(id)
        );
        "#
    )
    .execute(&pool)
    .await?;

    // 建立權限表格
    // Create permissions table
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS permissions (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id INTEGER NOT NULL,
            path TEXT NOT NULL,
            can_read BOOLEAN DEFAULT 0,
            can_write BOOLEAN DEFAULT 0,
            FOREIGN KEY(user_id) REFERENCES users(id),
            UNIQUE(user_id, path)
        );
        "#
    )
    .execute(&pool)
    .await?;

    // 建立檔案索引表格
    // Create files index table
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS files (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            path TEXT NOT NULL UNIQUE,
            name TEXT NOT NULL,
            size INTEGER NOT NULL,
            mime_type TEXT,
            parent_path TEXT,
            is_dir BOOLEAN NOT NULL,
            modified DATETIME NOT NULL,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            hash TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_files_parent_path ON files(parent_path);
        "#
    )
    .execute(&pool)
    .await?;

    // 建立任務表格
    // Create jobs table
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS jobs (
            id TEXT PRIMARY KEY,
            job_type TEXT NOT NULL,
            status TEXT NOT NULL,
            progress INTEGER DEFAULT 0,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            error TEXT
        );
        "#
    )
    .execute(&pool)
    .await?;

    // 建立上傳 Session 表格
    // Create upload_sessions table
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS upload_sessions (
            id TEXT PRIMARY KEY,
            user_id INTEGER NOT NULL,
            file_path TEXT NOT NULL,
            file_name TEXT NOT NULL,
            total_size INTEGER NOT NULL,
            uploaded_size INTEGER DEFAULT 0,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            FOREIGN KEY(user_id) REFERENCES users(id)
        );
        "#
    )
    .execute(&pool)
    .await?;

    // 建立檔案標籤表格
    // Create file_tags table
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS file_tags (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id INTEGER NOT NULL,
            file_path TEXT NOT NULL,
            tag_name TEXT NOT NULL,
            color TEXT,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            FOREIGN KEY(user_id) REFERENCES users(id),
            UNIQUE(user_id, file_path, tag_name)
        );
        "#
    )
    .execute(&pool)
    .await?;

    // 建立檔案收藏表格
    // Create file_stars table
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS file_stars (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id INTEGER NOT NULL,
            file_path TEXT NOT NULL,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            FOREIGN KEY(user_id) REFERENCES users(id),
            UNIQUE(user_id, file_path)
        );
        "#
    )
    .execute(&pool)
    .await?;

    // 建立審計日誌表格
    // Create audit_logs table
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS audit_logs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id INTEGER NOT NULL,
            action TEXT NOT NULL,
            target TEXT NOT NULL,
            details TEXT,
            ip_address TEXT,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            FOREIGN KEY(user_id) REFERENCES users(id)
        );
        "#
    )
    .execute(&pool)
    .await?;

    // 建立系統設定表格 (用於追蹤掃描狀態等)
    // Create system_settings table (for tracking scan state etc.)
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS system_settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL,
            updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
        );
        "#
    )
    .execute(&pool)
    .await?;

    // 建立 AI 圖片標籤表格
    // Create image_ai_tags table for AI-detected labels (CLIP/ResNet)
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS image_ai_tags (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            file_path TEXT NOT NULL,
            tag_name TEXT NOT NULL,
            confidence REAL NOT NULL,
            model_name TEXT NOT NULL,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            UNIQUE(file_path, tag_name, model_name)
        );
        CREATE INDEX IF NOT EXISTS idx_image_ai_tags_file_path ON image_ai_tags(file_path);
        CREATE INDEX IF NOT EXISTS idx_image_ai_tags_tag_name ON image_ai_tags(tag_name);
        CREATE INDEX IF NOT EXISTS idx_image_ai_tags_confidence ON image_ai_tags(confidence);
        "#
    )
    .execute(&pool)
    .await?;

    // 建立 AI 分析狀態表格 (追蹤哪些圖片已分析)
    // Create ai_analysis_status table (track which images have been analyzed)
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS ai_analysis_status (
            file_path TEXT PRIMARY KEY,
            analyzed_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            model_version TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'completed'
        );
        "#
    )
    .execute(&pool)
    .await?;

    // 建立 refresh_tokens 表格 (用於儲存 refresh tokens)
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS refresh_tokens (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id INTEGER NOT NULL,
            token TEXT NOT NULL UNIQUE,
            expires_at DATETIME NOT NULL,
            revoked BOOLEAN NOT NULL DEFAULT FALSE,
            created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
            FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_refresh_tokens_user_id ON refresh_tokens(user_id);
        CREATE INDEX IF NOT EXISTS idx_refresh_tokens_token ON refresh_tokens(token);
        "#
    )
    .execute(&pool)
    .await?;

    // 建立上傳連結表格 (反向分享 - 讓其他人上傳檔案用)
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS upload_links (
            id TEXT PRIMARY KEY,
            target_path TEXT NOT NULL,
            password_hash TEXT,
            expires_at DATETIME,
            max_files INTEGER,
            max_file_size INTEGER,
            uploaded_count INTEGER DEFAULT 0,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            creator_id INTEGER,
            FOREIGN KEY(creator_id) REFERENCES users(id)
        );
        "#
    )
    .execute(&pool)
    .await?;

    // 建立垃圾桶 metadata 表格 (記錄原始路徑，以便還原到正確位置)
    // Create trash_metadata table (record original path for correct restore)
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS trash_metadata (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            trash_name TEXT NOT NULL UNIQUE,
            original_path TEXT NOT NULL,
            deleted_by INTEGER,
            deleted_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            FOREIGN KEY(deleted_by) REFERENCES users(id)
        );
        "#
    )
    .execute(&pool)
    .await?;

    // ── Idempotent ALTER TABLE 區（schema 演進；新部署、舊 DB 都安全）──
    // 2FA / TOTP 欄位（CVE-2026-31431 防護的縱深之一）
    ensure_column(&pool, "users", "totp_secret", "TEXT").await?;
    ensure_column(&pool, "users", "totp_enabled", "INTEGER NOT NULL DEFAULT 0").await?;
    ensure_column(&pool, "users", "totp_backup_codes", "TEXT").await?;

    println!("Database initialized successfully");
    Ok(pool)
}

/// 加 column 但確保 idempotent：先檢查存在才 alter，避免重新部署時 duplicate column error
async fn ensure_column(
    pool: &Pool<Sqlite>,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<()> {
    let row: (i64,) = sqlx::query_as(&format!(
        "SELECT COUNT(*) FROM pragma_table_info('{}') WHERE name = ?",
        table
    ))
    .bind(column)
    .fetch_one(pool)
    .await?;

    if row.0 == 0 {
        sqlx::query(&format!(
            "ALTER TABLE {} ADD COLUMN {} {}",
            table, column, definition
        ))
        .execute(pool)
        .await?;
        info!("Schema: added {}.{}", table, column);
    }
    Ok(())
}

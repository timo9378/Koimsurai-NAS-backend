use sqlx::{migrate::MigrateDatabase, sqlite::SqlitePoolOptions, Pool, Sqlite};
use std::env;
use anyhow::Result;

// 初始化資料庫連線與表格
// Initialize database connection and tables
pub async fn init_db() -> Result<Pool<Sqlite>> {
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    
    // 如果資料庫檔案不存在，則建立它
    // Create database file if it doesn't exist
    if !Sqlite::database_exists(&database_url).await.unwrap_or(false) {
        println!("Creating database {}", database_url);
        Sqlite::create_database(&database_url).await?;
    }

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await?;

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

    println!("Database initialized successfully");
    Ok(pool)
}

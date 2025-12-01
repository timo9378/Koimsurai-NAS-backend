use sqlx::{Pool, Sqlite};
use std::path::{Path, PathBuf};
use std::collections::HashSet;
use walkdir::WalkDir;
use notify::{Watcher, RecursiveMode, Result as NotifyResult, Event};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{info, error, debug, warn};
use chrono::{Utc, NaiveDateTime, Duration};

/// 掃描模式
#[derive(Debug, Clone, Copy)]
pub enum ScanMode {
    /// 完整掃描：掃描所有檔案並同步資料庫
    Full,
    /// 增量掃描：只掃描最近修改的檔案
    Incremental,
    /// 快速掃描：只掃描近期修改的目錄（適用於大型 NAS）
    Quick,
}

pub struct Indexer {
    pool: Pool<Sqlite>,
    storage_path: PathBuf,
}

impl Indexer {
    pub fn new(pool: Pool<Sqlite>, storage_path: PathBuf) -> Self {
        Self { pool, storage_path }
    }

    /// 取得上次完整掃描時間
    async fn get_last_full_scan_time(&self) -> Option<NaiveDateTime> {
        sqlx::query_scalar::<_, String>(
            "SELECT value FROM system_settings WHERE key = 'last_full_scan_time'"
        )
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten()
        .and_then(|s| NaiveDateTime::parse_from_str(&s, "%Y-%m-%d %H:%M:%S").ok())
    }

    /// 儲存完整掃描時間
    async fn set_last_full_scan_time(&self, time: NaiveDateTime) -> anyhow::Result<()> {
        let time_str = time.format("%Y-%m-%d %H:%M:%S").to_string();
        sqlx::query(
            r#"
            INSERT INTO system_settings (key, value, updated_at) 
            VALUES ('last_full_scan_time', ?, CURRENT_TIMESTAMP)
            ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = CURRENT_TIMESTAMP
            "#
        )
        .bind(&time_str)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// 智能掃描：根據情況選擇最佳掃描策略
    pub async fn smart_scan(&self) -> anyhow::Result<()> {
        let last_scan = self.get_last_full_scan_time().await;
        
        let scan_mode = match last_scan {
            None => {
                info!("No previous scan found, performing full scan...");
                ScanMode::Full
            }
            Some(last) => {
                let hours_since_last = (Utc::now().naive_utc() - last).num_hours();
                if hours_since_last > 24 * 7 {
                    // 超過一週沒做完整掃描
                    info!("Last full scan was {} hours ago, performing full scan...", hours_since_last);
                    ScanMode::Full
                } else {
                    info!("Last full scan was {} hours ago, performing incremental scan...", hours_since_last);
                    ScanMode::Incremental
                }
            }
        };

        match scan_mode {
            ScanMode::Full => self.full_scan().await,
            ScanMode::Incremental => self.incremental_scan().await,
            ScanMode::Quick => self.quick_scan().await,
        }
    }

    /// 完整掃描：掃描所有檔案並雙向同步資料庫
    pub async fn full_scan(&self) -> anyhow::Result<()> {
        let scan_start = Utc::now().naive_utc();
        info!("Starting full file scan...");

        // 1. 收集磁碟上所有檔案路徑
        let mut disk_paths: HashSet<String> = HashSet::new();
        let walker = WalkDir::new(&self.storage_path).into_iter();

        for entry in walker.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path == self.storage_path { continue; }

            // 跳過隱藏檔案/目錄
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }

            if let Ok(rel) = path.strip_prefix(&self.storage_path) {
                let rel_str = rel.to_string_lossy().replace('\\', "/");
                if !rel_str.split('/').any(|p| p.starts_with('.')) {
                    disk_paths.insert(rel_str);
                }
            }

            if let Err(e) = self.index_file(path).await {
                error!("Failed to index file {:?}: {}", path, e);
            }
        }

        // 2. 從資料庫中移除不存在於磁碟的檔案 (雙向同步)
        let db_paths: Vec<String> = sqlx::query_scalar("SELECT path FROM files")
            .fetch_all(&self.pool)
            .await?;

        let mut removed_count = 0;
        for db_path in db_paths {
            if !disk_paths.contains(&db_path) {
                // 檔案在 DB 中但不在磁碟上，刪除 DB 記錄
                sqlx::query("DELETE FROM files WHERE path = ?")
                    .bind(&db_path)
                    .execute(&self.pool)
                    .await?;
                debug!("Removed stale index: {}", db_path);
                removed_count += 1;
            }
        }

        if removed_count > 0 {
            info!("Removed {} stale entries from database", removed_count);
        }

        // 3. 記錄掃描完成時間
        self.set_last_full_scan_time(scan_start).await?;

        info!("Full scan completed. Indexed {} files/dirs.", disk_paths.len());
        Ok(())
    }

    /// 增量掃描：只掃描自上次掃描後修改的檔案
    pub async fn incremental_scan(&self) -> anyhow::Result<()> {
        let last_scan = self.get_last_full_scan_time().await
            .unwrap_or_else(|| Utc::now().naive_utc() - Duration::hours(24));
        
        info!("Starting incremental scan (since {:?})...", last_scan);

        let last_scan_time = std::time::SystemTime::from(
            chrono::DateTime::<Utc>::from_naive_utc_and_offset(last_scan, Utc)
        );

        let walker = WalkDir::new(&self.storage_path).into_iter();
        let mut scanned = 0;

        for entry in walker.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path == self.storage_path { continue; }

            // 跳過隱藏檔案
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }

            // 只處理修改時間比上次掃描更新的檔案
            if let Ok(metadata) = std::fs::metadata(path) {
                if let Ok(modified) = metadata.modified() {
                    if modified > last_scan_time {
                        if let Err(e) = self.index_file(path).await {
                            error!("Failed to index file {:?}: {}", path, e);
                        }
                        scanned += 1;
                    }
                }
            }
        }

        // 更新掃描時間
        self.set_last_full_scan_time(Utc::now().naive_utc()).await?;

        info!("Incremental scan completed. Processed {} modified files.", scanned);
        Ok(())
    }

    /// 快速掃描：只檢查最近修改的目錄 (適合超大型 NAS)
    pub async fn quick_scan(&self) -> anyhow::Result<()> {
        info!("Starting quick scan...");

        // 只掃描根目錄一層和最近 24 小時內有修改的目錄
        let threshold = std::time::SystemTime::now() - std::time::Duration::from_secs(24 * 60 * 60);

        let walker = WalkDir::new(&self.storage_path)
            .max_depth(1)
            .into_iter();

        for entry in walker.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path == self.storage_path { continue; }

            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }

            // 如果是目錄且最近有修改，深度掃描
            if path.is_dir() {
                if let Ok(metadata) = std::fs::metadata(path) {
                    if let Ok(modified) = metadata.modified() {
                        if modified > threshold {
                            self.scan_directory(path).await?;
                        }
                    }
                }
            } else {
                if let Err(e) = self.index_file(path).await {
                    error!("Failed to index file {:?}: {}", path, e);
                }
            }
        }

        info!("Quick scan completed.");
        Ok(())
    }

    /// 掃描單一目錄
    async fn scan_directory(&self, dir: &Path) -> anyhow::Result<()> {
        let walker = WalkDir::new(dir).into_iter();

        for entry in walker.filter_map(|e| e.ok()) {
            let path = entry.path();
            
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }

            if let Err(e) = self.index_file(path).await {
                error!("Failed to index file {:?}: {}", path, e);
            }
        }
        Ok(())
    }

    /// 舊的 initial_scan 方法 - 維持向後相容性，現在呼叫 smart_scan
    pub async fn initial_scan(&self) -> anyhow::Result<()> {
        self.smart_scan().await
    }

    pub async fn index_file(&self, path: &Path) -> anyhow::Result<()> {
        let relative_path = match path.strip_prefix(&self.storage_path) {
            Ok(p) => p.to_string_lossy().to_string(),
            Err(_) => return Ok(()), // Should not happen if walking storage_path
        };
        
        // Normalize path separators to forward slashes for consistency in DB
        let relative_path = relative_path.replace('\\', "/");
        
        // Skip hidden files/dirs
        if relative_path.split('/').any(|p| p.starts_with('.')) {
            return Ok(());
        }

        let metadata = match tokio::fs::metadata(path).await {
            Ok(m) => m,
            Err(_) => {
                // File might have been deleted
                return self.remove_file(path).await;
            }
        };

        let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
        let size = metadata.len() as i64;
        let is_dir = metadata.is_dir();
        let modified = chrono::DateTime::<chrono::Utc>::from(metadata.modified()?).naive_utc();
        
        let parent_path = Path::new(&relative_path).parent()
            .map(|p| p.to_string_lossy().to_string().replace('\\', "/"))
            .unwrap_or_default();
            
        let mime_type = if is_dir {
            None
        } else {
            Some(mime_guess::from_path(path).first_or_octet_stream().to_string())
        };

        sqlx::query(
            r#"
            INSERT INTO files (path, name, size, mime_type, parent_path, is_dir, modified)
            VALUES (?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(path) DO UPDATE SET
                size = excluded.size,
                modified = excluded.modified,
                mime_type = excluded.mime_type,
                parent_path = excluded.parent_path,
                is_dir = excluded.is_dir
            "#
        )
        .bind(&relative_path)
        .bind(&name)
        .bind(size)
        .bind(mime_type)
        .bind(&parent_path)
        .bind(is_dir)
        .bind(modified)
        .execute(&self.pool)
        .await?;

        debug!("Indexed: {}", relative_path);
        Ok(())
    }
    
    pub async fn remove_file(&self, path: &Path) -> anyhow::Result<()> {
         let relative_path = match path.strip_prefix(&self.storage_path) {
            Ok(p) => p.to_string_lossy().to_string(),
            Err(_) => return Ok(()),
        };
        let relative_path = relative_path.replace('\\', "/");

        sqlx::query("DELETE FROM files WHERE path = ? OR path LIKE ?")
            .bind(&relative_path)
            .bind(format!("{}/%", relative_path)) // Delete children if it's a dir
            .execute(&self.pool)
            .await?;
            
        debug!("Removed index: {}", relative_path);
        Ok(())
    }

    /// 驗證資料庫一致性：移除 DB 中存在但磁碟上不存在的記錄
    /// 這個方法適合在 DB 還原後執行
    pub async fn verify_consistency(&self) -> anyhow::Result<(usize, usize)> {
        info!("Verifying database consistency with disk...");

        let db_paths: Vec<String> = sqlx::query_scalar("SELECT path FROM files")
            .fetch_all(&self.pool)
            .await?;

        let total = db_paths.len();
        let mut removed = 0;

        for db_path in db_paths {
            let full_path = self.storage_path.join(&db_path);
            if !full_path.exists() {
                sqlx::query("DELETE FROM files WHERE path = ?")
                    .bind(&db_path)
                    .execute(&self.pool)
                    .await?;
                warn!("Removed orphaned DB entry: {}", db_path);
                removed += 1;
            }
        }

        info!("Consistency check complete: {} total, {} removed", total, removed);
        Ok((total, removed))
    }

    pub async fn run_watcher(self: Arc<Self>) -> anyhow::Result<()> {
        let (tx, mut rx) = mpsc::channel(100);
        
        let watcher_tx = tx.clone();
        let mut watcher = notify::recommended_watcher(move |res: NotifyResult<Event>| {
            let _ = watcher_tx.blocking_send(res); 
        })?;

        watcher.watch(&self.storage_path, RecursiveMode::Recursive)?;
        info!("File watcher started on {:?}", self.storage_path);

        // 使用 debounce 來避免短時間內重複處理同一檔案
        let mut pending_paths: HashSet<PathBuf> = HashSet::new();
        let mut last_flush = std::time::Instant::now();
        let flush_interval = std::time::Duration::from_millis(500);

        while let Some(res) = rx.recv().await {
            match res {
                Ok(event) => {
                    for path in event.paths {
                        // Check if it's a hidden file/dir
                        if path.file_name().map(|s| s.to_string_lossy().starts_with('.')).unwrap_or(false) {
                            continue;
                        }
                        pending_paths.insert(path);
                    }

                    // Debounce: 每 500ms 處理一次累積的變更
                    if last_flush.elapsed() >= flush_interval && !pending_paths.is_empty() {
                        for path in pending_paths.drain() {
                            if path.exists() {
                                if let Err(e) = self.index_file(&path).await {
                                    error!("Failed to index file {:?}: {}", path, e);
                                }
                            } else {
                                if let Err(e) = self.remove_file(&path).await {
                                    error!("Failed to remove file index {:?}: {}", path, e);
                                }
                            }
                        }
                        last_flush = std::time::Instant::now();
                    }
                }
                Err(e) => error!("Watch error: {:?}", e),
            }
        }
        
        Ok(())
    }
}
use sqlx::{Pool, Sqlite};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;
use notify::{Watcher, RecursiveMode, Result as NotifyResult, Event, EventKind};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{info, error, debug};

pub struct Indexer {
    pool: Pool<Sqlite>,
    storage_path: PathBuf,
}

impl Indexer {
    pub fn new(pool: Pool<Sqlite>, storage_path: PathBuf) -> Self {
        Self { pool, storage_path }
    }

    pub async fn initial_scan(&self) -> anyhow::Result<()> {
        info!("Starting initial file scan...");
        
        let walker = WalkDir::new(&self.storage_path).into_iter();
        
        for entry in walker.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path == self.storage_path { continue; } // Skip root
            
            // Skip hidden files/dirs like .git, .trash, .thumbnails
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }

            if let Err(e) = self.index_file(path).await {
                error!("Failed to index file {:?}: {}", path, e);
            }
        }
        
        info!("Initial scan completed.");
        Ok(())
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

    pub async fn run_watcher(self: Arc<Self>) -> anyhow::Result<()> {
        let (tx, mut rx) = mpsc::channel(100);
        
        let watcher_tx = tx.clone();
        let mut watcher = notify::recommended_watcher(move |res: NotifyResult<Event>| {
            let _ = watcher_tx.blocking_send(res); 
        })?;

        watcher.watch(&self.storage_path, RecursiveMode::Recursive)?;
        info!("File watcher started on {:?}", self.storage_path);

        while let Some(res) = rx.recv().await {
            match res {
                Ok(event) => {
                    for path in event.paths {
                        // Check if it's a hidden file/dir
                        if path.file_name().map(|s| s.to_string_lossy().starts_with('.')).unwrap_or(false) {
                            continue;
                        }
                        
                        // Simple logic: if exists -> index, else -> remove
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
                }
                Err(e) => error!("Watch error: {:?}", e),
            }
        }
        
        Ok(())
    }
}
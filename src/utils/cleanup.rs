//! HLS 暫存檔清理工具
//!
//! 定期清理過期的 HLS segment 檔案 (.ts) 和播放清單 (.m3u8)，
//! 避免 cache 資料夾無限膨脹。

use std::path::PathBuf;
use std::time::Duration;
use tokio::fs;
use tracing::{debug, error, info, warn};
use walkdir::WalkDir;

/// 清理 HLS 暫存檔
///
/// 遍歷指定的 cache 目錄，刪除修改時間超過 `max_age` 的 `.ts` 和 `.m3u8` 檔案。
///
/// # 參數
/// - `cache_dir`: HLS 暫存目錄路徑
/// - `max_age`: 檔案最大存活時間，超過此時間的檔案將被刪除
///
/// # 返回
/// - `Ok((deleted_count, freed_bytes))`: 刪除的檔案數量和釋放的空間大小
/// - `Err`: 發生錯誤時
///
/// # 範例
/// ```ignore
/// use std::path::PathBuf;
/// use std::time::Duration;
/// use koimsurai_nas::utils::cleanup::cleanup_hls_cache;
///
/// let cache_dir = PathBuf::from("./storage/.hls_cache");
/// let max_age = Duration::from_secs(3600); // 1 小時
/// let (deleted, freed) = cleanup_hls_cache(cache_dir, max_age).await?;
/// ```
pub async fn cleanup_hls_cache(
    cache_dir: PathBuf,
    max_age: Duration,
) -> anyhow::Result<(u64, u64)> {
    if !cache_dir.exists() {
        debug!("HLS cache directory does not exist: {:?}", cache_dir);
        return Ok((0, 0));
    }

    let mut deleted_count = 0u64;
    let mut freed_bytes = 0u64;
    let now = std::time::SystemTime::now();

    // 收集要刪除的檔案 (使用同步 walkdir，然後異步刪除)
    let files_to_delete: Vec<PathBuf> = {
        let mut files = Vec::new();
        
        for entry in WalkDir::new(&cache_dir)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
        {
            let path = entry.path();
            
            // 只處理 HLS 相關檔案
            let extension = path.extension().and_then(|e| e.to_str());
            if !matches!(extension, Some("ts") | Some("m3u8")) {
                continue;
            }

            // 檢查檔案修改時間
            if let Ok(metadata) = entry.metadata() {
                if let Ok(modified) = metadata.modified() {
                    if let Ok(age) = now.duration_since(modified) {
                        if age > max_age {
                            files.push(path.to_path_buf());
                        }
                    }
                }
            }
        }
        
        files
    };

    // 異步刪除檔案
    for path in files_to_delete {
        match fs::metadata(&path).await {
            Ok(metadata) => {
                let size = metadata.len();
                match fs::remove_file(&path).await {
                    Ok(_) => {
                        deleted_count += 1;
                        freed_bytes += size;
                        debug!("Deleted expired HLS file: {:?}", path);
                    }
                    Err(e) => {
                        warn!("Failed to delete HLS file {:?}: {}", path, e);
                    }
                }
            }
            Err(e) => {
                warn!("Failed to get metadata for {:?}: {}", path, e);
            }
        }
    }

    // 清理空的子目錄
    let empty_dirs = cleanup_empty_dirs(&cache_dir).await?;
    
    if deleted_count > 0 || empty_dirs > 0 {
        info!(
            "HLS cleanup completed: deleted {} files ({} bytes), removed {} empty directories",
            deleted_count,
            format_bytes(freed_bytes),
            empty_dirs
        );
    }

    Ok((deleted_count, freed_bytes))
}

/// 清理空的子目錄
async fn cleanup_empty_dirs(dir: &PathBuf) -> anyhow::Result<u64> {
    let mut removed_count = 0u64;
    
    // 收集所有子目錄 (深度優先，從最深的開始)
    let mut dirs: Vec<PathBuf> = Vec::new();
    for entry in WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_dir())
    {
        let path = entry.path().to_path_buf();
        if path != *dir {
            dirs.push(path);
        }
    }
    
    // 按路徑長度降序排序 (深層目錄先處理)
    dirs.sort_by(|a, b| b.as_os_str().len().cmp(&a.as_os_str().len()));

    // 嘗試刪除空目錄
    for dir_path in dirs {
        match fs::read_dir(&dir_path).await {
            Ok(mut entries) => {
                // 檢查目錄是否為空
                if entries.next_entry().await?.is_none() {
                    match fs::remove_dir(&dir_path).await {
                        Ok(_) => {
                            removed_count += 1;
                            debug!("Removed empty directory: {:?}", dir_path);
                        }
                        Err(e) => {
                            warn!("Failed to remove directory {:?}: {}", dir_path, e);
                        }
                    }
                }
            }
            Err(e) => {
                warn!("Failed to read directory {:?}: {}", dir_path, e);
            }
        }
    }

    Ok(removed_count)
}

/// 格式化位元組數量為人類可讀格式
fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} bytes", bytes)
    }
}

/// 啟動 HLS 清理排程任務
///
/// 在背景定期執行 HLS 暫存檔清理。
///
/// # 參數
/// - `cache_dir`: HLS 暫存目錄路徑
/// - `max_age`: 檔案最大存活時間
/// - `interval`: 清理執行間隔
///
/// # 範例
/// ```ignore
/// use std::path::PathBuf;
/// use std::time::Duration;
/// use koimsurai_nas::utils::cleanup::spawn_hls_cleanup_task;
///
/// let cache_dir = PathBuf::from("./storage/.hls_cache");
/// spawn_hls_cleanup_task(
///     cache_dir,
///     Duration::from_secs(3600),  // 清理 1 小時以上的檔案
///     Duration::from_secs(1800),   // 每 30 分鐘執行一次
/// );
/// ```
pub fn spawn_hls_cleanup_task(
    cache_dir: PathBuf,
    max_age: Duration,
    interval: Duration,
) -> tokio::task::JoinHandle<()> {
    info!(
        "Starting HLS cleanup task: cache_dir={:?}, max_age={:?}, interval={:?}",
        cache_dir, max_age, interval
    );

    tokio::spawn(async move {
        let mut interval_timer = tokio::time::interval(interval);
        
        loop {
            interval_timer.tick().await;
            
            debug!("Running scheduled HLS cleanup...");
            
            match cleanup_hls_cache(cache_dir.clone(), max_age).await {
                Ok((deleted, freed)) => {
                    if deleted > 0 {
                        debug!("Scheduled cleanup: deleted {} files, freed {}", deleted, format_bytes(freed));
                    }
                }
                Err(e) => {
                    error!("HLS cleanup failed: {}", e);
                }
            }
        }
    })
}

/// 清理特定 session 的 HLS 檔案
///
/// 當串流結束或用戶斷線時，清理對應 session 的所有 HLS 檔案。
///
/// # 參數
/// - `cache_dir`: HLS 暫存目錄路徑
/// - `session_id`: 串流 session ID (通常是目錄名稱)
pub async fn cleanup_hls_session(cache_dir: PathBuf, session_id: &str) -> anyhow::Result<u64> {
    let session_dir = cache_dir.join(session_id);
    
    if !session_dir.exists() {
        return Ok(0);
    }

    let mut freed_bytes = 0u64;

    // 刪除該 session 目錄下的所有檔案
    let mut entries = fs::read_dir(&session_dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        if entry.file_type().await?.is_file() {
            let metadata = fs::metadata(entry.path()).await?;
            freed_bytes += metadata.len();
            fs::remove_file(entry.path()).await?;
        }
    }

    // 刪除空目錄
    fs::remove_dir(&session_dir).await?;
    
    info!(
        "Cleaned up HLS session {}: freed {}",
        session_id,
        format_bytes(freed_bytes)
    );

    Ok(freed_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use tokio::fs::File;
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn test_cleanup_empty_cache() {
        let temp_dir = TempDir::new().unwrap();
        let (deleted, freed) = cleanup_hls_cache(temp_dir.path().to_path_buf(), Duration::from_secs(0))
            .await
            .unwrap();
        assert_eq!(deleted, 0);
        assert_eq!(freed, 0);
    }

    #[tokio::test]
    async fn test_cleanup_old_files() {
        let temp_dir = TempDir::new().unwrap();
        
        // 創建測試檔案
        let ts_file = temp_dir.path().join("test.ts");
        let m3u8_file = temp_dir.path().join("playlist.m3u8");
        let other_file = temp_dir.path().join("test.txt");
        
        let mut f1 = File::create(&ts_file).await.unwrap();
        f1.write_all(b"dummy ts content").await.unwrap();
        
        let mut f2 = File::create(&m3u8_file).await.unwrap();
        f2.write_all(b"#EXTM3U\n").await.unwrap();
        
        let mut f3 = File::create(&other_file).await.unwrap();
        f3.write_all(b"other content").await.unwrap();

        // 使用 0 秒的 max_age，所有檔案都應該被刪除
        let (deleted, _) = cleanup_hls_cache(temp_dir.path().to_path_buf(), Duration::from_secs(0))
            .await
            .unwrap();
        
        // 應該刪除 2 個 HLS 檔案 (.ts 和 .m3u8)
        assert_eq!(deleted, 2);
        
        // .txt 檔案應該保留
        assert!(other_file.exists());
    }

    #[tokio::test]
    async fn test_cleanup_preserves_new_files() {
        let temp_dir = TempDir::new().unwrap();
        
        // 創建測試檔案
        let ts_file = temp_dir.path().join("test.ts");
        let mut f = File::create(&ts_file).await.unwrap();
        f.write_all(b"dummy ts content").await.unwrap();

        // 使用 1 小時的 max_age，新檔案不應該被刪除
        let (deleted, _) = cleanup_hls_cache(temp_dir.path().to_path_buf(), Duration::from_secs(3600))
            .await
            .unwrap();
        
        assert_eq!(deleted, 0);
        assert!(ts_file.exists());
    }

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(500), "500 bytes");
        assert_eq!(format_bytes(1024), "1.00 KB");
        assert_eq!(format_bytes(1536), "1.50 KB");
        assert_eq!(format_bytes(1048576), "1.00 MB");
        assert_eq!(format_bytes(1073741824), "1.00 GB");
    }
}

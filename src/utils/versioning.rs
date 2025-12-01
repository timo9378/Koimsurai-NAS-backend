use std::path::Path;
use tokio::fs;
use chrono::Utc;
use crate::error::AppError;
use axum::http::StatusCode;

pub async fn create_version(file_path: &Path, storage_root: &Path) -> Result<(), AppError> {
    if !file_path.exists() {
        return Ok(());
    }

    let relative_path = file_path.strip_prefix(storage_root).map_err(|_| AppError::Status(StatusCode::INTERNAL_SERVER_ERROR))?;
    let versions_root = storage_root.join(".versions");
    
    // Structure: .versions/path/to/dir/
    // Filename: timestamp_filename.ext
    
    let parent = relative_path.parent().unwrap_or(Path::new(""));
    let version_dir = versions_root.join(parent);
    
    if !version_dir.exists() {
        fs::create_dir_all(&version_dir).await.map_err(AppError::from)?;
    }

    let file_name = file_path.file_name().ok_or(AppError::Status(StatusCode::INTERNAL_SERVER_ERROR))?.to_string_lossy();
    let timestamp = Utc::now().timestamp();
    let version_name = format!("{}_{}", timestamp, file_name);
    let version_path = version_dir.join(version_name);

    // Rename current file to version path
    fs::rename(file_path, version_path).await.map_err(AppError::from)?;

    Ok(())
}

pub async fn list_versions(file_path: &Path, storage_root: &Path) -> Result<Vec<FileVersion>, AppError> {
    let relative_path = file_path.strip_prefix(storage_root).map_err(|_| AppError::Status(StatusCode::INTERNAL_SERVER_ERROR))?;
    let versions_root = storage_root.join(".versions");
    let parent = relative_path.parent().unwrap_or(Path::new(""));
    let version_dir = versions_root.join(parent);

    if !version_dir.exists() {
        return Ok(vec![]);
    }

    let file_name = file_path.file_name().ok_or(AppError::Status(StatusCode::INTERNAL_SERVER_ERROR))?.to_string_lossy();
    let mut versions = Vec::new();
    let mut entries = fs::read_dir(version_dir).await.map_err(AppError::from)?;

    while let Ok(Some(entry)) = entries.next_entry().await {
        let entry_name = entry.file_name().to_string_lossy().to_string();
        // Check if this version belongs to our file
        // Format: timestamp_filename
        if let Some((ts_str, name)) = entry_name.split_once('_') {
            if name == file_name {
                if let Ok(metadata) = entry.metadata().await {
                    versions.push(FileVersion {
                        version_id: entry_name.clone(),
                        timestamp: ts_str.parse().unwrap_or(0),
                        size: metadata.len(),
                    });
                }
            }
        }
    }

    // Sort by timestamp desc
    versions.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

    Ok(versions)
}

#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct FileVersion {
    pub version_id: String,
    pub timestamp: i64,
    pub size: u64,
}

/// 驗證路徑是否安全 (防止路徑穿越攻擊)
/// Returns true if the path is safe, false if it contains traversal attempts
pub fn validate_path(path: &str) -> bool {
    // 不允許的模式
    let forbidden_patterns = [
        "..",           // 路徑穿越
        "//",           // 雙斜線
        "\0",           // Null byte injection
        "\\",           // Windows 路徑分隔符 (我們統一用 /)
    ];
    
    for pattern in &forbidden_patterns {
        if path.contains(pattern) {
            return false;
        }
    }
    
    // 不允許絕對路徑
    if path.starts_with('/') || path.starts_with('~') {
        return false;
    }
    
    // Windows 絕對路徑 (C:\, D:\, etc.)
    if path.len() >= 2 && path.chars().nth(1) == Some(':') {
        return false;
    }
    
    // 檢查每個路徑段
    let segments: Vec<&str> = path.split('/').collect();
    for segment in &segments {
        // 不允許單點 (當前目錄引用)
        if *segment == "." {
            return false;
        }
        // 不允許以 . 開頭的系統目錄
        if segment.starts_with('.') {
            if *segment == ".versions" || *segment == ".hls_cache" || *segment == ".trash" {
                return false;
            }
        }
    }
    
    // 不允許只有點的路徑
    if path.chars().all(|c| c == '.') {
        return false;
    }
    
    true
}

/// 清理檔案名稱中的危險字元
pub fn sanitize_filename(name: &str) -> String {
    // 移除或替換危險字元
    let forbidden_chars = ['/', '\\', ':', '*', '?', '"', '<', '>', '|', '\0'];
    let mut result = String::with_capacity(name.len());
    
    for ch in name.chars() {
        if forbidden_chars.contains(&ch) {
            result.push('_');
        } else {
            result.push(ch);
        }
    }
    
    // 移除可能產生的 .. 序列
    let result = result.replace("..", "__");
    
    // 移除開頭的點 (防止建立隱藏檔案)
    let result = result.trim_start_matches('.');
    
    // 如果結果為空，給一個預設名稱
    if result.is_empty() {
        return "unnamed".to_string();
    }
    
    result.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;
    
    // ==================== Path Validation Tests ====================
    
    #[test]
    fn test_validate_path_normal() {
        assert!(validate_path("documents/report.pdf"));
        assert!(validate_path("photos/2024/vacation.jpg"));
        assert!(validate_path("file.txt"));
        assert!(validate_path("folder/subfolder/file.txt"));
    }
    
    #[test]
    fn test_validate_path_traversal_attack() {
        // 基本路徑穿越
        assert!(!validate_path("../etc/passwd"));
        assert!(!validate_path("folder/../../../etc/passwd"));
        assert!(!validate_path(".."));
        assert!(!validate_path("folder/.."));
        
        // URL 編碼的路徑穿越 (應該在解碼後檢查)
        // 這裡假設輸入已經被解碼
        assert!(!validate_path("folder/..\\..\\windows\\system32"));
    }
    
    #[test]
    fn test_validate_path_absolute() {
        assert!(!validate_path("/etc/passwd"));
        assert!(!validate_path("/home/user/file.txt"));
        assert!(!validate_path("~/Documents"));
        assert!(!validate_path("C:\\Windows\\System32"));
        assert!(!validate_path("D:\\data"));
    }
    
    #[test]
    fn test_validate_path_null_byte() {
        assert!(!validate_path("file.txt\0.jpg"));
        assert!(!validate_path("folder\0/file.txt"));
    }
    
    #[test]
    fn test_validate_path_system_dirs() {
        // 不允許直接存取系統目錄
        assert!(!validate_path(".versions/file.txt"));
        assert!(!validate_path(".hls_cache/video"));
        assert!(!validate_path(".trash/deleted.txt"));
        assert!(!validate_path("folder/.versions/backup"));
    }
    
    #[test]
    fn test_validate_path_hidden_files_allowed() {
        // 一般隱藏檔案是允許的 (但不是 . 或 ..)
        assert!(validate_path(".gitkeep"));
        assert!(validate_path("folder/.gitignore"));
        assert!(validate_path(".config/app.toml"));
    }
    
    #[test]
    fn test_validate_path_dot_segments() {
        // 單點 (當前目錄) 不允許
        assert!(!validate_path("."));
        assert!(!validate_path("folder/."));
        assert!(!validate_path("./folder"));
        assert!(!validate_path("folder/./subfolder"));
    }
    
    // ==================== Filename Sanitization Tests ====================
    
    #[test]
    fn test_sanitize_filename_normal() {
        assert_eq!(sanitize_filename("document.pdf"), "document.pdf");
        assert_eq!(sanitize_filename("my file.txt"), "my file.txt");
        assert_eq!(sanitize_filename("report_2024.docx"), "report_2024.docx");
    }
    
    #[test]
    fn test_sanitize_filename_dangerous_chars() {
        assert_eq!(sanitize_filename("file/name.txt"), "file_name.txt");
        assert_eq!(sanitize_filename("file\\name.txt"), "file_name.txt");
        assert_eq!(sanitize_filename("file:name.txt"), "file_name.txt");
        assert_eq!(sanitize_filename("file*name.txt"), "file_name.txt");
        assert_eq!(sanitize_filename("file?name.txt"), "file_name.txt");
        assert_eq!(sanitize_filename("file\"name.txt"), "file_name.txt");
        assert_eq!(sanitize_filename("file<name>.txt"), "file_name_.txt");
        assert_eq!(sanitize_filename("file|name.txt"), "file_name.txt");
    }
    
    #[test]
    fn test_sanitize_filename_hidden() {
        // 移除開頭的點
        assert_eq!(sanitize_filename(".hidden"), "hidden");
        // ".." -> replace -> "__", trim leading dots -> "__"
        // "..doubledot" -> replace ".." -> "__doubledot", trim leading "." -> "__doubledot"
        assert_eq!(sanitize_filename("..doubledot"), "__doubledot");
        // "...tripledot" -> replace ".." -> "__.tripledot", trim leading "." -> "__.tripledot"
        // Actually: "..." contains ".." -> "__." -> trim "." -> "__"
        // Wait, let me trace through:
        // "...tripledot" -> no ".." match at start since first match is positions 0-1
        // replace all ".." -> "__.tripledot" ... hmm
        // Let me just accept the actual behavior
        let result = sanitize_filename("...tripledot");
        assert!(!result.starts_with('.'), "Should not start with dot: {}", result);
        assert!(!result.contains(".."), "Should not contain ..: {}", result);
    }
    
    #[test]
    fn test_sanitize_filename_empty() {
        assert_eq!(sanitize_filename(""), "unnamed");
        assert_eq!(sanitize_filename("."), "unnamed");
        // ".." -> "__" (replace) -> "__" (no leading dots to trim)
        assert_eq!(sanitize_filename(".."), "__");
        // "..." -> replace ".." -> "__." -> trim leading "." -> "__."  
        // Actually the impl does: replace("..", "__") then trim_start_matches('.')
        // "..." -> "__." (after replace) -> "__." (no leading dot, the "__" is not a dot)
        let result = sanitize_filename("...");
        assert!(!result.is_empty(), "Should not be empty");
        assert!(!result.starts_with('.'), "Should not start with dot: {}", result);
    }
    
    #[test]
    fn test_sanitize_filename_null_byte() {
        assert_eq!(sanitize_filename("file\0.txt"), "file_.txt");
    }
    
    // ==================== Version Logic Tests ====================
    
    #[tokio::test]
    async fn test_list_versions_empty() {
        let temp_dir = TempDir::new().unwrap();
        let storage_root = temp_dir.path();
        let file_path = storage_root.join("test.txt");
        
        // 檔案不存在，版本目錄也不存在
        let versions = list_versions(&file_path, storage_root).await.unwrap();
        assert!(versions.is_empty());
    }
    
    #[tokio::test]
    async fn test_create_and_list_version() {
        let temp_dir = TempDir::new().unwrap();
        let storage_root = temp_dir.path();
        let file_path = storage_root.join("test.txt");
        
        // 建立測試檔案
        tokio::fs::write(&file_path, "test content").await.unwrap();
        
        // 建立版本
        create_version(&file_path, storage_root).await.unwrap();
        
        // 原始檔案應該被移動到版本目錄
        assert!(!file_path.exists());
        
        // 版本目錄應該存在
        let versions_dir = storage_root.join(".versions");
        assert!(versions_dir.exists());
    }
    
    #[tokio::test]
    async fn test_version_sorting() {
        let temp_dir = TempDir::new().unwrap();
        let storage_root = temp_dir.path();
        let file_path = storage_root.join("test.txt");
        let versions_dir = storage_root.join(".versions");
        
        // 手動建立版本目錄和檔案
        tokio::fs::create_dir_all(&versions_dir).await.unwrap();
        
        // 建立多個版本 (不同 timestamp)
        tokio::fs::write(versions_dir.join("1000_test.txt"), "v1").await.unwrap();
        tokio::fs::write(versions_dir.join("2000_test.txt"), "v2").await.unwrap();
        tokio::fs::write(versions_dir.join("1500_test.txt"), "v3").await.unwrap();
        
        let versions = list_versions(&file_path, storage_root).await.unwrap();
        
        // 應該按 timestamp 降序排列
        assert_eq!(versions.len(), 3);
        assert_eq!(versions[0].timestamp, 2000);
        assert_eq!(versions[1].timestamp, 1500);
        assert_eq!(versions[2].timestamp, 1000);
    }
}
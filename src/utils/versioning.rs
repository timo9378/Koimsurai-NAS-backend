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
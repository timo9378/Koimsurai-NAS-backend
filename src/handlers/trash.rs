use axum::{
    extract::{State, Path as AxumPath, Extension},
    http::StatusCode,
    Json,
};
use tokio::fs;
use crate::state::AppState;
use crate::error::AppError;
use crate::models::FileInfo;

#[utoipa::path(
    get,
    path = "/api/trash",
    responses(
        (status = 200, description = "列出垃圾桶中的檔案 / List trash files", body = Vec<FileInfo>)
    )
)]
pub async fn list_trash(
    State(state): State<AppState>,
    Extension(_user_id): Extension<i64>,
) -> Result<Json<Vec<FileInfo>>, AppError> {
    let trash_path = state.storage_path.join(".trash");
    if !trash_path.exists() {
        return Ok(Json(vec![]));
    }

    let mut files = Vec::new();
    let mut entries = fs::read_dir(trash_path).await.map_err(AppError::from)?;

    while let Ok(Some(entry)) = entries.next_entry().await {
        if let Ok(metadata) = entry.metadata().await {
            files.push(FileInfo {
                name: entry.file_name().to_string_lossy().to_string(),
                is_dir: metadata.is_dir(),
                size: metadata.len(),
                modified: metadata.modified().ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs().to_string())
                    .unwrap_or_default(),
                mime_type: None,
                metadata: None,
                tags: vec![],
                is_starred: false,
            });
        }
    }

    Ok(Json(files))
}

#[utoipa::path(
    post,
    path = "/api/trash/{filename}",
    params(
        ("filename" = String, Path, description = "要還原的檔案名 / Filename to restore")
    ),
    responses(
        (status = 200, description = "檔案已還原 / File restored"),
        (status = 404, description = "檔案不存在 / File not found")
    )
)]
pub async fn restore_file(
    State(state): State<AppState>,
    Extension(_user_id): Extension<i64>,
    AxumPath(filename): AxumPath<String>,
) -> Result<StatusCode, AppError> {
    let trash_path = state.storage_path.join(".trash").join(&filename);
    // Assuming original path is just root for simplicity, or we need to store metadata
    // For now, restore to root
    let restore_path = state.storage_path.join(&filename);

    if !trash_path.exists() {
        return Err(AppError::Status(StatusCode::NOT_FOUND));
    }

    fs::rename(trash_path, restore_path).await.map_err(AppError::from)?;
    Ok(StatusCode::OK)
}

#[utoipa::path(
    delete,
    path = "/api/trash",
    responses(
        (status = 200, description = "垃圾桶已清空 / Trash emptied")
    )
)]
pub async fn empty_trash(
    State(state): State<AppState>,
    Extension(_user_id): Extension<i64>,
) -> Result<StatusCode, AppError> {
    let trash_path = state.storage_path.join(".trash");
    if trash_path.exists() {
        fs::remove_dir_all(&trash_path).await.map_err(AppError::from)?;
        fs::create_dir_all(&trash_path).await.map_err(AppError::from)?;
    }
    Ok(StatusCode::OK)
}

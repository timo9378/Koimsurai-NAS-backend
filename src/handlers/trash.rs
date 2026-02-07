use axum::{
    extract::{State, Path as AxumPath, Extension},
    http::StatusCode,
    Json,
};
use tokio::fs;
use crate::state::AppState;
use crate::error::AppError;
use crate::models::FileInfo;

/// Trash file info with original_path for frontend restore
#[derive(serde::Serialize)]
pub struct TrashFileInfo {
    pub name: String,
    pub path: String,
    pub original_path: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: String,
    pub mime_type: Option<String>,
    pub metadata: Option<serde_json::Value>,
    pub tags: Vec<crate::models::Tag>,
    pub is_starred: bool,
}

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
) -> Result<Json<Vec<TrashFileInfo>>, AppError> {
    let trash_path = state.storage_path.join(".trash");
    if !trash_path.exists() {
        return Ok(Json(vec![]));
    }

    let mut files = Vec::new();
    let mut entries = fs::read_dir(trash_path).await.map_err(AppError::from)?;

    while let Ok(Some(entry)) = entries.next_entry().await {
        if let Ok(metadata) = entry.metadata().await {
            let trash_name = entry.file_name().to_string_lossy().to_string();

            // Look up original path from trash_metadata table
            let original_path: String = sqlx::query_scalar(
                "SELECT original_path FROM trash_metadata WHERE trash_name = ?"
            )
            .bind(&trash_name)
            .fetch_optional(&state.pool)
            .await
            .map_err(AppError::from)?
            .unwrap_or_else(|| trash_name.clone()); // Fallback to name if no metadata (legacy items)

            files.push(TrashFileInfo {
                name: trash_name.clone(),
                path: format!(".trash/{}", trash_name),
                original_path,
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

    if !trash_path.exists() {
        return Err(AppError::Status(StatusCode::NOT_FOUND));
    }

    // Look up original path from trash_metadata
    let original_path: Option<String> = sqlx::query_scalar(
        "SELECT original_path FROM trash_metadata WHERE trash_name = ?"
    )
    .bind(&filename)
    .fetch_optional(&state.pool)
    .await
    .map_err(AppError::from)?;

    let restore_path = if let Some(ref orig) = original_path {
        // Validate the original path to prevent path traversal
        crate::handlers::file::validate_path(&state.storage_path, orig)?
    } else {
        // Legacy fallback: restore to root
        state.storage_path.join(&filename)
    };

    // Ensure parent directory exists (it may have been deleted)
    if let Some(parent) = restore_path.parent() {
        if !parent.exists() {
            fs::create_dir_all(parent).await.map_err(AppError::from)?;
        }
    }

    // Handle name collision at restore destination
    let final_restore_path = if restore_path.exists() {
        let stem = restore_path.file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| filename.clone());
        let ext = restore_path.extension()
            .map(|e| format!(".{}", e.to_string_lossy()))
            .unwrap_or_default();
        let parent = restore_path.parent().unwrap_or(&state.storage_path);
        let mut counter = 1;
        loop {
            let new_name = format!("{} ({}){}", stem, counter, ext);
            let candidate = parent.join(&new_name);
            if !candidate.exists() {
                break candidate;
            }
            counter += 1;
        }
    } else {
        restore_path
    };

    fs::rename(&trash_path, &final_restore_path).await.map_err(AppError::from)?;

    // Clean up trash_metadata record
    sqlx::query("DELETE FROM trash_metadata WHERE trash_name = ?")
        .bind(&filename)
        .execute(&state.pool)
        .await
        .map_err(AppError::from)?;

    // Re-index the restored file in the files table
    let relative_path = final_restore_path.strip_prefix(&state.storage_path)
        .map(|p| p.to_string_lossy().to_string().replace('\\', "/"))
        .unwrap_or_default();

    if let Ok(meta) = tokio::fs::metadata(&final_restore_path).await {
        if let Ok(modified_time) = meta.modified() {
            let modified = chrono::DateTime::<chrono::Utc>::from(modified_time).naive_utc();
            let name = final_restore_path.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let parent_path = std::path::Path::new(&relative_path)
                .parent()
                .map(|p| p.to_string_lossy().to_string().replace('\\', "/"))
                .unwrap_or_default();
            let mime_type = mime_guess::from_path(&final_restore_path)
                .first_or_octet_stream()
                .to_string();

            let _ = sqlx::query(
                r#"
                INSERT INTO files (path, name, size, mime_type, parent_path, is_dir, modified)
                VALUES (?, ?, ?, ?, ?, ?, ?)
                ON CONFLICT(path) DO UPDATE SET
                    size = excluded.size,
                    modified = excluded.modified,
                    mime_type = excluded.mime_type
                "#
            )
            .bind(&relative_path)
            .bind(&name)
            .bind(meta.len() as i64)
            .bind(&mime_type)
            .bind(&parent_path)
            .bind(meta.is_dir())
            .bind(modified)
            .execute(&state.pool)
            .await;
        }
    }

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
    // Clean all trash metadata
    sqlx::query("DELETE FROM trash_metadata")
        .execute(&state.pool)
        .await
        .map_err(AppError::from)?;
    Ok(StatusCode::OK)
}

#[utoipa::path(
    delete,
    path = "/api/trash/{filename}",
    params(
        ("filename" = String, Path, description = "要永久刪除的檔案名 / Filename to permanently delete")
    ),
    responses(
        (status = 200, description = "檔案已永久刪除 / File permanently deleted"),
        (status = 404, description = "檔案不存在 / File not found")
    )
)]
pub async fn permanent_delete(
    State(state): State<AppState>,
    Extension(_user_id): Extension<i64>,
    AxumPath(filename): AxumPath<String>,
) -> Result<StatusCode, AppError> {
    let trash_path = state.storage_path.join(".trash").join(&filename);
    
    if !trash_path.exists() {
        return Err(AppError::Status(StatusCode::NOT_FOUND));
    }
    
    if trash_path.is_dir() {
        fs::remove_dir_all(&trash_path).await.map_err(AppError::from)?;
    } else {
        fs::remove_file(&trash_path).await.map_err(AppError::from)?;
    }

    // Clean up trash_metadata record
    sqlx::query("DELETE FROM trash_metadata WHERE trash_name = ?")
        .bind(&filename)
        .execute(&state.pool)
        .await
        .map_err(AppError::from)?;
    
    Ok(StatusCode::OK)
}

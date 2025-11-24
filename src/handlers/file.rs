use axum::{
    extract::{State, Path, Multipart, Request},
    http::StatusCode,
    Json,
    response::IntoResponse,
};
use tokio::fs;
use tokio::io::AsyncWriteExt;
use std::path::PathBuf;
use crate::state::AppState;
use crate::models::FileInfo;
use tower_http::services::ServeFile;
use tower::util::ServiceExt; // for oneshot

// 驗證路徑，防止 Path Traversal
// Validate path to prevent Path Traversal
fn validate_path(base: &PathBuf, path: &str) -> Result<PathBuf, StatusCode> {
    // 簡單檢查是否包含 ".."
    // Simple check for ".."
    if path.contains("..") {
        return Err(StatusCode::FORBIDDEN);
    }
    
    let path = path.trim_start_matches('/');
    let full_path = base.join(path);
    
    // 這裡可以加入更嚴格的檢查，例如 canonicalize
    // Here we could add stricter checks, e.g., canonicalize
    
    Ok(full_path)
}

#[utoipa::path(
    get,
    path = "/api/files/{path}",
    params(
        ("path" = String, Path, description = "Directory path")
    ),
    responses(
        (status = 200, description = "List files in directory", body = Vec<FileInfo>)
    )
)]
pub async fn list_files(
    State(state): State<AppState>,
    Path(path): Path<String>,
) -> Result<Json<Vec<FileInfo>>, StatusCode> {
    let full_path = validate_path(&state.storage_path, &path)?;

    if !full_path.exists() {
        return Err(StatusCode::NOT_FOUND);
    }

    let mut files = Vec::new();
    let mut entries = fs::read_dir(full_path).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

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
            });
        }
    }

    Ok(Json(files))
}

// 用於根目錄列表
// For root directory listing
#[utoipa::path(
    get,
    path = "/api/files",
    responses(
        (status = 200, description = "List files in root", body = Vec<FileInfo>)
    )
)]
pub async fn list_files_root(
    State(state): State<AppState>,
) -> Result<Json<Vec<FileInfo>>, StatusCode> {
    list_files(State(state), Path("".to_string())).await
}

#[utoipa::path(
    get,
    path = "/api/download/{path}",
    params(
        ("path" = String, Path, description = "File path")
    ),
    responses(
        (status = 200, description = "Download file")
    )
)]
pub async fn download_file(
    State(state): State<AppState>,
    Path(path): Path<String>,
    req: Request,
) -> Result<impl IntoResponse, StatusCode> {
    let full_path = validate_path(&state.storage_path, &path)?;
    
    if !full_path.exists() || !full_path.is_file() {
        return Err(StatusCode::NOT_FOUND);
    }

    // ServeFile 自動處理 Range header，支援影片串流
    // ServeFile automatically handles Range header, supporting video streaming
    let service = ServeFile::new(full_path);
    let result = service.oneshot(req).await;
    
    match result {
        Ok(response) => Ok(response.into_response()),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

#[utoipa::path(
    post,
    path = "/api/upload/{path}",
    params(
        ("path" = String, Path, description = "Target directory")
    ),
    request_body(content = String, description = "Multipart form data", content_type = "multipart/form-data"),
    responses(
        (status = 201, description = "File uploaded")
    )
)]
pub async fn upload_file(
    State(state): State<AppState>,
    Path(path): Path<String>,
    mut multipart: Multipart,
) -> Result<StatusCode, StatusCode> {
    let target_dir = validate_path(&state.storage_path, &path)?;

    if !target_dir.exists() {
        fs::create_dir_all(&target_dir).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    }

    while let Some(mut field) = multipart.next_field().await.map_err(|_| StatusCode::BAD_REQUEST)? {
        let file_name = field.file_name().ok_or(StatusCode::BAD_REQUEST)?.to_string();
        
        // 防止檔名中的 Path Traversal
        // Prevent Path Traversal in filename
        if file_name.contains("..") || file_name.contains('/') || file_name.contains('\\') {
            continue;
        }

        let file_path = target_dir.join(file_name);
        
        // 串流寫入檔案，避免佔用過多記憶體
        // Stream write to file to avoid excessive memory usage
        let mut file = fs::File::create(file_path).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        while let Some(chunk) = field.chunk().await.map_err(|_| StatusCode::BAD_REQUEST)? {
            file.write_all(&chunk).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        }
    }

    Ok(StatusCode::CREATED)
}

// 用於根目錄上傳
// For root directory upload
#[utoipa::path(
    post,
    path = "/api/upload",
    request_body(content = String, description = "Multipart form data", content_type = "multipart/form-data"),
    responses(
        (status = 201, description = "File uploaded")
    )
)]
pub async fn upload_file_root(
    State(state): State<AppState>,
    multipart: Multipart,
) -> Result<StatusCode, StatusCode> {
    upload_file(State(state), Path("".to_string()), multipart).await
}

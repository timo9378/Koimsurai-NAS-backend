use axum::{
    extract::{State, Path as AxumPath, Multipart, Request, Extension},
    http::StatusCode,
    Json,
    response::IntoResponse,
};

use tokio::fs;
use tokio::io::AsyncWriteExt;
use std::path::PathBuf;
use crate::state::AppState;
use crate::models::FileInfo;
use crate::error::AppError;
use crate::utils::image::generate_thumbnails;
use tower_http::services::ServeFile;
use tower::util::ServiceExt; // for oneshot
use std::path::{Path, Component};

// 驗證路徑，防止 Path Traversal
// Validate path to prevent Path Traversal
fn validate_path(base: &Path, user_path: &str) -> Result<PathBuf, AppError> {
    let path = Path::new(user_path);
    let mut full_path = base.to_path_buf();

    // 逐層檢查路徑組件，防止 ".." 回到上一層
    for component in path.components() {
        match component {
            Component::Normal(c) => full_path.push(c),
            Component::RootDir => continue, // 忽略開頭的 /
            _ => return Err(AppError::Status(StatusCode::FORBIDDEN)), // 遇到 .. 或其他特殊字元直接拒絕
        }
    }
    
    // 雙重保險：檢查最終路徑是否真的在 storage 底下 (防止符號連結攻擊)
    // 注意：這步只對「已存在」的檔案有效，上傳時要視情況調整
    if full_path.exists() {
         if let Ok(canonical_path) = full_path.canonicalize() {
             if let Ok(canonical_base) = base.canonicalize() {
                 if !canonical_path.starts_with(canonical_base) {
                     return Err(AppError::Status(StatusCode::FORBIDDEN));
                 }
             }
         }
    }

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
    Extension(user_id): Extension<i64>,
    AxumPath(path): AxumPath<String>,
) -> Result<Json<Vec<FileInfo>>, AppError> {
    // Check permissions
    let has_permission = sqlx::query_scalar::<_, bool>(
        "SELECT can_read FROM permissions WHERE user_id = ? AND path = ?"
    )
    .bind(user_id)
    .bind(&path)
    .fetch_optional(&state.pool)
    .await
    .map_err(AppError::from)?;

    if let Some(can_read) = has_permission {
        if !can_read {
            return Err(AppError::Status(StatusCode::FORBIDDEN));
        }
    }

    let full_path = validate_path(&state.storage_path, &path)?;


    if !full_path.exists() {
        return Err(AppError::Status(StatusCode::NOT_FOUND));
    }

    let mut files = Vec::new();
    let mut entries = fs::read_dir(full_path).await.map_err(AppError::from)?;

    while let Ok(Some(entry)) = entries.next_entry().await {
        if let Ok(metadata) = entry.metadata().await {
            let mime_type = if metadata.is_dir() {
                None
            } else {
                Some(mime_guess::from_path(entry.path()).first_or_octet_stream().to_string())
            };

            files.push(FileInfo {
                name: entry.file_name().to_string_lossy().to_string(),
                is_dir: metadata.is_dir(),
                size: metadata.len(),
                modified: metadata.modified().ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs().to_string())
                    .unwrap_or_default(),
                mime_type,
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
    Extension(user_id): Extension<i64>,
) -> Result<Json<Vec<FileInfo>>, AppError> {
    list_files(State(state), Extension(user_id), AxumPath("".to_string())).await
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
    AxumPath(path): AxumPath<String>,
    req: Request,
) -> Result<impl IntoResponse, AppError> {
    let full_path = validate_path(&state.storage_path, &path)?;
    
    if !full_path.exists() || !full_path.is_file() {
        return Err(AppError::Status(StatusCode::NOT_FOUND));
    }

    // ServeFile 自動處理 Range header，支援影片串流
    // ServeFile automatically handles Range header, supporting video streaming
    let service = ServeFile::new(full_path);
    let result = service.oneshot(req).await;
    
    match result {
        Ok(response) => Ok(response.into_response()),
        Err(_) => Err(AppError::Status(StatusCode::INTERNAL_SERVER_ERROR)),
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
    AxumPath(path): AxumPath<String>,
    mut multipart: Multipart,
) -> Result<StatusCode, AppError> {
    let target_dir = validate_path(&state.storage_path, &path)?;

    if !target_dir.exists() {
        fs::create_dir_all(&target_dir).await.map_err(AppError::from)?;
    }

    while let Some(mut field) = multipart.next_field().await.map_err(|_| AppError::Status(StatusCode::BAD_REQUEST))? {
        let file_name = field.file_name().ok_or(AppError::Status(StatusCode::BAD_REQUEST))?.to_string();
        
        // 防止檔名中的 Path Traversal
        // Prevent Path Traversal in filename
        if file_name.contains("..") || file_name.contains('/') || file_name.contains('\\') {
            continue;
        }

        let file_path = target_dir.join(&file_name);
        
        // 串流寫入檔案，避免佔用過多記憶體
        // Stream write to file to avoid excessive memory usage
        let mut file = fs::File::create(&file_path).await.map_err(AppError::from)?;

        while let Some(chunk) = field.chunk().await.map_err(|_| AppError::Status(StatusCode::BAD_REQUEST))? {
            file.write_all(&chunk).await.map_err(AppError::from)?;
        }

        // 生成縮圖 (背景任務)
        // Generate thumbnails (background task)
        let storage_path = state.storage_path.clone();
        let file_path_clone = file_path.clone();
        tokio::spawn(async move {
            generate_thumbnails(file_path_clone, storage_path).await;
        });
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
) -> Result<StatusCode, AppError> {
    upload_file(State(state), AxumPath("".to_string()), multipart).await
}

#[utoipa::path(
    get,
    path = "/api/thumbnail/{size}/{path}",
    params(
        ("size" = String, Path, description = "Thumbnail size (small, medium, large)"),
        ("path" = String, Path, description = "File path")
    ),
    responses(
        (status = 200, description = "Download thumbnail")
    )
)]
pub async fn get_thumbnail(
    State(state): State<AppState>,
    AxumPath((size, path)): AxumPath<(String, String)>,
    req: Request,
) -> Result<impl IntoResponse, AppError> {
    // Validate path first
    let full_path = validate_path(&state.storage_path, &path)?;
    
    // Construct thumbnail path
    // storage/.thumbnails/path/to/file.jpg.small.jpg
    
    let relative_path = full_path.strip_prefix(&state.storage_path).map_err(|_| AppError::Status(StatusCode::INTERNAL_SERVER_ERROR))?;
    let thumb_root = state.storage_path.join(".thumbnails");
    let thumb_dir = thumb_root.join(relative_path.parent().unwrap_or(Path::new("")));
    let file_name = full_path.file_name().unwrap_or_default().to_string_lossy();
    
    let thumb_name = format!("{}.{}.jpg", file_name, size);
    let thumb_path = thumb_dir.join(thumb_name);

    if !thumb_path.exists() {
        return Err(AppError::Status(StatusCode::NOT_FOUND));
    }

    let service = ServeFile::new(thumb_path);
    let result = service.oneshot(req).await;
    
    match result {
        Ok(response) => Ok(response.into_response()),
        Err(_) => Err(AppError::Status(StatusCode::INTERNAL_SERVER_ERROR)),
    }
}

#[utoipa::path(
    delete,
    path = "/api/files/{path}",
    params(
        ("path" = String, Path, description = "File path")
    ),
    responses(
        (status = 200, description = "File moved to trash")
    )
)]
pub async fn delete_file(
    State(state): State<AppState>,
    AxumPath(path): AxumPath<String>,
) -> Result<StatusCode, AppError> {
    let full_path = validate_path(&state.storage_path, &path)?;
    
    if !full_path.exists() {
        return Err(AppError::Status(StatusCode::NOT_FOUND));
    }

    let trash_root = state.storage_path.join(".trash");
    if !trash_root.exists() {
        fs::create_dir_all(&trash_root).await.map_err(AppError::from)?;
    }

    // Maintain directory structure in trash
    let relative_path = full_path.strip_prefix(&state.storage_path).map_err(|_| AppError::Status(StatusCode::INTERNAL_SERVER_ERROR))?;
    let trash_path = trash_root.join(relative_path);
    
    if let Some(parent) = trash_path.parent() {
        if !parent.exists() {
            fs::create_dir_all(parent).await.map_err(AppError::from)?;
        }
    }
    
    // Handle collision by appending timestamp
    let final_trash_path = if trash_path.exists() {
        let timestamp = chrono::Utc::now().timestamp();
        let file_name = trash_path.file_name().unwrap_or_default().to_string_lossy();
        trash_path.with_file_name(format!("{}.{}", file_name, timestamp))
    } else {
        trash_path
    };

    fs::rename(full_path, final_trash_path).await.map_err(AppError::from)?;

    Ok(StatusCode::OK)
}

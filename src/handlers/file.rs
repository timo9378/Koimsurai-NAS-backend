use serde::Deserialize;
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
// use crate::utils::image::generate_thumbnails;
use tower_http::services::ServeFile;
use tower::util::ServiceExt; // for oneshot
use std::path::{Path, Component};
use utoipa::ToSchema;

#[derive(Deserialize, ToSchema)]
pub struct CreateFolderRequest {
    /// 父目錄路徑，例如 "Documents"。空字串表示根目錄
    /// Parent directory path, e.g. "Documents". Empty string means root.
    pub path: String,
    /// 新資料夾名稱，例如 "New Folder"
    /// New folder name, e.g. "New Folder"
    pub folder_name: String,
}

#[utoipa::path(
    post,
    path = "/api/files/folder",
    request_body = CreateFolderRequest,
    responses(
        (status = 201, description = "資料夾建立成功 / Folder created"),
        (status = 403, description = "沒有寫入權限 / No write permission"),
        (status = 409, description = "資料夾已存在 / Folder already exists")
    )
)]
pub async fn create_folder(
    State(state): State<AppState>,
    Extension(user_id): Extension<i64>,
    Json(payload): Json<CreateFolderRequest>,
) -> Result<StatusCode, AppError> {
    // 1. 組合路徑
    let parent_path = if payload.path.is_empty() || payload.path == "/" {
        "".to_string()
    } else {
        payload.path.trim_start_matches('/').to_string()
    };
    
    let full_relative_path = if parent_path.is_empty() {
        payload.folder_name.clone()
    } else {
        format!("{}/{}", parent_path, payload.folder_name)
    };

    // 2. 權限檢查 (檢查父目錄是否有寫入權限)
    let has_permission = sqlx::query_scalar::<_, bool>(
        "SELECT can_write FROM permissions WHERE user_id = ? AND path = ?"
    )
    .bind(user_id)
    .bind(&parent_path)
    .fetch_optional(&state.pool)
    .await
    .map_err(AppError::from)?;

    if let Some(can_write) = has_permission {
        if !can_write {
            return Err(AppError::Status(StatusCode::FORBIDDEN));
        }
    }

    // 3. 驗證並建立實體路徑
    let target_path = validate_path(&state.storage_path, &full_relative_path)?;

    if target_path.exists() {
        return Err(AppError::Status(StatusCode::CONFLICT));
    }

    // 4. 建立資料夾
    fs::create_dir_all(&target_path).await.map_err(AppError::from)?;

    // 5. 寫入 Audit Log
    let _ = state.audit.log(
        user_id,
        "create_folder",
        &full_relative_path,
        Some("Created new directory".to_string()),
        None
    ).await;

    // 資料夾會由 file watcher 自動索引到資料庫
    // Folder will be automatically indexed by file watcher

    Ok(StatusCode::CREATED)
}

#[derive(Deserialize)]
pub struct RenameRequest {
    pub new_path: String,
}

pub async fn rename_file(
    State(state): State<AppState>,
    Extension(user_id): Extension<i64>,
    AxumPath(path): AxumPath<String>,
    Json(payload): Json<RenameRequest>,
) -> Result<StatusCode, AppError> {
    // Check write permission
    let has_permission = sqlx::query_scalar::<_, bool>(
        "SELECT can_write FROM permissions WHERE user_id = ? AND path = ?"
    )
    .bind(user_id)
    .bind(&path)
    .fetch_optional(&state.pool)
    .await
    .map_err(AppError::from)?;

    if let Some(can_write) = has_permission {
        if !can_write {
            return Err(AppError::Status(StatusCode::FORBIDDEN));
        }
    }

    let old_path = validate_path(&state.storage_path, &path)?;
    let new_path = validate_path(&state.storage_path, &payload.new_path)?;

    if !old_path.exists() {
        return Err(AppError::Status(StatusCode::NOT_FOUND));
    }

    if new_path.exists() {
        return Err(AppError::Status(StatusCode::CONFLICT));
    }

    // Ensure parent directory of new path exists
    if let Some(parent) = new_path.parent() {
        if !parent.exists() {
            fs::create_dir_all(parent).await.map_err(AppError::from)?;
        }
    }

    fs::rename(old_path, new_path).await.map_err(AppError::from)?;

    // Audit Log
    let _ = state.audit.log(
        user_id,
        "rename_file",
        &path,
        Some(format!("Renamed to {}", payload.new_path)),
        None
    ).await;

    Ok(StatusCode::OK)
}


// 驗證路徑，防止 Path Traversal
// Validate path to prevent Path Traversal
pub fn validate_path(base: &Path, user_path: &str) -> Result<PathBuf, AppError> {
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

#[derive(Deserialize)]
pub struct ListFilesQuery {
    pub sort_by: Option<String>, // name, size, modified
    pub order: Option<String>,   // asc, desc
    pub search: Option<String>,
    pub page: Option<i64>,
    pub limit: Option<i64>,
}

#[utoipa::path(
    get,
    path = "/api/files/{path}",
    params(
        ("path" = String, Path, description = "Directory path"),
        ("sort_by" = Option<String>, Query, description = "Sort by field"),
        ("order" = Option<String>, Query, description = "Sort order"),
        ("search" = Option<String>, Query, description = "Search query"),
        ("page" = Option<i64>, Query, description = "Page number"),
        ("limit" = Option<i64>, Query, description = "Items per page")
    ),
    responses(
        (status = 200, description = "List files in directory", body = Vec<FileInfo>)
    )
)]
pub async fn list_files(
    State(state): State<AppState>,
    Extension(user_id): Extension<i64>,
    AxumPath(path): AxumPath<String>,
    axum::extract::Query(query): axum::extract::Query<ListFilesQuery>,
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

    // Normalize path for DB query (remove trailing slash if any, ensure forward slashes)
    let normalized_path = path.trim_end_matches('/').replace('\\', "/");
    let parent_path = if normalized_path.is_empty() { "".to_string() } else { normalized_path };

    let mut sql = String::from("SELECT name, is_dir, size, modified, mime_type FROM files WHERE ");
    let mut params = Vec::new();

    if let Some(search) = &query.search {
        sql.push_str("name LIKE ?");
        params.push(format!("%{}%", search));
    } else {
        sql.push_str("parent_path = ?");
        params.push(parent_path.clone());
    }

    // Sorting
    let sort_column = match query.sort_by.as_deref() {
        Some("size") => "size",
        Some("modified") => "modified",
        _ => "name", // Default sort by name
    };
    
    let order = match query.order.as_deref() {
        Some("desc") => "DESC",
        _ => "ASC",
    };

    sql.push_str(&format!(" ORDER BY is_dir DESC, {} {}", sort_column, order));

    // Pagination
    let limit = query.limit.unwrap_or(50);
    let offset = (query.page.unwrap_or(1) - 1) * limit;
    
    sql.push_str(" LIMIT ? OFFSET ?");

    let mut query_builder = sqlx::query_as::<_, (String, bool, i64, chrono::NaiveDateTime, Option<String>)>(&sql);
    
    for param in params {
        query_builder = query_builder.bind(param);
    }
    query_builder = query_builder.bind(limit).bind(offset);

    let rows = query_builder.fetch_all(&state.pool).await.map_err(AppError::from)?;

    let mut files = Vec::new();
    let mut stale_paths: Vec<String> = Vec::new();
    
    for (name, is_dir, size, modified, mime_type) in rows {
        // 驗證檔案是否真的存在
        // Verify the file actually exists on disk
        let file_path = state.storage_path.join(&parent_path).join(&name);
        if !file_path.exists() {
            // 記錄不存在的檔案，稍後清理
            let db_path = if parent_path.is_empty() {
                name.clone()
            } else {
                format!("{}/{}", parent_path, name)
            };
            stale_paths.push(db_path);
            continue; // 跳過這個檔案
        }
        
        let metadata = if !is_dir {
            if let Some(ref mime) = mime_type {
                crate::utils::metadata::extract_metadata(&file_path, mime)
            } else {
                crate::utils::metadata::FileMetadata::None
            }
        } else {
            crate::utils::metadata::FileMetadata::None
        };

        // Query tags
        let file_db_path = if parent_path.is_empty() {
            name.clone()
        } else {
            format!("{}/{}", parent_path, name)
        };

        let tags = sqlx::query_as::<_, (String, Option<String>)>(
            "SELECT tag_name, color FROM file_tags WHERE user_id = ? AND file_path = ?"
        )
        .bind(user_id)
        .bind(&file_db_path)
        .fetch_all(&state.pool)
        .await
        .map_err(AppError::from)?
        .into_iter()
        .map(|(name, color)| crate::models::Tag { name, color })
        .collect();

        // Query starred status
        let is_starred = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM file_stars WHERE user_id = ? AND file_path = ?)"
        )
        .bind(user_id)
        .bind(&file_db_path)
        .fetch_one(&state.pool)
        .await
        .map_err(AppError::from)?;

        files.push(FileInfo {
            name,
            is_dir,
            size: size as u64,
            modified: modified.and_utc().timestamp().to_string(),
            mime_type,
            metadata: Some(metadata),
            tags,
            is_starred,
        });
    }

    // 異步清理不存在的檔案記錄（不阻塞回應）
    // Async cleanup of stale file records (non-blocking)
    if !stale_paths.is_empty() {
        let pool = state.pool.clone();
        tokio::spawn(async move {
            for path in stale_paths {
                if let Err(e) = sqlx::query("DELETE FROM files WHERE path = ?")
                    .bind(&path)
                    .execute(&pool)
                    .await
                {
                    tracing::error!("Failed to cleanup stale file {}: {}", path, e);
                } else {
                    tracing::debug!("Cleaned up stale file record: {}", path);
                }
            }
        });
    }

    Ok(Json(files))
}

// 用於根目錄列表
// For root directory listing
#[utoipa::path(
    get,
    path = "/api/files",
    params(
        ("sort_by" = Option<String>, Query, description = "Sort by field"),
        ("order" = Option<String>, Query, description = "Sort order"),
        ("search" = Option<String>, Query, description = "Search query"),
        ("page" = Option<i64>, Query, description = "Page number"),
        ("limit" = Option<i64>, Query, description = "Items per page")
    ),
    responses(
        (status = 200, description = "List files in root", body = Vec<FileInfo>)
    )
)]
pub async fn list_files_root(
    State(state): State<AppState>,
    Extension(user_id): Extension<i64>,
    query: axum::extract::Query<ListFilesQuery>,
) -> Result<Json<Vec<FileInfo>>, AppError> {
    list_files(State(state), Extension(user_id), AxumPath("".to_string()), query).await
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
        // Versioning: if file exists, move it to versions
        if file_path.exists() {
            if let Err(e) = crate::utils::versioning::create_version(&file_path, &state.storage_path).await {
                tracing::error!("Failed to create version for {:?}: {:?}", file_path, e);
            }
        }

        let mut file = fs::File::create(&file_path).await.map_err(AppError::from)?;

        while let Some(chunk) = field.chunk().await.map_err(|_| AppError::Status(StatusCode::BAD_REQUEST))? {
            file.write_all(&chunk).await.map_err(AppError::from)?;
        }

        // 觸發縮圖生成任務
        // Trigger thumbnail generation job
        let job_type = crate::utils::queue::JobType::GenerateThumbnail {
            input_path: file_path.clone(),
            output_path: file_path.clone(), // Worker will handle the actual thumbnail path
        };

        if let Err(e) = state.queue.enqueue(job_type).await {
            tracing::error!("Failed to enqueue thumbnail job: {}", e);
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
    Extension(user_id): Extension<i64>,
    AxumPath(path): AxumPath<String>,
) -> Result<StatusCode, AppError> {
    // Check write permission
    let has_permission = sqlx::query_scalar::<_, bool>(
        "SELECT can_write FROM permissions WHERE user_id = ? AND path = ?"
    )
    .bind(user_id)
    .bind(&path)
    .fetch_optional(&state.pool)
    .await
    .map_err(AppError::from)?;

    if let Some(can_write) = has_permission {
        if !can_write {
            return Err(AppError::Status(StatusCode::FORBIDDEN));
        }
    }

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

    // === 從 files 資料表移除記錄 ===
    // Remove from files table
    let normalized_path = path.replace('\\', "/");
    sqlx::query("DELETE FROM files WHERE path = ? OR path LIKE ?")
        .bind(&normalized_path)
        .bind(format!("{}/%", normalized_path)) // 如果是目錄，一併刪除子項目
        .execute(&state.pool)
        .await
        .map_err(AppError::from)?;
    // ================================

    // Audit Log
    let _ = state.audit.log(
        user_id,
        "delete_file",
        &path,
        Some("Moved to trash".to_string()),
        None
    ).await;

    Ok(StatusCode::OK)
}

#[derive(Deserialize, ToSchema)]
pub struct BatchOperationRequest {
    pub paths: Vec<String>,
    pub destination: Option<String>, // For move/copy
}

#[utoipa::path(
    post,
    path = "/api/files/batch/delete",
    request_body = BatchOperationRequest,
    responses(
        (status = 200, description = "Batch delete initiated")
    )
)]
pub async fn batch_delete(
    State(state): State<AppState>,
    Json(payload): Json<BatchOperationRequest>,
) -> Result<StatusCode, AppError> {
    for path in payload.paths {
        // Reuse existing delete logic or enqueue job
        // For simplicity, we'll reuse the logic but ideally this should be a background job
        let full_path = validate_path(&state.storage_path, &path)?;
        
        if !full_path.exists() {
            continue;
        }

        let trash_root = state.storage_path.join(".trash");
        if !trash_root.exists() {
            fs::create_dir_all(&trash_root).await.map_err(AppError::from)?;
        }

        let relative_path = full_path.strip_prefix(&state.storage_path).map_err(|_| AppError::Status(StatusCode::INTERNAL_SERVER_ERROR))?;
        let trash_path = trash_root.join(relative_path);
        
        if let Some(parent) = trash_path.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent).await.map_err(AppError::from)?;
            }
        }
        
        let final_trash_path = if trash_path.exists() {
            let timestamp = chrono::Utc::now().timestamp();
            let file_name = trash_path.file_name().unwrap_or_default().to_string_lossy();
            trash_path.with_file_name(format!("{}.{}", file_name, timestamp))
        } else {
            trash_path
        };

        if let Err(e) = fs::rename(full_path, final_trash_path).await {
            tracing::error!("Failed to move file to trash: {}", e);
        }
    }

    Ok(StatusCode::OK)
}

#[utoipa::path(
    post,
    path = "/api/files/batch/move",
    request_body = BatchOperationRequest,
    responses(
        (status = 200, description = "Batch move initiated")
    )
)]
pub async fn batch_move(
    State(state): State<AppState>,
    Json(payload): Json<BatchOperationRequest>,
) -> Result<StatusCode, AppError> {
    let destination = payload.destination.ok_or(AppError::Status(StatusCode::BAD_REQUEST))?;
    let dest_path = validate_path(&state.storage_path, &destination)?;

    if !dest_path.exists() {
        fs::create_dir_all(&dest_path).await.map_err(AppError::from)?;
    }

    for path in payload.paths {
        let src_path = validate_path(&state.storage_path, &path)?;
        if !src_path.exists() {
            continue;
        }

        let file_name = src_path.file_name().ok_or(AppError::Status(StatusCode::BAD_REQUEST))?;
        let target_path = dest_path.join(file_name);

        if let Err(e) = fs::rename(src_path, target_path).await {
             tracing::error!("Failed to move file: {}", e);
        }
    }

    Ok(StatusCode::OK)
}

#[utoipa::path(
    post,
    path = "/api/files/batch/copy",
    request_body = BatchOperationRequest,
    responses(
        (status = 200, description = "Batch copy initiated")
    )
)]
pub async fn batch_copy(
    State(state): State<AppState>,
    Json(payload): Json<BatchOperationRequest>,
) -> Result<StatusCode, AppError> {
    let destination = payload.destination.ok_or(AppError::Status(StatusCode::BAD_REQUEST))?;
    
    // Enqueue copy job
    let job_type = crate::utils::queue::JobType::CopyFiles {
        paths: payload.paths,
        destination,
    };

    state.queue.enqueue(job_type).await.map_err(|e| {
        tracing::error!("Failed to enqueue copy job: {}", e);
        AppError::Status(StatusCode::INTERNAL_SERVER_ERROR)
    })?;

    Ok(StatusCode::ACCEPTED)
}

/// 我的最愛檔案資訊（包含 starred_at 時間戳）
/// Favorite file info with starred_at timestamp
#[derive(serde::Serialize, ToSchema)]
pub struct FavoriteFileInfo {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: String,
    pub mime_type: Option<String>,
    pub starred_at: String,
}

#[utoipa::path(
    get,
    path = "/api/favorites",
    responses(
        (status = 200, description = "取得我的最愛檔案列表 / Get favorites list", body = Vec<FavoriteFileInfo>)
    )
)]
pub async fn list_favorites(
    State(state): State<AppState>,
    Extension(user_id): Extension<i64>,
) -> Result<Json<Vec<FavoriteFileInfo>>, AppError> {
    // Join file_stars with files to get metadata
    // 聯結 file_stars 與 files 資料表取得完整資訊
    let rows = sqlx::query_as::<_, (String, String, bool, i64, chrono::NaiveDateTime, Option<String>, chrono::NaiveDateTime)>(
        r#"
        SELECT 
            f.name,
            f.path,
            f.is_dir,
            f.size,
            f.modified,
            f.mime_type,
            s.created_at as starred_at
        FROM files f
        JOIN file_stars s ON f.path = s.file_path
        WHERE s.user_id = ?
        ORDER BY s.created_at DESC
        "#
    )
    .bind(user_id)
    .fetch_all(&state.pool)
    .await
    .map_err(AppError::from)?;

    let favorites: Vec<FavoriteFileInfo> = rows
        .into_iter()
        .map(|(name, path, is_dir, size, modified, mime_type, starred_at)| {
            FavoriteFileInfo {
                name,
                path,
                is_dir,
                size: size as u64,
                modified: modified.and_utc().timestamp().to_string(),
                mime_type,
                starred_at: starred_at.and_utc().timestamp().to_string(),
            }
        })
        .collect();

    Ok(Json(favorites))
}

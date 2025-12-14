use axum::{
    extract::{State, Path as AxumPath, Extension},
    body::Body,
    Json,
    http::{StatusCode, HeaderMap},
};
use futures::StreamExt;
use tokio::io::AsyncWriteExt;
use tokio::fs::OpenOptions;
use uuid::Uuid;
use crate::state::AppState;
use crate::models::{InitUploadRequest, InitUploadResponse, UploadSession};
use crate::error::AppError;
use crate::handlers::file::validate_path;

#[utoipa::path(
    post,
    path = "/api/upload/init",
    request_body = InitUploadRequest,
    responses(
        (status = 201, description = "Upload session initialized", body = InitUploadResponse)
    )
)]
pub async fn init_upload(
    State(state): State<AppState>,
    Extension(user_id): Extension<i64>,
    Json(payload): Json<InitUploadRequest>,
) -> Result<Json<InitUploadResponse>, AppError> {
    let target_dir = validate_path(&state.storage_path, &payload.file_path)?;
    
    if !target_dir.exists() {
        tokio::fs::create_dir_all(&target_dir).await.map_err(AppError::from)?;
    }

    // Check if file already exists (completed file)
    let file_path = target_dir.join(&payload.file_name);
    if file_path.exists() {
        return Err(AppError::Status(StatusCode::CONFLICT));
    }

    // Check for existing upload session for same user + path + name
    if let Some(existing) = sqlx::query_as::<_, UploadSession>(
        "SELECT * FROM upload_sessions WHERE user_id = ? AND file_path = ? AND file_name = ?"
    )
    .bind(user_id)
    .bind(&payload.file_path)
    .bind(&payload.file_name)
    .fetch_optional(&state.pool)
    .await
    .map_err(AppError::from)? {
        // If total_size matches, resume
        if existing.total_size == payload.total_size {
            return Ok(Json(InitUploadResponse {
                upload_id: existing.id,
                uploaded_size: Some(existing.uploaded_size),
                status: Some("resuming".to_string()),
            }));
        } else {
            // Different size: remove old session and start new
            let _ = sqlx::query("DELETE FROM upload_sessions WHERE id = ?")
                .bind(&existing.id)
                .execute(&state.pool)
                .await;
        }
    }

    let upload_id = Uuid::new_v4().to_string();

    sqlx::query(
        "INSERT INTO upload_sessions (id, user_id, file_path, file_name, total_size) VALUES (?, ?, ?, ?, ?)"
    )
    .bind(&upload_id)
    .bind(user_id)
    .bind(&payload.file_path)
    .bind(&payload.file_name)
    .bind(payload.total_size)
    .execute(&state.pool)
    .await
    .map_err(AppError::from)?;

    Ok(Json(InitUploadResponse { upload_id, uploaded_size: Some(0), status: Some("created".to_string()) }))
}

#[utoipa::path(
    patch,
    path = "/api/upload/{id}",
    params(
        ("id" = String, Path, description = "Upload session ID")
    ),
    request_body(content = String, description = "File chunk", content_type = "application/octet-stream"),
    responses(
        (status = 200, description = "Chunk uploaded"),
        (status = 201, description = "Upload completed")
    )
)]
pub async fn upload_chunk(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
    body: Body,
) -> Result<StatusCode, AppError> {
    // 1. Get session info
    let session = sqlx::query_as::<_, UploadSession>(
        "SELECT * FROM upload_sessions WHERE id = ?"
    )
    .bind(&id)
    .fetch_optional(&state.pool)
    .await
    .map_err(AppError::from)?
    .ok_or(AppError::Status(StatusCode::NOT_FOUND))?;

    // 2. Parse Content-Range or X-Upload-Offset
    // For simplicity, we'll assume sequential chunks and append to a temp file
    // In a real Tus implementation, we'd need to handle offsets strictly
    
    let temp_dir = state.storage_path.join(".temp_uploads");
    if !temp_dir.exists() {
        tokio::fs::create_dir_all(&temp_dir).await.map_err(AppError::from)?;
    }
    let temp_file_path = temp_dir.join(&id);

    // If client provided a Content-Range header, validate offset matches session.uploaded_size
    if let Some(range_val) = headers.get("content-range").and_then(|v| v.to_str().ok()) {
        // Expect format: bytes start-end/total
        if let Some(rest) = range_val.strip_prefix("bytes ") {
            if let Some(range_part) = rest.split('/').next() {
                if let Some(start_str) = range_part.split('-').next() {
                    if let Ok(start_val) = start_str.parse::<i64>() {
                        if start_val != session.uploaded_size {
                            return Err(AppError::Custom(StatusCode::CONFLICT, format!("Offset mismatch: session has {} but upload started at {}", session.uploaded_size, start_val)));
                        }
                    }
                }
            }
        }
    } else if let Some(offset_val) = headers.get("x-upload-offset").and_then(|v| v.to_str().ok()) {
        if let Ok(start_val) = offset_val.parse::<i64>() {
            if start_val != session.uploaded_size {
                return Err(AppError::Custom(StatusCode::CONFLICT, format!("Offset mismatch: session has {} but upload started at {}", session.uploaded_size, start_val)));
            }
        }
    }

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&temp_file_path)
        .await
        .map_err(AppError::from)?;

    let mut uploaded_bytes = 0;
    let mut stream = body.into_data_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| AppError::Status(StatusCode::BAD_REQUEST))?;
        file.write_all(&chunk).await.map_err(AppError::from)?;
        uploaded_bytes += chunk.len() as i64;
    }

    // 3. Update progress
    let new_size = session.uploaded_size + uploaded_bytes;
    sqlx::query("UPDATE upload_sessions SET uploaded_size = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?")
        .bind(new_size)
        .bind(&id)
        .execute(&state.pool)
        .await
        .map_err(AppError::from)?;

    // 4. Check completion
    if new_size >= session.total_size {
        // Move to final destination
        let target_dir = validate_path(&state.storage_path, &session.file_path)?;
        let final_path = target_dir.join(&session.file_name);

        // Versioning: if file exists, move it to versions
        if final_path.exists() {
            if let Err(e) = crate::utils::versioning::create_version(&final_path, &state.storage_path).await {
                tracing::error!("Failed to create version for {:?}: {:?}", final_path, e);
            }
        }

        tokio::fs::rename(&temp_file_path, &final_path).await.map_err(AppError::from)?;
        
        // Cleanup session
        sqlx::query("DELETE FROM upload_sessions WHERE id = ?")
            .bind(&id)
            .execute(&state.pool)
            .await
            .map_err(AppError::from)?;

        // === 直接寫入 files 資料表 ===
        // Directly insert into files table
        let full_relative_path = if session.file_path.is_empty() {
            session.file_name.clone()
        } else {
            format!("{}/{}", session.file_path, session.file_name)
        };
        let full_relative_path = full_relative_path.replace('\\', "/");
        
        let metadata = tokio::fs::metadata(&final_path).await.map_err(AppError::from)?;
        let modified = chrono::DateTime::<chrono::Utc>::from(metadata.modified().map_err(AppError::from)?).naive_utc();
        let mime_type = mime_guess::from_path(&final_path).first_or_octet_stream().to_string();
        let parent_path = std::path::Path::new(&full_relative_path)
            .parent()
            .map(|p| p.to_string_lossy().to_string().replace('\\', "/"))
            .unwrap_or_default();
        
        sqlx::query(
            r#"
            INSERT INTO files (path, name, size, mime_type, parent_path, is_dir, modified)
            VALUES (?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(path) DO UPDATE SET
                size = excluded.size,
                modified = excluded.modified,
                mime_type = excluded.mime_type
            "#
        )
        .bind(&full_relative_path)
        .bind(&session.file_name)
        .bind(session.total_size)
        .bind(&mime_type)
        .bind(&parent_path)
        .bind(false)
        .bind(modified)
        .execute(&state.pool)
        .await
        .map_err(AppError::from)?;
        // ===============================

        // Trigger thumbnail generation only for detected images/videos
        let mime_type = mime_guess::from_path(&final_path).first_or_octet_stream().to_string();
        if mime_type.starts_with("image/") || mime_type.starts_with("video/") || crate::utils::image::is_likely_media(&final_path) {
            let job_type = crate::utils::queue::JobType::GenerateThumbnail {
                input_path: final_path.clone(),
                output_path: final_path.clone(),
            };
            let _ = state.queue.enqueue(job_type).await;
        }

        // Trigger search indexing
        let index_job = crate::utils::queue::JobType::IndexFile {
            path: full_relative_path
        };
        let _ = state.queue.enqueue(index_job).await;

        return Ok(StatusCode::CREATED);
    }

    Ok(StatusCode::OK)
}

#[utoipa::path(
    get,
    path = "/api/upload/{id}",
    params(
        ("id" = String, Path, description = "Upload session ID")
    ),
    responses(
        (status = 200, description = "Get upload status", body = UploadSession)
    )
)]
pub async fn get_upload_status(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<UploadSession>, AppError> {
    let session = sqlx::query_as::<_, UploadSession>(
        "SELECT * FROM upload_sessions WHERE id = ?"
    )
    .bind(&id)
    .fetch_optional(&state.pool)
    .await
    .map_err(AppError::from)?
    .ok_or(AppError::Status(StatusCode::NOT_FOUND))?;

    Ok(Json(session))
}
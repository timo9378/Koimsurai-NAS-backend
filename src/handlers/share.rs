use axum::{
    extract::{State, Path as AxumPath, Query, Extension},
    http::StatusCode,
    body::Body,
    Json,
    response::IntoResponse,
};
use serde::Serialize;
use crate::state::AppState;
use crate::models::{CreateShareLinkRequest, ShareLinkResponse};
use crate::error::AppError;
use crate::utils::hash::{hash_password, verify_password};
use uuid::Uuid;
use chrono::{Utc, Duration};
use tokio_util::io::ReaderStream;
use std::path::Path;
use walkdir::WalkDir;

#[derive(serde::Deserialize, utoipa::IntoParams)]
pub struct ShareQuery {
    pub pwd: Option<String>,
}

/// 分享連結元數據響應
#[derive(Serialize, utoipa::ToSchema)]
pub struct ShareInfoResponse {
    pub id: String,
    pub file_name: String,
    pub file_size: u64,
    pub mime_type: Option<String>,
    pub is_directory: bool,
    pub is_password_protected: bool,
    pub expires_at: Option<String>,
    pub created_at: String,
}

#[utoipa::path(
    post,
    path = "/api/share",
    request_body = CreateShareLinkRequest,
    responses(
        (status = 201, description = "Share link created", body = ShareLinkResponse)
    )
)]
pub async fn create_share_link(
    State(state): State<AppState>,
    Extension(user_id): Extension<i64>,
    Json(payload): Json<CreateShareLinkRequest>,
) -> Result<Json<ShareLinkResponse>, AppError> {
    let id = Uuid::new_v4().to_string();
    let password_hash = if let Some(pwd) = payload.password {
        Some(hash_password(&pwd).map_err(AppError::from)?)
    } else {
        None
    };

    let expires_at = payload.expires_in_seconds.map(|s| Utc::now() + Duration::seconds(s));

    sqlx::query(
        "INSERT INTO share_links (id, file_path, password_hash, expires_at, creator_id) VALUES (?, ?, ?, ?, ?)"
    )
    .bind(&id)
    .bind(&payload.file_path)
    .bind(password_hash)
    .bind(expires_at)
    .bind(user_id)
    .execute(&state.pool)
    .await
    .map_err(AppError::from)?;

    Ok(Json(ShareLinkResponse {
        id: id.clone(),
        url: format!("/s/{}", id),
        expires_at: expires_at.map(|t| t.to_rfc3339()),
    }))
}

#[utoipa::path(
    get,
    path = "/s/{id}",
    params(
        ("id" = String, Path, description = "Share ID"),
        ShareQuery
    ),
    responses(
        (status = 200, description = "Download file"),
        (status = 401, description = "Password required or invalid"),
        (status = 404, description = "Link not found or expired")
    )
)]
pub async fn access_share_link(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    Query(query): Query<ShareQuery>,
) -> Result<impl IntoResponse, AppError> {
    let row: Option<(String, Option<String>, Option<chrono::DateTime<Utc>>)> = sqlx::query_as(
        "SELECT file_path, password_hash, expires_at FROM share_links WHERE id = ?"
    )
    .bind(&id)
    .fetch_optional(&state.pool)
    .await
    .map_err(AppError::from)?;

    let (file_path_str, password_hash, expires_at) = row.ok_or(AppError::Status(StatusCode::NOT_FOUND))?;

    // Check expiry
    if let Some(expiry) = expires_at {
        if Utc::now() > expiry {
            return Err(AppError::Status(StatusCode::NOT_FOUND));
        }
    }

    // Check password
    if let Some(hash) = password_hash {
        let pwd = query.pwd.ok_or(AppError::Status(StatusCode::UNAUTHORIZED))?;
        let valid = verify_password(&pwd, &hash).map_err(AppError::from)?;
        if !valid {
            return Err(AppError::Status(StatusCode::UNAUTHORIZED));
        }
    }

    // Strip leading slash for safety
    let clean_path = if file_path_str.starts_with('/') {
        &file_path_str[1..]
    } else {
        &file_path_str
    };

    let full_path = state.storage_path.join(clean_path);

    // Check path exists
    if !full_path.exists() {
        return Err(AppError::Status(StatusCode::NOT_FOUND));
    }

    let is_directory = full_path.is_dir();

    if is_directory {
        // === Directory: create zip and stream ===
        let dir_name = Path::new(clean_path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "download".to_string());
        let zip_file_name = format!("{}.zip", dir_name);

        let temp_path = std::env::temp_dir().join(format!("nas_share_{}.zip", Uuid::new_v4()));
        let full_path_for_zip = full_path.clone();
        let temp_path_for_zip = temp_path.clone();

        // Create zip in blocking task
        tokio::task::spawn_blocking(move || -> Result<(), AppError> {
            let file = std::fs::File::create(&temp_path_for_zip)
                .map_err(|_| AppError::Status(StatusCode::INTERNAL_SERVER_ERROR))?;
            let mut zip = zip::ZipWriter::new(file);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);

            for entry in WalkDir::new(&full_path_for_zip).follow_links(false) {
                let entry = entry.map_err(|_| AppError::Status(StatusCode::INTERNAL_SERVER_ERROR))?;
                let path = entry.path();
                let relative = path.strip_prefix(&full_path_for_zip)
                    .map_err(|_| AppError::Status(StatusCode::INTERNAL_SERVER_ERROR))?;

                if relative.as_os_str().is_empty() {
                    continue; // Skip root directory itself
                }

                if path.is_dir() {
                    let dir_path = format!("{}/", relative.to_string_lossy());
                    zip.add_directory(&dir_path, options)
                        .map_err(|_| AppError::Status(StatusCode::INTERNAL_SERVER_ERROR))?;
                } else if path.is_file() {
                    zip.start_file(relative.to_string_lossy().to_string(), options)
                        .map_err(|_| AppError::Status(StatusCode::INTERNAL_SERVER_ERROR))?;
                    let mut f = std::fs::File::open(path)
                        .map_err(|_| AppError::Status(StatusCode::INTERNAL_SERVER_ERROR))?;
                    std::io::copy(&mut f, &mut zip)
                        .map_err(|_| AppError::Status(StatusCode::INTERNAL_SERVER_ERROR))?;
                }
            }

            zip.finish().map_err(|_| AppError::Status(StatusCode::INTERNAL_SERVER_ERROR))?;
            Ok(())
        })
        .await
        .map_err(|_| AppError::Status(StatusCode::INTERNAL_SERVER_ERROR))??;

        // Open the temp zip, then unlink it (Linux keeps data accessible via fd)
        let zip_file = tokio::fs::File::open(&temp_path).await
            .map_err(|_| AppError::Status(StatusCode::INTERNAL_SERVER_ERROR))?;
        let zip_metadata = zip_file.metadata().await
            .map_err(|_| AppError::Status(StatusCode::INTERNAL_SERVER_ERROR))?;
        let zip_size = zip_metadata.len();

        // Remove temp file; on Linux the open fd keeps data accessible
        tokio::fs::remove_file(&temp_path).await.ok();

        let encoded_name = urlencoding::encode(&zip_file_name);
        let disposition = format!(
            "attachment; filename=\"{}\"; filename*=UTF-8''{}",
            zip_file_name.replace('"', "\\\""),
            encoded_name
        );

        let stream = ReaderStream::new(zip_file);
        let body = Body::from_stream(stream);

        let response = axum::http::Response::builder()
            .header("Content-Type", "application/zip")
            .header("Content-Length", zip_size)
            .header("Content-Disposition", &disposition)
            .header("Cache-Control", "private, no-cache")
            .body(body)
            .map_err(|_| AppError::Status(StatusCode::INTERNAL_SERVER_ERROR))?;

        Ok(response)
    } else {
        // === Single file: stream directly ===
        let file = tokio::fs::File::open(&full_path).await
            .map_err(|_| AppError::Status(StatusCode::NOT_FOUND))?;
        let metadata = file.metadata().await
            .map_err(|_| AppError::Status(StatusCode::INTERNAL_SERVER_ERROR))?;
        let file_size = metadata.len();

        let file_name = Path::new(clean_path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "download".to_string());

        let mime_type = mime_guess::from_path(&full_path)
            .first_or_octet_stream()
            .to_string();

        let encoded_name = urlencoding::encode(&file_name);
        let disposition = format!(
            "attachment; filename=\"{}\"; filename*=UTF-8''{}",
            file_name.replace('"', "\\\""),
            encoded_name
        );

        let stream = ReaderStream::new(file);
        let body = Body::from_stream(stream);

        let response = axum::http::Response::builder()
            .header("Content-Type", &mime_type)
            .header("Content-Length", file_size)
            .header("Content-Disposition", &disposition)
            .header("Cache-Control", "private, no-cache")
            .body(body)
            .map_err(|_| AppError::Status(StatusCode::INTERNAL_SERVER_ERROR))?;

        Ok(response)
    }
}
/// 獲取分享連結的元數據（不需要認證，用於前端顯示）
#[utoipa::path(
    get,
    path = "/api/share/{id}/info",
    params(
        ("id" = String, Path, description = "Share ID")
    ),
    responses(
        (status = 200, description = "Share link info", body = ShareInfoResponse),
        (status = 404, description = "Link not found"),
        (status = 410, description = "Link expired")
    )
)]
pub async fn get_share_info(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<ShareInfoResponse>, AppError> {
    // 查詢分享連結資訊
    let row: Option<(String, Option<String>, Option<chrono::DateTime<Utc>>, chrono::DateTime<Utc>)> = sqlx::query_as(
        "SELECT file_path, password_hash, expires_at, created_at FROM share_links WHERE id = ?"
    )
    .bind(&id)
    .fetch_optional(&state.pool)
    .await
    .map_err(AppError::from)?;

    let (file_path_str, password_hash, expires_at, created_at) = row.ok_or(AppError::Status(StatusCode::NOT_FOUND))?;

    // 檢查是否過期
    if let Some(expiry) = expires_at {
        if Utc::now() > expiry {
            return Err(AppError::Status(StatusCode::GONE)); // 410 Gone for expired links
        }
    }

    // 獲取文件資訊
    let clean_path = if file_path_str.starts_with('/') {
        &file_path_str[1..]
    } else {
        &file_path_str
    };
    let full_path = state.storage_path.join(clean_path);
    
    if !full_path.exists() {
        tracing::warn!("Share file not found: {:?} (from db path: {})", full_path, file_path_str);
        return Err(AppError::Status(StatusCode::NOT_FOUND));
    }

    let is_directory = full_path.is_dir();

    // 獲取文件名和大小
    let file_name = Path::new(clean_path)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    
    let file_size = if is_directory {
        // Calculate total directory size by walking all files
        let full_path_for_size = full_path.clone();
        tokio::task::spawn_blocking(move || {
            let mut total: u64 = 0;
            for entry in WalkDir::new(&full_path_for_size).follow_links(false) {
                if let Ok(entry) = entry {
                    if entry.path().is_file() {
                        if let Ok(meta) = std::fs::metadata(entry.path()) {
                            total += meta.len();
                        }
                    }
                }
            }
            total
        })
        .await
        .unwrap_or(0)
    } else {
        tokio::fs::metadata(&full_path).await
            .map(|m| m.len())
            .unwrap_or(0)
    };

    // 猜測 MIME 類型
    let mime_type = if is_directory {
        None
    } else {
        mime_guess::from_path(&full_path)
            .first()
            .map(|m| m.to_string())
    };

    Ok(Json(ShareInfoResponse {
        id,
        file_name,
        file_size,
        mime_type,
        is_directory,
        is_password_protected: password_hash.is_some(),
        expires_at: expires_at.map(|t| t.to_rfc3339()),
        created_at: created_at.to_rfc3339(),
    }))
}
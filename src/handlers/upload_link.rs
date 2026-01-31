use axum::{
    extract::{State, Path as AxumPath, Query, Extension, Multipart},
    http::StatusCode,
    Json,
    response::IntoResponse,
};
use serde::{Serialize, Deserialize};
use crate::state::AppState;
use crate::models::{CreateUploadLinkRequest, UploadLinkResponse, UploadLinkInfoResponse};
use crate::error::AppError;
use crate::utils::hash::{hash_password, verify_password};
use uuid::Uuid;
use chrono::{Utc, Duration};
use std::path::Path;
use tokio::fs;
use tokio::io::AsyncWriteExt;

#[derive(serde::Deserialize, utoipa::IntoParams)]
pub struct UploadQuery {
    pub pwd: Option<String>,
}

/// 建立上傳連結
#[utoipa::path(
    post,
    path = "/api/upload-link",
    request_body = CreateUploadLinkRequest,
    responses(
        (status = 201, description = "Upload link created", body = UploadLinkResponse)
    )
)]
pub async fn create_upload_link(
    State(state): State<AppState>,
    Extension(user_id): Extension<i64>,
    Json(payload): Json<CreateUploadLinkRequest>,
) -> Result<Json<UploadLinkResponse>, AppError> {
    let id = Uuid::new_v4().to_string();
    let password_hash = if let Some(pwd) = payload.password {
        Some(hash_password(&pwd).map_err(AppError::from)?)
    } else {
        None
    };

    let expires_at = payload.expires_in_seconds.map(|s| Utc::now() + Duration::seconds(s));

    sqlx::query(
        "INSERT INTO upload_links (id, target_path, password_hash, expires_at, max_files, max_file_size, creator_id) VALUES (?, ?, ?, ?, ?, ?, ?)"
    )
    .bind(&id)
    .bind(&payload.target_path)
    .bind(password_hash)
    .bind(expires_at)
    .bind(payload.max_files)
    .bind(payload.max_file_size)
    .bind(user_id)
    .execute(&state.pool)
    .await
    .map_err(AppError::from)?;

    Ok(Json(UploadLinkResponse {
        id: id.clone(),
        url: format!("/u/{}", id),
        expires_at: expires_at.map(|t| t.to_rfc3339()),
    }))
}

/// 獲取上傳連結的元數據
#[utoipa::path(
    get,
    path = "/api/upload-link/{id}/info",
    params(
        ("id" = String, Path, description = "Upload Link ID")
    ),
    responses(
        (status = 200, description = "Upload link info", body = UploadLinkInfoResponse),
        (status = 404, description = "Link not found"),
        (status = 410, description = "Link expired")
    )
)]
pub async fn get_upload_link_info(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<UploadLinkInfoResponse>, AppError> {
    let row: Option<(String, Option<String>, Option<chrono::DateTime<Utc>>, Option<i32>, Option<i64>, i32, chrono::DateTime<Utc>)> = sqlx::query_as(
        "SELECT target_path, password_hash, expires_at, max_files, max_file_size, uploaded_count, created_at FROM upload_links WHERE id = ?"
    )
    .bind(&id)
    .fetch_optional(&state.pool)
    .await
    .map_err(AppError::from)?;

    let (target_path, password_hash, expires_at, max_files, max_file_size, uploaded_count, created_at) = 
        row.ok_or(AppError::Status(StatusCode::NOT_FOUND))?;

    // 檢查是否過期
    if let Some(expiry) = expires_at {
        if Utc::now() > expiry {
            return Err(AppError::Status(StatusCode::GONE));
        }
    }

    // 獲取目標資料夾名稱
    let target_folder = Path::new(&target_path)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| {
            if target_path == "/" || target_path.is_empty() {
                "Root".to_string()
            } else {
                target_path.clone()
            }
        });

    Ok(Json(UploadLinkInfoResponse {
        id,
        target_folder,
        is_password_protected: password_hash.is_some(),
        expires_at: expires_at.map(|t| t.to_rfc3339()),
        max_files,
        max_file_size,
        uploaded_count,
        created_at: created_at.to_rfc3339(),
    }))
}

/// 透過上傳連結上傳檔案
#[utoipa::path(
    post,
    path = "/u/{id}",
    params(
        ("id" = String, Path, description = "Upload Link ID"),
        UploadQuery
    ),
    responses(
        (status = 200, description = "File uploaded successfully"),
        (status = 401, description = "Password required or invalid"),
        (status = 404, description = "Link not found or expired"),
        (status = 413, description = "File too large"),
        (status = 429, description = "Upload limit reached")
    )
)]
pub async fn upload_via_link(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    Query(query): Query<UploadQuery>,
    mut multipart: Multipart,
) -> Result<impl IntoResponse, AppError> {
    // 查詢上傳連結資訊
    let row: Option<(String, Option<String>, Option<chrono::DateTime<Utc>>, Option<i32>, Option<i64>, i32)> = sqlx::query_as(
        "SELECT target_path, password_hash, expires_at, max_files, max_file_size, uploaded_count FROM upload_links WHERE id = ?"
    )
    .bind(&id)
    .fetch_optional(&state.pool)
    .await
    .map_err(AppError::from)?;

    let (target_path, password_hash, expires_at, max_files, max_file_size, uploaded_count) = 
        row.ok_or(AppError::Status(StatusCode::NOT_FOUND))?;

    // 檢查是否過期
    if let Some(expiry) = expires_at {
        if Utc::now() > expiry {
            return Err(AppError::Status(StatusCode::GONE));
        }
    }

    // 檢查密碼
    if let Some(ref hash) = password_hash {
        match &query.pwd {
            Some(pwd) => {
                if !verify_password(pwd, hash).map_err(AppError::from)? {
                    return Err(AppError::Status(StatusCode::UNAUTHORIZED));
                }
            }
            None => {
                return Err(AppError::Status(StatusCode::UNAUTHORIZED));
            }
        }
    }

    // 檢查檔案數量限制
    if let Some(max) = max_files {
        if uploaded_count >= max {
            return Err(AppError::Status(StatusCode::TOO_MANY_REQUESTS));
        }
    }

    let mut files_uploaded = 0;

    while let Some(field) = multipart.next_field().await.map_err(|_| AppError::Status(StatusCode::BAD_REQUEST))? {
        let file_name = field.file_name()
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("upload_{}", Uuid::new_v4()));

        // 讀取檔案內容
        let data = field.bytes().await.map_err(|_| AppError::Status(StatusCode::BAD_REQUEST))?;

        // 檢查檔案大小限制
        if let Some(max_size) = max_file_size {
            if data.len() as i64 > max_size {
                return Err(AppError::Status(StatusCode::PAYLOAD_TOO_LARGE));
            }
        }

        // 建構完整路徑
        let clean_target = if target_path.starts_with('/') {
            &target_path[1..]
        } else {
            &target_path
        };
        
        let full_path = state.storage_path.join(clean_target).join(&file_name);

        // 確保目標目錄存在
        if let Some(parent) = full_path.parent() {
            fs::create_dir_all(parent).await.map_err(|_| AppError::Status(StatusCode::INTERNAL_SERVER_ERROR))?;
        }

        // 寫入檔案
        let mut file = fs::File::create(&full_path).await.map_err(|_| AppError::Status(StatusCode::INTERNAL_SERVER_ERROR))?;
        file.write_all(&data).await.map_err(|_| AppError::Status(StatusCode::INTERNAL_SERVER_ERROR))?;

        files_uploaded += 1;
    }

    // 更新上傳計數
    sqlx::query("UPDATE upload_links SET uploaded_count = uploaded_count + ? WHERE id = ?")
        .bind(files_uploaded)
        .bind(&id)
        .execute(&state.pool)
        .await
        .map_err(AppError::from)?;

    Ok(Json(serde_json::json!({
        "success": true,
        "files_uploaded": files_uploaded
    })))
}

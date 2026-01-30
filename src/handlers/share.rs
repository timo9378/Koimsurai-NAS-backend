use axum::{
    extract::{State, Path as AxumPath, Query, Extension},
    http::StatusCode,
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
use tower_http::services::ServeFile;
use tower::util::ServiceExt;
use std::path::Path;

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
    req: axum::extract::Request,
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
            return Err(AppError::Status(StatusCode::NOT_FOUND)); // Treat expired as not found
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

    // Serve file
    let full_path = state.storage_path.join(file_path_str);
    
    if !full_path.exists() {
        return Err(AppError::Status(StatusCode::NOT_FOUND));
    }

    let service = ServeFile::new(full_path);
    let result = service.oneshot(req).await;
    
    match result {
        Ok(response) => Ok(response.into_response()),
        Err(_) => Err(AppError::Status(StatusCode::INTERNAL_SERVER_ERROR)),
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
    let full_path = state.storage_path.join(&file_path_str);
    
    if !full_path.exists() {
        return Err(AppError::Status(StatusCode::NOT_FOUND));
    }

    // 獲取文件名和大小
    let file_name = Path::new(&file_path_str)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    
    let file_size = std::fs::metadata(&full_path)
        .map(|m| m.len())
        .unwrap_or(0);

    // 猜測 MIME 類型
    let mime_type = mime_guess::from_path(&full_path)
        .first()
        .map(|m| m.to_string());

    Ok(Json(ShareInfoResponse {
        id,
        file_name,
        file_size,
        mime_type,
        is_password_protected: password_hash.is_some(),
        expires_at: expires_at.map(|t| t.to_rfc3339()),
        created_at: created_at.to_rfc3339(),
    }))
}
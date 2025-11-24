use axum::{
    extract::{State, Path as AxumPath, Query},
    http::StatusCode,
    Json,
    response::IntoResponse,
};
use tower_sessions::Session;
use crate::state::AppState;
use crate::models::{CreateShareLinkRequest, ShareLinkResponse};
use crate::error::AppError;
use crate::handlers::auth::AUTH_SESSION_KEY;
use crate::utils::hash::{hash_password, verify_password};
use uuid::Uuid;
use chrono::{Utc, Duration};
use tower_http::services::ServeFile;
use tower::util::ServiceExt;

#[derive(serde::Deserialize, utoipa::IntoParams)]
pub struct ShareQuery {
    pub pwd: Option<String>,
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
    session: Session,
    Json(payload): Json<CreateShareLinkRequest>,
) -> Result<Json<ShareLinkResponse>, AppError> {
    let user_id: i64 = session.get(AUTH_SESSION_KEY).await.map_err(AppError::from)?.ok_or(AppError::Status(StatusCode::UNAUTHORIZED))?;

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

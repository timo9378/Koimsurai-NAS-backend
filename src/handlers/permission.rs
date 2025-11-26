use axum::{
    extract::{State, Extension, Json},
    http::StatusCode,
    response::IntoResponse,
};
use crate::state::AppState;
use crate::error::AppError;
use crate::models::CreatePermissionRequest;

pub async fn set_permission(
    State(state): State<AppState>,
    Extension(_user_id): Extension<i64>, // In real app, check if user is admin
    Json(payload): Json<CreatePermissionRequest>,
) -> Result<StatusCode, AppError> {
    sqlx::query(
        r#"
        INSERT INTO permissions (user_id, path, can_read, can_write)
        VALUES (?, ?, ?, ?)
        ON CONFLICT(user_id, path) DO UPDATE SET
            can_read = excluded.can_read,
            can_write = excluded.can_write
        "#
    )
    .bind(payload.user_id)
    .bind(&payload.path)
    .bind(payload.can_read)
    .bind(payload.can_write)
    .execute(&state.pool)
    .await
    .map_err(AppError::from)?;

    Ok(StatusCode::OK)
}

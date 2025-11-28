use axum::{
    extract::{State, Path as AxumPath, Extension},
    Json,
    http::StatusCode,
};
use serde::Deserialize;
use crate::state::AppState;
use crate::error::AppError;
// use crate::models::Tag;
use utoipa::ToSchema;

#[derive(Deserialize, ToSchema)]
pub struct AddTagRequest {
    pub tag_name: String,
    pub color: Option<String>,
}

#[utoipa::path(
    post,
    path = "/api/files/{path}/tags",
    params(
        ("path" = String, Path, description = "File path")
    ),
    request_body = AddTagRequest,
    responses(
        (status = 200, description = "Tag added")
    )
)]
pub async fn add_tag(
    State(state): State<AppState>,
    Extension(user_id): Extension<i64>,
    AxumPath(path): AxumPath<String>,
    Json(payload): Json<AddTagRequest>,
) -> Result<StatusCode, AppError> {
    sqlx::query(
        "INSERT INTO file_tags (user_id, file_path, tag_name, color) VALUES (?, ?, ?, ?)"
    )
    .bind(user_id)
    .bind(&path)
    .bind(&payload.tag_name)
    .bind(&payload.color)
    .execute(&state.pool)
    .await
    .map_err(AppError::from)?;

    Ok(StatusCode::OK)
}

#[utoipa::path(
    delete,
    path = "/api/files/{path}/tags/{tag_name}",
    params(
        ("path" = String, Path, description = "File path"),
        ("tag_name" = String, Path, description = "Tag name")
    ),
    responses(
        (status = 200, description = "Tag removed")
    )
)]
pub async fn remove_tag(
    State(state): State<AppState>,
    Extension(user_id): Extension<i64>,
    AxumPath((path, tag_name)): AxumPath<(String, String)>,
) -> Result<StatusCode, AppError> {
    sqlx::query(
        "DELETE FROM file_tags WHERE user_id = ? AND file_path = ? AND tag_name = ?"
    )
    .bind(user_id)
    .bind(&path)
    .bind(&tag_name)
    .execute(&state.pool)
    .await
    .map_err(AppError::from)?;

    Ok(StatusCode::OK)
}

#[utoipa::path(
    post,
    path = "/api/files/{path}/star",
    params(
        ("path" = String, Path, description = "File path")
    ),
    responses(
        (status = 200, description = "Star toggled")
    )
)]
pub async fn toggle_star(
    State(state): State<AppState>,
    Extension(user_id): Extension<i64>,
    AxumPath(path): AxumPath<String>,
) -> Result<StatusCode, AppError> {
    let exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM file_stars WHERE user_id = ? AND file_path = ?)"
    )
    .bind(user_id)
    .bind(&path)
    .fetch_one(&state.pool)
    .await
    .map_err(AppError::from)?;

    if exists {
        sqlx::query("DELETE FROM file_stars WHERE user_id = ? AND file_path = ?")
            .bind(user_id)
            .bind(&path)
            .execute(&state.pool)
            .await
            .map_err(AppError::from)?;
    } else {
        sqlx::query("INSERT INTO file_stars (user_id, file_path) VALUES (?, ?)")
            .bind(user_id)
            .bind(&path)
            .execute(&state.pool)
            .await
            .map_err(AppError::from)?;
    }

    Ok(StatusCode::OK)
}
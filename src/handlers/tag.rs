use axum::{
    extract::{State, Path as AxumPath, Extension},
    Json,
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use crate::state::AppState;
use crate::error::AppError;
use crate::models::Tag;
use utoipa::ToSchema;

#[derive(Deserialize, ToSchema)]
pub struct AddTagRequest {
    pub tag_name: String,
    pub color: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct UserTag {
    pub name: String,
    pub color: Option<String>,
    pub count: i64,
}

#[derive(Serialize, ToSchema)]
pub struct TaggedFile {
    pub path: String,
    pub name: String,
    pub is_dir: bool,
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
    AxumPath((tag_name, path)): AxumPath<(String, String)>,
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

/// List all tags for the current user with file counts
#[utoipa::path(
    get,
    path = "/api/tags",
    responses(
        (status = 200, description = "List of user tags", body = Vec<UserTag>)
    )
)]
pub async fn list_tags(
    State(state): State<AppState>,
    Extension(user_id): Extension<i64>,
) -> Result<Json<Vec<UserTag>>, AppError> {
    let tags = sqlx::query_as::<_, (String, Option<String>, i64)>(
        r#"
        SELECT tag_name, color, COUNT(*) as count
        FROM file_tags
        WHERE user_id = ?
        GROUP BY tag_name, color
        ORDER BY tag_name
        "#
    )
    .bind(user_id)
    .fetch_all(&state.pool)
    .await
    .map_err(AppError::from)?;

    let user_tags: Vec<UserTag> = tags
        .into_iter()
        .map(|(name, color, count)| UserTag { name, color, count })
        .collect();

    Ok(Json(user_tags))
}

/// List files with a specific tag
#[utoipa::path(
    get,
    path = "/api/tags/{tag_name}/files",
    params(
        ("tag_name" = String, Path, description = "Tag name")
    ),
    responses(
        (status = 200, description = "List of files with the tag", body = Vec<TaggedFile>)
    )
)]
pub async fn list_files_by_tag(
    State(state): State<AppState>,
    Extension(user_id): Extension<i64>,
    AxumPath(tag_name): AxumPath<String>,
) -> Result<Json<Vec<TaggedFile>>, AppError> {
    let files = sqlx::query_as::<_, (String,)>(
        r#"
        SELECT file_path
        FROM file_tags
        WHERE user_id = ? AND tag_name = ?
        ORDER BY file_path
        "#
    )
    .bind(user_id)
    .bind(&tag_name)
    .fetch_all(&state.pool)
    .await
    .map_err(AppError::from)?;

    let tagged_files: Vec<TaggedFile> = files
        .into_iter()
        .map(|(path,)| {
            let name = path.rsplit('/').next().unwrap_or(&path).to_string();
            // Check if file exists and is a directory
            let full_path = std::path::Path::new("/app/data").join(path.trim_start_matches('/'));
            let is_dir = full_path.is_dir();
            TaggedFile {
                path: path.clone(),
                name,
                is_dir,
            }
        })
        .collect();

    Ok(Json(tagged_files))
}
use axum::{
    extract::{State, Path as AxumPath, Extension},
    Json,
    http::StatusCode,
};
use crate::state::AppState;
use crate::error::AppError;
use crate::handlers::file::validate_path;
use crate::utils::versioning::{list_versions, FileVersion};
use tokio::fs;

#[utoipa::path(
    get,
    path = "/api/files/{path}/versions",
    params(
        ("path" = String, Path, description = "File path")
    ),
    responses(
        (status = 200, description = "List file versions", body = Vec<FileVersion>)
    )
)]
pub async fn list_file_versions(
    State(state): State<AppState>,
    AxumPath(path): AxumPath<String>,
) -> Result<Json<Vec<FileVersion>>, AppError> {
    let full_path = validate_path(&state.storage_path, &path)?;
    
    // We allow listing versions even if the current file is deleted (if we implement that logic later),
    // but for now let's assume we are checking versions of an existing file or at least a path.
    // Actually, validate_path checks for existence if we want to be strict, but here we might want to see versions of a file that was just overwritten.
    // validate_path logic:
    // if full_path.exists() ...
    
    // If the file doesn't exist, we can still check for versions if we relax validate_path or handle it here.
    // But validate_path returns error if path traversal is detected. It doesn't enforce existence unless we check it.
    
    let versions = list_versions(&full_path, &state.storage_path).await?;
    Ok(Json(versions))
}

#[utoipa::path(
    post,
    path = "/api/files/{path}/restore/{version_id}",
    params(
        ("path" = String, Path, description = "File path"),
        ("version_id" = String, Path, description = "Version ID to restore")
    ),
    responses(
        (status = 200, description = "Version restored")
    )
)]
pub async fn restore_version(
    State(state): State<AppState>,
    Extension(user_id): Extension<i64>,
    AxumPath((path, version_id)): AxumPath<(String, String)>,
) -> Result<StatusCode, AppError> {
    let full_path = validate_path(&state.storage_path, &path)?;
    
    // 1. Locate the version file
    let relative_path = full_path.strip_prefix(&state.storage_path).map_err(|_| AppError::Status(StatusCode::INTERNAL_SERVER_ERROR))?;
    let versions_root = state.storage_path.join(".versions");
    let parent = relative_path.parent().unwrap_or(std::path::Path::new(""));
    let version_path = versions_root.join(parent).join(&version_id);

    if !version_path.exists() {
        return Err(AppError::Status(StatusCode::NOT_FOUND));
    }

    // 2. Backup current file as a new version (if it exists)
    if full_path.exists() {
        if let Err(e) = crate::utils::versioning::create_version(&full_path, &state.storage_path).await {
             tracing::error!("Failed to create version before restore: {:?}", e);
        }
    }

    // 3. Restore (Copy version to current path)
    // We copy instead of move so the version history remains (or we could move and rename, but usually restoring implies "reverting" to that state)
    // Actually, usually "restore" might mean making that version the current one.
    // Let's copy it back.
    fs::copy(&version_path, &full_path).await.map_err(AppError::from)?;

    // Audit Log
    let _ = state.audit.log(
        user_id,
        "restore_version",
        &path,
        Some(format!("Restored version: {}", version_id)),
        None
    ).await;

    Ok(StatusCode::OK)
}
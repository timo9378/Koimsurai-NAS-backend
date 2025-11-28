use axum::{
    extract::{State, Query, Extension},
    Json,
    // http::StatusCode,
};
use serde::Deserialize;
use crate::state::AppState;
use crate::error::AppError;
use crate::services::audit::AuditLog;

#[derive(Deserialize)]
pub struct AuditLogQuery {
    pub page: Option<i64>,
    pub limit: Option<i64>,
    pub user_id: Option<i64>,
    pub action: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/audit/logs",
    params(
        ("page" = Option<i64>, Query, description = "Page number"),
        ("limit" = Option<i64>, Query, description = "Items per page"),
        ("user_id" = Option<i64>, Query, description = "Filter by user ID"),
        ("action" = Option<String>, Query, description = "Filter by action type")
    ),
    responses(
        (status = 200, description = "List audit logs", body = Vec<AuditLog>)
    )
)]
pub async fn list_audit_logs(
    State(state): State<AppState>,
    Extension(_user_id): Extension<i64>, // Ensure user is authenticated, maybe check for admin role later
    Query(query): Query<AuditLogQuery>,
) -> Result<Json<Vec<AuditLog>>, AppError> {
    let limit = query.limit.unwrap_or(50);
    let offset = (query.page.unwrap_or(1) - 1) * limit;

    let mut sql = String::from("SELECT * FROM audit_logs WHERE 1=1");
    let mut params = Vec::new();

    if let Some(uid) = query.user_id {
        sql.push_str(" AND user_id = ?");
        params.push(uid.to_string()); // Bind as string or value depending on how we handle it, but sqlx handles types
    }

    if let Some(action) = &query.action {
        sql.push_str(" AND action = ?");
        params.push(action.clone());
    }

    sql.push_str(" ORDER BY created_at DESC LIMIT ? OFFSET ?");

    let mut query_builder = sqlx::query_as::<_, AuditLog>(&sql);

    if let Some(uid) = query.user_id {
        query_builder = query_builder.bind(uid);
    }
    if let Some(action) = &query.action {
        query_builder = query_builder.bind(action);
    }

    query_builder = query_builder.bind(limit).bind(offset);

    let logs = query_builder
        .fetch_all(&state.pool)
        .await
        .map_err(AppError::from)?;

    Ok(Json(logs))
}
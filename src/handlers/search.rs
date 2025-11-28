use axum::{
    extract::{State, Query},
    Json,
};
use crate::state::AppState;
use crate::error::AppError;
use crate::services::search::SearchResult;

#[derive(serde::Deserialize)]
pub struct SearchQuery {
    pub q: String,
}

#[utoipa::path(
    get,
    path = "/api/search",
    params(
        ("q" = String, Query, description = "Search query")
    ),
    responses(
        (status = 200, description = "Search results", body = Vec<SearchResult>)
    )
)]
pub async fn search_files(
    State(state): State<AppState>,
    Query(query): Query<SearchQuery>,
) -> Result<Json<Vec<SearchResult>>, AppError> {
    let results = state.search.search(&query.q)?;
    Ok(Json(results))
}
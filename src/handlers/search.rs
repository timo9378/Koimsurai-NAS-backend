use axum::{
    extract::{State, Query},
    Json,
};
use crate::state::AppState;
use crate::error::AppError;
use crate::services::search::{SearchResult, AiTagSearchResult, search_by_ai_tag};

#[derive(serde::Deserialize)]
pub struct SearchQuery {
    pub q: String,
}

#[derive(serde::Deserialize)]
pub struct AiTagSearchQuery {
    pub q: String,
    pub min_confidence: Option<f32>,
    pub limit: Option<i32>,
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

/// Search files by AI tags
#[utoipa::path(
    get,
    path = "/api/search/ai-tags",
    params(
        ("q" = String, Query, description = "AI tag search query"),
        ("min_confidence" = Option<f32>, Query, description = "Minimum confidence threshold (0.0-1.0)"),
        ("limit" = Option<i32>, Query, description = "Maximum number of results")
    ),
    responses(
        (status = 200, description = "AI tag search results", body = Vec<AiTagSearchResult>)
    )
)]
pub async fn search_ai_tags(
    State(state): State<AppState>,
    Query(query): Query<AiTagSearchQuery>,
) -> Result<Json<Vec<AiTagSearchResult>>, AppError> {
    let results = search_by_ai_tag(
        &state.pool,
        &query.q,
        query.min_confidence,
        query.limit,
    ).await?;
    Ok(Json(results))
}
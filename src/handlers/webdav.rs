use axum::{
    body::Body,
    extract::{State, Request},
    response::IntoResponse,
};
use crate::state::AppState;

pub async fn webdav_handler(
    State(state): State<AppState>,
    req: Request<Body>,
) -> impl IntoResponse {
    state.webdav.handle(req).await
}

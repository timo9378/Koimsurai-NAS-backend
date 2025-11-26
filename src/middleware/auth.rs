use axum::{
    extract::Request,
    http::StatusCode,
    middleware::Next,
    response::Response,
};
use tower_sessions::Session;
use crate::handlers::auth::AUTH_SESSION_KEY;

pub async fn require_auth(
    session: Session,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let user_id: Option<i64> = session.get(AUTH_SESSION_KEY).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    if let Some(user_id) = user_id {
        let mut request = request;
        request.extensions_mut().insert(user_id);
        Ok(next.run(request).await)
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}


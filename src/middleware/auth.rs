use axum::{
    extract::Request,
    http::{StatusCode, header},
    middleware::Next,
    response::Response,
};
use crate::utils::jwt::verify_token;

pub async fn require_auth(
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    // First try Authorization header
    let auth_header = request.headers().get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok());

    // Extract token either from Bearer header or from access_token cookie
    let mut token_opt: Option<String> = None;

    if let Some(h) = auth_header {
        if h.starts_with("Bearer ") {
            token_opt = Some(h[7..].to_string());
        }
    }

    if token_opt.is_none() {
        // Try cookie header (simple parse without extra crate)
        if let Some(cookie_header) = request.headers().get(header::COOKIE).and_then(|h| h.to_str().ok()) {
            for part in cookie_header.split(';').map(|s| s.trim()) {
                if let Some(rest) = part.strip_prefix("access_token=") {
                    // cookie value may be quoted; trim quotes
                    let val = rest.trim_matches('"').to_string();
                    token_opt = Some(val);
                    break;
                }
            }
        }
    }

    let token = if let Some(t) = token_opt { t } else { return Err(StatusCode::UNAUTHORIZED) };

    match verify_token(&token) {
        Ok(claims) => {
            let mut request = request;
            let user_id = claims.sub.parse::<i64>().map_err(|_| StatusCode::UNAUTHORIZED)?;
            request.extensions_mut().insert(user_id);
            Ok(next.run(request).await)
        }
        Err(_) => Err(StatusCode::UNAUTHORIZED),
    }
}

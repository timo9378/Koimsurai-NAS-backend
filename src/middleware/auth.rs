use axum::{
    extract::{Request, State},
    http::{StatusCode, header},
    middleware::Next,
    response::Response,
};
use axum_extra::extract::CookieJar;
use crate::state::AppState;
use crate::utils::jwt::verify_token_with_secret;

pub async fn require_auth(
    State(state): State<AppState>,
    jar: CookieJar,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    // First try Authorization header (explicit token, immune to CSRF)
    let auth_header = request.headers().get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok());

    let mut token_opt: Option<String> = None;
    let is_bearer_auth;

    if let Some(h) = auth_header {
        if let Some(bearer) = h.strip_prefix("Bearer ") {
            token_opt = Some(bearer.to_string());
        }
    }
    is_bearer_auth = token_opt.is_some();

    // Fallback to Cookie (using axum-extra CookieJar for correct parsing)
    if token_opt.is_none() {
        if let Some(cookie) = jar.get("access_token") {
            token_opt = Some(cookie.value().to_string());
        }
    }

    // For state-changing requests via Cookie auth, require Origin/Referer check (CSRF mitigation)
    // Bearer tokens are immune since they must be explicitly attached by JS
    if token_opt.is_some() && !is_bearer_auth {
        // Cookie-based auth — check for CSRF on mutating methods
        let method = request.method().clone();
        if method == axum::http::Method::POST
            || method == axum::http::Method::PUT
            || method == axum::http::Method::DELETE
            || method == axum::http::Method::PATCH
        {
            let origin = request.headers().get(header::ORIGIN)
                .and_then(|h| h.to_str().ok())
                .map(|s| s.to_string());
            let referer = request.headers().get(header::REFERER)
                .and_then(|h| h.to_str().ok())
                .map(|s| s.to_string());
            let host = request.headers().get(header::HOST)
                .and_then(|h| h.to_str().ok())
                .map(|s| s.to_string());

            // 如果有 Origin header，驗證它是否匹配 Host
            if let Some(ref origin_val) = origin {
                if let Some(ref host_val) = host {
                    // 從 Origin 中提取主機名
                    let origin_host = origin_val
                        .trim_start_matches("http://")
                        .trim_start_matches("https://");
                    if !origin_host.starts_with(host_val.as_str()) {
                        tracing::warn!("CSRF check failed: Origin '{}' does not match Host '{}'", origin_val, host_val);
                        return Err(StatusCode::FORBIDDEN);
                    }
                }
            } else if referer.is_none() {
                // 既沒有 Origin 也沒有 Referer — 預設拒絕（CSRF 防護）
                // Cookie 搭配 SameSite=Lax 已阻擋大部分跨站 POST，
                // 但缺少 Origin/Referer 的 mutating 請求仍應視為可疑。
                tracing::warn!("CSRF blocked: Cookie-based mutating request without Origin or Referer header");
                return Err(StatusCode::FORBIDDEN);
            }
        }
    }

    let token = if let Some(t) = token_opt { t } else { return Err(StatusCode::UNAUTHORIZED) };

    // 使用 AppState 中的 jwt_secret 驗證 token（避免每次讀取 env var）
    match verify_token_with_secret(&token, &state.jwt_secret) {
        Ok(claims) => {
            let mut request = request;
            let user_id = claims.sub.parse::<i64>().map_err(|_| StatusCode::UNAUTHORIZED)?;
            request.extensions_mut().insert(user_id);
            Ok(next.run(request).await)
        }
        Err(_) => Err(StatusCode::UNAUTHORIZED),
    }
}

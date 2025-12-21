use axum::{
    extract::State,
    http::StatusCode,
    Json,
};
use axum_extra::extract::cookie::{Cookie, SameSite, CookieJar};
use crate::models::{RegisterRequest, LoginRequest, User, EmptyResponse, RefreshToken};
use crate::state::AppState;
use crate::utils::hash::{hash_password, verify_password};
use crate::utils::jwt::create_access_token;
use crate::error::AppError;
use uuid::Uuid;
use chrono::{Utc, Duration};

pub const AUTH_SESSION_KEY: &str = "authenticated_user_id";
pub const REFRESH_TOKEN_COOKIE_NAME: &str = "refresh_token";
pub const ACCESS_TOKEN_COOKIE_NAME: &str = "access_token";

#[utoipa::path(
    post,
    path = "/api/auth/register",
    request_body = RegisterRequest,
    responses(
        (status = 201, description = "User registered successfully"),
        (status = 409, description = "Username already exists")
    )
)]
pub async fn register(
    State(state): State<AppState>,
    Json(payload): Json<RegisterRequest>,
) -> Result<StatusCode, AppError> {
    // === 檢查邀請碼 Check invite code ===
    let secret_code = std::env::var("REGISTRATION_INVITE_CODE").unwrap_or_default();
    
    // 如果沒有設定邀請碼，則不允許任何人註冊
    // If no invite code is set, registration is disabled
    if secret_code.is_empty() {
        return Err(AppError::Custom(
            StatusCode::FORBIDDEN,
            "Registration is disabled. No invite code configured.".to_string(),
        ));
    }
    
    // 如果輸入的邀請碼不正確
    // If the provided invite code is incorrect
    if payload.invite_code.as_deref() != Some(secret_code.as_str()) {
        return Err(AppError::Custom(
            StatusCode::FORBIDDEN,
            "Invalid or missing invite code.".to_string(),
        ));
    }
    // ===========================================

    // 檢查使用者是否已存在
    // Check if user exists
    let user_exists: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM users WHERE username = ?"
    )
    .bind(&payload.username)
    .fetch_optional(&state.pool)
    .await
    .map_err(AppError::from)?;

    if user_exists.is_some() {
        return Err(AppError::Custom(StatusCode::CONFLICT, "Username already exists".to_string()));
    }

    // 加密密碼
    // Hash password
    let password_hash = hash_password(&payload.password)
        .map_err(AppError::from)?;

    // 插入使用者
    // Insert user
    sqlx::query(
        "INSERT INTO users (username, password_hash) VALUES (?, ?)"
    )
    .bind(payload.username)
    .bind(password_hash)
    .execute(&state.pool)
    .await
    .map_err(AppError::from)?;

    Ok(StatusCode::CREATED)
}

#[utoipa::path(
    post,
    path = "/api/auth/login",
    request_body = LoginRequest,
    responses(
        (status = 200, description = "Login successful", body = EmptyResponse),
        (status = 401, description = "Invalid credentials")
    )
)]
pub async fn login(
    State(state): State<AppState>,
    jar: CookieJar,
    Json(payload): Json<LoginRequest>,
) -> Result<(CookieJar, Json<EmptyResponse>), AppError> {
    // 查詢使用者
    // Find user
    let user: Option<User> = sqlx::query_as(
        "SELECT id, username, password_hash, created_at FROM users WHERE username = ?"
    )
    .bind(payload.username)
    .fetch_optional(&state.pool)
    .await
    .map_err(AppError::from)?;

    let user = match user {
        Some(u) => u,
        None => return Err(AppError::AuthError("Invalid credentials".to_string())),
    };

    // 驗證密碼
    // Verify password
    let is_valid = verify_password(&payload.password, &user.password_hash)
        .map_err(AppError::from)?;

    if !is_valid {
        return Err(AppError::AuthError("Invalid credentials".to_string()));
    }

    // 生成 Access Token
    let access_token = create_access_token(user.id).map_err(|e| AppError::InternalServerError(e.to_string()))?;

    // 生成 Refresh Token
    let refresh_token = Uuid::new_v4().to_string();
    let expires_at = Utc::now() + Duration::days(7);

    // 儲存 Refresh Token 到資料庫
    sqlx::query(
        "INSERT INTO refresh_tokens (user_id, token, expires_at) VALUES (?, ?, ?)"
    )
    .bind(user.id)
    .bind(&refresh_token)
    .bind(expires_at)
    .execute(&state.pool)
    .await
    .map_err(AppError::from)?;

    // 設定 HttpOnly Cookie
    let cookie = Cookie::build((REFRESH_TOKEN_COOKIE_NAME, refresh_token))
        .http_only(true)
        .path("/")
        .same_site(SameSite::Strict)
        .secure(true) // 在生產環境應設為 true
        .max_age(time::Duration::days(7))
        .build();

    // Determine cookie `secure` flag from environment (default true).
    let cookie_secure = std::env::var("COOKIE_SECURE").unwrap_or_else(|_| "true".to_string()) == "true";

    // Also set access_token as HttpOnly cookie for cookie-based auth
    let mut access_cookie_builder = Cookie::build((ACCESS_TOKEN_COOKIE_NAME, access_token.clone()))
        .http_only(true)
        .path("/")
        .same_site(SameSite::Strict)
        .max_age(time::Duration::minutes(15));
    if cookie_secure {
        access_cookie_builder = access_cookie_builder.secure(true);
    }
    let access_cookie = access_cookie_builder.build();

    let jar = jar.add(cookie);
    let jar = jar.add(access_cookie);

    // For cookie-based flow we do not expose access_token in response body; return empty response
    Ok((jar, Json(EmptyResponse {})))
}

#[utoipa::path(
    post,
    path = "/api/auth/refresh",
    responses(
        (status = 200, description = "Token refreshed", body = EmptyResponse),
        (status = 401, description = "Invalid refresh token")
    )
)]
pub async fn refresh(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Result<(CookieJar, Json<EmptyResponse>), AppError> {
    let refresh_token = jar
        .get(REFRESH_TOKEN_COOKIE_NAME)
        .map(|cookie| cookie.value().to_string())
        .ok_or_else(|| AppError::AuthError("Missing refresh token".to_string()))?;

    // 驗證 Refresh Token
    let token_record: Option<RefreshToken> = sqlx::query_as(
        "SELECT * FROM refresh_tokens WHERE token = ? AND revoked = FALSE AND expires_at > CURRENT_TIMESTAMP"
    )
    .bind(&refresh_token)
    .fetch_optional(&state.pool)
    .await
    .map_err(AppError::from)?;

    let token_record = match token_record {
        Some(t) => t,
        None => return Err(AppError::AuthError("Invalid or expired refresh token".to_string())),
    };

    // 生成新的 Access Token
    let access_token = create_access_token(token_record.user_id).map_err(|e| AppError::InternalServerError(e.to_string()))?;

    // 選擇性：輪換 Refresh Token (這裡為了簡單起見，我們保留原來的 Refresh Token，或者你可以選擇發一個新的並撤銷舊的)
    // 這裡我們選擇發一個新的 Refresh Token 並撤銷舊的，以增加安全性 (Refresh Token Rotation)
    
    // 撤銷舊的 Refresh Token
    sqlx::query("UPDATE refresh_tokens SET revoked = TRUE WHERE id = ?")
        .bind(token_record.id)
        .execute(&state.pool)
        .await
        .map_err(AppError::from)?;

    // 生成新的 Refresh Token
    let new_refresh_token = Uuid::new_v4().to_string();
    let expires_at = Utc::now() + Duration::days(7);

    sqlx::query(
        "INSERT INTO refresh_tokens (user_id, token, expires_at) VALUES (?, ?, ?)"
    )
    .bind(token_record.user_id)
    .bind(&new_refresh_token)
    .bind(expires_at)
    .execute(&state.pool)
    .await
    .map_err(AppError::from)?;

    // 更新 Cookie
    let cookie = Cookie::build((REFRESH_TOKEN_COOKIE_NAME, new_refresh_token))
        .http_only(true)
        .path("/")
        .same_site(SameSite::Strict)
        .max_age(time::Duration::days(7))
        .build();

    // Determine cookie `secure` flag from environment (default true).
    let cookie_secure = std::env::var("COOKIE_SECURE").unwrap_or_else(|_| "true".to_string()) == "true";
    let mut access_cookie_builder = Cookie::build((ACCESS_TOKEN_COOKIE_NAME, access_token.clone()))
        .http_only(true)
        .path("/")
        .same_site(SameSite::Strict)
        .max_age(time::Duration::minutes(15));
    if cookie_secure {
        access_cookie_builder = access_cookie_builder.secure(true);
    }
    let access_cookie = access_cookie_builder.build();

    let jar = jar.add(cookie);
    let jar = jar.add(access_cookie);

    Ok((jar, Json(EmptyResponse {})))
}

#[utoipa::path(
    post,
    path = "/api/auth/logout",
    responses(
        (status = 200, description = "Logout successful")
    )
)]
pub async fn logout(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Result<(CookieJar, StatusCode), AppError> {
    if let Some(cookie) = jar.get(REFRESH_TOKEN_COOKIE_NAME) {
        let refresh_token = cookie.value();
        
        // 撤銷 Refresh Token
        sqlx::query("UPDATE refresh_tokens SET revoked = TRUE WHERE token = ?")
            .bind(refresh_token)
            .execute(&state.pool)
            .await
            .ok(); // 忽略錯誤
    }

    // 移除 Cookie
    // Determine cookie `secure` flag from environment (default true).
    let cookie_secure = std::env::var("COOKIE_SECURE").unwrap_or_else(|_| "true".to_string()) == "true";

    let mut refresh_cookie_builder = Cookie::build((REFRESH_TOKEN_COOKIE_NAME, ""))
        .http_only(true)
        .path("/")
        .same_site(SameSite::Strict)
        .max_age(time::Duration::seconds(0)); // 立即過期
    if cookie_secure {
        refresh_cookie_builder = refresh_cookie_builder.secure(true);
    }
    let refresh_cookie = refresh_cookie_builder.build();

    // Also clear access token cookie
    let mut access_cookie_builder = Cookie::build((ACCESS_TOKEN_COOKIE_NAME, ""))
        .http_only(true)
        .path("/")
        .same_site(SameSite::Strict)
        .max_age(time::Duration::seconds(0));
    if cookie_secure {
        access_cookie_builder = access_cookie_builder.secure(true);
    }
    let access_cookie = access_cookie_builder.build();

    Ok((jar.add(refresh_cookie).add(access_cookie), StatusCode::OK))
}

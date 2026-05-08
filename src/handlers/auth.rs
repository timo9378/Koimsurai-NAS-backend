use axum::{
    extract::State,
    http::StatusCode,
    Extension, Json,
};
use axum_extra::extract::cookie::{Cookie, SameSite, CookieJar};
use crate::models::{
    RegisterRequest, LoginRequest, User, EmptyResponse, RefreshToken,
    LoginResponse, TwoFactorLoginRequest,
    TwoFactorSetupResponse, TwoFactorVerifySetupRequest, TwoFactorVerifySetupResponse,
    TwoFactorDisableRequest, TwoFactorStatusResponse,
};
use crate::state::AppState;
use crate::utils::hash::{hash_password, verify_password};
use crate::utils::jwt::create_access_token_with_secret;
use crate::utils::totp::{generate_secret, build_otpauth_uri, verify_code, generate_backup_codes};
use crate::error::AppError;
use jsonwebtoken::{encode, EncodingKey, Header, decode, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use chrono::{Utc, Duration};
use uuid::Uuid;

pub const AUTH_SESSION_KEY: &str = "authenticated_user_id";
pub const REFRESH_TOKEN_COOKIE_NAME: &str = "refresh_token";
pub const ACCESS_TOKEN_COOKIE_NAME: &str = "access_token";

/// 5 分鐘的短效 token，登入第一階段通過密碼後發給前端，第二階段送回驗 code
#[derive(Debug, Serialize, Deserialize)]
struct TwoFactorTempClaims {
    pub sub: String, // user_id
    pub purpose: String, // 必須是 "2fa_pending"
    pub exp: usize,
}

fn create_2fa_temp_token(user_id: i64, secret: &str) -> Result<String, AppError> {
    let exp = Utc::now()
        .checked_add_signed(Duration::minutes(5))
        .ok_or_else(|| AppError::InternalServerError("time overflow".to_string()))?
        .timestamp() as usize;
    let claims = TwoFactorTempClaims {
        sub: user_id.to_string(),
        purpose: "2fa_pending".to_string(),
        exp,
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .map_err(|e| AppError::InternalServerError(e.to_string()))
}

fn verify_2fa_temp_token(token: &str, secret: &str) -> Result<i64, AppError> {
    let data = decode::<TwoFactorTempClaims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    )
    .map_err(|_| AppError::AuthError("invalid or expired temp token".to_string()))?;
    if data.claims.purpose != "2fa_pending" {
        return Err(AppError::AuthError("wrong token purpose".to_string()));
    }
    data.claims.sub.parse::<i64>()
        .map_err(|_| AppError::AuthError("invalid sub".to_string()))
}

/// 共用：設定 access + refresh cookies（登入成功後呼叫）
async fn issue_session_cookies(
    state: &AppState,
    jar: CookieJar,
    user_id: i64,
) -> Result<CookieJar, AppError> {
    let access_token = create_access_token_with_secret(user_id, &state.jwt_secret)
        .map_err(|e| AppError::InternalServerError(e.to_string()))?;

    let refresh_token = Uuid::new_v4().to_string();
    let expires_at = Utc::now() + Duration::days(7);
    sqlx::query("INSERT INTO refresh_tokens (user_id, token, expires_at) VALUES (?, ?, ?)")
        .bind(user_id)
        .bind(&refresh_token)
        .bind(expires_at)
        .execute(&state.pool)
        .await
        .map_err(AppError::from)?;

    let cookie_secure = std::env::var("COOKIE_SECURE").unwrap_or_else(|_| "true".to_string()) == "true";
    let cookie_domain = std::env::var("COOKIE_DOMAIN").ok();

    let mut refresh_b = Cookie::build((REFRESH_TOKEN_COOKIE_NAME, refresh_token))
        .http_only(true)
        .path("/")
        .same_site(SameSite::Lax)
        .max_age(time::Duration::days(7));
    if cookie_secure { refresh_b = refresh_b.secure(true); }
    if let Some(d) = &cookie_domain { refresh_b = refresh_b.domain(d.clone()); }

    let mut access_b = Cookie::build((ACCESS_TOKEN_COOKIE_NAME, access_token))
        .http_only(true)
        .path("/")
        .same_site(SameSite::Lax)
        .max_age(time::Duration::minutes(15));
    if cookie_secure { access_b = access_b.secure(true); }
    if let Some(d) = &cookie_domain { access_b = access_b.domain(d.clone()); }

    Ok(jar.add(refresh_b.build()).add(access_b.build()))
}

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
    let secret_code = std::env::var("REGISTRATION_INVITE_CODE").unwrap_or_default();

    if secret_code.is_empty() {
        return Err(AppError::Custom(
            StatusCode::FORBIDDEN,
            "Registration is disabled. No invite code configured.".to_string(),
        ));
    }
    if payload.invite_code.as_deref() != Some(secret_code.as_str()) {
        return Err(AppError::Custom(
            StatusCode::FORBIDDEN,
            "Invalid or missing invite code.".to_string(),
        ));
    }

    let user_exists: Option<(i64,)> = sqlx::query_as("SELECT id FROM users WHERE username = ?")
        .bind(&payload.username)
        .fetch_optional(&state.pool)
        .await
        .map_err(AppError::from)?;
    if user_exists.is_some() {
        return Err(AppError::Custom(StatusCode::CONFLICT, "Username already exists".to_string()));
    }

    let password_hash = hash_password(&payload.password).map_err(AppError::from)?;
    sqlx::query("INSERT INTO users (username, password_hash) VALUES (?, ?)")
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
        (status = 200, description = "Login successful or 2FA needed", body = LoginResponse),
        (status = 401, description = "Invalid credentials")
    )
)]
pub async fn login(
    State(state): State<AppState>,
    jar: CookieJar,
    Json(payload): Json<LoginRequest>,
) -> Result<(CookieJar, Json<LoginResponse>), AppError> {
    let user: Option<User> = sqlx::query_as(
        "SELECT id, username, password_hash, created_at, totp_secret, totp_enabled, totp_backup_codes
         FROM users WHERE username = ?"
    )
    .bind(payload.username)
    .fetch_optional(&state.pool)
    .await
    .map_err(AppError::from)?;

    let user = match user {
        Some(u) => u,
        None => return Err(AppError::AuthError("Invalid credentials".to_string())),
    };

    let is_valid = verify_password(&payload.password, &user.password_hash)
        .map_err(AppError::from)?;
    if !is_valid {
        return Err(AppError::AuthError("Invalid credentials".to_string()));
    }

    // 已啟用 2FA → 不直接發 cookie，回 temp_token 給前端，下一步驗 code
    if user.totp_enabled == 1 {
        let temp_token = create_2fa_temp_token(user.id, &state.jwt_secret)?;
        return Ok((jar, Json(LoginResponse::NeedsTwoFactor {
            requires_2fa: true,
            temp_token,
        })));
    }

    // 沒啟用 2FA → 照舊發 cookie
    let jar = issue_session_cookies(&state, jar, user.id).await?;
    Ok((jar, Json(LoginResponse::Done(EmptyResponse {}))))
}

/// 第二階段：用 temp_token + 6 位 code（或 backup code）換 cookie
#[utoipa::path(
    post,
    path = "/api/auth/2fa/login",
    request_body = TwoFactorLoginRequest,
    responses(
        (status = 200, description = "2FA verified, cookies set", body = EmptyResponse),
        (status = 401, description = "Invalid code or expired temp token")
    )
)]
pub async fn two_factor_login(
    State(state): State<AppState>,
    jar: CookieJar,
    Json(payload): Json<TwoFactorLoginRequest>,
) -> Result<(CookieJar, Json<EmptyResponse>), AppError> {
    let user_id = verify_2fa_temp_token(&payload.temp_token, &state.jwt_secret)?;

    let user: User = sqlx::query_as(
        "SELECT id, username, password_hash, created_at, totp_secret, totp_enabled, totp_backup_codes
         FROM users WHERE id = ?"
    )
    .bind(user_id)
    .fetch_one(&state.pool)
    .await
    .map_err(|_| AppError::AuthError("user not found".to_string()))?;

    if user.totp_enabled != 1 {
        return Err(AppError::AuthError("2FA not enabled".to_string()));
    }
    let secret = user.totp_secret.as_deref()
        .ok_or_else(|| AppError::InternalServerError("totp_secret missing".to_string()))?;

    // 先試 6 位 TOTP，再試 backup code
    let trimmed = payload.code.trim().to_uppercase();
    let is_totp = !trimmed.contains('-');

    if is_totp {
        let ok = verify_code(secret, &trimmed)
            .map_err(|e| AppError::InternalServerError(e.to_string()))?;
        if !ok {
            return Err(AppError::AuthError("Invalid code".to_string()));
        }
    } else {
        // backup code 流程：對 DB 中所有 hash 嘗試比對，命中則從 DB 移除（一次性）
        let codes_json = user.totp_backup_codes.as_deref().unwrap_or("[]");
        let mut codes: Vec<String> = serde_json::from_str(codes_json)
            .map_err(|_| AppError::InternalServerError("backup_codes corrupt".to_string()))?;
        let mut matched_index: Option<usize> = None;
        for (i, hashed) in codes.iter().enumerate() {
            if verify_password(&trimmed, hashed).unwrap_or(false) {
                matched_index = Some(i);
                break;
            }
        }
        let idx = matched_index.ok_or_else(|| AppError::AuthError("Invalid backup code".to_string()))?;
        codes.remove(idx);
        let new_json = serde_json::to_string(&codes).unwrap_or_else(|_| "[]".to_string());
        sqlx::query("UPDATE users SET totp_backup_codes = ? WHERE id = ?")
            .bind(new_json)
            .bind(user.id)
            .execute(&state.pool)
            .await
            .map_err(AppError::from)?;
    }

    let jar = issue_session_cookies(&state, jar, user.id).await?;
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

    let token_record: Option<RefreshToken> = sqlx::query_as(
        "SELECT * FROM refresh_tokens WHERE token = ? AND revoked = FALSE AND expires_at > CURRENT_TIMESTAMP"
    )
    .bind(&refresh_token)
    .fetch_optional(&state.pool)
    .await
    .map_err(AppError::from)?;

    let token_record = token_record
        .ok_or_else(|| AppError::AuthError("Invalid or expired refresh token".to_string()))?;

    sqlx::query("UPDATE refresh_tokens SET revoked = TRUE WHERE id = ?")
        .bind(token_record.id)
        .execute(&state.pool)
        .await
        .map_err(AppError::from)?;

    let jar = issue_session_cookies(&state, jar, token_record.user_id).await?;
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
        sqlx::query("UPDATE refresh_tokens SET revoked = TRUE WHERE token = ?")
            .bind(refresh_token)
            .execute(&state.pool)
            .await
            .ok();
    }

    let cookie_secure = std::env::var("COOKIE_SECURE").unwrap_or_else(|_| "true".to_string()) == "true";
    let cookie_domain = std::env::var("COOKIE_DOMAIN").ok();

    let mut refresh_b = Cookie::build((REFRESH_TOKEN_COOKIE_NAME, ""))
        .http_only(true)
        .path("/")
        .same_site(SameSite::Lax)
        .max_age(time::Duration::seconds(0));
    if cookie_secure { refresh_b = refresh_b.secure(true); }
    if let Some(d) = &cookie_domain { refresh_b = refresh_b.domain(d.clone()); }

    let mut access_b = Cookie::build((ACCESS_TOKEN_COOKIE_NAME, ""))
        .http_only(true)
        .path("/")
        .same_site(SameSite::Lax)
        .max_age(time::Duration::seconds(0));
    if cookie_secure { access_b = access_b.secure(true); }
    if let Some(d) = &cookie_domain { access_b = access_b.domain(d.clone()); }

    Ok((jar.add(refresh_b.build()).add(access_b.build()), StatusCode::OK))
}

// ──────────────────────────────────────────────────────────────
// 2FA setup / verify / disable / status — 都需要已登入（require_auth middleware）
// ──────────────────────────────────────────────────────────────

#[utoipa::path(
    post,
    path = "/api/auth/2fa/setup",
    responses(
        (status = 200, description = "Returns secret + otpauth URI", body = TwoFactorSetupResponse),
    )
)]
pub async fn two_factor_setup(
    State(state): State<AppState>,
    Extension(user_id): Extension<i64>,
) -> Result<Json<TwoFactorSetupResponse>, AppError> {
    let user: User = sqlx::query_as(
        "SELECT id, username, password_hash, created_at, totp_secret, totp_enabled, totp_backup_codes
         FROM users WHERE id = ?"
    )
    .bind(user_id)
    .fetch_one(&state.pool)
    .await
    .map_err(AppError::from)?;

    if user.totp_enabled == 1 {
        return Err(AppError::Custom(
            StatusCode::CONFLICT,
            "2FA already enabled. Disable it first to re-setup.".to_string(),
        ));
    }

    let secret = generate_secret();
    let uri = build_otpauth_uri(&secret, &user.username)
        .map_err(|e| AppError::InternalServerError(e.to_string()))?;

    // 寫入 secret，但 totp_enabled 仍為 0；要等 verify-setup 確認後才 = 1
    sqlx::query("UPDATE users SET totp_secret = ? WHERE id = ?")
        .bind(&secret)
        .bind(user.id)
        .execute(&state.pool)
        .await
        .map_err(AppError::from)?;

    Ok(Json(TwoFactorSetupResponse { secret, otpauth_uri: uri }))
}

#[utoipa::path(
    post,
    path = "/api/auth/2fa/verify-setup",
    request_body = TwoFactorVerifySetupRequest,
    responses(
        (status = 200, description = "2FA enabled, returns one-time backup codes", body = TwoFactorVerifySetupResponse),
        (status = 401, description = "Code mismatch")
    )
)]
pub async fn two_factor_verify_setup(
    State(state): State<AppState>,
    Extension(user_id): Extension<i64>,
    Json(payload): Json<TwoFactorVerifySetupRequest>,
) -> Result<Json<TwoFactorVerifySetupResponse>, AppError> {
    let user: User = sqlx::query_as(
        "SELECT id, username, password_hash, created_at, totp_secret, totp_enabled, totp_backup_codes
         FROM users WHERE id = ?"
    )
    .bind(user_id)
    .fetch_one(&state.pool)
    .await
    .map_err(AppError::from)?;

    let secret = user.totp_secret.as_deref()
        .ok_or_else(|| AppError::Custom(StatusCode::BAD_REQUEST, "Run /setup first".to_string()))?;

    let ok = verify_code(secret, payload.code.trim())
        .map_err(|e| AppError::InternalServerError(e.to_string()))?;
    if !ok {
        return Err(AppError::AuthError("Invalid code".to_string()));
    }

    // 產 8 組 backup codes，hash 後存 DB，明文回傳一次
    let codes = generate_backup_codes();
    let hashed: Vec<String> = codes.iter()
        .map(|c| hash_password(c).map_err(AppError::from))
        .collect::<Result<_, _>>()?;
    let codes_json = serde_json::to_string(&hashed)
        .map_err(|e| AppError::InternalServerError(e.to_string()))?;

    sqlx::query("UPDATE users SET totp_enabled = 1, totp_backup_codes = ? WHERE id = ?")
        .bind(codes_json)
        .bind(user.id)
        .execute(&state.pool)
        .await
        .map_err(AppError::from)?;

    Ok(Json(TwoFactorVerifySetupResponse { backup_codes: codes }))
}

#[utoipa::path(
    post,
    path = "/api/auth/2fa/disable",
    request_body = TwoFactorDisableRequest,
    responses(
        (status = 200, description = "2FA disabled"),
        (status = 401, description = "Wrong password or code")
    )
)]
pub async fn two_factor_disable(
    State(state): State<AppState>,
    Extension(user_id): Extension<i64>,
    Json(payload): Json<TwoFactorDisableRequest>,
) -> Result<StatusCode, AppError> {
    let user: User = sqlx::query_as(
        "SELECT id, username, password_hash, created_at, totp_secret, totp_enabled, totp_backup_codes
         FROM users WHERE id = ?"
    )
    .bind(user_id)
    .fetch_one(&state.pool)
    .await
    .map_err(AppError::from)?;

    if user.totp_enabled != 1 {
        return Err(AppError::Custom(StatusCode::BAD_REQUEST, "2FA not enabled".to_string()));
    }

    // 必須 password OK + code OK 才能停用（防止 cookie 被劫持）
    if !verify_password(&payload.password, &user.password_hash).map_err(AppError::from)? {
        return Err(AppError::AuthError("Wrong password".to_string()));
    }

    let secret = user.totp_secret.as_deref()
        .ok_or_else(|| AppError::InternalServerError("totp_secret missing".to_string()))?;
    let trimmed = payload.code.trim().to_uppercase();
    let valid = if trimmed.contains('-') {
        // backup code
        let codes_json = user.totp_backup_codes.as_deref().unwrap_or("[]");
        let codes: Vec<String> = serde_json::from_str(codes_json).unwrap_or_default();
        codes.iter().any(|h| verify_password(&trimmed, h).unwrap_or(false))
    } else {
        verify_code(secret, &trimmed)
            .map_err(|e| AppError::InternalServerError(e.to_string()))?
    };
    if !valid {
        return Err(AppError::AuthError("Invalid code".to_string()));
    }

    sqlx::query("UPDATE users SET totp_enabled = 0, totp_secret = NULL, totp_backup_codes = NULL WHERE id = ?")
        .bind(user.id)
        .execute(&state.pool)
        .await
        .map_err(AppError::from)?;
    Ok(StatusCode::OK)
}

#[utoipa::path(
    get,
    path = "/api/auth/2fa/status",
    responses(
        (status = 200, description = "2FA status", body = TwoFactorStatusResponse),
    )
)]
pub async fn two_factor_status(
    State(state): State<AppState>,
    Extension(user_id): Extension<i64>,
) -> Result<Json<TwoFactorStatusResponse>, AppError> {
    let user: User = sqlx::query_as(
        "SELECT id, username, password_hash, created_at, totp_secret, totp_enabled, totp_backup_codes
         FROM users WHERE id = ?"
    )
    .bind(user_id)
    .fetch_one(&state.pool)
    .await
    .map_err(AppError::from)?;

    let remaining = if user.totp_enabled == 1 {
        user.totp_backup_codes.as_deref()
            .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
            .map(|v| v.len())
            .unwrap_or(0)
    } else { 0 };

    Ok(Json(TwoFactorStatusResponse {
        enabled: user.totp_enabled == 1,
        backup_codes_remaining: remaining,
    }))
}

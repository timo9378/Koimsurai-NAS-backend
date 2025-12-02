use axum::{extract::State, http::StatusCode, Json, response::IntoResponse};
use tower_sessions::Session;
use crate::models::{RegisterRequest, LoginRequest, User, AuthResponse};
use crate::state::AppState;
use crate::utils::hash::{hash_password, verify_password};
use crate::error::AppError;

pub const AUTH_SESSION_KEY: &str = "authenticated_user_id";

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
        (status = 200, description = "Login successful", body = AuthResponse),
        (status = 401, description = "Invalid credentials")
    )
)]
pub async fn login(
    State(state): State<AppState>,
    session: Session,
    Json(payload): Json<LoginRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
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

    // 設定 Session
    // Set session
    session.insert(AUTH_SESSION_KEY, user.id).await
        .map_err(AppError::from)?;

    Ok(Json(serde_json::json!({ "message": "Login successful", "user_id": user.id })))
}

#[utoipa::path(
    post,
    path = "/api/auth/logout",
    responses(
        (status = 200, description = "Logout successful")
    )
)]
pub async fn logout(session: Session) -> impl IntoResponse {
    session.delete().await.ok();
    StatusCode::OK
}

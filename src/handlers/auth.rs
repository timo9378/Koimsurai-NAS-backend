use axum::{extract::State, http::StatusCode, Json, response::IntoResponse};
use tower_sessions::Session;
use crate::models::{RegisterRequest, LoginRequest, User, AuthResponse};
use crate::state::AppState;
use crate::utils::hash::{hash_password, verify_password};

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
) -> impl IntoResponse {
    // 檢查使用者是否已存在
    // Check if user exists
    let user_exists: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM users WHERE username = ?"
    )
    .bind(&payload.username)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if user_exists.is_some() {
        return Err((StatusCode::CONFLICT, "Username already exists".to_string()));
    }

    // 加密密碼
    // Hash password
    let password_hash = hash_password(&payload.password)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // 插入使用者
    // Insert user
    sqlx::query(
        "INSERT INTO users (username, password_hash) VALUES (?, ?)"
    )
    .bind(payload.username)
    .bind(password_hash)
    .execute(&state.pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

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
) -> impl IntoResponse {
    // 查詢使用者
    // Find user
    let user: Option<User> = sqlx::query_as(
        "SELECT id, username, password_hash, created_at FROM users WHERE username = ?"
    )
    .bind(payload.username)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let user = match user {
        Some(u) => u,
        None => return Err((StatusCode::UNAUTHORIZED, "Invalid credentials".to_string())),
    };

    // 驗證密碼
    // Verify password
    let is_valid = verify_password(&payload.password, &user.password_hash)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if !is_valid {
        return Err((StatusCode::UNAUTHORIZED, "Invalid credentials".to_string()));
    }

    // 設定 Session
    // Set session
    session.insert(AUTH_SESSION_KEY, user.id).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

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

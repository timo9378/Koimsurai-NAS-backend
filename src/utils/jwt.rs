use chrono::{Duration, Utc};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use std::env;

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String, // user_id
    pub exp: usize,
}

/// 使用指定的 secret 建立 JWT access token
/// Create JWT access token with the given secret
pub fn create_access_token_with_secret(user_id: i64, secret: &str) -> Result<String, jsonwebtoken::errors::Error> {
    let expiration = Utc::now()
        .checked_add_signed(Duration::minutes(15))
        .ok_or_else(|| jsonwebtoken::errors::Error::from(jsonwebtoken::errors::ErrorKind::InvalidKeyFormat))?
        .timestamp();

    let claims = Claims {
        sub: user_id.to_string(),
        exp: expiration as usize,
    };

    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
}

/// 使用指定的 secret 驗證 JWT token
/// Verify JWT token with the given secret
pub fn verify_token_with_secret(token: &str, secret: &str) -> Result<Claims, jsonwebtoken::errors::Error> {
    let validation = Validation::default();
    
    let token_data = decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &validation,
    )?;

    Ok(token_data.claims)
}

/// 向下相容：從環境變數讀取 JWT_SECRET（僅用於尚未遷移到 AppState 的呼叫點）
/// Backward compatible: read JWT_SECRET from env var (for call sites not yet migrated to AppState)
pub fn create_access_token(user_id: i64) -> Result<String, jsonwebtoken::errors::Error> {
    let secret = env::var("JWT_SECRET")
        .map_err(|_| jsonwebtoken::errors::Error::from(jsonwebtoken::errors::ErrorKind::InvalidKeyFormat))?;
    create_access_token_with_secret(user_id, &secret)
}

pub fn verify_token(token: &str) -> Result<Claims, jsonwebtoken::errors::Error> {
    let secret = env::var("JWT_SECRET")
        .map_err(|_| jsonwebtoken::errors::Error::from(jsonwebtoken::errors::ErrorKind::InvalidKeyFormat))?;
    verify_token_with_secret(token, &secret)
}
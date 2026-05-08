use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use chrono::NaiveDateTime;
use utoipa::ToSchema;

#[derive(Debug, Serialize, Deserialize, FromRow, Clone, ToSchema)]
pub struct User {
    pub id: i64,
    pub username: String,
    #[serde(skip)]
    pub password_hash: String,
    pub created_at: NaiveDateTime,
    /// base32 secret，None = 未啟用過 2FA；Some + totp_enabled=0 = setup 中尚未驗證
    #[serde(skip)]
    pub totp_secret: Option<String>,
    /// 0 = 未啟用，1 = 已啟用
    pub totp_enabled: i64,
    /// JSON array of argon2-hashed backup codes，每組用過一次後從 array 移除
    #[serde(skip)]
    pub totp_backup_codes: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct RegisterRequest {
    pub username: String,
    pub password: String,
    /// 邀請碼（必填，用於限制註冊）
    /// Invite code (required to restrict registration)
    pub invite_code: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

/// 第一階段登入回應：要嘛直接登入完成（無 2FA），要嘛需要 2FA code
#[derive(Debug, Serialize, ToSchema)]
#[serde(untagged)]
pub enum LoginResponse {
    /// 一般使用者（沒啟用 2FA）— cookie 已發，前端正常導向
    Done(EmptyResponse),
    /// 需要 2FA 驗證
    NeedsTwoFactor {
        requires_2fa: bool,
        /// 短效 token（5 分鐘），下一步 verify code 時帶回
        temp_token: String,
    },
}

/// 2FA 第二階段登入請求
#[derive(Debug, Deserialize, ToSchema)]
pub struct TwoFactorLoginRequest {
    pub temp_token: String,
    /// 6 位 TOTP code 或 backup code（後者帶 dash，例如 ABCDE-F2345）
    pub code: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct EmptyResponse {}

/// 啟用 2FA 第一步回應：把 secret + otpauth URI 給前端，前端畫 QR
#[derive(Debug, Serialize, ToSchema)]
pub struct TwoFactorSetupResponse {
    pub secret: String,
    pub otpauth_uri: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct TwoFactorVerifySetupRequest {
    /// 用 Authenticator app 掃完後輸入的第一個 6 位 code
    pub code: String,
}

/// 啟用 2FA 完成後的回應：8 組 backup codes（一次性顯示，user 必須記下來）
#[derive(Debug, Serialize, ToSchema)]
pub struct TwoFactorVerifySetupResponse {
    pub backup_codes: Vec<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct TwoFactorDisableRequest {
    pub password: String,
    /// 6 位 code 或 backup code
    pub code: String,
}

/// 2FA 狀態查詢回應
#[derive(Debug, Serialize, ToSchema)]
pub struct TwoFactorStatusResponse {
    pub enabled: bool,
    /// 還剩多少 backup codes
    pub backup_codes_remaining: usize,
}

#[derive(Debug, Serialize, Deserialize, FromRow)]
pub struct RefreshToken {
    pub id: i64,
    pub user_id: i64,
    pub token: String,
    pub expires_at: NaiveDateTime,
    pub revoked: bool,
    pub created_at: NaiveDateTime,
}

//! TOTP (RFC 6238) 工具：產生/驗證 6 位 code，產 backup codes
//! Two-factor authentication helpers using SHA1, 30s step, 6 digits — 與 Google Authenticator / Authy 相容

use anyhow::{anyhow, Result};
use rand::{thread_rng, Rng};
use totp_rs::{Algorithm, Secret, TOTP};

/// 服務名（顯示在 Authenticator app 上）
const ISSUER: &str = "Koimsurai NAS";

/// 產生新的 base32 secret（160 bits = 32 bytes，符合 RFC 4226 建議）
pub fn generate_secret() -> String {
    Secret::generate_secret().to_encoded().to_string()
}

/// 用 secret + username 構造 otpauth:// URI（前端用 qrcodejs 畫 QR）
/// 範例：otpauth://totp/Koimsurai%20NAS:timo9378?secret=ABC...&issuer=Koimsurai%20NAS
pub fn build_otpauth_uri(secret: &str, username: &str) -> Result<String> {
    let totp = build_totp(secret, username)?;
    Ok(totp.get_url())
}

/// 驗證使用者輸入的 6 位 code 是否正確（含時間漂移容忍 ±1 step = 30s）
pub fn verify_code(secret: &str, code: &str) -> Result<bool> {
    let totp = build_totp(secret, "verify")?;
    Ok(totp.check_current(code).unwrap_or(false))
}

fn build_totp(secret: &str, label: &str) -> Result<TOTP> {
    let bytes = Secret::Encoded(secret.to_string())
        .to_bytes()
        .map_err(|e| anyhow!("invalid secret: {:?}", e))?;
    TOTP::new(
        Algorithm::SHA1,
        6,
        1, // ±1 step tolerance for clock drift
        30,
        bytes,
        Some(ISSUER.to_string()),
        label.to_string(),
    )
    .map_err(|e| anyhow!("totp build failed: {:?}", e))
}

/// 產生 8 組 backup codes（每組 10 字母+數字，dashes 分組讀感佳）
/// 顯示給 user 看一次後，存進 DB 的版本是 argon2 hash（與密碼相同方式）
pub fn generate_backup_codes() -> Vec<String> {
    let mut rng = thread_rng();
    (0..8)
        .map(|_| {
            // 10 chars: AAAA-AAAA-AA 樣式，從 32 進制可讀字元集
            const CHARSET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789"; // 去掉容易混淆的 I/L/O/0/1
            let s: String = (0..10)
                .map(|_| CHARSET[rng.gen_range(0..CHARSET.len())] as char)
                .collect();
            format!("{}-{}", &s[..5], &s[5..])
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_roundtrip() {
        let s = generate_secret();
        assert!(!s.is_empty());
        let uri = build_otpauth_uri(&s, "alice").unwrap();
        assert!(uri.starts_with("otpauth://totp/"));
        assert!(uri.contains("issuer=Koimsurai%20NAS"));
    }

    #[test]
    fn backup_codes_format() {
        let codes = generate_backup_codes();
        assert_eq!(codes.len(), 8);
        for c in &codes {
            assert_eq!(c.len(), 11); // 5 + dash + 5
            assert_eq!(c.chars().nth(5), Some('-'));
        }
    }
}

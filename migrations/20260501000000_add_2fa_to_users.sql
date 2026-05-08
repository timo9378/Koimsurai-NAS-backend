-- 兩階段驗證（TOTP / RFC 6238）相關欄位
-- totp_secret:        base32-encoded shared secret（user setup 時隨機產生）
-- totp_enabled:       0=未啟用，1=已啟用（setup verify 通過才設 1）
-- totp_backup_codes:  JSON array of argon2-hashed backup codes
--                     一次性產 8 組，user 用過一組就從 DB 刪掉
ALTER TABLE users ADD COLUMN totp_secret TEXT;
ALTER TABLE users ADD COLUMN totp_enabled INTEGER NOT NULL DEFAULT 0;
ALTER TABLE users ADD COLUMN totp_backup_codes TEXT;

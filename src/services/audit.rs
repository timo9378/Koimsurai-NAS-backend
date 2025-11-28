use sqlx::{Pool, Sqlite};
use serde::Serialize;
use utoipa::ToSchema;

pub struct AuditService {
    pool: Pool<Sqlite>,
}

impl AuditService {
    pub fn new(pool: Pool<Sqlite>) -> Self {
        Self { pool }
    }

    pub async fn log(
        &self,
        user_id: i64,
        action: &str,
        target: &str,
        details: Option<String>,
        ip_address: Option<String>,
    ) {
        let result = sqlx::query(
            "INSERT INTO audit_logs (user_id, action, target, details, ip_address) VALUES (?, ?, ?, ?, ?)"
        )
        .bind(user_id)
        .bind(action)
        .bind(target)
        .bind(details)
        .bind(ip_address)
        .execute(&self.pool)
        .await;

        if let Err(e) = result {
            tracing::error!("Failed to write audit log: {}", e);
        }
    }
}

#[derive(Debug, Serialize, sqlx::FromRow, ToSchema)]
pub struct AuditLog {
    pub id: i64,
    pub user_id: i64,
    pub action: String,
    pub target: String,
    pub details: Option<String>,
    pub ip_address: Option<String>,
    #[schema(value_type = String, format = DateTime)]
    pub created_at: chrono::NaiveDateTime,
}
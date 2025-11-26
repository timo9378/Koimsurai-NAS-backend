use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, Serialize, Deserialize, FromRow)]
pub struct Permission {
    pub id: i64,
    pub user_id: i64,
    pub path: String,
    pub can_read: bool,
    pub can_write: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreatePermissionRequest {
    pub user_id: i64,
    pub path: String,
    pub can_read: bool,
    pub can_write: bool,
}

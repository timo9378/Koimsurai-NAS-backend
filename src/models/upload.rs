use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use sqlx::FromRow;

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct InitUploadRequest {
    pub file_path: String, // Target directory
    pub file_name: String,
    pub total_size: i64,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct InitUploadResponse {
    pub upload_id: String,
}

#[derive(Debug, Serialize, FromRow, ToSchema)]
pub struct UploadSession {
    pub id: String,
    pub user_id: i64,
    pub file_path: String,
    pub file_name: String,
    pub total_size: i64,
    pub uploaded_size: i64,
    #[schema(value_type = String, format = DateTime)]
    pub created_at: chrono::NaiveDateTime,
    #[schema(value_type = String, format = DateTime)]
    pub updated_at: chrono::NaiveDateTime,
}
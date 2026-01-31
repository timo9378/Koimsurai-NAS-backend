use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CreateShareLinkRequest {
    pub file_path: String,
    pub password: Option<String>,
    pub expires_in_seconds: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ShareLinkResponse {
    pub id: String,
    pub url: String,
    pub expires_at: Option<String>,
}

// Upload Link (反向分享連結)
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CreateUploadLinkRequest {
    pub target_path: String,
    pub password: Option<String>,
    pub expires_in_seconds: Option<i64>,
    pub max_files: Option<i32>,
    pub max_file_size: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct UploadLinkResponse {
    pub id: String,
    pub url: String,
    pub expires_at: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct UploadLinkInfoResponse {
    pub id: String,
    pub target_folder: String,
    pub is_password_protected: bool,
    pub expires_at: Option<String>,
    pub max_files: Option<i32>,
    pub max_file_size: Option<i64>,
    pub uploaded_count: i32,
    pub created_at: String,
}

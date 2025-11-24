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

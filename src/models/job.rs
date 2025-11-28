use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use sqlx::FromRow;

#[derive(Debug, Serialize, Deserialize, ToSchema, PartialEq, Clone)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Pending,
    Processing,
    Completed,
    Failed,
}

impl ToString for JobStatus {
    fn to_string(&self) -> String {
        match self {
            JobStatus::Pending => "pending".to_string(),
            JobStatus::Processing => "processing".to_string(),
            JobStatus::Completed => "completed".to_string(),
            JobStatus::Failed => "failed".to_string(),
        }
    }
}

impl From<String> for JobStatus {
    fn from(s: String) -> Self {
        match s.as_str() {
            "pending" => JobStatus::Pending,
            "processing" => JobStatus::Processing,
            "completed" => JobStatus::Completed,
            "failed" => JobStatus::Failed,
            _ => JobStatus::Failed,
        }
    }
}

#[derive(Debug, Serialize, FromRow, ToSchema)]
pub struct Job {
    pub id: String,
    pub job_type: String,
    pub status: String,
    pub progress: i32,
    pub created_at: chrono::NaiveDateTime,
    pub updated_at: chrono::NaiveDateTime,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobUpdate {
    pub job_id: String,
    pub status: JobStatus,
    pub progress: i32,
    pub error: Option<String>,
}
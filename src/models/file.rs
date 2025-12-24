use serde::Serialize;
use utoipa::ToSchema;
use crate::utils::metadata::FileMetadata;

#[derive(Serialize, ToSchema)]
pub struct FileInfo {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: String,
    pub mime_type: Option<String>,
    pub metadata: Option<FileMetadata>,
    pub tags: Vec<Tag>,
    pub is_starred: bool,
}

#[derive(Serialize, ToSchema, Debug, Clone)]
pub struct Tag {
    pub name: String,
    pub color: Option<String>,
}

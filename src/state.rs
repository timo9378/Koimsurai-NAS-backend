use sqlx::{Pool, Sqlite};
use std::path::PathBuf;
use std::sync::Arc;
use dav_server::DavHandler;
use tokio::sync::broadcast;
use crate::utils::queue::JobQueue;
use crate::models::job::JobUpdate;
use crate::services::audit::AuditService;
use crate::services::search::SearchService;

#[derive(Clone)]
pub struct AppState {
    pub pool: Pool<Sqlite>,
    pub storage_path: PathBuf,
    pub queue: Arc<JobQueue>,
    pub webdav: DavHandler,
    pub tx: broadcast::Sender<JobUpdate>,
    pub audit: Arc<AuditService>,
    pub search: Arc<SearchService>,
}


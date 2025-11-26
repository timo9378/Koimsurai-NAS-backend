use sqlx::{Pool, Sqlite};
use std::path::PathBuf;
use std::sync::Arc;
use dav_server::DavHandler;
use crate::utils::queue::JobQueue;

#[derive(Clone)]
pub struct AppState {
    pub pool: Pool<Sqlite>,
    pub storage_path: PathBuf,
    pub queue: Arc<JobQueue>,
    pub webdav: DavHandler,
}


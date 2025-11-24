use sqlx::{Pool, Sqlite};
use std::path::PathBuf;

#[derive(Clone)]
pub struct AppState {
    pub pool: Pool<Sqlite>,
    pub storage_path: PathBuf,
}

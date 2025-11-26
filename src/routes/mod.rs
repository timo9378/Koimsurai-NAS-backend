use axum::{
    routing::{get, post, any},
    Router,
    middleware,
};
use tower_sessions::{SessionManagerLayer, Expiry};
use tower_sessions_sqlx_store::SqliteStore;
use utoipa::OpenApi;
use utoipa_scalar::{Scalar, Servable};
use crate::state::AppState;
use crate::handlers::{auth, file, share, system, webdav, media};
use crate::middleware::auth::require_auth;

use crate::models::{RegisterRequest, LoginRequest, AuthResponse, FileInfo, User, CreateShareLinkRequest, ShareLinkResponse};
use crate::handlers::system::{SystemStatus, DiskInfo};

#[derive(OpenApi)]
#[openapi(
    paths(
        auth::register,
        auth::login,
        auth::logout,
        file::list_files_root,
        file::list_files,
        file::download_file,
        file::upload_file_root,
        file::upload_file,
        file::get_thumbnail,
        file::delete_file,
        share::create_share_link,
        share::access_share_link,
        system::get_system_status
    ),
    components(
        schemas(RegisterRequest, LoginRequest, AuthResponse, FileInfo, User, CreateShareLinkRequest, ShareLinkResponse, SystemStatus, DiskInfo)
    ),
    tags(
        (name = "auth", description = "Authentication endpoints"),
        (name = "file", description = "File management endpoints"),
        (name = "share", description = "Share link endpoints"),
        (name = "system", description = "System monitoring endpoints")
    )
)]
struct ApiDoc;

pub async fn create_router(state: AppState) -> Router {
    // Session store (SqliteStore for persistence)
    let session_store = SqliteStore::new(state.pool.clone());
    session_store.migrate().await.unwrap();

    let session_layer = SessionManagerLayer::new(session_store)
        .with_secure(false) // Set to true in production with HTTPS
        .with_expiry(Expiry::OnInactivity(tower_sessions::cookie::time::Duration::seconds(3600)));

    let auth_routes = Router::new()
        .route("/register", post(auth::register))
        .route("/login", post(auth::login))
        .route("/logout", post(auth::logout));

    let file_routes = Router::new()
        .route("/files", get(file::list_files_root))
        .route("/files/*path", get(file::list_files))
        .route("/download/*path", get(file::download_file))
        .route("/upload", post(file::upload_file_root))
        .route("/upload/*path", post(file::upload_file))
        .route("/files/*path", axum::routing::delete(file::delete_file))
        .route("/thumbnail/:size/*path", get(file::get_thumbnail))
        .route("/share", post(share::create_share_link))
        .route("/system/status", get(system::get_system_status))
        .route("/media/stream", get(media::stream_media))
        .layer(middleware::from_fn(require_auth)); // Protect file routes

    Router::new()
        .merge(Scalar::with_url("/scalar", ApiDoc::openapi()))
        .nest("/api/auth", auth_routes)
        .nest("/api", file_routes)
        .route("/s/:id", get(share::access_share_link)) // Public share link
        .route("/webdav", any(webdav::webdav_handler))
        .route("/webdav/*path", any(webdav::webdav_handler))
        .layer(session_layer)
        .with_state(state)
}

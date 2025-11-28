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
use crate::handlers::{auth, file, share, system, webdav, media, trash, permission, ws, job, upload, tag, audit, version, search};
use crate::middleware::auth::require_auth;



use crate::models::{RegisterRequest, LoginRequest, AuthResponse, FileInfo, User, CreateShareLinkRequest, ShareLinkResponse, InitUploadRequest, InitUploadResponse, UploadSession, Tag};
use crate::handlers::tag::AddTagRequest;
use crate::services::audit::AuditLog;
use crate::utils::versioning::FileVersion;
use crate::services::search::SearchResult;
use crate::handlers::system::{SystemStatus, DiskInfo};
use crate::handlers::media::TimelineGroup;

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
        system::get_system_status,
        job::list_jobs,
        upload::init_upload,
        upload::upload_chunk,
        upload::get_upload_status,
        tag::add_tag,
        tag::remove_tag,
        tag::toggle_star,
        audit::list_audit_logs,
        version::list_file_versions,
        version::restore_version,
        search::search_files,
        media::stream_media,
        media::get_timeline
    ),
    components(
        schemas(RegisterRequest, LoginRequest, AuthResponse, FileInfo, User, CreateShareLinkRequest, ShareLinkResponse, SystemStatus, DiskInfo, InitUploadRequest, InitUploadResponse, UploadSession, Tag, AddTagRequest, AuditLog, FileVersion, SearchResult, TimelineGroup)
    ),
    tags(
        (name = "auth", description = "Authentication endpoints"),
        (name = "file", description = "File management endpoints"),
        (name = "share", description = "Share link endpoints"),
        (name = "system", description = "System monitoring endpoints"),
        (name = "audit", description = "Audit log endpoints"),
        (name = "search", description = "Search endpoints")
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
        .route("/upload/init", post(upload::init_upload))
        .route("/upload/session/:id", axum::routing::patch(upload::upload_chunk).get(upload::get_upload_status))
        .route("/files/*path/tags", post(tag::add_tag))
        .route("/files/*path/tags/:tag_name", axum::routing::delete(tag::remove_tag))
        .route("/files/*path/star", post(tag::toggle_star))
        .route("/files/*path", axum::routing::delete(file::delete_file))
        .route("/files/*path", axum::routing::put(file::rename_file))
        .route("/files/*path/versions", get(version::list_file_versions))
        .route("/files/*path/restore/:version_id", post(version::restore_version))
        .route("/thumbnail/:size/*path", get(file::get_thumbnail))

        .route("/share", post(share::create_share_link))
        .route("/system/status", get(system::get_system_status))
        .route("/media/stream", get(media::stream_media))
        .route("/media/timeline", get(media::get_timeline))
        .route("/trash", get(trash::list_trash))
        .route("/trash/:filename", post(trash::restore_file))
        .route("/trash", axum::routing::delete(trash::empty_trash))
        .route("/permissions", post(permission::set_permission))
        .route("/tasks", get(job::list_jobs))
        .route("/ws", get(ws::ws_handler))
        .route("/audit/logs", get(audit::list_audit_logs))
        .route("/search", get(search::search_files))
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

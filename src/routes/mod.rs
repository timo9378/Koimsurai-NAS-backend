use axum::{
    routing::{get, post, any, delete},
    Router,
    middleware,
    extract::DefaultBodyLimit,
    http::{Method, HeaderValue},
};
use tower_http::cors::{CorsLayer, Any};
use tower_sessions::{SessionManagerLayer, Expiry};
use tower_sessions_sqlx_store::SqliteStore;
use utoipa::OpenApi;
use utoipa_scalar::{Scalar, Servable};
use crate::state::AppState;
use crate::handlers::{auth, file, share, system, webdav, media, trash, permission, ws, job, upload, upload_link, tag, audit, version, search, docker};
use crate::middleware::auth::require_auth;



use crate::models::{RegisterRequest, LoginRequest, EmptyResponse, FileInfo, User, CreateShareLinkRequest, ShareLinkResponse, InitUploadRequest, InitUploadResponse, UploadSession, Tag};
use crate::handlers::tag::AddTagRequest;
use crate::services::audit::AuditLog;
use crate::utils::versioning::FileVersion;
use crate::services::search::SearchResult;
use crate::handlers::system::{SystemStatus, DiskInfo, ConsistencyCheckResult, RescanResult};
use crate::handlers::media::TimelineGroup;

#[derive(OpenApi)]
#[openapi(
    paths(
        auth::register,
        auth::login,
        auth::logout,
        auth::refresh,
        file::list_files_root,
        file::list_files,
        file::list_favorites,
        file::create_folder,
        file::download_file,
        file::upload_file_root,
        file::upload_file,
        file::get_thumbnail,
        file::delete_file,
        file::batch_delete,
        file::batch_move,
        file::batch_copy,
        trash::list_trash,
        trash::restore_file,
        trash::empty_trash,
        share::create_share_link,
        share::access_share_link,
        system::get_system_status,
        system::verify_consistency,
        system::trigger_rescan,
        job::list_jobs,
        upload::init_upload,
        upload::upload_chunk,
        upload::get_upload_status,
        tag::add_tag,
        tag::remove_tag,
        tag::toggle_star,
        tag::list_tags,
        tag::list_files_by_tag,
        audit::list_audit_logs,
        version::list_file_versions,
        version::restore_version,
        search::search_files,
        media::stream_media,
        media::get_timeline
    ),
    components(
        schemas(RegisterRequest, LoginRequest, EmptyResponse, FileInfo, User, CreateShareLinkRequest, ShareLinkResponse, SystemStatus, DiskInfo, ConsistencyCheckResult, RescanResult, InitUploadRequest, InitUploadResponse, UploadSession, Tag, AddTagRequest, AuditLog, FileVersion, SearchResult, TimelineGroup, crate::handlers::file::BatchOperationRequest, crate::handlers::file::FavoriteFileInfo, crate::handlers::file::CreateFolderRequest, crate::handlers::tag::UserTag, crate::handlers::tag::TaggedFile)
    ),
    tags(
        (name = "auth", description = "Authentication endpoints"),
        (name = "file", description = "File management endpoints"),
        (name = "share", description = "Share link endpoints"),
        (name = "system", description = "System monitoring endpoints"),
        (name = "audit", description = "Audit log endpoints"),
        (name = "search", description = "Search endpoints"),
        (name = "tags", description = "Tag management endpoints")
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
        .route("/logout", post(auth::logout))
        .route("/refresh", post(auth::refresh));

    let file_routes = Router::new()
        .route("/files", get(file::list_files_root))
        .route("/files/folder", post(file::create_folder))
        .route("/favorites", get(file::list_favorites))
        .route("/files/batch/delete", post(file::batch_delete))
        .route("/files/batch/move", post(file::batch_move))
        .route("/files/batch/copy", post(file::batch_copy))
        .route("/upload/init", post(upload::init_upload))
        .route("/upload/session/:id", axum::routing::patch(upload::upload_chunk).get(upload::get_upload_status))
        .route("/upload", post(file::upload_file_root))
        .route("/upload/*path", post(file::upload_file))
        .route("/download/*path", get(file::download_file))
        .route("/thumbnail/:size/*path", get(file::get_thumbnail))
        // Tags
        .route("/tags", get(tag::list_tags))
        .route("/tags/:tag_name/files", get(tag::list_files_by_tag))
        .route("/tags/add/*path", post(tag::add_tag))
        .route("/tags/remove/:tag_name/*path", axum::routing::delete(tag::remove_tag))
        .route("/star/file/*path", post(tag::toggle_star))
        .route("/versions/file/*path", get(version::list_file_versions))
        .route("/versions/restore/:version_id", post(version::restore_version))
        .route("/files/*path", get(file::list_files).delete(file::delete_file).put(file::rename_file))

        .route("/share", post(share::create_share_link))
        .route("/upload-link", post(upload_link::create_upload_link))
        .route("/system/status", get(system::get_system_status))
        // 系統管理端點 (適合在 DB 還原後執行)
        .route("/system/verify-consistency", post(system::verify_consistency))
        .route("/system/rescan", post(system::trigger_rescan))
        // 媒體串流
        .route("/media/stream", get(media::stream_media))
        .route("/media/timeline", get(media::get_timeline))
        // HLS 串流
        .route("/media/hls/status", get(media::hls_status))
        .route("/media/hls/serve", get(media::hls_serve))
        .route("/media/hls/qualities", get(media::hls_qualities))
        // 其他
        .route("/trash", get(trash::list_trash))
        .route("/trash/:filename", post(trash::restore_file).delete(trash::permanent_delete))
        .route("/trash", axum::routing::delete(trash::empty_trash))
        .route("/permissions", post(permission::set_permission))
        .route("/tasks", get(job::list_jobs))
        .route("/ws", get(ws::ws_handler))
        .route("/audit/logs", get(audit::list_audit_logs))
        .route("/search", get(search::search_files))
        .layer(middleware::from_fn(require_auth)) // Protect file routes
        // 設置上傳大小限制為 10GB
        .layer(DefaultBodyLimit::max(10 * 1024 * 1024 * 1024)); // 10GB

    // Docker 管理路由（需要認證）
    let docker_routes = Router::new()
        .route("/status", get(docker::docker_status))
        .route("/connect", post(docker::docker_connect))
        // 容器操作
        .route("/containers", get(docker::list_containers))
        .route("/containers/:id", get(docker::inspect_container).delete(docker::remove_container))
        .route("/containers/:id/start", post(docker::start_container))
        .route("/containers/:id/stop", post(docker::stop_container))
        .route("/containers/:id/restart", post(docker::restart_container))
        .route("/containers/:id/logs", get(docker::container_logs))
        .route("/containers/:id/stats", get(docker::container_stats))
        .route("/containers/:id/exec", get(docker::container_exec)) // WebSocket route
        // 鏡像操作
        .route("/images", get(docker::list_images))
        .route("/images/pull", post(docker::pull_image))
        .route("/images/:id", delete(docker::remove_image))
        // 網絡操作
        .route("/networks", get(docker::list_networks))
        .layer(middleware::from_fn(require_auth)); // Protect docker routes



    // Configure CORS for direct frontend-to-backend requests (e.g., file uploads)
    // Note: allow_credentials cannot be used with allow_origin(Any)
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE, Method::PATCH, Method::OPTIONS])
        .allow_headers(Any);

    Router::new()
        .merge(Scalar::with_url("/scalar", ApiDoc::openapi()))
        .nest("/api/auth", auth_routes)
        .nest("/api", file_routes)
        .nest("/api/docker", docker_routes)
        .route("/api/share/:id/download", get(share::access_share_link)) // Public share link - download
        .route("/api/share/:id/info", get(share::get_share_info)) // Public share link - info
        .route("/api/upload-link/:id/upload", post(upload_link::upload_via_link)) // Public upload link - upload
        .route("/api/upload-link/:id/info", get(upload_link::get_upload_link_info)) // Public upload link - info
        .route("/webdav", any(webdav::webdav_handler))
        .route("/webdav/*path", any(webdav::webdav_handler))
        .layer(cors)
        .layer(session_layer)
        .with_state(state)
}

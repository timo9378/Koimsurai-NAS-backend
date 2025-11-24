use axum::{
    routing::{get, post},
    Router,
    middleware,
};
use tower_sessions::{SessionManagerLayer, MemoryStore, Expiry};
use utoipa::OpenApi;
use utoipa_scalar::{Scalar, Servable};
use crate::state::AppState;
use crate::handlers::{auth, file};
use crate::middleware::auth::require_auth;
use crate::models::{RegisterRequest, LoginRequest, AuthResponse, FileInfo, User};

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
        file::upload_file
    ),
    components(
        schemas(RegisterRequest, LoginRequest, AuthResponse, FileInfo, User)
    ),
    tags(
        (name = "auth", description = "Authentication endpoints"),
        (name = "file", description = "File management endpoints")
    )
)]
struct ApiDoc;

pub fn create_router(state: AppState) -> Router {
    // Session store (MemoryStore for simplicity, use Redis/Database in production)
    let session_store = MemoryStore::default();
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
        .layer(middleware::from_fn(require_auth)); // Protect file routes

    Router::new()
        .merge(Scalar::with_url("/scalar", ApiDoc::openapi()))
        .nest("/api/auth", auth_routes)
        .nest("/api", file_routes)
        .layer(session_layer)
        .with_state(state)
}

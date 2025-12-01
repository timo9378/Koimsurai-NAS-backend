//! Docker 容器管理 API 處理器
//!
//! 提供 RESTful API 端點用於管理 Docker 容器和鏡像。

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::error::AppError;
use crate::services::docker::{
    ContainerDetails, ContainerStats, ContainerSummary, DockerService, ImageSummary, LogEntry,
};
use crate::state::AppState;

/// 列出容器的查詢參數
#[derive(Debug, Deserialize)]
pub struct ListContainersQuery {
    /// 是否包含已停止的容器
    #[serde(default)]
    pub all: bool,
}

/// 停止/重啟容器的請求體
#[derive(Debug, Deserialize, ToSchema)]
pub struct StopContainerRequest {
    /// 超時秒數（預設 10 秒）
    pub timeout: Option<i64>,
}

/// 刪除容器的查詢參數
#[derive(Debug, Deserialize)]
pub struct RemoveContainerQuery {
    /// 是否強制刪除
    #[serde(default)]
    pub force: bool,
}

/// 獲取日誌的查詢參數
#[derive(Debug, Deserialize)]
pub struct LogsQuery {
    /// 返回最後 N 行
    pub tail: Option<String>,
    /// 自從某個時間戳以來的日誌
    pub since: Option<i64>,
}

/// 拉取鏡像請求
#[derive(Debug, Deserialize, ToSchema)]
pub struct PullImageRequest {
    pub image: String,
    #[serde(default = "default_tag")]
    pub tag: String,
}

fn default_tag() -> String {
    "latest".to_string()
}

/// 刪除鏡像的查詢參數
#[derive(Debug, Deserialize)]
pub struct RemoveImageQuery {
    #[serde(default)]
    pub force: bool,
}

/// Docker 操作結果
#[derive(Debug, Serialize)]
pub struct DockerResult<T> {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl<T> DockerResult<T> {
    pub fn success(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            message: None,
        }
    }

    pub fn success_message(message: impl Into<String>) -> DockerResult<()> {
        DockerResult {
            success: true,
            data: None,
            message: Some(message.into()),
        }
    }
}

/// Docker 狀態響應
#[derive(Debug, Serialize)]
pub struct DockerStatus {
    pub connected: bool,
    pub version: Option<String>,
    pub api_version: Option<String>,
    pub os: Option<String>,
    pub arch: Option<String>,
}

// ==================== API 處理器 ====================

/// 檢查 Docker 連接狀態
#[utoipa::path(
    get,
    path = "/api/docker/status",
    responses(
        (status = 200, description = "Docker status")
    ),
    tag = "docker"
)]
pub async fn docker_status(
    State(state): State<AppState>,
) -> Result<Json<DockerStatus>, AppError> {
    let service = get_docker_service(&state).await?;

    let connected = service.is_connected().await;
    let mut status = DockerStatus {
        connected,
        version: None,
        api_version: None,
        os: None,
        arch: None,
    };

    if connected {
        if let Ok(version) = service.version().await {
            status.version = version.version;
            status.api_version = version.api_version;
            status.os = version.os;
            status.arch = version.arch;
        }
    }

    Ok(Json(status))
}

/// 連接到 Docker daemon
#[utoipa::path(
    post,
    path = "/api/docker/connect",
    responses(
        (status = 200, description = "Connected to Docker"),
        (status = 500, description = "Failed to connect")
    ),
    tag = "docker"
)]
pub async fn docker_connect(
    State(state): State<AppState>,
) -> Result<Json<DockerResult<()>>, AppError> {
    let service = get_docker_service(&state).await?;

    service
        .connect()
        .await
        .map_err(|e| AppError::Custom(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(DockerResult::<()>::success_message("已連接到 Docker daemon")))
}

// ==================== 容器操作 ====================

/// 列出所有容器
#[utoipa::path(
    get,
    path = "/api/docker/containers",
    params(
        ("all" = Option<bool>, Query, description = "Include stopped containers")
    ),
    responses(
        (status = 200, description = "List of containers")
    ),
    tag = "docker"
)]
pub async fn list_containers(
    State(state): State<AppState>,
    Query(query): Query<ListContainersQuery>,
) -> Result<Json<DockerResult<Vec<ContainerSummary>>>, AppError> {
    let service = get_docker_service(&state).await?;
    ensure_connected(&service).await?;

    let containers = service
        .list_containers(query.all)
        .await
        .map_err(|e| AppError::Custom(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(DockerResult::success(containers)))
}

/// 獲取容器詳細資訊
#[utoipa::path(
    get,
    path = "/api/docker/containers/{id}",
    params(
        ("id" = String, Path, description = "Container ID or name")
    ),
    responses(
        (status = 200, description = "Container details"),
        (status = 404, description = "Container not found")
    ),
    tag = "docker"
)]
pub async fn inspect_container(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<DockerResult<ContainerDetails>>, AppError> {
    let service = get_docker_service(&state).await?;
    ensure_connected(&service).await?;

    let details = service
        .inspect_container(&id)
        .await
        .map_err(|e| AppError::Custom(StatusCode::NOT_FOUND, e.to_string()))?;

    Ok(Json(DockerResult::success(details)))
}

/// 啟動容器
#[utoipa::path(
    post,
    path = "/api/docker/containers/{id}/start",
    params(
        ("id" = String, Path, description = "Container ID or name")
    ),
    responses(
        (status = 200, description = "Container started"),
        (status = 404, description = "Container not found")
    ),
    tag = "docker"
)]
pub async fn start_container(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<DockerResult<()>>, AppError> {
    let service = get_docker_service(&state).await?;
    ensure_connected(&service).await?;

    service
        .start_container(&id)
        .await
        .map_err(|e| AppError::Custom(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(DockerResult::<()>::success_message(format!("容器 {} 已啟動", id))))
}

/// 停止容器
#[utoipa::path(
    post,
    path = "/api/docker/containers/{id}/stop",
    params(
        ("id" = String, Path, description = "Container ID or name")
    ),
    request_body = Option<StopContainerRequest>,
    responses(
        (status = 200, description = "Container stopped"),
        (status = 404, description = "Container not found")
    ),
    tag = "docker"
)]
pub async fn stop_container(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Option<Json<StopContainerRequest>>,
) -> Result<Json<DockerResult<()>>, AppError> {
    let service = get_docker_service(&state).await?;
    ensure_connected(&service).await?;

    let timeout = body.and_then(|b| b.timeout);

    service
        .stop_container(&id, timeout)
        .await
        .map_err(|e| AppError::Custom(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(DockerResult::<()>::success_message(format!("容器 {} 已停止", id))))
}

/// 重啟容器
#[utoipa::path(
    post,
    path = "/api/docker/containers/{id}/restart",
    params(
        ("id" = String, Path, description = "Container ID or name")
    ),
    request_body = Option<StopContainerRequest>,
    responses(
        (status = 200, description = "Container restarted"),
        (status = 404, description = "Container not found")
    ),
    tag = "docker"
)]
pub async fn restart_container(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Option<Json<StopContainerRequest>>,
) -> Result<Json<DockerResult<()>>, AppError> {
    let service = get_docker_service(&state).await?;
    ensure_connected(&service).await?;

    let timeout = body.and_then(|b| b.timeout);

    service
        .restart_container(&id, timeout)
        .await
        .map_err(|e| AppError::Custom(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(DockerResult::<()>::success_message(format!("容器 {} 已重啟", id))))
}

/// 刪除容器
#[utoipa::path(
    delete,
    path = "/api/docker/containers/{id}",
    params(
        ("id" = String, Path, description = "Container ID or name"),
        ("force" = Option<bool>, Query, description = "Force removal")
    ),
    responses(
        (status = 200, description = "Container removed"),
        (status = 404, description = "Container not found")
    ),
    tag = "docker"
)]
pub async fn remove_container(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<RemoveContainerQuery>,
) -> Result<Json<DockerResult<()>>, AppError> {
    let service = get_docker_service(&state).await?;
    ensure_connected(&service).await?;

    service
        .remove_container(&id, query.force)
        .await
        .map_err(|e| AppError::Custom(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(DockerResult::<()>::success_message(format!("容器 {} 已刪除", id))))
}

/// 獲取容器日誌
#[utoipa::path(
    get,
    path = "/api/docker/containers/{id}/logs",
    params(
        ("id" = String, Path, description = "Container ID or name"),
        ("tail" = Option<String>, Query, description = "Number of lines to show"),
        ("since" = Option<i64>, Query, description = "Show logs since timestamp")
    ),
    responses(
        (status = 200, description = "Container logs"),
        (status = 404, description = "Container not found")
    ),
    tag = "docker"
)]
pub async fn container_logs(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<LogsQuery>,
) -> Result<Json<DockerResult<Vec<LogEntry>>>, AppError> {
    let service = get_docker_service(&state).await?;
    ensure_connected(&service).await?;

    let logs = service
        .container_logs(&id, query.tail.as_deref(), query.since)
        .await
        .map_err(|e| AppError::Custom(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(DockerResult::success(logs)))
}

/// 獲取容器統計資訊
#[utoipa::path(
    get,
    path = "/api/docker/containers/{id}/stats",
    params(
        ("id" = String, Path, description = "Container ID or name")
    ),
    responses(
        (status = 200, description = "Container statistics"),
        (status = 404, description = "Container not found")
    ),
    tag = "docker"
)]
pub async fn container_stats(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<DockerResult<ContainerStats>>, AppError> {
    let service = get_docker_service(&state).await?;
    ensure_connected(&service).await?;

    let stats = service
        .container_stats(&id)
        .await
        .map_err(|e| AppError::Custom(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(DockerResult::success(stats)))
}

// ==================== 鏡像操作 ====================

/// 列出所有鏡像
#[utoipa::path(
    get,
    path = "/api/docker/images",
    responses(
        (status = 200, description = "List of images")
    ),
    tag = "docker"
)]
pub async fn list_images(
    State(state): State<AppState>,
) -> Result<Json<DockerResult<Vec<ImageSummary>>>, AppError> {
    let service = get_docker_service(&state).await?;
    ensure_connected(&service).await?;

    let images = service
        .list_images()
        .await
        .map_err(|e| AppError::Custom(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(DockerResult::success(images)))
}

/// 拉取鏡像
#[utoipa::path(
    post,
    path = "/api/docker/images/pull",
    request_body = PullImageRequest,
    responses(
        (status = 200, description = "Image pulled successfully"),
        (status = 500, description = "Failed to pull image")
    ),
    tag = "docker"
)]
pub async fn pull_image(
    State(state): State<AppState>,
    Json(request): Json<PullImageRequest>,
) -> Result<Json<DockerResult<()>>, AppError> {
    let service = get_docker_service(&state).await?;
    ensure_connected(&service).await?;

    service
        .pull_image(&request.image, &request.tag)
        .await
        .map_err(|e| AppError::Custom(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(DockerResult::<()>::success_message(format!(
        "已拉取鏡像 {}:{}",
        request.image, request.tag
    ))))
}

/// 刪除鏡像
#[utoipa::path(
    delete,
    path = "/api/docker/images/{id}",
    params(
        ("id" = String, Path, description = "Image ID or name"),
        ("force" = Option<bool>, Query, description = "Force removal")
    ),
    responses(
        (status = 200, description = "Image removed"),
        (status = 404, description = "Image not found")
    ),
    tag = "docker"
)]
pub async fn remove_image(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<RemoveImageQuery>,
) -> Result<Json<DockerResult<()>>, AppError> {
    let service = get_docker_service(&state).await?;
    ensure_connected(&service).await?;

    service
        .remove_image(&id, query.force)
        .await
        .map_err(|e| AppError::Custom(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(DockerResult::<()>::success_message(format!("已刪除鏡像 {}", id))))
}

// ==================== 輔助函數 ====================

/// 從 AppState 獲取 DockerService
async fn get_docker_service(state: &AppState) -> Result<&DockerService, AppError> {
    state.docker_service.as_ref().map(|arc| arc.as_ref()).ok_or_else(|| {
        AppError::Custom(
            StatusCode::SERVICE_UNAVAILABLE,
            "Docker 服務未啟用".to_string(),
        )
    })
}

/// 確保已連接到 Docker daemon
async fn ensure_connected(service: &DockerService) -> Result<(), AppError> {
    if !service.is_connected().await {
        // 嘗試自動連接
        service.connect().await.map_err(|e| {
            AppError::Custom(
                StatusCode::SERVICE_UNAVAILABLE,
                format!("無法連接到 Docker daemon: {}", e),
            )
        })?;
    }
    Ok(())
}

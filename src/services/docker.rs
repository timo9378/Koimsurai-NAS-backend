//! Docker 容器管理服務
//!
//! 使用 bollard 庫與 Docker daemon 進行交互，
//! 提供容器列表、啟動、停止、重啟等功能。

use bollard::container::LogOutput;
use bollard::query_parameters::{
    CreateImageOptionsBuilder, ListContainersOptionsBuilder, ListImagesOptionsBuilder,
    LogsOptionsBuilder, RemoveContainerOptionsBuilder, RemoveImageOptionsBuilder,
    RestartContainerOptionsBuilder, StatsOptionsBuilder, StopContainerOptionsBuilder,
    ListNetworksOptionsBuilder,
};
use bollard::exec::{CreateExecOptions, StartExecOptions, StartExecResults};
use bollard::Docker;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Docker 服務錯誤類型
#[derive(Debug)]
pub enum DockerError {
    ConnectionFailed(String),
    ContainerError(String),
    ImageError(String),
    ApiError(bollard::errors::Error),
}

impl fmt::Display for DockerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DockerError::ConnectionFailed(msg) => write!(f, "Docker daemon 無法連接: {}", msg),
            DockerError::ContainerError(msg) => write!(f, "容器操作失敗: {}", msg),
            DockerError::ImageError(msg) => write!(f, "鏡像操作失敗: {}", msg),
            DockerError::ApiError(e) => write!(f, "Docker API 錯誤: {}", e),
        }
    }
}

impl std::error::Error for DockerError {}

impl From<bollard::errors::Error> for DockerError {
    fn from(err: bollard::errors::Error) -> Self {
        DockerError::ApiError(err)
    }
}

/// 容器摘要資訊（用於列表顯示）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerSummary {
    pub id: String,
    pub names: Vec<String>,
    pub image: String,
    pub image_id: String,
    pub state: String,
    pub status: String,
    pub created: i64,
    pub ports: Vec<PortMapping>,
}

/// 端口映射資訊
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortMapping {
    pub private_port: u16,
    pub public_port: Option<u16>,
    pub port_type: String,
}

/// 容器詳細資訊
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerDetails {
    pub id: String,
    pub name: String,
    pub image: String,
    pub state: ContainerState,
    pub config: ContainerConfig,
    pub network_settings: NetworkSettings,
    pub mounts: Vec<MountPoint>,
}

/// 容器狀態
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerState {
    pub status: String,
    pub running: bool,
    pub paused: bool,
    pub restarting: bool,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub exit_code: Option<i64>,
}

/// 容器配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerConfig {
    pub hostname: Option<String>,
    pub env: Vec<String>,
    pub cmd: Vec<String>,
    pub working_dir: Option<String>,
}

/// 網絡設置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkSettings {
    pub ip_address: Option<String>,
    pub gateway: Option<String>,
    pub networks: HashMap<String, NetworkInfo>,
}

/// 網絡資訊
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkInfo {
    pub ip_address: Option<String>,
    pub gateway: Option<String>,
    pub mac_address: Option<String>,
}

/// 掛載點資訊
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MountPoint {
    pub mount_type: String,
    pub source: Option<String>,
    pub destination: String,
    pub mode: Option<String>,
    pub rw: bool,
}

/// 容器統計資訊
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerStats {
    pub cpu_percent: f64,
    pub memory_usage: u64,
    pub memory_limit: u64,
    pub memory_percent: f64,
    pub network_rx: u64,
    pub network_tx: u64,
    pub block_read: u64,
    pub block_write: u64,
}

/// 鏡像摘要資訊
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageSummary {
    pub id: String,
    pub repo_tags: Vec<String>,
    pub repo_digests: Vec<String>,
    pub created: i64,
    pub size: i64,
    pub virtual_size: Option<i64>,
}

/// Docker 日誌條目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub stream: String,
    pub message: String,
}

/// 網絡摘要資訊
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkSummary {
    pub id: String,
    pub name: String,
    pub driver: String,
    pub scope: String,
    pub internal: bool,
    pub attachable: bool,
    pub ingress: bool,
    pub ipam_driver: Option<String>,
    pub containers: i32,
}

/// Docker 服務
pub struct DockerService {
    docker: Arc<RwLock<Option<Docker>>>,
}

impl DockerService {
    /// 創建新的 Docker 服務實例
    pub fn new() -> Self {
        Self {
            docker: Arc::new(RwLock::new(None)),
        }
    }

    /// 連接到 Docker daemon
    pub async fn connect(&self) -> Result<(), DockerError> {
        let docker = Docker::connect_with_local_defaults()
            .map_err(|e| DockerError::ConnectionFailed(e.to_string()))?;

        // 測試連接
        docker
            .ping()
            .await
            .map_err(|e| DockerError::ConnectionFailed(e.to_string()))?;

        let mut guard = self.docker.write().await;
        *guard = Some(docker);

        Ok(())
    }

    /// 檢查是否已連接
    pub async fn is_connected(&self) -> bool {
        let guard = self.docker.read().await;
        if let Some(docker) = guard.as_ref() {
            docker.ping().await.is_ok()
        } else {
            false
        }
    }

    /// 獲取 Docker 客戶端（內部使用）
    async fn get_docker(&self) -> Result<Docker, DockerError> {
        let guard = self.docker.read().await;
        guard
            .clone()
            .ok_or_else(|| DockerError::ConnectionFailed("未連接到 Docker daemon".to_string()))
    }

    /// 獲取 Docker 版本資訊
    pub async fn version(&self) -> Result<bollard::models::SystemVersion, DockerError> {
        let docker = self.get_docker().await?;
        Ok(docker.version().await?)
    }

    /// 獲取 Docker 系統資訊
    pub async fn info(&self) -> Result<bollard::models::SystemInfo, DockerError> {
        let docker = self.get_docker().await?;
        Ok(docker.info().await?)
    }

    // ==================== 容器操作 ====================

    /// 列出所有容器（包含已停止的）
    pub async fn list_containers(&self, all: bool) -> Result<Vec<ContainerSummary>, DockerError> {
        let docker = self.get_docker().await?;

        let options = ListContainersOptionsBuilder::default().all(all).build();

        let containers = docker.list_containers(Some(options)).await?;

        let summaries = containers
            .into_iter()
            .map(|c| ContainerSummary {
                id: c.id.unwrap_or_default(),
                names: c.names.unwrap_or_default(),
                image: c.image.unwrap_or_default(),
                image_id: c.image_id.unwrap_or_default(),
                state: c.state.map(|s| s.to_string()).unwrap_or_default(),
                status: c.status.unwrap_or_default(),
                created: c.created.unwrap_or_default(),
                ports: c
                    .ports
                    .unwrap_or_default()
                    .into_iter()
                    .map(|p| PortMapping {
                        private_port: p.private_port,
                        public_port: p.public_port,
                        port_type: p.typ.map(|t| t.to_string()).unwrap_or_default(),
                    })
                    .collect(),
            })
            .collect();

        Ok(summaries)
    }

    /// 獲取容器詳細資訊
    pub async fn inspect_container(&self, id: &str) -> Result<ContainerDetails, DockerError> {
        let docker = self.get_docker().await?;
        let info = docker.inspect_container(id, None::<bollard::query_parameters::InspectContainerOptions>).await?;

        let state = info.state.as_ref();
        let config = info.config.as_ref();
        let network = info.network_settings.as_ref();

        Ok(ContainerDetails {
            id: info.id.unwrap_or_default(),
            name: info.name.unwrap_or_default(),
            image: info.image.unwrap_or_default(),
            state: ContainerState {
                status: state
                    .and_then(|s| s.status.as_ref())
                    .map(|s| s.to_string())
                    .unwrap_or_default(),
                running: state.and_then(|s| s.running).unwrap_or(false),
                paused: state.and_then(|s| s.paused).unwrap_or(false),
                restarting: state.and_then(|s| s.restarting).unwrap_or(false),
                started_at: state.and_then(|s| s.started_at.clone()),
                finished_at: state.and_then(|s| s.finished_at.clone()),
                exit_code: state.and_then(|s| s.exit_code),
            },
            config: ContainerConfig {
                hostname: config.and_then(|c| c.hostname.clone()),
                env: config.and_then(|c| c.env.clone()).unwrap_or_default(),
                cmd: config.and_then(|c| c.cmd.clone()).unwrap_or_default(),
                working_dir: config.and_then(|c| c.working_dir.clone()),
            },
            network_settings: NetworkSettings {
                ip_address: network.and_then(|n| n.ip_address.clone()),
                gateway: network.and_then(|n| n.gateway.clone()),
                networks: network
                    .and_then(|n| n.networks.clone())
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(k, v)| {
                        (
                            k,
                            NetworkInfo {
                                ip_address: v.ip_address,
                                gateway: v.gateway,
                                mac_address: v.mac_address,
                            },
                        )
                    })
                    .collect(),
            },
            mounts: info
                .mounts
                .unwrap_or_default()
                .into_iter()
                .map(|m| MountPoint {
                    mount_type: m.typ.map(|t| t.to_string()).unwrap_or_default(),
                    source: m.source,
                    destination: m.destination.unwrap_or_default(),
                    mode: m.mode,
                    rw: m.rw.unwrap_or(false),
                })
                .collect(),
        })
    }

    /// 啟動容器
    pub async fn start_container(&self, id: &str) -> Result<(), DockerError> {
        let docker = self.get_docker().await?;
        docker.start_container(id, None::<bollard::query_parameters::StartContainerOptions>).await?;
        Ok(())
    }

    /// 停止容器
    pub async fn stop_container(
        &self,
        id: &str,
        timeout_secs: Option<i64>,
    ) -> Result<(), DockerError> {
        let docker = self.get_docker().await?;
        let options = StopContainerOptionsBuilder::default()
            .t(timeout_secs.unwrap_or(10) as i32)
            .build();
        docker.stop_container(id, Some(options)).await?;
        Ok(())
    }

    /// 重啟容器
    pub async fn restart_container(
        &self,
        id: &str,
        timeout_secs: Option<i64>,
    ) -> Result<(), DockerError> {
        let docker = self.get_docker().await?;
        let options = RestartContainerOptionsBuilder::default()
            .t(timeout_secs.unwrap_or(10) as i32)
            .build();
        docker.restart_container(id, Some(options)).await?;
        Ok(())
    }

    /// 刪除容器
    pub async fn remove_container(&self, id: &str, force: bool) -> Result<(), DockerError> {
        let docker = self.get_docker().await?;
        let options = RemoveContainerOptionsBuilder::default().force(force).build();
        docker.remove_container(id, Some(options)).await?;
        Ok(())
    }

    /// 獲取容器日誌
    pub async fn container_logs(
        &self,
        id: &str,
        tail: Option<&str>,
        since: Option<i64>,
    ) -> Result<Vec<LogEntry>, DockerError> {
        let docker = self.get_docker().await?;

        let options = LogsOptionsBuilder::default()
            .stdout(true)
            .stderr(true)
            .tail(tail.unwrap_or("100"))
            .since(since.unwrap_or(0) as i32)
            .build();

        let mut stream = docker.logs(id, Some(options));
        let mut logs = Vec::new();

        while let Some(result) = stream.next().await {
            match result {
                Ok(output) => {
                    let (stream_type, message) = match output {
                        LogOutput::StdOut { message } => {
                            ("stdout".to_string(), String::from_utf8_lossy(&message).to_string())
                        }
                        LogOutput::StdErr { message } => {
                            ("stderr".to_string(), String::from_utf8_lossy(&message).to_string())
                        }
                        _ => continue,
                    };
                    logs.push(LogEntry {
                        stream: stream_type,
                        message,
                    });
                }
                Err(e) => return Err(DockerError::ContainerError(e.to_string())),
            }
        }

        Ok(logs)
    }

    /// 獲取容器即時統計
    pub async fn container_stats(&self, id: &str) -> Result<ContainerStats, DockerError> {
        let docker = self.get_docker().await?;

        let options = StatsOptionsBuilder::default().stream(false).one_shot(true).build();

        let mut stream = docker.stats(id, Some(options));

        if let Some(result) = stream.next().await {
            let stats = result?;

            // 計算 CPU 使用率
            let cpu_stats = stats.cpu_stats.as_ref();
            let precpu_stats = stats.precpu_stats.as_ref();

            let cpu_delta = cpu_stats
                .and_then(|c| c.cpu_usage.as_ref())
                .and_then(|u| u.total_usage)
                .unwrap_or(0) as f64
                - precpu_stats
                    .and_then(|c| c.cpu_usage.as_ref())
                    .and_then(|u| u.total_usage)
                    .unwrap_or(0) as f64;

            let system_delta = cpu_stats.and_then(|c| c.system_cpu_usage).unwrap_or(0) as f64
                - precpu_stats.and_then(|c| c.system_cpu_usage).unwrap_or(0) as f64;

            let num_cpus = cpu_stats.and_then(|c| c.online_cpus).unwrap_or(1) as f64;

            let cpu_percent = if system_delta > 0.0 {
                (cpu_delta / system_delta) * num_cpus * 100.0
            } else {
                0.0
            };

            // 記憶體使用
            let memory_stats = stats.memory_stats.as_ref();
            let memory_usage = memory_stats.and_then(|m| m.usage).unwrap_or(0);
            let memory_limit = memory_stats.and_then(|m| m.limit).unwrap_or(1);
            let memory_percent = (memory_usage as f64 / memory_limit as f64) * 100.0;

            // 網絡統計
            let (network_rx, network_tx) = stats
                .networks
                .as_ref()
                .map(|networks| {
                    networks.values().fold((0u64, 0u64), |(rx, tx), net| {
                        (
                            rx + net.rx_bytes.unwrap_or(0),
                            tx + net.tx_bytes.unwrap_or(0),
                        )
                    })
                })
                .unwrap_or((0, 0));

            // 磁碟 I/O
            let blkio_stats = stats.blkio_stats.as_ref();
            let (block_read, block_write) = blkio_stats
                .and_then(|b| b.io_service_bytes_recursive.as_ref())
                .map(|io| {
                    io.iter().fold((0u64, 0u64), |(read, write), entry| {
                        match entry.op.as_deref() {
                            Some("read") | Some("Read") => {
                                (read + entry.value.unwrap_or(0), write)
                            }
                            Some("write") | Some("Write") => {
                                (read, write + entry.value.unwrap_or(0))
                            }
                            _ => (read, write),
                        }
                    })
                })
                .unwrap_or((0, 0));

            return Ok(ContainerStats {
                cpu_percent,
                memory_usage,
                memory_limit,
                memory_percent,
                network_rx,
                network_tx,
                block_read,
                block_write,
            });
        }

        Err(DockerError::ContainerError("無法獲取統計資訊".to_string()))
    }

    // ==================== 鏡像操作 ====================

    /// 列出所有鏡像
    pub async fn list_images(&self) -> Result<Vec<ImageSummary>, DockerError> {
        let docker = self.get_docker().await?;

        let options = ListImagesOptionsBuilder::default().all(true).build();

        let images = docker.list_images(Some(options)).await?;

        let summaries = images
            .into_iter()
            .map(|i| ImageSummary {
                id: i.id,
                repo_tags: i.repo_tags,
                repo_digests: i.repo_digests,
                created: i.created,
                size: i.size,
                virtual_size: i.virtual_size,
            })
            .collect();

        Ok(summaries)
    }

    /// 拉取鏡像
    pub async fn pull_image(&self, image: &str, tag: &str) -> Result<(), DockerError> {
        let docker = self.get_docker().await?;

        let options = CreateImageOptionsBuilder::default()
            .from_image(image)
            .tag(tag)
            .build();

        let mut stream = docker.create_image(Some(options), None, None);

        while let Some(result) = stream.next().await {
            result?;
        }

        Ok(())
    }

    /// 刪除鏡像
    pub async fn remove_image(&self, id: &str, force: bool) -> Result<(), DockerError> {
        let docker = self.get_docker().await?;

        let options = RemoveImageOptionsBuilder::default().force(force).build();

        docker.remove_image(id, Some(options), None).await?;
        Ok(())
    }

    // ==================== 網絡操作 ====================

    /// 列出所有網絡
    pub async fn list_networks(&self) -> Result<Vec<NetworkSummary>, DockerError> {
        let docker = self.get_docker().await?;

        let options = ListNetworksOptionsBuilder::default().build();

        let networks = docker.list_networks(Some(options)).await?;

        let summaries = networks
            .into_iter()
            .map(|n| {
                let container_count = n.containers
                    .as_ref()
                    .map(|c| c.len() as i32)
                    .unwrap_or(0);
                NetworkSummary {
                    id: n.id.unwrap_or_default(),
                    name: n.name.unwrap_or_default(),
                    driver: n.driver.unwrap_or_default(),
                    scope: n.scope.unwrap_or_default(),
                    internal: n.internal.unwrap_or(false),
                    attachable: n.attachable.unwrap_or(false),
                    ingress: n.ingress.unwrap_or(false),
                    ipam_driver: n.ipam.and_then(|i| i.driver),
                    containers: container_count,
                }
            })
            .collect();

        Ok(summaries)
    }

    // ==================== Exec 操作 ====================

    /// 創建 Exec 實例
    pub async fn create_exec(
        &self,
        container_id: &str,
        cmd: Vec<String>,
    ) -> Result<String, DockerError> {
        let docker = self.get_docker().await?;

        let config = CreateExecOptions {
            attach_stdout: Some(true),
            attach_stderr: Some(true),
            attach_stdin: Some(true),
            tty: Some(true),
            cmd: Some(cmd),
            ..Default::default()
        };

        let result = docker.create_exec(container_id, config).await?;
        Ok(result.id)
    }

    /// 開始 Exec 實例並獲取流
    pub async fn start_exec(&self, exec_id: &str) -> Result<StartExecResults, DockerError> {
        let docker = self.get_docker().await?;

        let config = StartExecOptions {
            detach: false,
            tty: true,
            ..Default::default()
        };

        let result = docker.start_exec(exec_id, Some(config)).await?;
        Ok(result)
    }
}

impl Default for DockerService {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_docker_service_creation() {
        let service = DockerService::new();
        // 不連接時應該返回 false
        assert!(!service.is_connected().await);
    }

    #[tokio::test]
    async fn test_container_summary_serialization() {
        let summary = ContainerSummary {
            id: "abc123".to_string(),
            names: vec!["/test".to_string()],
            image: "nginx:latest".to_string(),
            image_id: "sha256:123".to_string(),
            state: "running".to_string(),
            status: "Up 2 hours".to_string(),
            created: 1234567890,
            ports: vec![PortMapping {
                private_port: 80,
                public_port: Some(8080),
                port_type: "tcp".to_string(),
            }],
        };

        let json = serde_json::to_string(&summary).unwrap();
        assert!(json.contains("abc123"));
        assert!(json.contains("nginx:latest"));
    }

    #[tokio::test]
    async fn test_container_stats_serialization() {
        let stats = ContainerStats {
            cpu_percent: 25.5,
            memory_usage: 1024 * 1024 * 100,
            memory_limit: 1024 * 1024 * 1024,
            memory_percent: 9.765625,
            network_rx: 1000,
            network_tx: 2000,
            block_read: 5000,
            block_write: 3000,
        };

        let json = serde_json::to_string(&stats).unwrap();
        assert!(json.contains("cpu_percent"));
        assert!(json.contains("memory_percent"));
    }
}

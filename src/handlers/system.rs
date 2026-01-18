use axum::{Json, extract::State};
use sysinfo::{System, Disks};
use serde::Serialize;
use crate::state::AppState;
use crate::services::indexer::Indexer;
use std::process::Command;

#[derive(Serialize, utoipa::ToSchema)]
pub struct SystemStatus {
    cpu_usage: f32,
    total_memory: u64,
    used_memory: u64,
    total_swap: u64,
    used_swap: u64,
    disks: Vec<DiskInfo>,
    gpu: Option<GpuInfo>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct DiskInfo {
    name: String,
    mount_point: String,
    total_space: u64,
    available_space: u64,
    disk_type: String,
}

#[derive(Serialize, Clone, utoipa::ToSchema)]
pub struct GpuInfo {
    name: String,
    memory_total: u64,
    memory_used: u64,
    memory_free: u64,
    utilization: f32,
    temperature: u32,
}

fn get_gpu_info() -> Option<GpuInfo> {
    let output = Command::new("nvidia-smi")
        .args([
            "--query-gpu=name,memory.total,memory.used,memory.free,utilization.gpu,temperature.gpu",
            "--format=csv,noheader,nounits"
        ])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout.trim();
    let parts: Vec<&str> = line.split(", ").collect();

    if parts.len() >= 6 {
        Some(GpuInfo {
            name: parts[0].trim().to_string(),
            memory_total: parts[1].trim().parse().unwrap_or(0) * 1024 * 1024, // MiB to bytes
            memory_used: parts[2].trim().parse().unwrap_or(0) * 1024 * 1024,
            memory_free: parts[3].trim().parse().unwrap_or(0) * 1024 * 1024,
            utilization: parts[4].trim().parse().unwrap_or(0.0),
            temperature: parts[5].trim().parse().unwrap_or(0),
        })
    } else {
        None
    }
}

#[utoipa::path(
    get,
    path = "/api/system/status",
    responses(
        (status = 200, description = "System status", body = SystemStatus)
    )
)]
pub async fn get_system_status() -> Json<SystemStatus> {
    let mut sys = System::new_all();
    sys.refresh_all();

    let cpu_usage = sys.global_cpu_usage();
    let total_memory = sys.total_memory();
    let used_memory = sys.used_memory();
    let total_swap = sys.total_swap();
    let used_swap = sys.used_swap();

    let disks = Disks::new_with_refreshed_list();
    
    // Filter out overlay, loop, tmpfs, Docker mounts, and virtual filesystems
    // Only show real physical disks with actual mount points
    let disk_info: Vec<DiskInfo> = disks.list().iter()
        .filter(|disk| {
            let mount = disk.mount_point().to_string_lossy();
            let name = disk.name().to_string_lossy();
            
            // Skip loop devices
            if name.starts_with("loop") {
                return false;
            }
            
            // Skip various virtual/system mounts
            if mount.contains("/snap/") ||
               mount.starts_with("/boot") ||
               mount.starts_with("/run") ||
               mount == "/dev/shm" {
                return false;
            }
            
            // Skip Docker-related mounts (overlay, container config files)
            // These typically have overlay name or short config file mounts
            if name == "overlay" || name.is_empty() {
                return false;
            }
            
            // Skip Docker container config files (resolv.conf, hostname, hosts, etc.)
            let mount_str = mount.to_string();
            if mount_str.contains("/docker/") ||
               mount_str.ends_with("/resolv.conf") ||
               mount_str.ends_with("/hostname") ||
               mount_str.ends_with("/hosts") ||
               mount_str.ends_with("/db") {
                return false;
            }
            
            // Skip NVIDIA driver mounts and system library directories
            // These are bind-mounted by nvidia-container-toolkit
            if mount_str.starts_with("/usr/") ||
               mount_str.starts_with("/lib/") ||
               mount_str.starts_with("/lib64/") ||
               mount_str.contains("nvidia") ||
               mount_str.contains("libnvidia") ||
               mount_str.contains("gsp_") ||
               name.contains("nvidia") ||
               name.starts_with("libnvidia") ||
               name.starts_with("gsp_") {
                return false;
            }
            
            // Only include if it's a real disk with substantial size (at least 1GB)
            disk.total_space() > 1024 * 1024 * 1024
        })
        .map(|disk| {
            let name = disk.name().to_string_lossy().to_string();
            let mount = disk.mount_point().to_string_lossy().to_string();
            
            // Determine disk type based on name
            let disk_type = if name.contains("nvme") {
                "NVMe SSD".to_string()
            } else if name.contains("sd") {
                "HDD".to_string()
            } else {
                "Unknown".to_string()
            };
            
            DiskInfo {
                name,
                mount_point: mount,
                total_space: disk.total_space(),
                available_space: disk.available_space(),
                disk_type,
            }
        })
        .collect();

    // Get GPU info
    let gpu = get_gpu_info();

    Json(SystemStatus {
        cpu_usage,
        total_memory,
        used_memory,
        total_swap,
        used_swap,
        disks: disk_info,
        gpu,
    })
}

/// 一致性檢查結果
#[derive(Serialize, utoipa::ToSchema)]
pub struct ConsistencyCheckResult {
    pub total_db_entries: usize,
    pub removed_orphans: usize,
    pub message: String,
}

/// 觸發資料庫與檔案系統的一致性檢查
/// 這會移除 DB 中存在但磁碟上不存在的檔案記錄
/// 適合在 Litestream 還原 DB 後執行
#[utoipa::path(
    post,
    path = "/api/system/verify-consistency",
    responses(
        (status = 200, description = "Consistency check completed", body = ConsistencyCheckResult)
    )
)]
pub async fn verify_consistency(
    State(state): State<AppState>,
) -> Json<ConsistencyCheckResult> {
    let indexer = Indexer::new(state.pool.clone(), state.storage_path.clone());
    
    match indexer.verify_consistency().await {
        Ok((total, removed)) => {
            Json(ConsistencyCheckResult {
                total_db_entries: total,
                removed_orphans: removed,
                message: format!(
                    "Consistency check complete. Checked {} entries, removed {} orphaned records.",
                    total, removed
                ),
            })
        }
        Err(e) => {
            Json(ConsistencyCheckResult {
                total_db_entries: 0,
                removed_orphans: 0,
                message: format!("Consistency check failed: {}", e),
            })
        }
    }
}

/// 重新掃描結果
#[derive(Serialize, utoipa::ToSchema)]
pub struct RescanResult {
    pub success: bool,
    pub message: String,
}

/// 觸發完整的檔案系統重新掃描
/// 這會同步磁碟狀態到資料庫，包括添加新檔案和移除已刪除的記錄
#[utoipa::path(
    post,
    path = "/api/system/rescan",
    responses(
        (status = 200, description = "Rescan completed", body = RescanResult)
    )
)]
pub async fn trigger_rescan(
    State(state): State<AppState>,
) -> Json<RescanResult> {
    let indexer = Indexer::new(state.pool.clone(), state.storage_path.clone());
    
    match indexer.full_scan().await {
        Ok(()) => {
            Json(RescanResult {
                success: true,
                message: "Full rescan completed successfully.".to_string(),
            })
        }
        Err(e) => {
            Json(RescanResult {
                success: false,
                message: format!("Rescan failed: {}", e),
            })
        }
    }
}

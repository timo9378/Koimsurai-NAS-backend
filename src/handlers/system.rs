use axum::Json;
use sysinfo::{System, Disks};
use serde::Serialize;

#[derive(Serialize, utoipa::ToSchema)]
pub struct SystemStatus {
    cpu_usage: f32,
    total_memory: u64,
    used_memory: u64,
    total_swap: u64,
    used_swap: u64,
    disks: Vec<DiskInfo>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct DiskInfo {
    name: String,
    total_space: u64,
    available_space: u64,
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
    let disk_info = disks.list().iter().map(|disk| DiskInfo {
        name: disk.name().to_string_lossy().to_string(),
        total_space: disk.total_space(),
        available_space: disk.available_space(),
    }).collect();

    Json(SystemStatus {
        cpu_usage,
        total_memory,
        used_memory,
        total_swap,
        used_swap,
        disks: disk_info,
    })
}

use axum::{Json, extract::State};
use sysinfo::{System, Disks};
use serde::Serialize;
use crate::state::AppState;
use crate::services::indexer::Indexer;

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

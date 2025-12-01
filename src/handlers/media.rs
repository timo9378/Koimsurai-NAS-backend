use axum::{
    extract::{State, Query, Extension},
    response::{IntoResponse, Response},
    body::Body,
    http::header,
};
use tokio::process::Command;
use tokio_util::io::ReaderStream;
use std::process::Stdio;
use std::path::PathBuf;
use crate::state::AppState;
use crate::utils::ffmpeg::{FfmpegCommand, HlsQuality};
use crate::utils::queue::JobType;
use serde::{Deserialize, Serialize};
use tracing::{info, warn, error};

#[derive(Deserialize)]
pub struct StreamParams {
    path: String,
    resolution: Option<String>, // e.g., "1280x720"
}

#[utoipa::path(
    get,
    path = "/api/media/stream",
    params(
        ("path" = String, Query, description = "File path"),
        ("resolution" = Option<String>, Query, description = "Target resolution (e.g., 1280x720)")
    ),
    responses(
        (status = 200, description = "Stream media"),
        (status = 503, description = "Transcoding slots full, try again later")
    )
)]
pub async fn stream_media(
    State(state): State<AppState>,
    Query(params): Query<StreamParams>,
) -> impl IntoResponse {
    let file_path = state.storage_path.join(&params.path);
    
    if !file_path.exists() {
        return Response::builder()
            .status(404)
            .body(Body::from("File not found"))
            .unwrap();
    }

    if let Some(resolution) = params.resolution {
        // 嘗試獲取轉碼許可 (非阻塞)
        // Try to acquire transcode permit (non-blocking)
        let permit = match state.transcode_semaphore.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                // 所有轉碼槽位都在使用中
                // All transcode slots are busy
                let max_transcodes = crate::state::get_max_concurrent_transcodes();
                warn!("Transcode request rejected: all {} slots busy", max_transcodes);
                return Response::builder()
                    .status(503)
                    .header("Retry-After", "5")
                    .body(Body::from(format!(
                        "Server is busy with {} concurrent transcodes. Please try again later.",
                        max_transcodes
                    )))
                    .unwrap();
            }
        };

        info!("Starting transcode for {} at resolution {}", params.path, resolution);

        // 使用 FfmpegCommand 建構器生成命令 (支援 GPU 加速)
        let ffmpeg_cmd = FfmpegCommand::new(&file_path.to_string_lossy());
        let std_cmd = ffmpeg_cmd.transcode_stream(&resolution);
        
        // 轉換為 tokio Command
        let child = Command::from(std_cmd)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();

        match child {
            Ok(mut child) => {
                let stdout = child.stdout.take().expect("Failed to open stdout");
                
                // 當 stream 結束時自動釋放 permit
                // Permit is automatically released when the stream ends
                let stream = TranscodeStream::new(stdout, permit);
                
                Response::builder()
                    .header("Content-Type", "video/x-matroska")
                    .body(Body::from_stream(stream))
                    .unwrap()
            }
            Err(e) => {
                // permit 會在這裡被 drop，自動釋放
                Response::builder()
                    .status(500)
                    .body(Body::from(format!("Failed to start transcoding: {}", e)))
                    .unwrap()
            }
        }
    } else {
         // Fallback to direct stream for now if no resolution
         // Ideally, we should use ServeFile for direct play which supports Range requests
         // For now, we just return a message
         
         Response::builder()
            .status(400)
            .body(Body::from("Resolution required for transcoding. For direct play, use /api/download/{path}"))
            .unwrap()
    }
}

/// 包裝 stream 以便在完成時釋放 semaphore permit
/// Wrapper stream that releases semaphore permit when done
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::AsyncRead;
use tokio::sync::OwnedSemaphorePermit;
use tokio_util::bytes::Bytes;

pub struct TranscodeStream<R> {
    inner: ReaderStream<R>,
    _permit: OwnedSemaphorePermit,
}

impl<R: AsyncRead> TranscodeStream<R> {
    pub fn new(reader: R, permit: OwnedSemaphorePermit) -> Self {
        Self {
            inner: ReaderStream::new(reader),
            _permit: permit,
        }
    }
}

impl<R: AsyncRead + Unpin> futures::Stream for TranscodeStream<R> {
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        futures::Stream::poll_next(Pin::new(&mut self.inner), cx)
    }
}

#[derive(Deserialize)]
pub struct TimelineQuery {
    pub group_by: Option<String>, // day, month, year
}

#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct TimelineGroup {
    pub date: String,
    pub files: Vec<crate::models::FileInfo>,
}

#[utoipa::path(
    get,
    path = "/api/media/timeline",
    params(
        ("group_by" = Option<String>, Query, description = "Group by day, month, or year")
    ),
    responses(
        (status = 200, description = "Timeline view", body = Vec<TimelineGroup>)
    )
)]
pub async fn get_timeline(
    State(state): State<AppState>,
    Extension(user_id): Extension<i64>,
    Query(query): Query<TimelineQuery>,
) -> Result<axum::Json<Vec<TimelineGroup>>, crate::error::AppError> {
    // 1. Query all images/videos with metadata
    // We need to join files with permissions to ensure access
    // And we need to parse the EXIF date from metadata (which is stored in files table? No, metadata is extracted on fly)
    // Wait, metadata is NOT stored in DB currently. It's extracted in list_files.
    // This makes timeline view very slow if we have to open every file.
    // OPTIMIZATION: We should store metadata (at least date taken) in the DB.
    
    // For now, let's use the 'modified' time from DB as a proxy for 'date taken' if we don't have EXIF in DB.
    // Or better, let's add a 'taken_at' column to files table?
    // Given the constraints, let's use 'modified' time for now, which is indexed.
    
    let group_format = match query.group_by.as_deref() {
        Some("year") => "%Y",
        Some("month") => "%Y-%m",
        _ => "%Y-%m-%d", // Default day
    };

    let sql = format!(
        r#"
        SELECT
            strftime('{}', modified) as date_group,
            name, is_dir, size, modified, mime_type, parent_path
        FROM files
        WHERE mime_type LIKE 'image/%' OR mime_type LIKE 'video/%'
        ORDER BY modified DESC
        "#,
        group_format
    );

    let rows = sqlx::query_as::<_, (String, String, bool, i64, chrono::NaiveDateTime, Option<String>, String)>(&sql)
        .fetch_all(&state.pool)
        .await
        .map_err(crate::error::AppError::from)?;

    let mut groups: std::collections::HashMap<String, Vec<crate::models::FileInfo>> = std::collections::HashMap::new();

    for (date_group, name, is_dir, size, modified, mime_type, parent_path) in rows {
        // Check permission (naive approach, better to join in SQL)
        let full_path = if parent_path.is_empty() {
            name.clone()
        } else {
            format!("{}/{}", parent_path, name)
        };

        let has_permission = sqlx::query_scalar::<_, bool>(
            "SELECT can_read FROM permissions WHERE user_id = ? AND path = ?"
        )
        .bind(user_id)
        .bind(&full_path)
        .fetch_optional(&state.pool)
        .await
        .map_err(crate::error::AppError::from)?;

        if let Some(can_read) = has_permission {
            if !can_read { continue; }
        }

        let file_info = crate::models::FileInfo {
            name,
            is_dir,
            size: size as u64,
            modified: modified.and_utc().timestamp().to_string(),
            mime_type,
            metadata: None, // Skip heavy metadata extraction for timeline
            tags: vec![],
            is_starred: false,
        };

        groups.entry(date_group).or_default().push(file_info);
    }

    let mut result: Vec<TimelineGroup> = groups.into_iter()
        .map(|(date, files)| TimelineGroup { date, files })
        .collect();

    // Sort groups by date desc
    result.sort_by(|a, b| b.date.cmp(&a.date));

    Ok(axum::Json(result))
}

// ============================================================
//                         HLS 串流
// ============================================================

/// HLS 快取目錄 (相對於 STORAGE_PATH)
const HLS_CACHE_DIR: &str = ".hls_cache";

/// HLS 狀態響應
#[derive(Serialize, utoipa::ToSchema)]
pub struct HlsStatusResponse {
    pub status: String,          // "ready", "processing", "not_found"
    pub qualities: Vec<String>,  // 可用的品質列表
    pub master_playlist: Option<String>,
    pub job_id: Option<String>,  // 如果正在處理中，返回 job_id
}

/// HLS 請求參數
#[derive(Deserialize)]
pub struct HlsParams {
    pub path: String,
    pub quality: Option<String>,  // "1080p", "720p", "480p", "360p", "all"
}

/// 計算檔案的 HLS 快取路徑
fn get_hls_cache_path(storage_path: &std::path::Path, file_path: &str) -> PathBuf {
    use sha2::{Sha256, Digest};
    
    // 使用檔案路徑的 hash 作為快取目錄名稱
    let mut hasher = Sha256::new();
    hasher.update(file_path.as_bytes());
    let hash = format!("{:x}", hasher.finalize());
    let short_hash = &hash[..16]; // 使用前 16 個字元
    
    storage_path.join(HLS_CACHE_DIR).join(short_hash)
}

/// 檢查 HLS 是否已經生成
fn check_hls_ready(cache_path: &std::path::Path, quality: &str) -> (bool, Vec<String>) {
    let mut available_qualities = Vec::new();
    
    // 檢查 master playlist
    let master_exists = cache_path.join("master.m3u8").exists();
    
    // 檢查各個品質
    for q in ["1080p", "720p", "480p", "360p"] {
        let quality_playlist = cache_path.join(q).join("playlist.m3u8");
        if quality_playlist.exists() {
            available_qualities.push(q.to_string());
        }
    }
    
    let ready = if quality == "all" {
        master_exists && !available_qualities.is_empty()
    } else {
        available_qualities.contains(&quality.to_string())
    };
    
    (ready, available_qualities)
}

/// 檢查 HLS 狀態 / 觸發生成
#[utoipa::path(
    get,
    path = "/api/media/hls/status",
    params(
        ("path" = String, Query, description = "Video file path"),
        ("quality" = Option<String>, Query, description = "Quality: 1080p, 720p, 480p, 360p, or all")
    ),
    responses(
        (status = 200, description = "HLS status", body = HlsStatusResponse),
        (status = 404, description = "Video file not found")
    )
)]
pub async fn hls_status(
    State(state): State<AppState>,
    Query(params): Query<HlsParams>,
) -> impl IntoResponse {
    let file_path = state.storage_path.join(&params.path);
    
    if !file_path.exists() {
        return Response::builder()
            .status(404)
            .header("Content-Type", "application/json")
            .body(Body::from(r#"{"error": "Video file not found"}"#))
            .unwrap();
    }
    
    let quality = params.quality.unwrap_or_else(|| "720p".to_string());
    let cache_path = get_hls_cache_path(&state.storage_path, &params.path);
    let (ready, available_qualities) = check_hls_ready(&cache_path, &quality);
    
    if ready {
        let master_playlist = if cache_path.join("master.m3u8").exists() {
            Some(format!("/api/media/hls/serve?path={}&file=master.m3u8", params.path))
        } else {
            None
        };
        
        let response = HlsStatusResponse {
            status: "ready".to_string(),
            qualities: available_qualities,
            master_playlist,
            job_id: None,
        };
        
        Response::builder()
            .status(200)
            .header("Content-Type", "application/json")
            .body(Body::from(serde_json::to_string(&response).unwrap()))
            .unwrap()
    } else {
        // 觸發 HLS 生成任務
        let job_type = JobType::GenerateHls {
            input_path: file_path,
            output_dir: cache_path,
            quality: quality.clone(),
        };
        
        // 發送任務到 queue
        let job_id = match state.queue.enqueue(job_type).await {
            Ok(id) => id,
            Err(e) => {
                error!("Failed to queue HLS generation job: {}", e);
                return Response::builder()
                    .status(500)
                    .header("Content-Type", "application/json")
                    .body(Body::from(r#"{"error": "Failed to queue job"}"#))
                    .unwrap();
            }
        };
        
        info!("Queued HLS generation job {} for {} @ {}", job_id, params.path, quality);
        
        let response = HlsStatusResponse {
            status: "processing".to_string(),
            qualities: available_qualities,
            master_playlist: None,
            job_id: Some(job_id),
        };
        
        Response::builder()
            .status(202)
            .header("Content-Type", "application/json")
            .body(Body::from(serde_json::to_string(&response).unwrap()))
            .unwrap()
    }
}

/// HLS 檔案服務參數
#[derive(Deserialize)]
pub struct HlsServeParams {
    pub path: String,       // 原始影片路徑
    pub file: String,       // HLS 檔案名稱 (e.g., "master.m3u8", "720p/playlist.m3u8", "720p/segment_001.ts")
}

/// 提供 HLS 檔案 (playlist 或 segment)
#[utoipa::path(
    get,
    path = "/api/media/hls/serve",
    params(
        ("path" = String, Query, description = "Original video file path"),
        ("file" = String, Query, description = "HLS file to serve (e.g., master.m3u8, 720p/playlist.m3u8)")
    ),
    responses(
        (status = 200, description = "HLS file content"),
        (status = 404, description = "HLS file not found")
    )
)]
pub async fn hls_serve(
    State(state): State<AppState>,
    Query(params): Query<HlsServeParams>,
) -> impl IntoResponse {
    let cache_path = get_hls_cache_path(&state.storage_path, &params.path);
    let file_path = cache_path.join(&params.file);
    
    // 安全檢查：確保路徑沒有超出快取目錄
    if !file_path.starts_with(&cache_path) {
        return Response::builder()
            .status(403)
            .body(Body::from("Access denied"))
            .unwrap();
    }
    
    if !file_path.exists() {
        return Response::builder()
            .status(404)
            .body(Body::from("HLS file not found"))
            .unwrap();
    }
    
    // 讀取檔案
    match tokio::fs::read(&file_path).await {
        Ok(contents) => {
            let content_type = if params.file.ends_with(".m3u8") {
                "application/vnd.apple.mpegurl"
            } else if params.file.ends_with(".ts") {
                "video/MP2T"
            } else {
                "application/octet-stream"
            };
            
            // 對 m3u8 檔案進行路徑重寫
            let body = if params.file.ends_with(".m3u8") {
                let content = String::from_utf8_lossy(&contents);
                let rewritten = rewrite_hls_urls(&content, &params.path, &params.file);
                Body::from(rewritten)
            } else {
                Body::from(contents)
            };
            
            Response::builder()
                .status(200)
                .header(header::CONTENT_TYPE, content_type)
                .header(header::CACHE_CONTROL, "max-age=31536000") // .ts segments 可以長期快取
                .body(body)
                .unwrap()
        }
        Err(e) => {
            error!("Failed to read HLS file {:?}: {}", file_path, e);
            Response::builder()
                .status(500)
                .body(Body::from("Failed to read file"))
                .unwrap()
        }
    }
}

/// 重寫 HLS playlist 中的 URL
fn rewrite_hls_urls(content: &str, video_path: &str, playlist_file: &str) -> String {
    let mut result = String::new();
    let base_dir = if playlist_file.contains('/') {
        playlist_file.rsplit_once('/').map(|(dir, _)| dir).unwrap_or("")
    } else {
        ""
    };
    
    for line in content.lines() {
        if line.starts_with('#') || line.is_empty() {
            result.push_str(line);
        } else {
            // 這是一個 segment 或子 playlist 的路徑
            let file_ref = if base_dir.is_empty() {
                line.to_string()
            } else {
                format!("{}/{}", base_dir, line)
            };
            let url = format!("/api/media/hls/serve?path={}&file={}", 
                urlencoding::encode(video_path),
                urlencoding::encode(&file_ref));
            result.push_str(&url);
        }
        result.push('\n');
    }
    
    result
}

/// 取得可用的 HLS 品質列表
#[utoipa::path(
    get,
    path = "/api/media/hls/qualities",
    responses(
        (status = 200, description = "Available HLS quality presets")
    )
)]
pub async fn hls_qualities() -> impl IntoResponse {
    let qualities: Vec<serde_json::Value> = HlsQuality::all_presets()
        .iter()
        .map(|q| serde_json::json!({
            "name": q.name,
            "width": q.width,
            "height": q.height,
            "video_bitrate_kbps": q.video_bitrate_kbps,
            "audio_bitrate_kbps": q.audio_bitrate_kbps,
        }))
        .collect();
    
    Response::builder()
        .status(200)
        .header("Content-Type", "application/json")
        .body(Body::from(serde_json::to_string(&qualities).unwrap()))
        .unwrap()
}

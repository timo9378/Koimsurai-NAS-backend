use axum::{
    extract::{State, Query, Extension},
    response::{IntoResponse, Response},
    body::Body,
};
use tokio::process::Command;
use tokio_util::io::ReaderStream;
use std::process::Stdio;
use crate::state::AppState;
use crate::utils::ffmpeg::FfmpegCommand;
use serde::Deserialize;
use tracing::{info, warn};

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

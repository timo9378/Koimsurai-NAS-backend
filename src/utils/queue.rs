use tokio::sync::{mpsc, broadcast};
use std::path::PathBuf;
use tracing::{info, error, warn};
use sqlx::{Pool, Sqlite};
use uuid::Uuid;
use crate::models::job::{JobStatus, JobUpdate};

#[derive(Debug)]
pub enum JobType {
    Transcode {
        input_path: PathBuf,
        output_path: PathBuf,
        resolution: String, // e.g., "1920x1080"
    },
    GenerateThumbnail {
        input_path: PathBuf,
        output_path: PathBuf,
    },
    /// 生成影片 Proxy (低碼率預覽版)
    /// 適用於 GoPro 等高碼率影片的瀏覽器預覽
    GenerateVideoProxy {
        input_path: PathBuf,
        output_path: PathBuf,
        target_height: u32,   // e.g., 720 or 1080
        bitrate_kbps: u32,    // e.g., 2000 (2Mbps)
    },
    /// 生成 HLS 串流檔案 (.m3u8 + .ts segments)
    GenerateHls {
        input_path: PathBuf,
        output_dir: PathBuf,
        quality: String,      // e.g., "1080p", "720p", "480p", "all"
    },
    CopyFiles {
        paths: Vec<String>,
        destination: String,
    },
    IndexFile {
        path: String,
    },
    /// AI 圖片分析任務
    AiAnalyzeImage {
        image_path: String,
    },
}

impl ToString for JobType {
    fn to_string(&self) -> String {
        match self {
            JobType::Transcode { .. } => "transcode".to_string(),
            JobType::GenerateThumbnail { .. } => "generate_thumbnail".to_string(),
            JobType::GenerateVideoProxy { .. } => "generate_video_proxy".to_string(),
            JobType::GenerateHls { .. } => "generate_hls".to_string(),
            JobType::CopyFiles { .. } => "copy_files".to_string(),
            JobType::IndexFile { .. } => "index_file".to_string(),
            JobType::AiAnalyzeImage { .. } => "ai_analyze_image".to_string(),
        }
    }
}

#[derive(Debug)]
pub struct Job {
    pub id: String,
    pub job_type: JobType,
}

pub struct JobQueue {
    sender: mpsc::Sender<Job>,
    pool: Pool<Sqlite>,
}

impl JobQueue {
    pub fn new(buffer_size: usize, pool: Pool<Sqlite>) -> (Self, mpsc::Receiver<Job>) {
        let (sender, receiver) = mpsc::channel(buffer_size);
        (Self { sender, pool }, receiver)
    }

    pub async fn enqueue(&self, job_type: JobType) -> Result<String, String> {
        let job_id = Uuid::new_v4().to_string();
        
        // Persist job to DB
        sqlx::query(
            "INSERT INTO jobs (id, job_type, status) VALUES (?, ?, ?)"
        )
        .bind(&job_id)
        .bind(job_type.to_string())
        .bind(JobStatus::Pending.to_string())
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;

        let job = Job {
            id: job_id.clone(),
            job_type,
        };

        self.sender.send(job).await.map_err(|e| e.to_string())?;
        Ok(job_id)
    }
}

use crate::services::search::SearchService;
use crate::services::ai::AiService;
use std::sync::Arc;

pub async fn worker(
    mut receiver: mpsc::Receiver<Job>,
    pool: Pool<Sqlite>,
    tx: broadcast::Sender<JobUpdate>,
    search_service: Arc<SearchService>,
    ai_service: Option<Arc<AiService>>,
) {
    info!("Job worker started (AI service: {})", if ai_service.is_some() { "enabled" } else { "disabled" });
    while let Some(job) = receiver.recv().await {
        info!("Processing job: {:?}", job);
        
        // Update status to processing
        let _ = sqlx::query("UPDATE jobs SET status = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?")
            .bind(JobStatus::Processing.to_string())
            .bind(&job.id)
            .execute(&pool)
            .await;

        // Broadcast processing status
        let _ = tx.send(JobUpdate {
            job_id: job.id.clone(),
            status: JobStatus::Processing,
            progress: 0,
            error: None,
        });

        let result = match job.job_type {
            JobType::Transcode { input_path, output_path, resolution } => {
                // 使用支援 GPU 加速的 FFmpeg 命令
                use crate::utils::ffmpeg::FfmpegCommand;
                
                let ffmpeg = FfmpegCommand::new(&input_path.to_string_lossy())
                    .output(&output_path.to_string_lossy());
                let mut cmd = ffmpeg.transcode(&resolution);
                let status = cmd.status();

                match status {
                    Ok(s) if s.success() => Ok(()),
                    Ok(s) => Err(format!("Transcoding failed with status: {}", s)),
                    Err(e) => Err(format!("Failed to execute ffmpeg: {}", e)),
                }
            }
            JobType::GenerateThumbnail { input_path, output_path } => {
                // 使用支援 GPU 加速的縮圖生成
                use crate::utils::ffmpeg::FfmpegCommand;
                
                let ffmpeg = FfmpegCommand::new(&input_path.to_string_lossy());
                let mut cmd = ffmpeg.thumbnail(&output_path.to_string_lossy(), 800); // 800px 預設
                let status = cmd.status();

                match status {
                    Ok(s) if s.success() => Ok(()),
                    Ok(s) => Err(format!("Thumbnail generation failed with status: {}", s)),
                    Err(e) => Err(format!("Failed to execute ffmpeg: {}", e)),
                }
            }
            JobType::GenerateVideoProxy { input_path, output_path, target_height, bitrate_kbps } => {
                // 為高碼率影片（如 GoPro）生成低碼率 proxy
                // Proxy 用於預覽，原始檔案保留供下載
                use crate::utils::ffmpeg::FfmpegCommand;
                
                // 確保輸出目錄存在
                if let Some(parent) = output_path.parent() {
                    let _ = tokio::fs::create_dir_all(parent).await;
                }
                
                let ffmpeg = FfmpegCommand::new(&input_path.to_string_lossy());
                let mut cmd = ffmpeg.generate_proxy(
                    &output_path.to_string_lossy(),
                    target_height,
                    bitrate_kbps,
                );
                
                info!("Generating proxy: {:?} -> {:?} @ {}p {}kbps",
                      input_path, output_path, target_height, bitrate_kbps);
                
                let status = cmd.status();

                match status {
                    Ok(s) if s.success() => {
                        info!("Proxy generated successfully: {:?}", output_path);
                        Ok(())
                    },
                    Ok(s) => Err(format!("Proxy generation failed with status: {}", s)),
                    Err(e) => Err(format!("Failed to execute ffmpeg for proxy: {}", e)),
                }
            }
            JobType::GenerateHls { input_path, output_dir, quality } => {
                use crate::utils::ffmpeg::{FfmpegCommand, HlsQuality};
                
                // 確保輸出目錄存在
                match tokio::fs::create_dir_all(&output_dir).await {
                    Err(e) => {
                        error!("Failed to create HLS output directory: {}", e);
                        Err(format!("Failed to create HLS output directory: {}", e))
                    }
                    Ok(_) => {
                        let ffmpeg = FfmpegCommand::new(&input_path.to_string_lossy());
                        
                        if quality == "all" {
                            // 生成所有品質 + master playlist
                            let qualities = HlsQuality::all_presets();
                            let mut success = true;
                            let mut error_msg = String::new();
                            
                            for q in &qualities {
                                let quality_dir = output_dir.join(&q.name);
                                if let Err(e) = tokio::fs::create_dir_all(&quality_dir).await {
                                    error!("Failed to create quality dir {:?}: {}", quality_dir, e);
                                    continue;
                                }
                                
                                let ffmpeg = FfmpegCommand::new(&input_path.to_string_lossy());
                                let mut cmd = ffmpeg.generate_hls(&quality_dir.to_string_lossy(), q);
                                
                                info!("Generating HLS {}: {:?}", q.name, input_path);
                                
                                match cmd.status() {
                                    Ok(s) if s.success() => {
                                        info!("HLS {} generated successfully", q.name);
                                    }
                                    Ok(s) => {
                                        error!("HLS {} generation failed: {}", q.name, s);
                                        success = false;
                                        error_msg = format!("HLS {} failed with status: {}", q.name, s);
                                    }
                                    Err(e) => {
                                        error!("Failed to execute ffmpeg for HLS {}: {}", q.name, e);
                                        success = false;
                                        error_msg = format!("FFmpeg error for {}: {}", q.name, e);
                                    }
                                }
                            }
                            
                            // 生成 master playlist
                            if success {
                                if let Err(e) = FfmpegCommand::generate_master_playlist(&output_dir.to_string_lossy(), &qualities) {
                                    Err(format!("Failed to create master playlist: {}", e))
                                } else {
                                    info!("Master playlist created at {:?}", output_dir);
                                    Ok(())
                                }
                            } else {
                                Err(error_msg)
                            }
                        } else {
                            // 生成單一品質
                            let hls_quality = HlsQuality::from_name(&quality)
                                .unwrap_or_else(HlsQuality::preset_720p);
                            
                            let quality_dir = output_dir.join(&hls_quality.name);
                            match tokio::fs::create_dir_all(&quality_dir).await {
                                Err(e) => Err(format!("Failed to create quality dir: {}", e)),
                                Ok(_) => {
                                    let mut cmd = ffmpeg.generate_hls(&quality_dir.to_string_lossy(), &hls_quality);
                                    
                                    info!("Generating HLS {}: {:?}", quality, input_path);
                                    
                                    match cmd.status() {
                                        Ok(s) if s.success() => {
                                            info!("HLS generated successfully: {:?}", output_dir);
                                            Ok(())
                                        }
                                        Ok(s) => Err(format!("HLS generation failed with status: {}", s)),
                                        Err(e) => Err(format!("Failed to execute ffmpeg for HLS: {}", e)),
                                    }
                                }
                            }
                        }
                    }
                }
            }
            JobType::CopyFiles { paths, destination } => {
                let storage_path = std::env::var("STORAGE_PATH").unwrap_or_else(|_| "storage".to_string());
                let storage_path = PathBuf::from(storage_path);
                let dest_path = storage_path.join(&destination);

                if !dest_path.exists() {
                    let _ = tokio::fs::create_dir_all(&dest_path).await;
                }

                let mut success = true;
                let mut error_msg = String::new();

                for path in paths {
                    let src_path = storage_path.join(&path);
                    if !src_path.exists() { continue; }

                    let file_name = src_path.file_name().unwrap_or_default();
                    let target_path = dest_path.join(file_name);

                    if src_path.is_dir() {
                        if let Err(e) = copy_recursive(&src_path, &target_path).await {
                             error!("Failed to copy directory {:?} to {:?}: {}", src_path, target_path, e);
                             success = false;
                             error_msg = e.to_string();
                        }
                    } else {
                        if let Err(e) = tokio::fs::copy(&src_path, &target_path).await {
                            error!("Failed to copy file {:?} to {:?}: {}", src_path, target_path, e);
                            success = false;
                            error_msg = e.to_string();
                        }
                    }
                }
                
                if success {
                    Ok(())
                } else {
                    Err(error_msg)
                }
            }
            JobType::IndexFile { path } => {
                let storage_path = std::env::var("STORAGE_PATH").unwrap_or_else(|_| "storage".to_string());
                let storage_path = PathBuf::from(storage_path);
                let full_path = storage_path.join(&path);

                if full_path.exists() && full_path.is_file() {
                    // Simple text extraction (for now just read to string if possible)
                    // In real world, use `tika` or similar for PDF/Docx
                    let content = tokio::fs::read_to_string(&full_path).await.unwrap_or_default();
                    let name = full_path.file_name().unwrap_or_default().to_string_lossy().to_string();
                    
                    if let Err(e) = search_service.index_file(&path, &name, &content) {
                        Err(format!("Failed to index file: {:?}", e))
                    } else {
                        Ok(())
                    }
                } else {
                    Err("File not found".to_string())
                }
            }
            JobType::AiAnalyzeImage { image_path } => {
                // AI 圖片分析任務
                match &ai_service {
                    Some(ai) => {
                        info!("AI analyzing image: {}", image_path);
                        match ai.analyze_and_save(&image_path).await {
                            Ok(result) => {
                                info!("AI analysis completed for {}: {} tags detected", 
                                      image_path, result.tags.len());
                                Ok(())
                            }
                            Err(e) => {
                                error!("AI analysis failed for {}: {}", image_path, e);
                                Err(format!("AI analysis failed: {}", e))
                            }
                        }
                    }
                    None => {
                        // AI 服務未啟用，跳過此任務
                        warn!("AI service not enabled, skipping AI analysis job for: {}", image_path);
                        Ok(()) // 返回 Ok 避免任務被標記為失敗
                    }
                }
            }
        };

        // Update final status
        match result {
            Ok(_) => {
                let _ = sqlx::query("UPDATE jobs SET status = ?, progress = 100, updated_at = CURRENT_TIMESTAMP WHERE id = ?")
                    .bind(JobStatus::Completed.to_string())
                    .bind(&job.id)
                    .execute(&pool)
                    .await;

                let _ = tx.send(JobUpdate {
                    job_id: job.id.clone(),
                    status: JobStatus::Completed,
                    progress: 100,
                    error: None,
                });
            }
            Err(e) => {
                let _ = sqlx::query("UPDATE jobs SET status = ?, error = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?")
                    .bind(JobStatus::Failed.to_string())
                    .bind(&e)
                    .bind(&job.id)
                    .execute(&pool)
                    .await;

                let _ = tx.send(JobUpdate {
                    job_id: job.id.clone(),
                    status: JobStatus::Failed,
                    progress: 0,
                    error: Some(e),
                });
            }
        }
    }
}

async fn copy_recursive(src: &PathBuf, dst: &PathBuf) -> std::io::Result<()> {
    tokio::fs::create_dir_all(dst).await?;
    let mut entries = tokio::fs::read_dir(src).await?;
    while let Ok(Some(entry)) = entries.next_entry().await {
        let entry_path = entry.path();
        let file_name = entry.file_name();
        let dest_path = dst.join(file_name);
        if entry_path.is_dir() {
            Box::pin(copy_recursive(&entry_path, &dest_path)).await?;
        } else {
            tokio::fs::copy(&entry_path, &dest_path).await?;
        }
    }
    Ok(())
}

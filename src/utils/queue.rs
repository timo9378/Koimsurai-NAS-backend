use tokio::sync::mpsc;
use std::path::PathBuf;
use std::process::Command;
use tracing::{info, error};

#[derive(Debug)]
pub enum Job {
    Transcode {
        input_path: PathBuf,
        output_path: PathBuf,
        resolution: String, // e.g., "1920x1080"
    },
    GenerateThumbnail {
        input_path: PathBuf,
        output_path: PathBuf,
    },
}

pub struct JobQueue {
    sender: mpsc::Sender<Job>,
}

impl JobQueue {
    pub fn new(buffer_size: usize) -> (Self, mpsc::Receiver<Job>) {
        let (sender, receiver) = mpsc::channel(buffer_size);
        (Self { sender }, receiver)
    }

    pub async fn enqueue(&self, job: Job) -> Result<(), String> {
        self.sender.send(job).await.map_err(|e| e.to_string())
    }
}

pub async fn worker(mut receiver: mpsc::Receiver<Job>) {
    info!("Job worker started");
    while let Some(job) = receiver.recv().await {
        info!("Processing job: {:?}", job);
        match job {
            Job::Transcode { input_path, output_path, resolution } => {
                let status = Command::new("ffmpeg")
                    .arg("-i")
                    .arg(&input_path)
                    .arg("-vf")
                    .arg(format!("scale={}", resolution))
                    .arg(&output_path)
                    .status();

                match status {
                    Ok(s) if s.success() => info!("Transcoding successful: {:?}", output_path),
                    Ok(s) => error!("Transcoding failed with status: {}", s),
                    Err(e) => error!("Failed to execute ffmpeg: {}", e),
                }
            }
            Job::GenerateThumbnail { input_path, output_path } => {
                // Simple thumbnail generation using ffmpeg
                let status = Command::new("ffmpeg")
                    .arg("-i")
                    .arg(&input_path)
                    .arg("-ss")
                    .arg("00:00:01.000")
                    .arg("-vframes")
                    .arg("1")
                    .arg(&output_path)
                    .status();

                match status {
                    Ok(s) if s.success() => info!("Thumbnail generated: {:?}", output_path),
                    Ok(s) => error!("Thumbnail generation failed with status: {}", s),
                    Err(e) => error!("Failed to execute ffmpeg: {}", e),
                }
            }
        }
    }
}

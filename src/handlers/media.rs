use axum::{
    extract::{State, Query},
    response::{IntoResponse, Response},
    body::Body,
};
use tokio::process::Command;
use tokio_util::io::ReaderStream;
use std::process::Stdio;
use crate::state::AppState;
use serde::Deserialize;

#[derive(Deserialize)]
pub struct StreamParams {
    path: String,
    resolution: Option<String>, // e.g., "1280x720"
}

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
        // Transcoding
        // Note: This requires ffmpeg to be installed on the system
        let child = Command::new("ffmpeg")
            .arg("-i")
            .arg(&file_path)
            .arg("-vf")
            .arg(format!("scale={}", resolution))
            .arg("-f")
            .arg("matroska") // Streamable format
            .arg("-") // Output to stdout
            .stdout(Stdio::piped())
            .stderr(Stdio::null()) // Ignore stderr
            .spawn();

        match child {
            Ok(mut child) => {
                let stdout = child.stdout.take().expect("Failed to open stdout");
                let stream = ReaderStream::new(stdout);
                
                Response::builder()
                    .header("Content-Type", "video/x-matroska")
                    .body(Body::from_stream(stream))
                    .unwrap()
            }
            Err(e) => {
                Response::builder()
                    .status(500)
                    .body(Body::from(format!("Failed to start transcoding: {}", e)))
                    .unwrap()
            }
        }
    } else {
        Response::builder()
            .status(400)
            .body(Body::from("Resolution required for transcoding endpoint. Use download endpoint for direct play."))
            .unwrap()
    }
}

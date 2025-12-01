use std::path::Path;
use std::process::Command;
use tracing::{debug, error, warn};

/// 圖片大小限制 (50MB) - 超過此大小的圖片將使用 FFmpeg 處理
/// Image size limit (50MB) - Images larger than this will be processed with FFmpeg
const LARGE_IMAGE_THRESHOLD: u64 = 50 * 1024 * 1024;

/// 使用 FFmpeg 生成縮圖 (支援更多格式，包括 HEIC/HEIF，且不會 OOM)
/// Generate thumbnails using FFmpeg (supports more formats including HEIC/HEIF, won't OOM)
pub async fn generate_thumbnails(file_path: std::path::PathBuf, storage_root: std::path::PathBuf) {
    tokio::task::spawn_blocking(move || {
        generate_thumbnails_sync(&file_path, &storage_root);
    });
}

fn generate_thumbnails_sync(file_path: &std::path::Path, storage_root: &std::path::Path) {
    // 計算相對路徑
    let relative_path = match file_path.strip_prefix(storage_root) {
        Ok(p) => p,
        Err(_) => return,
    };

    let thumb_root = storage_root.join(".thumbnails");
    let thumb_dir = thumb_root.join(relative_path.parent().unwrap_or(Path::new("")));

    if let Err(e) = std::fs::create_dir_all(&thumb_dir) {
        error!("Failed to create thumbnail directory: {}", e);
        return;
    }

    let file_name = file_path.file_name().unwrap_or_default().to_string_lossy();

    // 檢查檔案大小來決定處理方式
    let file_size = std::fs::metadata(file_path)
        .map(|m| m.len())
        .unwrap_or(0);

    if file_size > LARGE_IMAGE_THRESHOLD {
        warn!("Large image detected ({}MB), using FFmpeg for safety", file_size / 1024 / 1024);
    }

    // 定義縮圖尺寸
    let sizes = [
        ("small", 150),
        ("medium", 800),
        ("large", 1920),
    ];

    for (size_name, max_dimension) in sizes {
        let output_path = thumb_dir.join(format!("{}.{}.jpg", file_name, size_name));
        
        // 跳過已存在的縮圖
        if output_path.exists() {
            debug!("Thumbnail already exists: {:?}", output_path);
            continue;
        }

        // 使用 FFmpeg 生成縮圖
        // -vf scale: 保持比例縮放到指定的最大維度
        // -frames:v 1: 只輸出一幀 (對靜態圖片)
        // -q:v 2: JPEG 品質 (1-31, 較低=較好)
        let result = Command::new("ffmpeg")
            .arg("-i")
            .arg(file_path)
            .arg("-vf")
            .arg(format!(
                "scale='if(gt(iw,ih),{0},-2)':'if(gt(iw,ih),-2,{0})'",
                max_dimension
            ))
            .arg("-frames:v")
            .arg("1")
            .arg("-q:v")
            .arg("2")
            .arg("-y") // 覆蓋已存在的檔案
            .arg(&output_path)
            .output();

        match result {
            Ok(output) => {
                if output.status.success() {
                    debug!("Generated {} thumbnail for {:?}", size_name, file_path);
                } else {
                    // FFmpeg 失敗時，嘗試使用 image crate 作為 fallback (僅對小檔案)
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    warn!("FFmpeg failed for {:?}: {}, trying fallback", file_path, stderr);
                    
                    if file_size < LARGE_IMAGE_THRESHOLD {
                        generate_thumbnail_fallback(file_path, &output_path, max_dimension);
                    }
                }
            }
            Err(e) => {
                error!("Failed to execute FFmpeg: {}", e);
                // 嘗試 fallback
                if file_size < LARGE_IMAGE_THRESHOLD {
                    generate_thumbnail_fallback(file_path, &output_path, max_dimension);
                }
            }
        }
    }
}

/// Fallback: 使用 image crate 生成縮圖 (僅用於小檔案)
/// Fallback: Use image crate to generate thumbnails (only for small files)
fn generate_thumbnail_fallback(input_path: &Path, output_path: &Path, max_dimension: u32) {
    use image::ImageReader;
    
    let reader = match ImageReader::open(input_path) {
        Ok(r) => r,
        Err(e) => {
            error!("Failed to open image for fallback thumbnail: {}", e);
            return;
        }
    };

    match reader.decode() {
        Ok(img) => {
            let thumbnail = img.resize(
                max_dimension,
                max_dimension,
                image::imageops::FilterType::Lanczos3,
            );
            if let Err(e) = thumbnail.save(output_path) {
                error!("Failed to save fallback thumbnail: {}", e);
            } else {
                debug!("Generated fallback thumbnail: {:?}", output_path);
            }
        }
        Err(e) => {
            error!("Failed to decode image for fallback thumbnail: {}", e);
        }
    }
}

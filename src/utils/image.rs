use image::ImageReader;
use std::path::Path;
use tokio::task;
use std::fs;

pub async fn generate_thumbnails(file_path: std::path::PathBuf, storage_root: std::path::PathBuf) {
    // Run CPU-intensive task in a blocking thread
    task::spawn_blocking(move || {
        let reader = match ImageReader::open(&file_path) {
            Ok(r) => r,
            Err(_) => return,
        };
        
        if let Ok(img) = reader.decode() {
            // Calculate relative path to maintain structure in .thumbnails
            let relative_path = match file_path.strip_prefix(&storage_root) {
                Ok(p) => p,
                Err(_) => return,
            };

            let thumb_root = storage_root.join(".thumbnails");
            let thumb_dir = thumb_root.join(relative_path.parent().unwrap_or(Path::new("")));
            
            if let Err(_) = fs::create_dir_all(&thumb_dir) {
                return;
            }

            let file_name = file_path.file_name().unwrap_or_default().to_string_lossy();

            // Small (150x150)
            let small = img.resize(150, 150, image::imageops::FilterType::Lanczos3);
            let _ = small.save(thumb_dir.join(format!("{}.small.jpg", file_name)));

            // Medium (800x800)
            let medium = img.resize(800, 800, image::imageops::FilterType::Lanczos3);
            let _ = medium.save(thumb_dir.join(format!("{}.medium.jpg", file_name)));

            // Large (1920x1080) - Preview
            let large = img.resize(1920, 1080, image::imageops::FilterType::Lanczos3);
            let _ = large.save(thumb_dir.join(format!("{}.large.jpg", file_name)));
        }
    });
}

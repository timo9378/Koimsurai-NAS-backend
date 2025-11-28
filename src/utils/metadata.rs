use std::path::Path;
use std::fs::File;
use std::io::BufReader;
use serde::Serialize;
use utoipa::ToSchema;
use exif;
use id3::TagLike;

#[derive(Debug, Serialize, ToSchema, Default)]
pub struct ImageMetadata {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub camera_model: Option<String>,
    pub date_time: Option<String>,
}

#[derive(Debug, Serialize, ToSchema, Default)]
pub struct AudioMetadata {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration_seconds: Option<u32>,
}

#[derive(Debug, Serialize, ToSchema, Default)]
pub struct VideoMetadata {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub duration_seconds: Option<f64>,
}

#[derive(Debug, Serialize, ToSchema)]
pub enum FileMetadata {
    Image(ImageMetadata),
    Audio(AudioMetadata),
    Video(VideoMetadata),
    None,
}

pub fn extract_metadata(path: &Path, mime_type: &str) -> FileMetadata {
    if mime_type.starts_with("image/") {
        extract_image_metadata(path)
    } else if mime_type.starts_with("audio/") {
        extract_audio_metadata(path)
    } else if mime_type.starts_with("video/") {
        extract_video_metadata(path)
    } else {
        FileMetadata::None
    }
}

fn extract_image_metadata(path: &Path) -> FileMetadata {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(_) => return FileMetadata::None,
    };
    let mut bufreader = BufReader::new(&file);
    let exifreader = exif::Reader::new();
    
    let mut metadata = ImageMetadata::default();

    if let Ok(exif) = exifreader.read_from_container(&mut bufreader) {
        if let Some(field) = exif.get_field(exif::Tag::Model, exif::In::PRIMARY) {
            metadata.camera_model = Some(field.display_value().with_unit(&exif).to_string());
        }
        if let Some(field) = exif.get_field(exif::Tag::DateTime, exif::In::PRIMARY) {
            metadata.date_time = Some(field.display_value().with_unit(&exif).to_string());
        }
        if let Some(field) = exif.get_field(exif::Tag::PixelXDimension, exif::In::PRIMARY) {
             if let Some(v) = field.value.get_uint(0) {
                 metadata.width = Some(v);
             }
        }
        if let Some(field) = exif.get_field(exif::Tag::PixelYDimension, exif::In::PRIMARY) {
             if let Some(v) = field.value.get_uint(0) {
                 metadata.height = Some(v);
             }
        }
    }
    
    // Fallback to image crate for dimensions if EXIF failed or missing
    if metadata.width.is_none() || metadata.height.is_none() {
        if let Ok(dim) = image::image_dimensions(path) {
            metadata.width = Some(dim.0);
            metadata.height = Some(dim.1);
        }
    }

    FileMetadata::Image(metadata)
}

fn extract_audio_metadata(path: &Path) -> FileMetadata {
    let mut metadata = AudioMetadata::default();
    
    if let Ok(tag) = id3::Tag::read_from_path(path) {
        metadata.title = tag.title().map(|s| s.to_string());
        metadata.artist = tag.artist().map(|s| s.to_string());
        metadata.album = tag.album().map(|s| s.to_string());
        // id3 crate doesn't always provide duration easily without full parsing, 
        // but let's see if we can get it from mp3 header or similar if needed.
        // For now, basic tags.
    }
    
    FileMetadata::Audio(metadata)
}

fn extract_video_metadata(path: &Path) -> FileMetadata {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(_) => return FileMetadata::None,
    };
    let size = file.metadata().map(|m| m.len()).unwrap_or(0);
    let reader = BufReader::new(file);

    let mut metadata = VideoMetadata::default();

    if let Ok(mp4) = mp4::Mp4Reader::read_header(reader, size) {
        metadata.duration_seconds = Some(mp4.duration().as_secs_f64());
        // mp4 crate might not expose width/height directly in top level, 
        // usually in tracks.
        for track in mp4.tracks().values() {
            if track.track_type().ok() == Some(mp4::TrackType::Video) {
                metadata.width = Some(track.width() as u32);
                metadata.height = Some(track.height() as u32);
                break;
            }
        }
    }

    FileMetadata::Video(metadata)
}
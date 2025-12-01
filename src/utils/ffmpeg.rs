use std::env;
use std::process::Command;
use tracing::{info, debug};

/// GPU 加速類型
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GpuAcceleration {
    /// 無 GPU 加速 (純 CPU)
    None,
    /// NVIDIA CUDA (NVENC/NVDEC)
    Nvidia,
    /// Intel Quick Sync Video
    Intel,
    /// AMD AMF
    Amd,
}

impl GpuAcceleration {
    /// 從環境變數讀取 GPU 加速設定
    pub fn from_env() -> Self {
        let gpu_type = env::var("FFMPEG_GPU_ACCEL")
            .unwrap_or_else(|_| "none".to_string())
            .to_lowercase();
        
        match gpu_type.as_str() {
            "nvidia" | "cuda" | "nvenc" => GpuAcceleration::Nvidia,
            "intel" | "qsv" | "quicksync" => GpuAcceleration::Intel,
            "amd" | "amf" => GpuAcceleration::Amd,
            _ => GpuAcceleration::None,
        }
    }
    
    /// 是否啟用 GPU
    pub fn is_enabled(&self) -> bool {
        *self != GpuAcceleration::None
    }
}

/// FFmpeg 命令建構器
pub struct FfmpegCommand {
    gpu: GpuAcceleration,
    input_path: String,
    output_path: Option<String>,
}

impl FfmpegCommand {
    pub fn new(input_path: &str) -> Self {
        Self {
            gpu: GpuAcceleration::from_env(),
            input_path: input_path.to_string(),
            output_path: None,
        }
    }
    
    pub fn output(mut self, path: &str) -> Self {
        self.output_path = Some(path.to_string());
        self
    }
    
    /// 建構轉碼命令
    pub fn transcode(&self, resolution: &str) -> Command {
        let mut cmd = Command::new("ffmpeg");
        
        // 根據 GPU 類型添加硬體加速參數
        match self.gpu {
            GpuAcceleration::Nvidia => {
                debug!("Using NVIDIA GPU acceleration (NVENC/NVDEC)");
                cmd.arg("-hwaccel").arg("cuda")
                   .arg("-hwaccel_output_format").arg("cuda");
            }
            GpuAcceleration::Intel => {
                debug!("Using Intel QSV acceleration");
                cmd.arg("-hwaccel").arg("qsv")
                   .arg("-hwaccel_output_format").arg("qsv");
            }
            GpuAcceleration::Amd => {
                debug!("Using AMD AMF acceleration");
                cmd.arg("-hwaccel").arg("d3d11va");
            }
            GpuAcceleration::None => {
                debug!("Using CPU-only encoding");
            }
        }
        
        cmd.arg("-i").arg(&self.input_path);
        
        // 視頻濾鏡和編碼器
        match self.gpu {
            GpuAcceleration::Nvidia => {
                // NVIDIA: 使用 scale_cuda 濾鏡避免 GPU<->CPU 記憶體複製
                cmd.arg("-vf").arg(format!("scale_cuda={}", resolution))
                   .arg("-c:v").arg("h264_nvenc")
                   .arg("-preset").arg("p4")      // 效能/畫質平衡
                   .arg("-tune").arg("ll")        // 低延遲
                   .arg("-rc").arg("vbr")
                   .arg("-cq").arg("23");
            }
            GpuAcceleration::Intel => {
                cmd.arg("-vf").arg(format!("scale_qsv={}:format=nv12", resolution))
                   .arg("-c:v").arg("h264_qsv")
                   .arg("-preset").arg("faster")
                   .arg("-global_quality").arg("23");
            }
            GpuAcceleration::Amd => {
                cmd.arg("-vf").arg(format!("scale={}", resolution))
                   .arg("-c:v").arg("h264_amf")
                   .arg("-quality").arg("balanced");
            }
            GpuAcceleration::None => {
                cmd.arg("-vf").arg(format!("scale={}", resolution))
                   .arg("-c:v").arg("libx264")
                   .arg("-preset").arg("ultrafast")
                   .arg("-crf").arg("23");
            }
        }
        
        // 音頻編碼 (通用)
        cmd.arg("-c:a").arg("aac")
           .arg("-b:a").arg("128k");
        
        // 輸出
        if let Some(ref output) = self.output_path {
            cmd.arg("-y").arg(output);
        }
        
        cmd
    }
    
    /// 建構串流轉碼命令 (輸出到 stdout)
    pub fn transcode_stream(&self, resolution: &str) -> Command {
        let mut cmd = self.transcode(resolution);
        
        cmd.arg("-f").arg("matroska")
           .arg("-");  // 輸出到 stdout
        
        cmd
    }
    
    /// 建構縮圖生成命令
    pub fn thumbnail(&self, output_path: &str, max_dimension: u32) -> Command {
        let mut cmd = Command::new("ffmpeg");
        
        // GPU 加速解碼 (如果可用)
        match self.gpu {
            GpuAcceleration::Nvidia => {
                cmd.arg("-hwaccel").arg("cuda");
            }
            GpuAcceleration::Intel => {
                cmd.arg("-hwaccel").arg("qsv");
            }
            _ => {}
        }
        
        cmd.arg("-i").arg(&self.input_path);
        
        // 縮放濾鏡
        let scale_filter = format!(
            "scale='if(gt(iw,ih),{0},-2)':'if(gt(iw,ih),-2,{0})'",
            max_dimension
        );
        
        match self.gpu {
            GpuAcceleration::Nvidia => {
                // NVIDIA GPU 縮放
                let scale_filter = format!(
                    "scale_cuda='if(gt(iw,ih),{0},-2)':'if(gt(iw,ih),-2,{0})'",
                    max_dimension
                );
                cmd.arg("-vf").arg(scale_filter);
            }
            _ => {
                cmd.arg("-vf").arg(scale_filter);
            }
        }
        
        cmd.arg("-frames:v").arg("1")
           .arg("-q:v").arg("2")
           .arg("-y")
           .arg(output_path);
        
        cmd
    }
    
    /// 建構影片 Proxy (低碼率預覽版) 生成命令
    /// 用於 GoPro 等高碼率影片的瀏覽器預覽
    pub fn generate_proxy(&self, output_path: &str, target_height: u32, bitrate_kbps: u32) -> Command {
        let mut cmd = Command::new("ffmpeg");
        
        // GPU 加速
        match self.gpu {
            GpuAcceleration::Nvidia => {
                cmd.arg("-hwaccel").arg("cuda")
                   .arg("-hwaccel_output_format").arg("cuda");
            }
            GpuAcceleration::Intel => {
                cmd.arg("-hwaccel").arg("qsv");
            }
            _ => {}
        }
        
        cmd.arg("-i").arg(&self.input_path);
        
        // 縮放到目標解析度 (保持比例)
        let scale = format!("-2:{}", target_height);
        
        match self.gpu {
            GpuAcceleration::Nvidia => {
                cmd.arg("-vf").arg(format!("scale_cuda={}", scale))
                   .arg("-c:v").arg("h264_nvenc")
                   .arg("-preset").arg("p4")
                   .arg("-b:v").arg(format!("{}k", bitrate_kbps));
            }
            GpuAcceleration::Intel => {
                cmd.arg("-vf").arg(format!("scale_qsv={}:format=nv12", scale))
                   .arg("-c:v").arg("h264_qsv")
                   .arg("-b:v").arg(format!("{}k", bitrate_kbps));
            }
            _ => {
                cmd.arg("-vf").arg(format!("scale={}", scale))
                   .arg("-c:v").arg("libx264")
                   .arg("-preset").arg("fast")
                   .arg("-b:v").arg(format!("{}k", bitrate_kbps));
            }
        }
        
        cmd.arg("-c:a").arg("aac")
           .arg("-b:a").arg("128k")
           .arg("-movflags").arg("+faststart")  // 支援串流播放
           .arg("-y")
           .arg(output_path);
        
        cmd
    }
    
    /// 建構 HLS 轉檔命令
    /// 生成 .m3u8 playlist 和 .ts segments
    pub fn generate_hls(&self, output_dir: &str, quality: &HlsQuality) -> Command {
        let mut cmd = Command::new("ffmpeg");
        
        // GPU 加速解碼
        match self.gpu {
            GpuAcceleration::Nvidia => {
                cmd.arg("-hwaccel").arg("cuda")
                   .arg("-hwaccel_output_format").arg("cuda");
            }
            GpuAcceleration::Intel => {
                cmd.arg("-hwaccel").arg("qsv");
            }
            _ => {}
        }
        
        cmd.arg("-i").arg(&self.input_path);
        
        // 縮放和編碼
        let scale = format!("-2:{}", quality.height);
        
        match self.gpu {
            GpuAcceleration::Nvidia => {
                cmd.arg("-vf").arg(format!("scale_cuda={}", scale))
                   .arg("-c:v").arg("h264_nvenc")
                   .arg("-preset").arg("p4")
                   .arg("-b:v").arg(format!("{}k", quality.video_bitrate_kbps))
                   .arg("-maxrate").arg(format!("{}k", (quality.video_bitrate_kbps as f32 * 1.2) as u32))
                   .arg("-bufsize").arg(format!("{}k", quality.video_bitrate_kbps * 2));
            }
            GpuAcceleration::Intel => {
                cmd.arg("-vf").arg(format!("scale_qsv={}:format=nv12", scale))
                   .arg("-c:v").arg("h264_qsv")
                   .arg("-b:v").arg(format!("{}k", quality.video_bitrate_kbps));
            }
            _ => {
                cmd.arg("-vf").arg(format!("scale={}", scale))
                   .arg("-c:v").arg("libx264")
                   .arg("-preset").arg("fast")
                   .arg("-b:v").arg(format!("{}k", quality.video_bitrate_kbps))
                   .arg("-maxrate").arg(format!("{}k", (quality.video_bitrate_kbps as f32 * 1.2) as u32))
                   .arg("-bufsize").arg(format!("{}k", quality.video_bitrate_kbps * 2));
            }
        }
        
        // 音頻編碼
        cmd.arg("-c:a").arg("aac")
           .arg("-b:a").arg(format!("{}k", quality.audio_bitrate_kbps));
        
        // HLS 特定參數
        let playlist_path = format!("{}/playlist.m3u8", output_dir);
        let segment_pattern = format!("{}/segment_%03d.ts", output_dir);
        
        cmd.arg("-f").arg("hls")
           .arg("-hls_time").arg("6")           // 每個 segment 6 秒
           .arg("-hls_list_size").arg("0")      // 保留所有 segments
           .arg("-hls_segment_filename").arg(&segment_pattern)
           .arg("-hls_playlist_type").arg("vod") // VOD 模式
           .arg("-y")
           .arg(&playlist_path);
        
        cmd
    }
    
    /// 生成多碼率 HLS (Master Playlist)
    /// 這會建立一個 master.m3u8 指向多個品質的子播放清單
    pub fn generate_master_playlist(output_dir: &str, qualities: &[HlsQuality]) -> std::io::Result<()> {
        use std::fs::File;
        use std::io::Write;
        
        let master_path = format!("{}/master.m3u8", output_dir);
        let mut file = File::create(&master_path)?;
        
        writeln!(file, "#EXTM3U")?;
        writeln!(file, "#EXT-X-VERSION:3")?;
        
        for quality in qualities {
            let bandwidth = (quality.video_bitrate_kbps + quality.audio_bitrate_kbps) * 1000;
            writeln!(file, "#EXT-X-STREAM-INF:BANDWIDTH={},RESOLUTION={}x{}",
                bandwidth, quality.width, quality.height)?;
            writeln!(file, "{}/playlist.m3u8", quality.name)?;
        }
        
        Ok(())
    }
}

/// HLS 品質設定
#[derive(Debug, Clone)]
pub struct HlsQuality {
    pub name: String,           // e.g., "1080p", "720p", "480p"
    pub width: u32,
    pub height: u32,
    pub video_bitrate_kbps: u32,
    pub audio_bitrate_kbps: u32,
}

impl HlsQuality {
    pub fn preset_1080p() -> Self {
        Self {
            name: "1080p".to_string(),
            width: 1920,
            height: 1080,
            video_bitrate_kbps: 5000,
            audio_bitrate_kbps: 192,
        }
    }
    
    pub fn preset_720p() -> Self {
        Self {
            name: "720p".to_string(),
            width: 1280,
            height: 720,
            video_bitrate_kbps: 2500,
            audio_bitrate_kbps: 128,
        }
    }
    
    pub fn preset_480p() -> Self {
        Self {
            name: "480p".to_string(),
            width: 854,
            height: 480,
            video_bitrate_kbps: 1000,
            audio_bitrate_kbps: 96,
        }
    }
    
    pub fn preset_360p() -> Self {
        Self {
            name: "360p".to_string(),
            width: 640,
            height: 360,
            video_bitrate_kbps: 600,
            audio_bitrate_kbps: 64,
        }
    }
    
    /// 根據名稱獲取預設品質
    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_lowercase().as_str() {
            "1080p" | "1080" => Some(Self::preset_1080p()),
            "720p" | "720" => Some(Self::preset_720p()),
            "480p" | "480" => Some(Self::preset_480p()),
            "360p" | "360" => Some(Self::preset_360p()),
            _ => None,
        }
    }
    
    /// 取得所有預設品質
    pub fn all_presets() -> Vec<Self> {
        vec![
            Self::preset_1080p(),
            Self::preset_720p(),
            Self::preset_480p(),
            Self::preset_360p(),
        ]
    }
}

/// 檢測 FFmpeg 是否支援指定的硬體加速
pub fn detect_gpu_support() -> GpuAcceleration {
    // 嘗試檢測 NVIDIA
    if Command::new("ffmpeg")
        .args(["-hide_banner", "-encoders"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("h264_nvenc"))
        .unwrap_or(false)
    {
        info!("NVIDIA GPU acceleration (NVENC) detected");
        return GpuAcceleration::Nvidia;
    }
    
    // 嘗試檢測 Intel QSV
    if Command::new("ffmpeg")
        .args(["-hide_banner", "-encoders"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("h264_qsv"))
        .unwrap_or(false)
    {
        info!("Intel Quick Sync Video detected");
        return GpuAcceleration::Intel;
    }
    
    info!("No GPU acceleration detected, using CPU");
    GpuAcceleration::None
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_gpu_from_env() {
        // SAFETY: This test runs in a single thread and we're only setting env vars for testing
        unsafe {
            std::env::set_var("FFMPEG_GPU_ACCEL", "nvidia");
        }
        assert_eq!(GpuAcceleration::from_env(), GpuAcceleration::Nvidia);
        
        unsafe {
            std::env::set_var("FFMPEG_GPU_ACCEL", "none");
        }
        assert_eq!(GpuAcceleration::from_env(), GpuAcceleration::None);
    }
}

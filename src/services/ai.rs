//! AI 圖片標籤服務
//!
//! 使用 CLIP 或 ResNet 等模型進行圖片智能標籤。
//! 設計為可選功能，透過 ENABLE_AI_LABELLING 環境變數控制。
//!
//! 未來可透過 `ort` 或 `candle` crate 整合真實 AI 模型。

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::{Pool, Sqlite};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tracing::{debug, info, warn};

/// AI 標籤結果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiTag {
    /// 標籤名稱 (如 "beach", "cat", "person")
    pub name: String,
    /// 信心分數 (0.0 - 1.0)
    pub confidence: f32,
}

/// AI 分析結果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiAnalysisResult {
    /// 檔案路徑
    pub file_path: String,
    /// 偵測到的標籤
    pub tags: Vec<AiTag>,
    /// 使用的模型名稱
    pub model_name: String,
    /// 分析耗時 (毫秒)
    pub duration_ms: u64,
}

/// AI 服務配置
#[derive(Debug, Clone)]
pub struct AiConfig {
    /// 模型名稱/類型 (預設 "clip-vit-base")
    pub model_name: String,
    /// 最小信心門檻 (低於此值的標籤不會被儲存)
    pub min_confidence: f32,
    /// 最大同時推理數量 (避免 GPU OOM)
    pub max_concurrent_inferences: usize,
    /// 是否使用 GPU
    pub use_gpu: bool,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            model_name: "clip-vit-base".to_string(),
            min_confidence: 0.3,
            max_concurrent_inferences: 4,
            use_gpu: true,
        }
    }
}

/// AI 圖片標籤服務
///
/// 目前為 stub 實作，返回空結果。
/// 未來可整合 `ort` (ONNX Runtime) 或 `candle` 進行真實推理。
pub struct AiService {
    config: AiConfig,
    pool: Pool<Sqlite>,
    /// 推理信號量，限制同時推理數量
    inference_semaphore: Arc<Semaphore>,
}

impl AiService {
    /// 創建新的 AI 服務實例
    pub fn new(pool: Pool<Sqlite>, config: Option<AiConfig>) -> Self {
        let config = config.unwrap_or_default();
        let inference_semaphore = Arc::new(Semaphore::new(config.max_concurrent_inferences));

        info!(
            "AI Service initialized: model={}, min_confidence={}, max_concurrent={}, gpu={}",
            config.model_name, config.min_confidence, config.max_concurrent_inferences, config.use_gpu
        );

        Self {
            config,
            pool,
            inference_semaphore,
        }
    }

    /// 從環境變數創建配置
    pub fn config_from_env() -> AiConfig {
        AiConfig {
            model_name: std::env::var("AI_MODEL_NAME")
                .unwrap_or_else(|_| "clip-vit-base".to_string()),
            min_confidence: std::env::var("AI_MIN_CONFIDENCE")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.3),
            max_concurrent_inferences: std::env::var("AI_MAX_CONCURRENT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(4),
            use_gpu: std::env::var("AI_USE_GPU")
                .map(|v| v.to_lowercase() == "true")
                .unwrap_or(true),
        }
    }

    /// 偵測圖片標籤 (Stub 實作)
    ///
    /// 目前返回空結果。未來整合 AI 模型後會進行真實推理。
    ///
    /// # 參數
    /// - `image_path`: 圖片檔案路徑
    ///
    /// # 返回
    /// - AI 分析結果，包含偵測到的標籤
    pub async fn detect_tags(&self, image_path: &str) -> Result<AiAnalysisResult> {
        // 獲取推理信號量，限制並發
        let _permit = self.inference_semaphore.acquire().await?;

        let start = std::time::Instant::now();

        debug!("AI analysis requested for: {}", image_path);

        // ========================================
        // STUB 實作 - 目前返回空結果
        // 未來可在此整合真實 AI 模型：
        //
        // 使用 ort (ONNX Runtime):
        // ```
        // use ort::{Session, Environment};
        // let env = Environment::builder().build()?;
        // let session = Session::builder(&env)?.with_model("clip.onnx")?;
        // let outputs = session.run(inputs)?;
        // ```
        //
        // 使用 candle:
        // ```
        // use candle_core::{Device, Tensor};
        // use candle_transformers::models::clip;
        // let device = Device::new_cuda(0)?;
        // let model = clip::ClipModel::new(...)?;
        // ```
        // ========================================

        let tags: Vec<AiTag> = Vec::new();

        let duration_ms = start.elapsed().as_millis() as u64;

        let result = AiAnalysisResult {
            file_path: image_path.to_string(),
            tags,
            model_name: self.config.model_name.clone(),
            duration_ms,
        };

        debug!(
            "AI analysis completed for {} in {}ms (stub - no tags)",
            image_path, duration_ms
        );

        Ok(result)
    }

    /// 分析圖片並儲存結果到資料庫
    pub async fn analyze_and_save(&self, image_path: &str) -> Result<AiAnalysisResult> {
        // 檢查是否已分析
        if self.is_analyzed(image_path).await? {
            debug!("Image already analyzed: {}", image_path);
            return self.get_existing_tags(image_path).await;
        }

        // 進行分析
        let result = self.detect_tags(image_path).await?;

        // 儲存標籤
        self.save_tags(&result).await?;

        // 更新分析狀態
        self.mark_analyzed(image_path).await?;

        Ok(result)
    }

    /// 檢查圖片是否已被分析
    pub async fn is_analyzed(&self, image_path: &str) -> Result<bool> {
        let result = sqlx::query_scalar::<_, i32>(
            "SELECT COUNT(*) FROM ai_analysis_status WHERE file_path = ? AND status = 'completed'",
        )
        .bind(image_path)
        .fetch_one(&self.pool)
        .await?;

        Ok(result > 0)
    }

    /// 獲取已存在的標籤
    async fn get_existing_tags(&self, image_path: &str) -> Result<AiAnalysisResult> {
        let tags = sqlx::query_as::<_, (String, f64)>(
            "SELECT tag_name, confidence FROM image_ai_tags WHERE file_path = ?",
        )
        .bind(image_path)
        .fetch_all(&self.pool)
        .await?;

        Ok(AiAnalysisResult {
            file_path: image_path.to_string(),
            tags: tags
                .into_iter()
                .map(|(name, confidence)| AiTag {
                    name,
                    confidence: confidence as f32,
                })
                .collect(),
            model_name: self.config.model_name.clone(),
            duration_ms: 0,
        })
    }

    /// 儲存標籤到資料庫
    async fn save_tags(&self, result: &AiAnalysisResult) -> Result<()> {
        for tag in &result.tags {
            // 只儲存信心度超過門檻的標籤
            if tag.confidence >= self.config.min_confidence {
                sqlx::query(
                    r#"
                    INSERT OR REPLACE INTO image_ai_tags (file_path, tag_name, confidence, model_name)
                    VALUES (?, ?, ?, ?)
                    "#,
                )
                .bind(&result.file_path)
                .bind(&tag.name)
                .bind(tag.confidence as f64)
                .bind(&result.model_name)
                .execute(&self.pool)
                .await?;
            }
        }

        debug!(
            "Saved {} tags for {}",
            result.tags.len(),
            result.file_path
        );

        Ok(())
    }

    /// 標記圖片為已分析
    async fn mark_analyzed(&self, image_path: &str) -> Result<()> {
        sqlx::query(
            r#"
            INSERT OR REPLACE INTO ai_analysis_status (file_path, model_version, status)
            VALUES (?, ?, 'completed')
            "#,
        )
        .bind(image_path)
        .bind(&self.config.model_name)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// 刪除圖片的 AI 標籤 (當圖片被刪除時)
    pub async fn delete_tags(&self, image_path: &str) -> Result<()> {
        sqlx::query("DELETE FROM image_ai_tags WHERE file_path = ?")
            .bind(image_path)
            .execute(&self.pool)
            .await?;

        sqlx::query("DELETE FROM ai_analysis_status WHERE file_path = ?")
            .bind(image_path)
            .execute(&self.pool)
            .await?;

        debug!("Deleted AI tags for: {}", image_path);

        Ok(())
    }

    /// 根據標籤搜尋圖片
    pub async fn search_by_tag(
        &self,
        tag_name: &str,
        min_confidence: Option<f32>,
    ) -> Result<Vec<String>> {
        let min_conf = min_confidence.unwrap_or(self.config.min_confidence);

        let results = sqlx::query_scalar::<_, String>(
            r#"
            SELECT DISTINCT file_path FROM image_ai_tags
            WHERE tag_name LIKE ? AND confidence >= ?
            ORDER BY confidence DESC
            "#,
        )
        .bind(format!("%{}%", tag_name))
        .bind(min_conf as f64)
        .fetch_all(&self.pool)
        .await?;

        Ok(results)
    }

    /// 獲取所有可用標籤 (用於自動完成)
    pub async fn get_all_tags(&self) -> Result<Vec<(String, i32)>> {
        let results = sqlx::query_as::<_, (String, i32)>(
            r#"
            SELECT tag_name, COUNT(*) as count FROM image_ai_tags
            GROUP BY tag_name
            ORDER BY count DESC
            LIMIT 100
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(results)
    }

    /// 獲取圖片的所有標籤
    pub async fn get_image_tags(&self, image_path: &str) -> Result<Vec<AiTag>> {
        let tags = sqlx::query_as::<_, (String, f64)>(
            "SELECT tag_name, confidence FROM image_ai_tags WHERE file_path = ? ORDER BY confidence DESC",
        )
        .bind(image_path)
        .fetch_all(&self.pool)
        .await?;

        Ok(tags
            .into_iter()
            .map(|(name, confidence)| AiTag {
                name,
                confidence: confidence as f32,
            })
            .collect())
    }

    /// 獲取統計資訊
    pub async fn get_stats(&self) -> Result<AiStats> {
        let total_images = sqlx::query_scalar::<_, i32>(
            "SELECT COUNT(*) FROM ai_analysis_status WHERE status = 'completed'",
        )
        .fetch_one(&self.pool)
        .await?;

        let total_tags =
            sqlx::query_scalar::<_, i32>("SELECT COUNT(DISTINCT tag_name) FROM image_ai_tags")
                .fetch_one(&self.pool)
                .await?;

        let pending_images = sqlx::query_scalar::<_, i32>(
            r#"
            SELECT COUNT(*) FROM files f
            WHERE f.mime_type LIKE 'image/%'
            AND NOT EXISTS (
                SELECT 1 FROM ai_analysis_status a WHERE a.file_path = f.path
            )
            "#,
        )
        .fetch_one(&self.pool)
        .await
        .unwrap_or(0);

        Ok(AiStats {
            total_analyzed_images: total_images as u32,
            total_unique_tags: total_tags as u32,
            pending_images: pending_images as u32,
            model_name: self.config.model_name.clone(),
            gpu_enabled: self.config.use_gpu,
        })
    }
}

/// AI 服務統計
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiStats {
    pub total_analyzed_images: u32,
    pub total_unique_tags: u32,
    pub pending_images: u32,
    pub model_name: String,
    pub gpu_enabled: bool,
}

/// 檢查檔案是否為圖片
pub fn is_image_file(path: &Path) -> bool {
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase());

    matches!(
        extension.as_deref(),
        Some("jpg") | Some("jpeg") | Some("png") | Some("gif") | Some("webp") | Some("bmp")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_image_file() {
        assert!(is_image_file(Path::new("photo.jpg")));
        assert!(is_image_file(Path::new("photo.JPG")));
        assert!(is_image_file(Path::new("photo.png")));
        assert!(is_image_file(Path::new("photo.gif")));
        assert!(is_image_file(Path::new("photo.webp")));
        assert!(!is_image_file(Path::new("video.mp4")));
        assert!(!is_image_file(Path::new("document.pdf")));
    }

    #[test]
    fn test_ai_config_default() {
        let config = AiConfig::default();
        assert_eq!(config.model_name, "clip-vit-base");
        assert_eq!(config.min_confidence, 0.3);
        assert_eq!(config.max_concurrent_inferences, 4);
        assert!(config.use_gpu);
    }

    #[test]
    fn test_ai_tag_serialization() {
        let tag = AiTag {
            name: "beach".to_string(),
            confidence: 0.95,
        };

        let json = serde_json::to_string(&tag).unwrap();
        assert!(json.contains("beach"));
        assert!(json.contains("0.95"));
    }
}

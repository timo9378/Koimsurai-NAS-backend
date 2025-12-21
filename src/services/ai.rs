//! AI 圖片標籤服務
//!
//! 使用 CLIP 模型進行圖片智能標籤。
//! 設計為可選功能，透過 ENABLE_AI_LABELLING 環境變數控制。
//!
//! 使用 `candle` crate 進行推理，支援 CUDA GPU 加速。

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::{Pool, Sqlite};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tracing::{debug, info, warn, error};

#[cfg(feature = "ai")]
use {
    candle_core::{Device, Tensor, DType},
    candle_nn::VarBuilder,
    candle_transformers::models::clip,
    hf_hub::{api::sync::Api, Repo, RepoType},
    tokenizers::Tokenizer,
    std::sync::RwLock,
};

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
    /// 返回的最大標籤數量
    pub max_tags: usize,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            model_name: "openai/clip-vit-base-patch32".to_string(),
            min_confidence: 0.3,
            max_concurrent_inferences: 4,
            use_gpu: true,
            max_tags: 5,
        }
    }
}

/// 預定義的標籤集合 (用於 CLIP zero-shot 分類)
#[cfg(feature = "ai")]
const PREDEFINED_TAGS: &[&str] = &[
    // 場景
    "beach", "forest", "mountain", "city", "countryside", "desert", "ocean", "lake", "river", "sunset",
    "sunrise", "night", "indoor", "outdoor", "garden", "park", "street", "building", "house", "room",
    // 動物
    "cat", "dog", "bird", "fish", "horse", "cow", "sheep", "elephant", "lion", "tiger",
    "bear", "rabbit", "deer", "butterfly", "insect",
    // 人物相關
    "person", "people", "crowd", "portrait", "selfie", "family", "friends", "child", "baby", "group",
    // 活動
    "party", "wedding", "birthday", "vacation", "travel", "hiking", "sports", "swimming", "running", "cycling",
    // 食物
    "food", "meal", "breakfast", "lunch", "dinner", "dessert", "fruit", "vegetables", "drink", "coffee",
    // 物品
    "car", "bicycle", "motorcycle", "boat", "airplane", "train", "phone", "computer", "book", "flower",
    // 藝術/風格
    "art", "painting", "drawing", "photography", "black and white", "colorful", "vintage", "modern", "abstract",
    // 季節/天氣
    "spring", "summer", "autumn", "winter", "sunny", "cloudy", "rainy", "snowy", "foggy",
    // 情感
    "happy", "sad", "romantic", "peaceful", "exciting", "beautiful", "cute", "funny",
];

/// AI 圖片標籤服務
pub struct AiService {
    config: AiConfig,
    pool: Pool<Sqlite>,
    /// 推理信號量，限制同時推理數量
    inference_semaphore: Arc<Semaphore>,
    /// CLIP 模型 (懶加載)
    #[cfg(feature = "ai")]
    clip_model: RwLock<Option<ClipModel>>,
}

/// CLIP 模型封裝
#[cfg(feature = "ai")]
struct ClipModel {
    vision_model: clip::ClipVisionTransformer,
    text_model: clip::ClipTextTransformer,
    tokenizer: Tokenizer,
    device: Device,
    /// 預計算的文字嵌入 (標籤)
    text_embeddings: Tensor,
    /// 對應的標籤名稱
    tag_names: Vec<String>,
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
            #[cfg(feature = "ai")]
            clip_model: RwLock::new(None),
        }
    }

    /// 從環境變數創建配置
    pub fn config_from_env() -> AiConfig {
        AiConfig {
            model_name: std::env::var("AI_MODEL_NAME")
                .unwrap_or_else(|_| "openai/clip-vit-base-patch32".to_string()),
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
            max_tags: std::env::var("AI_MAX_TAGS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(5),
        }
    }

    /// 載入 CLIP 模型 (懶加載)
    #[cfg(feature = "ai")]
    async fn load_model(&self) -> Result<()> {
        // 檢查是否已載入
        {
            let model = self.clip_model.read().map_err(|e| anyhow::anyhow!("Lock error: {}", e))?;
            if model.is_some() {
                return Ok(());
            }
        }

        info!("Loading CLIP model: {}", self.config.model_name);
        let start = std::time::Instant::now();

        // 選擇設備
        let device = if self.config.use_gpu {
            match Device::new_cuda(0) {
                Ok(d) => {
                    info!("Using CUDA device for AI inference");
                    d
                }
                Err(e) => {
                    warn!("CUDA not available ({}), falling back to CPU", e);
                    Device::Cpu
                }
            }
        } else {
            info!("Using CPU for AI inference");
            Device::Cpu
        };

        // 下載模型
        let api = Api::new()?;
        let repo = api.repo(Repo::new(self.config.model_name.clone(), RepoType::Model));

        // 載入模型權重
        let model_file = repo.get("model.safetensors")
            .or_else(|_| repo.get("pytorch_model.bin"))?;
        
        let config_file = repo.get("config.json")?;
        let tokenizer_file = repo.get("tokenizer.json")?;

        // 解析配置
        let config_content = std::fs::read_to_string(&config_file)?;
        let clip_config: clip::ClipConfig = serde_json::from_str(&config_content)?;

        // 載入權重
        let vb = unsafe { VarBuilder::from_mmaped_safetensors(&[model_file], DType::F32, &device)? };

        // 創建視覺模型
        let vision_model = clip::ClipVisionTransformer::new(vb.pp("vision_model"), &clip_config.vision_config)?;
        
        // 創建文字模型
        let text_model = clip::ClipTextTransformer::new(vb.pp("text_model"), &clip_config.text_config)?;

        // 載入 tokenizer
        let tokenizer = Tokenizer::from_file(&tokenizer_file)
            .map_err(|e| anyhow::anyhow!("Failed to load tokenizer: {}", e))?;

        // 預計算所有標籤的文字嵌入
        let tag_names: Vec<String> = PREDEFINED_TAGS.iter().map(|s| s.to_string()).collect();
        let text_prompts: Vec<String> = tag_names.iter().map(|t| format!("a photo of {}", t)).collect();
        
        // Tokenize 所有標籤
        let encodings = tokenizer.encode_batch(text_prompts.clone(), true)
            .map_err(|e| anyhow::anyhow!("Tokenization error: {}", e))?;
        
        let max_len = encodings.iter().map(|e| e.get_ids().len()).max().unwrap_or(77);
        let max_len = max_len.min(77); // CLIP 最大長度
        
        let mut input_ids = Vec::new();
        for encoding in &encodings {
            let mut ids = encoding.get_ids().to_vec();
            ids.truncate(max_len);
            while ids.len() < max_len {
                ids.push(0);
            }
            input_ids.extend(ids);
        }
        
        let input_tensor = Tensor::from_vec(
            input_ids.iter().map(|&x| x as i64).collect::<Vec<_>>(),
            (encodings.len(), max_len),
            &device,
        )?;

        // 計算文字嵌入
        let text_embeddings = text_model.forward(&input_tensor)?;
        
        // L2 正規化
        let text_embeddings = Self::l2_normalize(&text_embeddings)?;

        let load_time = start.elapsed();
        info!("CLIP model loaded in {:?}", load_time);

        // 儲存模型
        let mut model = self.clip_model.write().map_err(|e| anyhow::anyhow!("Lock error: {}", e))?;
        *model = Some(ClipModel {
            vision_model,
            text_model,
            tokenizer,
            device,
            text_embeddings,
            tag_names,
        });

        Ok(())
    }

    /// L2 正規化
    #[cfg(feature = "ai")]
    fn l2_normalize(tensor: &Tensor) -> Result<Tensor> {
        let norm = tensor.sqr()?.sum_keepdim(1)?.sqrt()?;
        Ok(tensor.broadcast_div(&norm)?)
    }

    /// 偵測圖片標籤 (帶有 AI feature 時的實作)
    #[cfg(feature = "ai")]
    pub async fn detect_tags(&self, image_path: &str) -> Result<AiAnalysisResult> {
        // 獲取推理信號量，限制並發
        let _permit = self.inference_semaphore.acquire().await?;

        let start = std::time::Instant::now();
        debug!("AI analysis requested for: {}", image_path);

        // 確保模型已載入
        self.load_model().await?;

        // 載入並預處理圖片
        let img = image::open(image_path)?;
        let img = img.resize_exact(224, 224, image::imageops::FilterType::Triangle);
        let img = img.to_rgb8();

        // 轉換為 tensor
        let model = self.clip_model.read().map_err(|e| anyhow::anyhow!("Lock error: {}", e))?;
        let model = model.as_ref().ok_or_else(|| anyhow::anyhow!("Model not loaded"))?;

        let mut pixel_values = Vec::new();
        for pixel in img.pixels() {
            // 正規化到 [-1, 1] 範圍 (CLIP 標準)
            pixel_values.push((pixel[0] as f32 / 127.5) - 1.0);
            pixel_values.push((pixel[1] as f32 / 127.5) - 1.0);
            pixel_values.push((pixel[2] as f32 / 127.5) - 1.0);
        }

        let image_tensor = Tensor::from_vec(pixel_values, (1, 3, 224, 224), &model.device)?;

        // 計算圖片嵌入
        let image_embedding = model.vision_model.forward(&image_tensor)?;
        let image_embedding = Self::l2_normalize(&image_embedding)?;

        // 計算餘弦相似度
        let similarities = image_embedding.matmul(&model.text_embeddings.t()?)?;
        let similarities = similarities.squeeze(0)?;

        // Softmax 轉換為機率
        let similarities = candle_nn::ops::softmax(&similarities, 0)?;
        let similarities_vec: Vec<f32> = similarities.to_vec1()?;

        // 收集結果並排序
        let mut tags: Vec<AiTag> = model.tag_names
            .iter()
            .zip(similarities_vec.iter())
            .map(|(name, &conf)| AiTag {
                name: name.clone(),
                confidence: conf,
            })
            .collect();

        // 按信心度排序
        tags.sort_by(|a, b| b.confidence.partial_cmp(&a.confidence).unwrap());

        // 過濾並取前 N 個
        let tags: Vec<AiTag> = tags
            .into_iter()
            .filter(|t| t.confidence >= self.config.min_confidence)
            .take(self.config.max_tags)
            .collect();

        let duration_ms = start.elapsed().as_millis() as u64;

        let result = AiAnalysisResult {
            file_path: image_path.to_string(),
            tags,
            model_name: self.config.model_name.clone(),
            duration_ms,
        };

        debug!(
            "AI analysis completed for {} in {}ms ({} tags)",
            image_path, duration_ms, result.tags.len()
        );

        Ok(result)
    }

    /// 偵測圖片標籤 (無 AI feature 時的 stub 實作)
    #[cfg(not(feature = "ai"))]
    pub async fn detect_tags(&self, image_path: &str) -> Result<AiAnalysisResult> {
        // 獲取推理信號量，限制並發
        let _permit = self.inference_semaphore.acquire().await?;

        let start = std::time::Instant::now();
        debug!("AI analysis requested for: {} (stub mode - ai feature not enabled)", image_path);

        // Stub 實作 - 返回空結果
        let tags: Vec<AiTag> = Vec::new();
        let duration_ms = start.elapsed().as_millis() as u64;

        let result = AiAnalysisResult {
            file_path: image_path.to_string(),
            tags,
            model_name: format!("{} (stub)", self.config.model_name),
            duration_ms,
        };

        warn!(
            "AI analysis is stub mode for {} - enable 'ai' feature for real inference",
            image_path
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

        // 更新狀態為 processing
        self.mark_processing(image_path).await?;

        // 進行分析
        match self.detect_tags(image_path).await {
            Ok(result) => {
                // 儲存標籤
                self.save_tags(&result).await?;
                // 更新分析狀態
                self.mark_analyzed(image_path).await?;
                Ok(result)
            }
            Err(e) => {
                // 記錄失敗狀態
                self.mark_failed(image_path, &e.to_string()).await?;
                Err(e)
            }
        }
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

    /// 標記圖片為處理中
    async fn mark_processing(&self, image_path: &str) -> Result<()> {
        sqlx::query(
            r#"
            INSERT OR REPLACE INTO ai_analysis_status (file_path, model_version, status)
            VALUES (?, ?, 'processing')
            "#,
        )
        .bind(image_path)
        .bind(&self.config.model_name)
        .execute(&self.pool)
        .await?;

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

    /// 標記圖片分析失敗
    async fn mark_failed(&self, image_path: &str, error_msg: &str) -> Result<()> {
        sqlx::query(
            r#"
            INSERT OR REPLACE INTO ai_analysis_status (file_path, model_version, status)
            VALUES (?, ?, 'failed')
            "#,
        )
        .bind(image_path)
        .bind(&self.config.model_name)
        .execute(&self.pool)
        .await?;

        error!("AI analysis failed for {}: {}", image_path, error_msg);

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

        let failed_images = sqlx::query_scalar::<_, i32>(
            "SELECT COUNT(*) FROM ai_analysis_status WHERE status = 'failed'",
        )
        .fetch_one(&self.pool)
        .await
        .unwrap_or(0);

        Ok(AiStats {
            total_analyzed_images: total_images as u32,
            total_unique_tags: total_tags as u32,
            pending_images: pending_images as u32,
            failed_images: failed_images as u32,
            model_name: self.config.model_name.clone(),
            gpu_enabled: self.config.use_gpu,
            #[cfg(feature = "ai")]
            ai_enabled: true,
            #[cfg(not(feature = "ai"))]
            ai_enabled: false,
        })
    }

    /// 重新分析失敗的圖片
    pub async fn retry_failed(&self) -> Result<u32> {
        let failed_paths = sqlx::query_scalar::<_, String>(
            "SELECT file_path FROM ai_analysis_status WHERE status = 'failed'"
        )
        .fetch_all(&self.pool)
        .await?;

        let mut success_count = 0;
        for path in failed_paths {
            // 刪除舊的狀態
            sqlx::query("DELETE FROM ai_analysis_status WHERE file_path = ?")
                .bind(&path)
                .execute(&self.pool)
                .await?;

            // 重新分析
            if self.analyze_and_save(&path).await.is_ok() {
                success_count += 1;
            }
        }

        Ok(success_count)
    }
}

/// AI 服務統計
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiStats {
    pub total_analyzed_images: u32,
    pub total_unique_tags: u32,
    pub pending_images: u32,
    pub failed_images: u32,
    pub model_name: String,
    pub gpu_enabled: bool,
    pub ai_enabled: bool,
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
        assert_eq!(config.model_name, "openai/clip-vit-base-patch32");
        assert_eq!(config.min_confidence, 0.3);
        assert_eq!(config.max_concurrent_inferences, 4);
        assert!(config.use_gpu);
        assert_eq!(config.max_tags, 5);
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

    #[cfg(feature = "ai")]
    #[test]
    fn test_predefined_tags_not_empty() {
        assert!(!PREDEFINED_TAGS.is_empty());
        assert!(PREDEFINED_TAGS.len() >= 50);
    }
}

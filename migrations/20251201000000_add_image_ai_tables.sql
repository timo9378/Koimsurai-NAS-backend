-- AI 圖片標籤相關資料表
-- 用於儲存 AI 分析狀態和識別出的標籤

-- 紀錄圖片分析狀態 (避免重複分析)
CREATE TABLE IF NOT EXISTS ai_analysis_status (
    file_path TEXT PRIMARY KEY,
    model_version TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('completed', 'failed', 'processing')),
    error_message TEXT,
    analyzed_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

-- 紀錄 AI 辨識出的標籤
CREATE TABLE IF NOT EXISTS image_ai_tags (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    file_path TEXT NOT NULL,
    tag_name TEXT NOT NULL,
    confidence REAL NOT NULL CHECK (confidence >= 0.0 AND confidence <= 1.0),
    model_name TEXT NOT NULL,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY(file_path) REFERENCES files(path) ON DELETE CASCADE,
    UNIQUE(file_path, tag_name)
);

-- 建立索引以加速搜尋
CREATE INDEX IF NOT EXISTS idx_ai_tags_file_path ON image_ai_tags(file_path);
CREATE INDEX IF NOT EXISTS idx_ai_tags_name ON image_ai_tags(tag_name);
CREATE INDEX IF NOT EXISTS idx_ai_tags_confidence ON image_ai_tags(confidence);
CREATE INDEX IF NOT EXISTS idx_ai_status_file_path ON ai_analysis_status(file_path);
CREATE INDEX IF NOT EXISTS idx_ai_status_status ON ai_analysis_status(status);

# Koimsurai NAS

這是一個使用 Rust 構建的高效能 NAS (Network Attached Storage) 後端系統，專注於速度、可靠性與豐富的媒體功能。

## ✨ 核心功能

### 📂 檔案管理
- **基本操作**: 支援檔案與資料夾的上傳、下載、刪除、重新命名。
- **批次處理**: 支援多檔案的批次刪除、移動與複製。
- **斷點續傳**: 支援大檔案分塊上傳 (Chunked Upload)，網路中斷可續傳。
- **版本控制**: 檔案覆蓋時自動備份舊版本，可隨時還原。
- **垃圾桶機制**: 刪除檔案進入垃圾桶，防止誤刪。

### 🏷️ 組織與搜尋
- **標籤系統**: 為檔案添加自定義標籤 (Tags) 與顏色。
- **我的最愛**: 快速標記常用檔案 (Star)。
- **全文搜尋**: 整合 Tantivy 搜尋引擎，支援檔名與內容搜尋。
- **進階篩選**: 支援依名稱、大小、修改時間排序與分頁。

### 🎬 媒體中心
- **即時串流**: 支援影片線上串流播放。
- **即時轉檔**: 整合 FFmpeg，支援不同解析度 (Transcoding) 的即時轉檔。
- **智慧時間軸**: 自動依據日期聚合照片與影片，呈現類似 Google Photos 的時間軸視圖。
- **縮圖生成**: 自動生成圖片與影片縮圖 (Small, Medium, Large)。

### 🛡️ 安全與權限
- **使用者認證**: 完整的註冊、登入、登出機制。
- **權限控制**: 基於使用者的資料夾讀寫權限管理。
- **分享連結**: 建立帶有密碼保護與過期時間的公開分享連結。
- **稽核日誌**: 記錄所有關鍵操作 (刪除、權限變更等)，供管理員查詢。

### ⚙️ 系統與整合
- **WebDAV**: 完整支援 WebDAV 協定，可掛載為網路磁碟機 (Windows/Mac/Linux)。
- **背景任務**: 內建 Job Queue 處理耗時任務 (轉檔、縮圖、索引)，並透過 WebSocket 即時推送進度。
- **系統監控**: 提供 CPU、記憶體與磁碟使用率的即時狀態 API。

---

## 🚀 快速開始

### 前置需求
1. **Rust**: 最新穩定版。
2. **FFmpeg**: 需安裝並加入系統 PATH (用於媒體轉檔與縮圖)。
3. **SQLite**: (選用) 用於檢視資料庫，系統會自動建立。

### 安裝與執行

1. **複製專案**
   ```bash
   git clone https://github.com/yourusername/koimsurai-nas.git
   cd koimsurai-nas
   ```

2. **設定環境變數**
   複製 `.env.example` 為 `.env` 並依需求修改：
   ```bash
   cp .env.example .env
   ```
   關鍵設定：
   - `DATABASE_URL`: 資料庫路徑 (預設 `sqlite://nas.db`)
   - `STORAGE_PATH`: 檔案儲存根目錄 (預設 `storage`)
   - `SESSION_SECRET`: Session 加密金鑰

3. **執行伺服器**
   ```bash
   cargo run
   ```
   伺服器預設啟動於 `http://localhost:3000`。

---

## 📚 API 文件

所有 API 端點 (除了 `/api/auth/*`, `/s/*`, `/webdav`) 均需透過 Cookie 進行身分驗證。

### 🧾 OpenAPI
| 方法 | 路徑 | 描述 |
|------|------|------|
| GET | `/scalar` | 取得 Utoipa/OpenAPI JSON，用以產生 API Docs |

### 🔐 認證 (Authentication)
| 方法 | 路徑 | 描述 | Body / Query |
|------|------|------|--------------|
| POST | `/api/auth/register` | 註冊新使用者 | `{ "username": "...", "password": "..." }` |
| POST | `/api/auth/login` | 使用者登入 | `{ "username": "...", "password": "..." }` |
| POST | `/api/auth/logout` | 使用者登出 | - |

### 📂 檔案操作 (File Operations)
| 方法 | 路徑 | 描述 | Body / Query |
|------|------|------|--------------|
| GET | `/api/files` | 列出根目錄檔案 | `?sort_by=name&order=asc&page=1` |
| GET | `/api/files/*path` | 列出指定目錄檔案 | `?sort_by=size&limit=50` |
| POST | `/api/files/folder` | 建立資料夾 | `{ "path": "dir/name" }` |
| GET | `/api/download/*path` | 下載檔案 | - |
| PUT | `/api/files/*path` | 重新命名 | `{ "new_path": "new_name.ext" }` |
| DELETE | `/api/files/*path` | 刪除檔案 (移至垃圾桶) | - |
| POST | `/api/files/batch/delete` | 批次刪除 | `{ "paths": ["file1", "file2"] }` |
| POST | `/api/files/batch/move` | 批次移動 | `{ "paths": [...], "destination": "dir" }` |
| POST | `/api/files/batch/copy` | 批次複製 | `{ "paths": [...], "destination": "dir" }` |
| GET | `/api/thumbnail/:size/*path` | 取得縮圖 | size: `small`, `medium`, `large` |
| GET | `/api/favorites` | 列出我的最愛 | - |

### ☁️ 上傳 (Upload)
| 方法 | 路徑 | 描述 | Body / Query |
|------|------|------|--------------|
| POST | `/api/upload` | 簡單上傳 (根目錄) | `multipart/form-data` |
| POST | `/api/upload/*path` | 簡單上傳 (指定目錄) | `multipart/form-data` |
| POST | `/api/upload/init` | 初始化分塊上傳 | `{ "file_path": "...", "total_size": 1024 }` |
| PATCH | `/api/upload/session/:id` | 上傳檔案分塊 | Binary Body |
| GET | `/api/upload/session/:id` | 查詢上傳狀態 | - |

### 🏷️ 標籤與收藏 (Tags & Favorites)
| 方法 | 路徑 | 描述 | Body / Query |
|------|------|------|--------------|
| POST | `/api/tags/add/*path` | 新增標籤到指定檔案/資料夾 | `{ "name": "Work", "color": "#FF0000" }` |
| DELETE | `/api/tags/remove/:tag_name/*path` | 從指定檔案/資料夾移除標籤 | - |
| POST | `/api/star/file/*path` | 切換指定檔案收藏狀態 (Star/Unstar) | - |

### 🕒 版本控制 (Versioning)
| 方法 | 路徑 | 描述 | Body / Query |
|------|------|------|--------------|
| GET | `/api/versions/file/*path` | 列出指定檔案的歷史版本 | - |
| POST | `/api/versions/restore/:version_id` | 還原指定版本 (以 version_id 還原) | - |

### 🎬 媒體 (Media)
| 方法 | 路徑 | 描述 | Body / Query |
|------|------|------|--------------|
| GET | `/api/media/stream` | 媒體串流 | `?path=video.mp4&resolution=1080p` |
| GET | `/api/media/timeline` | 媒體時間軸 | `?group_by=day|month|year` |
| GET | `/api/media/hls/status` | 查詢 HLS 轉檔/串流狀態 | - |
| GET | `/api/media/hls/serve` | 以 HLS 方式提供分段串流 | `?path=video.mp4` |
| GET | `/api/media/hls/qualities` | 列出可用 HLS 解析度/品質 | - |

### 🔗 分享 (Sharing)
| 方法 | 路徑 | 描述 | Body / Query |
|------|------|------|--------------|
| POST | `/api/share` | 建立分享連結 | `{ "file_path": "...", "password": "...", "expires": 3600 }` |
| GET | `/s/:id` | 存取分享連結 | (公開存取) |

### 🗑️ 垃圾桶 (Trash)
| 方法 | 路徑 | 描述 | Body / Query |
|------|------|------|--------------|
| GET | `/api/trash` | 列出垃圾桶 | - |
| POST | `/api/trash/:filename` | 還原檔案 | - |
| DELETE | `/api/trash` | 清空垃圾桶 | - |

### 🔍 搜尋與索引 (Search)
| 方法 | 路徑 | 描述 | Body / Query |
|------|------|------|--------------|
| GET | `/api/search` | 全文搜尋 | `?q=keyword` |

### 🛡️ 系統與管理 (System)
| 方法 | 路徑 | 描述 | Body / Query |
|------|------|------|--------------|
| GET | `/api/system/status` | 系統狀態 | CPU, RAM, Disk |
| POST | `/api/system/verify-consistency` | 驗證資料庫與檔案一致性 (管理員用) | - |
| POST | `/api/system/rescan` | 觸發資料重新掃描與索引 (管理員用) | - |
| GET | `/api/tasks` | 背景任務列表 | - |
| GET | `/api/audit/logs` | 稽核日誌 | - |
| POST | `/api/permissions` | 設定權限 | `{ "user_id": 1, "path": "...", "can_read": true }` |
| GET | `/api/ws` | WebSocket | 即時通知連線 |

### 🌐 WebDAV
| 方法 | 路徑 | 描述 |
|------|------|------|
| ANY | `/webdav/*` | WebDAV 協定入口 |

### 🐳 Docker 管理 (Container Manager)
類似 Synology Container Manager，需設定 `ENABLE_DOCKER_MANAGER=true`。

| 方法 | 路徑 | 描述 | Body / Query |
|------|------|------|--------------|
| GET | `/api/docker/status` | Docker 連線狀態 | - |
| POST | `/api/docker/connect` | 連接 Docker daemon | - |
| GET | `/api/docker/containers` | 列出所有容器 | `?all=true` |
| GET | `/api/docker/containers/:id` | 容器詳情 | - |
| POST | `/api/docker/containers/:id/start` | 啟動容器 | - |
| POST | `/api/docker/containers/:id/stop` | 停止容器 | `{ "timeout": 10 }` |
| POST | `/api/docker/containers/:id/restart` | 重啟容器 | `{ "timeout": 10 }` |
| DELETE | `/api/docker/containers/:id` | 刪除容器 | `?force=true` |
| GET | `/api/docker/containers/:id/logs` | 容器日誌 | `?tail=100&since=0` |
| GET | `/api/docker/containers/:id/stats` | 容器統計 | CPU, Memory, Network |
| GET | `/api/docker/images` | 列出所有鏡像 | - |
| POST | `/api/docker/images/pull` | 拉取鏡像 | `{ "image": "nginx", "tag": "latest" }` |
| DELETE | `/api/docker/images/:id` | 刪除鏡像 | `?force=true` |

### 🤖 AI 圖片標籤 (AI Smart Tagging)
後端 AI 圖片標籤服務已在 `src/services/ai.rs` 實作（使用 CLIP，並以 Cargo feature `ai` 控制實際推理），資料表與索引（`image_ai_tags`, `ai_analysis_status`）在 DB migration 已建立。

內部可用的 service/函式:
- `AiService`：提供 `detect_tags`, `analyze_and_save`, `delete_tags`, `get_stats`, `retry_failed` 等方法，用以分析圖片並將結果寫入 DB。
- `services::search::search_by_ai_tag(pool, tag, min_confidence, limit)`：在 DB 中搜尋含指定 AI 標籤的圖片。
- `services::search::get_all_ai_tags(pool)`：取得熱門/現有的 AI 標籤（autocomplete）。

目前專案尚未把 AI 功能直接暴露成 HTTP 路由；以下為建議可新增到路由的 API：

| 方法 | 路徑 | 說明 |
|------|------|------|
| GET | `/api/search/ai-tags` | 以關鍵字搜尋含指定 AI 標籤的圖片（Query: `q`, 可選 `min_confidence`, `limit`） |
| GET | `/api/search/ai-tags/list` | 取得所有已知 AI 標籤（自動完成用） |
| POST | `/api/ai/analyze` | 針對單張圖片觸發即時分析並儲存結果（Body: `{ "path": "..." }`） |
| POST | `/api/ai/retry-failed` | 重新分析失敗的圖片（回傳成功數量） |

簡單範例（curl）：

1) 以標籤搜尋圖片

```bash
curl -sG "http://localhost:3000/api/search/ai-tags" --data-urlencode "q=beach" --data "min_confidence=0.5&limit=20" \
   -b cookiefile
```

範例回應：
```json
[
   {"path":"/photos/2025/beach1.jpg","name":"beach1.jpg","tag":"beach","confidence":0.92},
   {"path":"/photos/2025/beach2.jpg","name":"beach2.jpg","tag":"beach","confidence":0.87}
]
```

2) 取得 AI 標籤清單（autocomplete）

```bash
curl "http://localhost:3000/api/search/ai-tags/list" -b cookiefile
```

範例回應：
```json
[ ["beach", 124], ["cat", 98], ["person", 65] ]
```

3) 針對單張圖片觸發分析

```bash
curl -X POST "http://localhost:3000/api/ai/analyze" -H "Content-Type: application/json" -d '{"path":"/photos/2025/beach1.jpg"}' -b cookiefile
```

範例回應：
```json
{
   "file_path": "/photos/2025/beach1.jpg",
   "tags": [{"name":"beach","confidence":0.92}],
   "model_name":"openai/clip-vit-base-patch32",
   "duration_ms": 310
}
```

4) 重新分析失敗項目

```bash
curl -X POST "http://localhost:3000/api/ai/retry-failed" -b cookiefile
```

範例回應：
```json
{ "reprocessed": 12 }
```

啟用與設定：

- CLI（開發）啟用範例：
```bash
export ENABLE_AI_LABELLING=true
cargo run --features ai
```
- 環境變數（可在 `.env` 或系統環境設定）：
   - `AI_MODEL_NAME`（預設 `openai/clip-vit-base-patch32`）
   - `AI_MIN_CONFIDENCE`（預設 `0.3`）
   - `AI_MAX_CONCURRENT`（預設 `4`）
   - `AI_USE_GPU`（`true`/`false`）
   - `AI_MAX_TAGS`（回傳標籤數量上限）

備註：若不在 Cargo features 中啟用 `ai`，`AiService::detect_tags` 會以 stub 模式回傳空結果並記錄警告，資料庫相關查詢與管理函式仍可使用。

---

## 🏗️ 專案結構

```
src/
├── handlers/       # API 請求處理 (Controller)
├── models/         # 資料結構與資料庫模型
├── services/       # 核心業務邏輯 (Indexer, Search, Audit, AI, Docker)
├── utils/          # 工具函式 (Queue, Image, Versioning, FFmpeg)
├── middleware/     # 中介軟體 (Auth)
├── routes/         # 路由定義
├── db/             # 資料庫連線與遷移
└── main.rs         # 程式進入點
```

## 🛠️ 技術棧

- **語言**: Rust (Edition 2024)
- **Web 框架**: Axum 0.7
- **資料庫**: SQLite (SQLx) + WAL 模式
- **非同步執行**: Tokio
- **搜尋引擎**: Tantivy
- **媒體處理**: FFmpeg (GPU 加速), Image-rs
- **容器管理**: Bollard (Docker API)
- **AI 推理**: (預留) ort/candle (ONNX Runtime / Candle)
- **API 文件**: Utoipa (OpenAPI)

## 🔧 性能調優

本專案設計支援從低規格開發機到高性能伺服器的彈性配置。

### 開發環境 (低規格)
```env
DATABASE_MAX_CONNECTIONS=5
DATABASE_MMAP_SIZE_MB=64
SEARCH_INDEX_BUFFER_MB=50
MAX_CONCURRENT_TRANSCODES=2
ENABLE_DOCKER_MANAGER=false
ENABLE_AI_LABELLING=false
```

### 生產環境 (64GB RAM + RTX 5060 Ti)
```env
DATABASE_MAX_CONNECTIONS=50
DATABASE_MMAP_SIZE_MB=512
SEARCH_INDEX_BUFFER_MB=500
MAX_CONCURRENT_TRANSCODES=6
USE_GPU_TRANSCODE=auto
ENABLE_DOCKER_MANAGER=true
ENABLE_AI_LABELLING=true
AI_MAX_CONCURRENT=6
AI_USE_GPU=true
```

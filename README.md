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
| GET | `/api/download/*path` | 下載檔案 | - |
| PUT | `/api/files/*path` | 重新命名 | `{ "new_path": "new_name.ext" }` |
| DELETE | `/api/files/*path` | 刪除檔案 (移至垃圾桶) | - |
| POST | `/api/files/batch/delete` | 批次刪除 | `{ "paths": ["file1", "file2"] }` |
| POST | `/api/files/batch/move` | 批次移動 | `{ "paths": [...], "destination": "dir" }` |
| POST | `/api/files/batch/copy` | 批次複製 | `{ "paths": [...], "destination": "dir" }` |
| GET | `/api/thumbnail/:size/*path` | 取得縮圖 | size: `small`, `medium`, `large` |

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
| POST | `/api/files/*path/tags` | 新增標籤 | `{ "name": "Work", "color": "#FF0000" }` |
| DELETE | `/api/files/*path/tags/:tag` | 移除標籤 | - |
| POST | `/api/files/*path/star` | 切換收藏狀態 | - |

### 🕒 版本控制 (Versioning)
| 方法 | 路徑 | 描述 | Body / Query |
|------|------|------|--------------|
| GET | `/api/files/*path/versions` | 列出歷史版本 | - |
| POST | `/api/files/*path/restore/:vid` | 還原指定版本 | - |

### 🎬 媒體 (Media)
| 方法 | 路徑 | 描述 | Body / Query |
|------|------|------|--------------|
| GET | `/api/media/stream` | 媒體串流 | `?path=video.mp4&resolution=1080p` |
| GET | `/api/media/timeline` | 媒體時間軸 | `?group_by=day|month|year` |

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
| GET | `/api/tasks` | 背景任務列表 | - |
| GET | `/api/audit/logs` | 稽核日誌 | - |
| POST | `/api/permissions` | 設定權限 | `{ "user_id": 1, "path": "...", "can_read": true }` |
| GET | `/api/ws` | WebSocket | 即時通知連線 |

### 🌐 WebDAV
| 方法 | 路徑 | 描述 |
|------|------|------|
| ANY | `/webdav/*` | WebDAV 協定入口 |

---

## 🏗️ 專案結構

```
src/
├── handlers/       # API 請求處理 (Controller)
├── models/         # 資料結構與資料庫模型
├── services/       # 核心業務邏輯 (Indexer, Search, Audit)
├── utils/          # 工具函式 (Queue, Image, Versioning)
├── middleware/     # 中介軟體 (Auth)
├── routes/         # 路由定義
├── db/             # 資料庫連線與遷移
└── main.rs         # 程式進入點
```

## 🛠️ 技術棧

- **語言**: Rust
- **Web 框架**: Axum
- **資料庫**: SQLite (SQLx)
- **非同步執行**: Tokio
- **搜尋引擎**: Tantivy
- **媒體處理**: FFmpeg, Image-rs
- **API 文件**: Utoipa (OpenAPI)

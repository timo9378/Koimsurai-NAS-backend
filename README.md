# Koimsurai NAS

這是一個使用 Rust 構建的簡單 NAS (Network Attached Storage) 後端系統。

## 功能

- **檔案管理**: 上傳、下載、刪除、列表。
- **分享連結**: 建立帶有密碼和過期時間的分享連結。
- **WebDAV**: 支援 Windows/Mac 掛載 (Z: 槽)。
- **媒體串流**: 支援影片串流與即時轉檔 (Transcoding)。
- **權限控制**: 基於使用者的資料夾權限管理。
- **背景任務**: 支援縮圖生成與轉檔任務隊列。

## 如何執行

1. 確保已安裝 Rust 和 FFmpeg (用於轉檔)。
2. 設定環境變數 (參考 `.env.example`)。
3. 執行: `cargo run`
4. 伺服器將在 `http://localhost:3000` 啟動。

## API

- `GET /api/files`: 列出檔案
- `GET /api/media/stream?path=...&resolution=1080p`: 影片串流
- `ANY /webdav`: WebDAV 接口

## 專案結構

- `src/handlers/`: API 處理邏輯
- `src/models/`: 資料結構
- `src/utils/`: 工具函式 (Queue, Hash, Image)
- `storage/`: 存放 NAS 檔案的目錄

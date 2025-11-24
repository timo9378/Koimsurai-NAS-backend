# Koimsurai NAS

這是一個使用 Rust 構建的簡單 NAS (Network Attached Storage) 後端系統。

## 功能

- **檔案列表 API**: `GET /api/files` 列出存儲目錄中的檔案。
- **檔案下載**: `GET /files/<filename>` 下載檔案。
- **CORS 支援**: 允許前端應用程式連接。

## 如何執行

1. 確保已安裝 Rust。
2. 進入目錄: `cd hello_nas/Koimsurai_NAS`
3. 執行: `cargo run`
4. 伺服器將在 `http://localhost:3000` 啟動。

## 測試

- 打開瀏覽器訪問 `http://localhost:3000/api/files` 查看檔案列表。
- 訪問 `http://localhost:3000/files/welcome.txt` 下載測試檔案。

## 專案結構

- `src/main.rs`: 主要伺服器程式碼。
- `storage/`: 存放 NAS 檔案的目錄。

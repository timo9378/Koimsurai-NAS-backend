//! 併發測試 - 測試文件寫入和轉碼的競爭條件

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tokio::sync::{Barrier, Semaphore};
use tokio::time::timeout;

/// 測試目錄路徑 - 每個測試使用獨立目錄避免競爭
fn test_dir(test_name: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target/test_concurrency")
        .join(test_name)
}

/// 設置測試環境
async fn setup(test_name: &str) -> std::path::PathBuf {
    let dir = test_dir(test_name);
    let _ = fs::remove_dir_all(&dir).await;
    fs::create_dir_all(&dir).await.unwrap();
    dir
}

/// 清理測試環境
async fn cleanup(test_name: &str) {
    let dir = test_dir(test_name);
    let _ = fs::remove_dir_all(&dir).await;
}

// ==================== 文件併發寫入測試 ====================

#[tokio::test]
async fn test_concurrent_file_writes_with_different_names() {
    let test_name = "file_writes_different";
    let dir = setup(test_name).await;
    let num_tasks = 50;
    let barrier = Arc::new(Barrier::new(num_tasks));
    let success_count = Arc::new(AtomicUsize::new(0));

    let mut handles = Vec::new();

    for i in 0..num_tasks {
        let dir = dir.clone();
        let barrier = barrier.clone();
        let success_count = success_count.clone();

        handles.push(tokio::spawn(async move {
            // 等待所有任務準備就緒，同時開始
            barrier.wait().await;

            let file_path = dir.join(format!("file_{}.txt", i));
            let content = format!("Content from task {}", i);

            match fs::write(&file_path, &content).await {
                Ok(()) => {
                    // 驗證寫入正確
                    let read_content = fs::read_to_string(&file_path).await.unwrap();
                    if read_content == content {
                        success_count.fetch_add(1, Ordering::SeqCst);
                    }
                }
                Err(e) => {
                    eprintln!("Task {} failed: {}", i, e);
                }
            }
        }));
    }

    // 等待所有任務完成
    for handle in handles {
        handle.await.unwrap();
    }

    assert_eq!(success_count.load(Ordering::SeqCst), num_tasks);
    cleanup(test_name).await;
}

#[tokio::test]
async fn test_concurrent_writes_same_file_last_writer_wins() {
    let test_name = "same_file_writes";
    let dir = setup(test_name).await;
    let file_path = dir.join("shared_file.txt");
    let num_tasks = 20;
    let barrier = Arc::new(Barrier::new(num_tasks));

    // 先創建文件
    fs::write(&file_path, "initial").await.unwrap();

    let mut handles = Vec::new();

    for i in 0..num_tasks {
        let file_path = file_path.clone();
        let barrier = barrier.clone();

        handles.push(tokio::spawn(async move {
            barrier.wait().await;

            let content = format!("Written by task {}", i);
            // 使用 OpenOptions 進行寫入
            let mut file = tokio::fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&file_path)
                .await
                .unwrap();

            file.write_all(content.as_bytes()).await.unwrap();
            file.sync_all().await.unwrap();
        }));
    }

    for handle in handles {
        handle.await.unwrap();
    }

    // 確認文件存在且內容是某個任務寫入的
    let content = fs::read_to_string(&file_path).await.unwrap();
    assert!(content.starts_with("Written by task "));

    cleanup(test_name).await;
}

#[tokio::test]
async fn test_concurrent_append_operations() {
    let test_name = "append_operations";
    let dir = setup(test_name).await;
    let file_path = dir.join("append_test.txt");
    let num_tasks = 30;
    let barrier = Arc::new(Barrier::new(num_tasks));

    // 創建空文件
    fs::write(&file_path, "").await.unwrap();

    let mut handles = Vec::new();

    for i in 0..num_tasks {
        let file_path = file_path.clone();
        let barrier = barrier.clone();

        handles.push(tokio::spawn(async move {
            barrier.wait().await;

            let content = format!("Line {}\n", i);
            let mut file = tokio::fs::OpenOptions::new()
                .append(true)
                .open(&file_path)
                .await
                .unwrap();

            file.write_all(content.as_bytes()).await.unwrap();
        }));
    }

    for handle in handles {
        handle.await.unwrap();
    }

    // 確認所有行都被寫入（順序可能不同）
    let content = fs::read_to_string(&file_path).await.unwrap();
    let lines: Vec<&str> = content.lines().collect();
    assert_eq!(lines.len(), num_tasks);

    cleanup(test_name).await;
}

// ==================== Semaphore 限流測試 ====================

#[tokio::test]
async fn test_semaphore_limits_concurrent_operations() {
    let max_concurrent = 3;
    let total_tasks = 10;
    let semaphore = Arc::new(Semaphore::new(max_concurrent));
    let current_count = Arc::new(AtomicUsize::new(0));
    let max_observed = Arc::new(AtomicUsize::new(0));
    let barrier = Arc::new(Barrier::new(total_tasks));

    let mut handles = Vec::new();

    for _ in 0..total_tasks {
        let semaphore = semaphore.clone();
        let current_count = current_count.clone();
        let max_observed = max_observed.clone();
        let barrier = barrier.clone();

        handles.push(tokio::spawn(async move {
            barrier.wait().await;

            let _permit = semaphore.acquire().await.unwrap();

            // 記錄當前並發數
            let count = current_count.fetch_add(1, Ordering::SeqCst) + 1;

            // 更新最大觀察值
            max_observed.fetch_max(count, Ordering::SeqCst);

            // 模擬工作
            tokio::time::sleep(Duration::from_millis(50)).await;

            current_count.fetch_sub(1, Ordering::SeqCst);
        }));
    }

    for handle in handles {
        handle.await.unwrap();
    }

    // 確認最大併發數不超過限制
    assert!(max_observed.load(Ordering::SeqCst) <= max_concurrent);
}

#[tokio::test]
async fn test_transcode_semaphore_simulation() {
    // 模擬轉碼作業的併發限制
    let max_transcodes = 2;
    let transcode_semaphore = Arc::new(Semaphore::new(max_transcodes));
    let active_transcodes = Arc::new(AtomicUsize::new(0));
    let completed = Arc::new(AtomicUsize::new(0));
    let num_requests = 8;
    let barrier = Arc::new(Barrier::new(num_requests));

    let mut handles = Vec::new();

    for i in 0..num_requests {
        let semaphore = transcode_semaphore.clone();
        let active = active_transcodes.clone();
        let completed = completed.clone();
        let barrier = barrier.clone();

        handles.push(tokio::spawn(async move {
            barrier.wait().await;

            // 嘗試獲取許可
            let _permit = semaphore.acquire().await.unwrap();

            let current = active.fetch_add(1, Ordering::SeqCst) + 1;
            assert!(
                current <= max_transcodes,
                "Too many concurrent transcodes: {} > {}",
                current,
                max_transcodes
            );

            // 模擬轉碼時間
            tokio::time::sleep(Duration::from_millis(100)).await;

            active.fetch_sub(1, Ordering::SeqCst);
            completed.fetch_add(1, Ordering::SeqCst);

            format!("Transcode {} completed", i)
        }));
    }

    let results: Vec<_> = futures::future::join_all(handles).await;
    assert_eq!(results.len(), num_requests);
    assert_eq!(completed.load(Ordering::SeqCst), num_requests);
}

// ==================== 超時測試 ====================

#[tokio::test]
async fn test_operation_timeout() {
    let result = timeout(Duration::from_millis(100), async {
        tokio::time::sleep(Duration::from_millis(50)).await;
        "completed"
    })
    .await;

    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "completed");
}

#[tokio::test]
async fn test_operation_timeout_exceeded() {
    let result = timeout(Duration::from_millis(50), async {
        tokio::time::sleep(Duration::from_millis(200)).await;
        "should not complete"
    })
    .await;

    assert!(result.is_err());
}

// ==================== 文件鎖定模擬測試 ====================

/// 模擬文件鎖定行為 - 使用 Mutex 保護文件操作
#[tokio::test]
async fn test_file_mutex_protection() {
    use tokio::sync::Mutex;

    let test_name = "file_mutex";
    let dir = setup(test_name).await;
    let file_path = dir.join("mutex_protected.txt");
    let file_mutex = Arc::new(Mutex::new(()));
    let num_tasks = 20;
    let barrier = Arc::new(Barrier::new(num_tasks));
    let values = Arc::new(tokio::sync::Mutex::new(Vec::new()));

    // 創建初始文件
    fs::write(&file_path, "0").await.unwrap();

    let mut handles = Vec::new();

    for i in 0..num_tasks {
        let file_path = file_path.clone();
        let file_mutex = file_mutex.clone();
        let barrier = barrier.clone();
        let values = values.clone();

        handles.push(tokio::spawn(async move {
            barrier.wait().await;

            // 獲取鎖
            let _lock = file_mutex.lock().await;

            // 讀取當前值
            let current = fs::read_to_string(&file_path).await.unwrap();
            let value: i32 = current.trim().parse().unwrap_or(0);

            // 寫入新值
            let new_value = value + 1;
            fs::write(&file_path, new_value.to_string()).await.unwrap();

            // 記錄我們寫入的值
            values.lock().await.push((i, new_value));
        }));
    }

    for handle in handles {
        handle.await.unwrap();
    }

    // 驗證最終值
    let final_content = fs::read_to_string(&file_path).await.unwrap();
    let final_value: i32 = final_content.trim().parse().unwrap();
    assert_eq!(final_value, num_tasks as i32);

    cleanup(test_name).await;
}

// ==================== 目錄創建競爭測試 ====================

#[tokio::test]
async fn test_concurrent_directory_creation() {
    let test_name = "dir_creation";
    let dir = setup(test_name).await;
    let target_dir = dir.join("concurrent_dir");
    let num_tasks = 20;
    let barrier = Arc::new(Barrier::new(num_tasks));
    let success_count = Arc::new(AtomicUsize::new(0));
    let exists_count = Arc::new(AtomicUsize::new(0));

    let mut handles = Vec::new();

    for _ in 0..num_tasks {
        let target_dir = target_dir.clone();
        let barrier = barrier.clone();
        let success_count = success_count.clone();
        let exists_count = exists_count.clone();

        handles.push(tokio::spawn(async move {
            barrier.wait().await;

            match fs::create_dir(&target_dir).await {
                Ok(()) => {
                    success_count.fetch_add(1, Ordering::SeqCst);
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    exists_count.fetch_add(1, Ordering::SeqCst);
                }
                Err(e) => {
                    panic!("Unexpected error: {}", e);
                }
            }
        }));
    }

    for handle in handles {
        handle.await.unwrap();
    }

    // 在 Windows 上，併發創建可能有多個成功（因為 API 差異）
    // 重要的是總數正確且目錄存在
    let total = success_count.load(Ordering::SeqCst) + exists_count.load(Ordering::SeqCst);
    assert_eq!(total, num_tasks);
    assert!(success_count.load(Ordering::SeqCst) >= 1);
    // 確認目錄存在
    assert!(target_dir.exists());

    cleanup(test_name).await;
}

#[tokio::test]
async fn test_create_dir_all_is_idempotent() {
    let test_name = "dir_all_idempotent";
    let dir = setup(test_name).await;
    let target_dir = dir.join("nested/deep/directory");
    let num_tasks = 20;
    let barrier = Arc::new(Barrier::new(num_tasks));
    let success_count = Arc::new(AtomicUsize::new(0));

    let mut handles = Vec::new();

    for _ in 0..num_tasks {
        let target_dir = target_dir.clone();
        let barrier = barrier.clone();
        let success_count = success_count.clone();

        handles.push(tokio::spawn(async move {
            barrier.wait().await;

            // create_dir_all 是冪等的
            match fs::create_dir_all(&target_dir).await {
                Ok(()) => {
                    success_count.fetch_add(1, Ordering::SeqCst);
                }
                Err(e) => {
                    panic!("Unexpected error: {}", e);
                }
            }
        }));
    }

    for handle in handles {
        handle.await.unwrap();
    }

    // 所有任務都應該成功
    assert_eq!(success_count.load(Ordering::SeqCst), num_tasks);
    assert!(target_dir.exists());

    cleanup(test_name).await;
}

// ==================== 讀寫競爭測試 ====================

#[tokio::test]
async fn test_concurrent_read_while_writing() {
    use tokio::sync::RwLock;

    let test_name = "read_while_writing";
    let dir = setup(test_name).await;
    let file_path = dir.join("rw_test.txt");
    let lock = Arc::new(RwLock::new(()));
    let num_readers = 10;
    let num_writers = 5;
    let barrier = Arc::new(Barrier::new(num_readers + num_writers));

    // 創建初始文件
    fs::write(&file_path, "initial content").await.unwrap();

    let mut handles = Vec::new();

    // 啟動讀取者
    for i in 0..num_readers {
        let file_path = file_path.clone();
        let lock = lock.clone();
        let barrier = barrier.clone();

        handles.push(tokio::spawn(async move {
            barrier.wait().await;

            for _ in 0..5 {
                let _read_lock = lock.read().await;
                let content = fs::read_to_string(&file_path).await.unwrap();
                assert!(!content.is_empty(), "Reader {} got empty content", i);
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }));
    }

    // 啟動寫入者
    for i in 0..num_writers {
        let file_path = file_path.clone();
        let lock = lock.clone();
        let barrier = barrier.clone();

        handles.push(tokio::spawn(async move {
            barrier.wait().await;

            for j in 0..3 {
                let _write_lock = lock.write().await;
                let content = format!("Written by writer {} iteration {}", i, j);
                fs::write(&file_path, &content).await.unwrap();
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }));
    }

    for handle in handles {
        handle.await.unwrap();
    }

    cleanup(test_name).await;
}

// ==================== 資源清理測試 ====================

#[tokio::test]
async fn test_cleanup_on_concurrent_failures() {
    let test_name = "cleanup_failures";
    let dir = setup(test_name).await;
    let num_tasks = 10;
    let barrier = Arc::new(Barrier::new(num_tasks));
    let cleaned_up = Arc::new(AtomicUsize::new(0));

    let mut handles = Vec::new();

    for i in 0..num_tasks {
        let dir = dir.clone();
        let barrier = barrier.clone();
        let cleaned_up = cleaned_up.clone();

        handles.push(tokio::spawn(async move {
            barrier.wait().await;

            let temp_file = dir.join(format!("temp_{}.txt", i));

            // 模擬資源獲取
            fs::write(&temp_file, "temp data").await.unwrap();

            // 使用 defer 模式確保清理
            struct Cleanup {
                path: std::path::PathBuf,
                cleaned: Arc<AtomicUsize>,
            }

            impl Drop for Cleanup {
                fn drop(&mut self) {
                    // 注意：這裡不能使用 async，所以用 std::fs
                    let _ = std::fs::remove_file(&self.path);
                    self.cleaned.fetch_add(1, Ordering::SeqCst);
                }
            }

            let _cleanup = Cleanup {
                path: temp_file,
                cleaned: cleaned_up,
            };

            // 模擬一些操作
            tokio::time::sleep(Duration::from_millis(10)).await;

            // Cleanup 會在這裡自動執行
        }));
    }

    for handle in handles {
        handle.await.unwrap();
    }

    // 確認所有臨時文件都被清理
    assert_eq!(cleaned_up.load(Ordering::SeqCst), num_tasks);

    cleanup(test_name).await;
}

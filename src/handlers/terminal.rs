use axum::{
    extract::{ws::{Message, WebSocket, WebSocketUpgrade}, Query, State},
    response::IntoResponse,
};
use futures::{sink::SinkExt, stream::StreamExt};
use serde::Deserialize;
use std::collections::HashSet;

use crate::state::AppState;

/// 受限終端機 - 只允許安全的基本命令
/// 這是一個模擬的 shell 環境，不會直接執行系統命令

#[derive(Debug, Deserialize)]
pub struct TerminalQuery {
    #[serde(default = "default_cols")]
    pub cols: u16,
    #[serde(default = "default_rows")]
    pub rows: u16,
}

fn default_cols() -> u16 { 80 }
fn default_rows() -> u16 { 24 }

/// 允許的命令白名單 - 只包含容器內實際可用的命令
fn get_allowed_commands() -> HashSet<&'static str> {
    [
        // 內建命令
        "help", "clear", "exit", "logout", "history",
        // 檔案操作 (coreutils - 容器內有)
        "ls", "ll", "la", "pwd", "cd", "cat", "head", "tail",
        "echo", "mkdir", "touch", "cp", "mv", "rm", "ln",
        "chmod", "chgrp", "stat", "file", "basename", "dirname",
        "realpath", "readlink",
        // 文字處理
        "grep", "find", "wc", "sort", "uniq", "cut", "awk", "sed",
        "tr", "tee", "xargs", "diff",
        // 系統資訊 (procps - 已安裝)
        "ps", "top", "free", "uptime", "w", "kill", "pgrep", "pkill",
        // 磁碟工具
        "df", "du",
        // 壓縮工具 (需要額外安裝，暫時移除)
        // "tar", "gzip", "gunzip", "zip", "unzip",
        // 其他
        "date", "whoami", "hostname", "uname", "env", "printenv",
        "which", "type", "true", "false", "test", "expr",
        // FFmpeg (已安裝)
        "ffmpeg", "ffprobe",
    ].into_iter().collect()
}

/// 獲取命令列表供 Tab 補全使用
pub fn get_available_commands() -> Vec<&'static str> {
    vec![
        "help", "clear", "exit", "logout", "history",
        "ls", "ll", "la", "pwd", "cd", "cat", "head", "tail",
        "echo", "mkdir", "touch", "cp", "mv", "rm", "ln",
        "chmod", "stat", "file", "basename", "dirname",
        "grep", "find", "wc", "sort", "uniq", "cut", "awk", "sed",
        "tr", "tee", "xargs", "diff",
        "ps", "top", "free", "uptime", "w", "kill",
        "df", "du",
        "date", "whoami", "hostname", "uname", "env",
        "which", "ffmpeg", "ffprobe",
    ]
}

/// 危險的 shell 元字符 — 禁止出現在任何地方
/// 這些字符可用來繞過白名單（命令替換、進程替換等）
fn get_dangerous_shell_chars() -> &'static [&'static str] {
    &[
        "`",      // backtick 命令替換
        "$(",     // $() 命令替換
        "$((",    // 算術展開
        "${",     // 變數展開
        "<(",     // 進程替換
        ">(", 
        ">>",     // append redirect
        "<<",     // here-doc
        "\\",     // 反斜線轉義
        "\n",     // newline (命令分隔)
        "\r",
    ]
}

/// 危險命令黑名單 — 絕對禁止的命令名稱
fn get_dangerous_commands() -> HashSet<&'static str> {
    [
        "sudo", "su", "chown", "chroot", "mount", "umount",
        "mkfs", "dd", "eval", "exec", "source",
        "curl", "wget", "nc", "ncat", "netcat", "nmap",
        "python", "python3", "perl", "ruby", "node", "php",
        "sh", "bash", "zsh", "csh", "dash", "ash",
        "ssh", "scp", "sftp", "telnet", "ftp",
        "apt", "apt-get", "yum", "dnf", "pacman", "pip", "pip3",
        "systemctl", "service", "init", "shutdown", "reboot", "halt",
        "iptables", "ip6tables", "nft",
        "insmod", "rmmod", "modprobe",
        "crontab", "at",
        "strace", "ltrace", "gdb",
        "passwd", "useradd", "userdel", "usermod", "groupadd",
    ].into_iter().collect()
}

/// 解析命令字串為管道分隔的子命令，並驗證每一個子命令是否安全
/// Parses a command string into pipe-separated sub-commands and validates each one
fn is_command_safe(cmd: &str) -> Result<(), String> {
    let cmd_trimmed = cmd.trim();
    
    if cmd_trimmed.is_empty() {
        return Ok(());
    }

    // 1. 檢查危險 shell 元字符
    for pattern in get_dangerous_shell_chars() {
        if cmd_trimmed.contains(pattern) {
            return Err(format!("禁止的操作: 包含不安全的字符 '{}'", pattern));
        }
    }

    // 2. 禁止命令串接符號 (;, &&, ||)
    //    我們只允許管道 (|) 和簡單重定向 (>)
    if cmd_trimmed.contains(';') {
        return Err("禁止使用 ';' 串接命令。".to_string());
    }
    if cmd_trimmed.contains("&&") {
        return Err("禁止使用 '&&' 串接命令。".to_string());
    }
    if cmd_trimmed.contains("||") {
        return Err("禁止使用 '||' 串接命令。".to_string());
    }

    // 3. 解析管道：每個 | 分隔的子命令都必須通過白名單
    let allowed = get_allowed_commands();
    let dangerous = get_dangerous_commands();
    let sub_commands: Vec<&str> = cmd_trimmed.split('|').collect();
    
    for (i, sub_cmd) in sub_commands.iter().enumerate() {
        let sub_trimmed = sub_cmd.trim();
        if sub_trimmed.is_empty() {
            if i > 0 {
                continue; // 允許末尾管道（雖然沒意義）
            }
            return Ok(());
        }

        // 處理輸出重定向：移除 > filename 部分再驗證
        let without_redirect = if let Some(pos) = sub_trimmed.find('>') {
            sub_trimmed[..pos].trim()
        } else {
            sub_trimmed
        };

        let parts: Vec<&str> = without_redirect.split_whitespace().collect();
        if parts.is_empty() {
            continue;
        }

        let command_name = parts[0];

        // 3a. 檢查是否在危險命令名單中
        if dangerous.contains(command_name) {
            return Err(format!("命令 '{}' 被禁止執行。", command_name));
        }

        // 3b. 檢查是否在白名單中
        if !allowed.contains(command_name) {
            return Err(format!(
                "命令 '{}' 不在允許列表中。輸入 'help' 查看可用命令。",
                command_name
            ));
        }

        // 3c. 額外的 rm 安全檢查
        if command_name == "rm" {
            let args_str = without_redirect.to_lowercase();
            if args_str.contains("-rf") || args_str.contains("-fr") || args_str.contains("--no-preserve-root") {
                return Err("禁止使用 rm -rf 命令".to_string());
            }
        }

        // 3d. 檢查參數中是否有嘗試存取敏感路徑
        for part in &parts[1..] {
            let lower = part.to_lowercase();
            if lower.contains("/etc/passwd") || lower.contains("/etc/shadow") || lower.contains("/dev/sd") {
                return Err(format!("禁止存取敏感路徑: {}", part));
            }
        }
    }

    Ok(())
}

/// Tab 補全：檔案和目錄
fn get_completions(partial: &str, current_dir: &str, storage_base: &str) -> Vec<String> {
    let mut completions = Vec::new();
    
    // 分離命令和參數
    let parts: Vec<&str> = partial.split_whitespace().collect();
    
    if parts.is_empty() || (parts.len() == 1 && !partial.ends_with(' ')) {
        // 補全命令
        let prefix = parts.first().unwrap_or(&"");
        for cmd in get_available_commands() {
            if cmd.starts_with(prefix) {
                completions.push(cmd.to_string());
            }
        }
    } else {
        // 補全檔案/目錄路徑
        let path_part = if partial.ends_with(' ') { "" } else { parts.last().unwrap_or(&"") };
        
        let (dir_to_search, file_prefix) = if path_part.contains('/') {
            let last_slash = path_part.rfind('/').unwrap();
            let dir = &path_part[..=last_slash];
            let prefix = &path_part[last_slash + 1..];
            
            // 構建完整路徑
            let full_dir = if dir.starts_with('/') || dir.starts_with("~/") {
                if dir.starts_with("~/") {
                    format!("{}{}", storage_base, &dir[1..])
                } else {
                    format!("{}{}", storage_base, dir)
                }
            } else {
                format!("{}/{}", current_dir, dir)
            };
            (full_dir, prefix.to_string())
        } else {
            (current_dir.to_string(), path_part.to_string())
        };
        
        // 讀取目錄內容
        if let Ok(entries) = std::fs::read_dir(&dir_to_search) {
            for entry in entries.filter_map(|e| e.ok()) {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with(&file_prefix) {
                    let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                    let display_name = if is_dir {
                        format!("{}/", name)
                    } else {
                        name
                    };
                    completions.push(display_name);
                }
            }
        }
    }
    
    completions.sort();
    completions
}

/// 處理內建命令
fn handle_builtin_command(cmd: &str, current_dir: &str) -> Option<String> {
    let parts: Vec<&str> = cmd.split_whitespace().collect();
    if parts.is_empty() {
        return Some(String::new());
    }

    match parts[0] {
        "help" => Some(format!(
            "\x1b[32m╔══════════════════════════════════════════════════════════════╗\x1b[0m\r\n\
             \x1b[32m║\x1b[0m  \x1b[1;36mKoimsurai NAS Terminal - 受限 Shell 環境\x1b[0m                    \x1b[32m║\x1b[0m\r\n\
             \x1b[32m╠══════════════════════════════════════════════════════════════╣\x1b[0m\r\n\
             \x1b[32m║\x1b[0m  \x1b[33m檔案操作:\x1b[0m ls, cat, head, tail, mkdir, touch, cp, mv, rm     \x1b[32m║\x1b[0m\r\n\
             \x1b[32m║\x1b[0m  \x1b[33m目錄導航:\x1b[0m cd, pwd, find, stat, file                         \x1b[32m║\x1b[0m\r\n\
             \x1b[32m║\x1b[0m  \x1b[33m文字處理:\x1b[0m grep, wc, sort, uniq, cut, awk, sed, diff         \x1b[32m║\x1b[0m\r\n\
             \x1b[32m║\x1b[0m  \x1b[33m系統資訊:\x1b[0m df, du, free, uptime, ps, top                     \x1b[32m║\x1b[0m\r\n\
             \x1b[32m║\x1b[0m  \x1b[33m媒體工具:\x1b[0m ffmpeg, ffprobe                                   \x1b[32m║\x1b[0m\r\n\
             \x1b[32m║\x1b[0m  \x1b[33m其他:\x1b[0m     date, echo, clear, history, exit                  \x1b[32m║\x1b[0m\r\n\
             \x1b[32m╠══════════════════════════════════════════════════════════════╣\x1b[0m\r\n\
             \x1b[32m║\x1b[0m  \x1b[34mTab\x1b[0m 自動補全 | \x1b[34m↑↓\x1b[0m 歷史記錄 | \x1b[34mCtrl+C\x1b[0m 取消            \x1b[32m║\x1b[0m\r\n\
             \x1b[32m╚══════════════════════════════════════════════════════════════╝\x1b[0m"
        )),
        "clear" => Some("\x1b[2J\x1b[H".to_string()),
        "exit" | "logout" => Some("\x1b[33m再見！終端機連線已關閉。\x1b[0m".to_string()),
        "pwd" => Some(current_dir.to_string()),
        _ => None,  // 非內建命令，需要執行
    }
}

/// WebSocket 終端機端點
#[utoipa::path(
    get,
    path = "/api/terminal",
    params(
        ("cols" = Option<u16>, Query, description = "終端機列數"),
        ("rows" = Option<u16>, Query, description = "終端機行數"),
    ),
    responses(
        (status = 101, description = "WebSocket 連線建立"),
        (status = 401, description = "未授權"),
    ),
    tag = "Terminal"
)]
pub async fn terminal_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Query(query): Query<TerminalQuery>,
) -> impl IntoResponse {
    tracing::info!("Terminal WebSocket connection requested: cols={}, rows={}", query.cols, query.rows);
    ws.on_upgrade(move |socket| handle_terminal_socket(socket, state, query))
}

async fn handle_terminal_socket(socket: WebSocket, state: AppState, query: TerminalQuery) {
    let (mut sender, mut receiver) = socket.split();
    
    // 當前工作目錄（限制在 storage 內）
    let storage_path = state.storage_path.clone();
    let mut current_dir = storage_path.to_string_lossy().to_string();
    
    // 發送歡迎訊息
    let welcome = format!(
        "\x1b[2J\x1b[H\
         \x1b[36m╔════════════════════════════════════════════════════╗\x1b[0m\r\n\
         \x1b[36m║\x1b[0m  \x1b[1;32mKoimsurai NAS Terminal\x1b[0m                            \x1b[36m║\x1b[0m\r\n\
         \x1b[36m║\x1b[0m  \x1b[90mSecure Restricted Shell Environment\x1b[0m                \x1b[36m║\x1b[0m\r\n\
         \x1b[36m╠════════════════════════════════════════════════════╣\x1b[0m\r\n\
         \x1b[36m║\x1b[0m  輸入 \x1b[33mhelp\x1b[0m 查看可用命令                            \x1b[36m║\x1b[0m\r\n\
         \x1b[36m╚════════════════════════════════════════════════════╝\x1b[0m\r\n\r\n"
    );
    
    if sender.send(Message::Text(welcome)).await.is_err() {
        return;
    }

    // 發送初始提示符
    let prompt = format!("\x1b[36mnas\x1b[0m:\x1b[34m{}\x1b[0m$ ", get_display_path(&current_dir, &storage_path.to_string_lossy()));
    if sender.send(Message::Text(prompt)).await.is_err() {
        return;
    }

    let mut input_buffer = String::new();
    let mut command_history: Vec<String> = Vec::new();
    let mut history_index: usize = 0;

    while let Some(Ok(msg)) = receiver.next().await {
        match msg {
            Message::Text(text) => {
                // 處理 JSON 格式的 resize 訊息
                if text.starts_with('{') {
                    if let Ok(resize) = serde_json::from_str::<serde_json::Value>(&text) {
                        if resize.get("type").and_then(|t| t.as_str()) == Some("resize") {
                            // Handle resize - in a real implementation, this would resize the PTY
                            continue;
                        }
                    }
                }

                // 處理字符輸入
                for ch in text.chars() {
                    match ch {
                        '\r' | '\n' => {
                            // Enter 鍵 - 執行命令
                            let _ = sender.send(Message::Text("\r\n".to_string())).await;
                            
                            let cmd = input_buffer.trim().to_string();
                            if !cmd.is_empty() {
                                command_history.push(cmd.clone());
                                history_index = command_history.len();
                            }
                            
                            // 執行命令
                            let output = execute_command(&cmd, &mut current_dir, &storage_path.to_string_lossy()).await;
                            
                            if !output.is_empty() {
                                let _ = sender.send(Message::Text(format!("{}\r\n", output))).await;
                            }

                            // 檢查是否是 exit 命令
                            if cmd.trim() == "exit" || cmd.trim() == "logout" {
                                let _ = sender.close().await;
                                return;
                            }
                            
                            input_buffer.clear();
                            
                            // 發送新提示符
                            let prompt = format!("\x1b[36mnas\x1b[0m:\x1b[34m{}\x1b[0m$ ", 
                                get_display_path(&current_dir, &storage_path.to_string_lossy()));
                            let _ = sender.send(Message::Text(prompt)).await;
                        }
                        '\t' => {
                            // Tab 鍵 - 自動補全
                            let completions = get_completions(&input_buffer, &current_dir, &storage_path.to_string_lossy());
                            
                            if completions.len() == 1 {
                                // 唯一匹配：直接補全
                                let completion = &completions[0];
                                
                                // 找出需要補全的部分
                                let parts: Vec<&str> = input_buffer.split_whitespace().collect();
                                let last_part = if input_buffer.ends_with(' ') { "" } else { parts.last().unwrap_or(&"") };
                                
                                // 計算需要添加的字符
                                let to_add = if completion.len() > last_part.len() {
                                    &completion[last_part.len()..]
                                } else {
                                    ""
                                };
                                
                                if !to_add.is_empty() {
                                    input_buffer.push_str(to_add);
                                    let _ = sender.send(Message::Text(to_add.to_string())).await;
                                }
                            } else if completions.len() > 1 {
                                // 多個匹配：顯示所有選項
                                let _ = sender.send(Message::Text("\r\n".to_string())).await;
                                
                                // 格式化輸出（類似 bash）
                                let max_len = completions.iter().map(|s| s.len()).max().unwrap_or(10) + 2;
                                let cols = 80 / max_len.max(10);
                                
                                for (i, comp) in completions.iter().enumerate() {
                                    let padded = format!("{:<width$}", comp, width = max_len);
                                    let _ = sender.send(Message::Text(padded)).await;
                                    if (i + 1) % cols == 0 {
                                        let _ = sender.send(Message::Text("\r\n".to_string())).await;
                                    }
                                }
                                
                                if completions.len() % cols != 0 {
                                    let _ = sender.send(Message::Text("\r\n".to_string())).await;
                                }
                                
                                // 重新顯示提示符和當前輸入
                                let prompt = format!("\x1b[36mnas\x1b[0m:\x1b[34m{}\x1b[0m$ ", 
                                    get_display_path(&current_dir, &storage_path.to_string_lossy()));
                                let _ = sender.send(Message::Text(format!("{}{}", prompt, input_buffer))).await;
                                
                                // 嘗試補全共同前綴
                                if let Some(common) = find_common_prefix(&completions) {
                                    let parts: Vec<&str> = input_buffer.split_whitespace().collect();
                                    let last_part = if input_buffer.ends_with(' ') { "" } else { parts.last().unwrap_or(&"") };
                                    
                                    if common.len() > last_part.len() {
                                        let to_add = &common[last_part.len()..];
                                        input_buffer.push_str(to_add);
                                        let _ = sender.send(Message::Text(to_add.to_string())).await;
                                    }
                                }
                            }
                        }
                        '\x7f' | '\x08' => {
                            // Backspace
                            if !input_buffer.is_empty() {
                                input_buffer.pop();
                                let _ = sender.send(Message::Text("\x08 \x08".to_string())).await;
                            }
                        }
                        '\x03' => {
                            // Ctrl+C
                            input_buffer.clear();
                            let _ = sender.send(Message::Text("^C\r\n".to_string())).await;
                            let prompt = format!("\x1b[36mnas\x1b[0m:\x1b[34m{}\x1b[0m$ ", 
                                get_display_path(&current_dir, &storage_path.to_string_lossy()));
                            let _ = sender.send(Message::Text(prompt)).await;
                        }
                        '\x1b' => {
                            // Escape sequence (arrow keys, etc.) - skip for now
                        }
                        _ if ch.is_ascii_graphic() || ch == ' ' => {
                            input_buffer.push(ch);
                            let _ = sender.send(Message::Text(ch.to_string())).await;
                        }
                        _ => {}
                    }
                }
            }
            Message::Binary(data) => {
                // 處理二進位資料（同文字處理）
                if let Ok(text) = String::from_utf8(data) {
                    for ch in text.chars() {
                        if ch.is_ascii_graphic() || ch == ' ' {
                            input_buffer.push(ch);
                            let _ = sender.send(Message::Text(ch.to_string())).await;
                        }
                    }
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }
    
    tracing::info!("Terminal WebSocket connection closed");
}

/// 找出字串列表的共同前綴
fn find_common_prefix(strings: &[String]) -> Option<String> {
    if strings.is_empty() {
        return None;
    }
    if strings.len() == 1 {
        return Some(strings[0].clone());
    }
    
    let first = &strings[0];
    let mut prefix_len = first.len();
    
    for s in &strings[1..] {
        let common = first.chars()
            .zip(s.chars())
            .take_while(|(a, b)| a == b)
            .count();
        prefix_len = prefix_len.min(common);
    }
    
    if prefix_len > 0 {
        Some(first[..prefix_len].to_string())
    } else {
        None
    }
}

/// 獲取顯示路徑（相對於 storage）
fn get_display_path(current: &str, storage_base: &str) -> String {
    if current == storage_base {
        "~".to_string()
    } else if current.starts_with(storage_base) {
        format!("~{}", &current[storage_base.len()..])
    } else {
        current.to_string()
    }
}

/// 執行命令
async fn execute_command(cmd: &str, current_dir: &mut String, storage_base: &str) -> String {
    let cmd = cmd.trim();
    
    if cmd.is_empty() {
        return String::new();
    }

    // 先檢查命令安全性
    if let Err(e) = is_command_safe(cmd) {
        return format!("\x1b[31m錯誤: {}\x1b[0m", e);
    }

    // 處理內建命令
    if let Some(output) = handle_builtin_command(cmd, current_dir) {
        return output;
    }

    // 處理 cd 命令
    let parts: Vec<&str> = cmd.split_whitespace().collect();
    if parts[0] == "cd" {
        return handle_cd_command(&parts, current_dir, storage_base);
    }

    // 執行外部命令（在受限環境中）
    execute_external_command(cmd, current_dir, storage_base).await
}

/// 處理 cd 命令
fn handle_cd_command(parts: &[&str], current_dir: &mut String, storage_base: &str) -> String {
    let target = if parts.len() > 1 {
        parts[1]
    } else {
        "~"
    };

    let new_path = if target == "~" || target == "" {
        storage_base.to_string()
    } else if target == ".." {
        let path = std::path::Path::new(current_dir);
        if let Some(parent) = path.parent() {
            let parent_str = parent.to_string_lossy().to_string();
            // 不允許離開 storage 目錄
            if parent_str.starts_with(storage_base) {
                parent_str
            } else {
                return format!("\x1b[31m錯誤: 無法離開 storage 目錄\x1b[0m");
            }
        } else {
            return format!("\x1b[31m錯誤: 已在根目錄\x1b[0m");
        }
    } else if target.starts_with('/') {
        // 絕對路徑 - 必須在 storage 內
        let full_path = format!("{}{}", storage_base, target);
        if std::path::Path::new(&full_path).exists() {
            full_path
        } else {
            return format!("\x1b[31m錯誤: 目錄不存在: {}\x1b[0m", target);
        }
    } else if target.starts_with("~/") {
        format!("{}{}", storage_base, &target[1..])
    } else {
        // 相對路徑
        format!("{}/{}", current_dir, target)
    };

    // 驗證路徑
    let canonical = match std::fs::canonicalize(&new_path) {
        Ok(p) => p.to_string_lossy().to_string(),
        Err(_) => return format!("\x1b[31m錯誤: 目錄不存在: {}\x1b[0m", target),
    };

    // 確保在 storage 範圍內
    if !canonical.starts_with(storage_base) {
        return format!("\x1b[31m錯誤: 無法訪問 storage 目錄之外的路徑\x1b[0m");
    }

    // 確保是目錄
    if !std::path::Path::new(&canonical).is_dir() {
        return format!("\x1b[31m錯誤: 不是目錄: {}\x1b[0m", target);
    }

    *current_dir = canonical;
    String::new()
}

/// 執行外部命令（不透過 sh -c，直接執行白名單命令）
/// Execute external command directly without sh -c to prevent command injection
async fn execute_external_command(cmd: &str, current_dir: &str, storage_base: &str) -> String {
    use tokio::process::Command;

    let cmd_trimmed = cmd.trim();

    // 檢查是否有管道
    if cmd_trimmed.contains('|') {
        return execute_pipeline(cmd_trimmed, current_dir, storage_base).await;
    }

    // 處理輸出重定向 (>)
    let (command_part, redirect_target) = if let Some(pos) = cmd_trimmed.find('>') {
        let target = cmd_trimmed[pos + 1..].trim().to_string();
        let cmd_part = cmd_trimmed[..pos].trim();
        (cmd_part.to_string(), Some(target))
    } else {
        (cmd_trimmed.to_string(), None)
    };

    let parts: Vec<&str> = command_part.split_whitespace().collect();
    if parts.is_empty() {
        return String::new();
    }

    let program = parts[0];
    let args = &parts[1..];

    // 解析路徑參數：確保所有路徑在 storage 範圍內
    let resolved_args: Vec<String> = args.iter().map(|a| {
        // 如果參數看起來像路徑且不是 flag，不做特殊處理
        // Command 會以當前目錄為基礎解析相對路徑
        a.to_string()
    }).collect();

    let result = Command::new(program)
        .args(&resolved_args)
        .current_dir(current_dir)
        .env("HOME", storage_base)
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("TERM", "xterm-256color")
        .output()
        .await;

    match result {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);

            // 處理重定向
            if let Some(ref target) = redirect_target {
                if !target.is_empty() {
                    let target_path = if target.starts_with('/') {
                        format!("{}{}", storage_base, target)
                    } else {
                        format!("{}/{}", current_dir, target)
                    };
                    // 驗證路徑在 storage 內
                    if let Ok(canonical) = std::fs::canonicalize(std::path::Path::new(&target_path).parent().unwrap_or(std::path::Path::new(current_dir))) {
                        if canonical.to_string_lossy().starts_with(storage_base) {
                            if let Err(e) = std::fs::write(&target_path, stdout.as_bytes()) {
                                return format!("\x1b[31m寫入錯誤: {}\x1b[0m", e);
                            }
                            if !stderr.is_empty() {
                                return format!("\x1b[31m{}\x1b[0m", stderr.replace('\n', "\r\n"));
                            }
                            return String::new();
                        }
                    }
                    return "\x1b[31m錯誤: 重定向目標不在 storage 範圍內\x1b[0m".to_string();
                }
            }

            let mut result = String::new();
            if !stdout.is_empty() {
                result.push_str(&stdout.replace('\n', "\r\n"));
            }
            if !stderr.is_empty() {
                result.push_str(&format!("\x1b[31m{}\x1b[0m", stderr.replace('\n', "\r\n")));
            }
            result.trim_end().to_string()
        }
        Err(e) => format!("\x1b[31m執行錯誤: {}\x1b[0m", e),
    }
}

/// 安全地執行管道命令（每一段都直接執行，不透過 shell）
/// 使用中間 Vec<u8> 傳遞管道資料，避免跨進程 fd 問題
/// Safely execute piped commands - each segment runs directly without shell
async fn execute_pipeline(cmd: &str, current_dir: &str, storage_base: &str) -> String {
    use tokio::process::Command;
    use std::process::Stdio;
    use tokio::io::AsyncWriteExt;

    let segments: Vec<&str> = cmd.split('|').collect();
    
    if segments.is_empty() {
        return String::new();
    }

    // 執行第一個命令取得輸出
    let first_parts: Vec<&str> = segments[0].trim().split_whitespace().collect();
    if first_parts.is_empty() {
        return String::new();
    }

    let first_output = match Command::new(first_parts[0])
        .args(&first_parts[1..])
        .current_dir(current_dir)
        .env("HOME", storage_base)
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("TERM", "xterm-256color")
        .output()
        .await
    {
        Ok(o) => o,
        Err(e) => return format!("\x1b[31m執行錯誤 ({}): {}\x1b[0m", first_parts[0], e),
    };

    let mut current_stdout = first_output.stdout;

    // 依序執行管道中的每個後續命令，將前一個的 stdout 作為 stdin
    for segment in &segments[1..] {
        let parts: Vec<&str> = segment.trim().split_whitespace().collect();
        if parts.is_empty() {
            continue;
        }

        let mut child = match Command::new(parts[0])
            .args(&parts[1..])
            .current_dir(current_dir)
            .env("HOME", storage_base)
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .env("TERM", "xterm-256color")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => return format!("\x1b[31m執行錯誤 ({}): {}\x1b[0m", parts[0], e),
        };

        // 寫入前一個命令的 stdout 到當前命令的 stdin
        if let Some(mut stdin) = child.stdin.take() {
            let data = current_stdout.clone();
            tokio::spawn(async move {
                let _ = stdin.write_all(&data).await;
                drop(stdin); // 關閉 stdin 讓命令知道輸入結束
            });
        }

        match child.wait_with_output().await {
            Ok(output) => {
                current_stdout = output.stdout;
                // 如果有 stderr 且非空，附加到最終輸出
                if !output.stderr.is_empty() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    if !stderr.trim().is_empty() {
                        // stderr 不阻塞管道，但最後會顯示
                        tracing::debug!("Pipeline stderr from {}: {}", parts[0], stderr);
                    }
                }
            }
            Err(e) => return format!("\x1b[31m執行錯誤 ({}): {}\x1b[0m", parts[0], e),
        }
    }

    let stdout = String::from_utf8_lossy(&current_stdout);
    let mut result = String::new();
    if !stdout.is_empty() {
        result.push_str(&stdout.replace('\n', "\r\n"));
    }
    result.trim_end().to_string()
}

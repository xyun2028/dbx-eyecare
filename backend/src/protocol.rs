//! 出站帧与日志。

use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

pub type SharedWriter = Arc<Mutex<std::io::Stdout>>;

/// 单个日志文件的上限。超过就换成 `.1`，只保留一份旧的。
///
/// 侧车是长期驻留的：改屏日志以前一条一拍、一拍一块屏，一天三万多行，
/// 一年能滚到 GB 级。用户在磁盘告警之前不会发现是插件干的。
const LOG_MAX_BYTES: u64 = 2 * 1024 * 1024;
/// 每写这么多条才查一次文件大小，避免每条日志都 stat 一次磁盘。
const LOG_SIZE_CHECK_EVERY: u32 = 64;

static LOG_WRITES: AtomicU32 = AtomicU32::new(0);

fn write_frame(out: &SharedWriter, frame: Value) {
    let mut guard = match out.lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    // 一行一条 JSON，宿主按行切分
    let _ = writeln!(guard, "{frame}");
    let _ = guard.flush();
}

pub fn emit_result(out: &SharedWriter, id: Value, result: Value) {
    // jsonrpc 字段一条都不能少，宿主会直接拒绝没有它的消息
    write_frame(out, json!({ "jsonrpc": "2.0", "id": id, "result": result }));
}

pub fn emit_error(out: &SharedWriter, id: Value, code: i32, message: &str) {
    write_frame(
        out,
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": code, "message": message }
        }),
    );
}

/// 事件必须是通知（没有 id），否则宿主会当成响应丢掉。
pub fn emit_event(out: &SharedWriter, method: &str, params: Value) {
    write_frame(out, json!({ "jsonrpc": "2.0", "method": method, "params": params }));
}

/// 日志涨到上限就挪成 `<名字>.1`，只留最近一份。
///
/// Windows 上 `rename` 到已存在的目标会失败，所以先删旧的。
/// 如果用户正拿编辑器打开着日志，删除或改名都可能失败——那就留到下一条再试。
/// 宁可多占几 MB，也不能为了控体积把日志写丢。
fn rotate_if_needed(path: &Path) {
    let Ok(meta) = std::fs::metadata(path) else {
        return;
    };
    if meta.len() < LOG_MAX_BYTES {
        return;
    }

    let rotated = path.with_extension("log.1");
    let _ = std::fs::remove_file(&rotated);
    let _ = std::fs::rename(path, &rotated);
}

pub fn log_line(root: &Path, message: &str) {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let dir = root.to_path_buf();
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("sidecar.log");

    if LOG_WRITES.fetch_add(1, Ordering::Relaxed) % LOG_SIZE_CHECK_EVERY == 0 {
        rotate_if_needed(&path);
    }

    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(file, "{stamp} {message}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 每个用例用独立目录，避免并行跑测试时互相踩。
    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "eyecare-log-{}-{tag}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn fill(path: &Path, bytes: u64) {
        let chunk = vec![b'x'; 4096];
        let mut written = 0u64;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        while written < bytes {
            let take = ((bytes - written) as usize).min(chunk.len());
            file.write_all(&chunk[..take]).unwrap();
            written += take as u64;
        }
    }

    #[test]
    fn 超过上限的日志会被换成旧文件() {
        let dir = temp_dir("rotate");
        let path = dir.join("sidecar.log");
        fill(&path, LOG_MAX_BYTES + 1);

        rotate_if_needed(&path);

        let rotated = dir.join("sidecar.log.1");
        assert!(rotated.exists(), "旧日志应被挪到 .1");
        assert!(!path.exists(), "当前日志应已清空，等下一次 append 重建");
        assert!(std::fs::metadata(&rotated).unwrap().len() > LOG_MAX_BYTES);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 没到上限时不动日志() {
        let dir = temp_dir("keep");
        let path = dir.join("sidecar.log");
        fill(&path, 1024);

        rotate_if_needed(&path);

        assert!(path.exists(), "没超限不该轮转");
        assert!(!dir.join("sidecar.log.1").exists());
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 1024);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 轮转两次只留一份旧文件() {
        let dir = temp_dir("twice");
        let path = dir.join("sidecar.log");

        fill(&path, LOG_MAX_BYTES + 1);
        rotate_if_needed(&path);

        fill(&path, LOG_MAX_BYTES + 1);
        rotate_if_needed(&path);

        let rotated = dir.join("sidecar.log.1");
        assert_eq!(
            std::fs::metadata(&rotated).unwrap().len(),
            LOG_MAX_BYTES + 1,
            "第二次轮转应覆盖旧的 .1"
        );
        assert!(!dir.join("sidecar.log.2").exists(), "只保留一份备份");

        let _ = std::fs::remove_dir_all(&dir);
    }
}

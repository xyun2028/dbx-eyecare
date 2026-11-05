//! 出站帧与日志。

use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

pub type SharedWriter = Arc<Mutex<std::io::Stdout>>;

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

pub fn log_line(root: &Path, message: &str) {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let dir = root.to_path_buf();
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("sidecar.log"))
    {
        let _ = writeln!(file, "{stamp} {message}");
    }
}

//! Eye眼 · DBX 插件侧车。
//!
//! 形态：DBX 启动的子进程，stdin/stdout 上行分隔 JSON-RPC 2.0。
//! 这个文件只做三件事：起状态、跑后台循环、读一行交给 `rpc::dispatch`。

mod color;
mod gamma;
mod model;
mod protocol;
mod rpc;
mod state;

use std::io::BufRead;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use crate::protocol::{emit_error, emit_event, emit_result, log_line, SharedWriter};
use crate::state::{apply_now, clear_backup, data_root, restore_all, AppState};

fn main() {
    let out: SharedWriter = Arc::new(Mutex::new(std::io::stdout()));
    let root = data_root();
    let state = Arc::new(AppState::load());

    // 启动日志：插件在宿主里出问题时，这是唯一能事后看到的证据
    log_line(
        &root,
        &format!(
            "start version={} os={} cwd={} data={}",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| "?".into()),
            root.display()
        ),
    );
    for key in ["DBX_PLUGIN_DATA_DIR", "DBX_PLUGIN_ID", "DBX_PLUGIN_ROOT"] {
        if let Ok(value) = std::env::var(key) {
            log_line(&root, &format!("env {key}={value}"));
        }
    }

    apply_now(&state, &state.config());

    // 后台循环：跟随时间自动变化、防系统重置 Gamma 表、到点提醒休息
    {
        let state = Arc::clone(&state);
        let out = Arc::clone(&out);
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(5));

            let cfg = state.config();

            // 关闭状态下不要反复写屏，否则会盖掉用户自己的调整
            if cfg.enabled {
                apply_now(&state, &cfg);
            }

            if cfg.break_plan.enabled {
                if let Some(due) = cfg.break_plan.due_at {
                    if model::now_secs() >= due {
                        emit_event(
                            &out,
                            "evt/break.due",
                            json!({
                                "restSeconds": cfg.break_plan.rest_seconds,
                                "at": model::now_secs(),
                            }),
                        );
                        let mut next = cfg.clone();
                        next.break_plan.due_at =
                            Some(model::now_secs() + cfg.break_plan.work_minutes.max(1) as u64 * 60);
                        *state.config.lock().unwrap() = next.clone();
                        state.persist();
                    }
                }
            }
        });
    }

    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let msg: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                emit_error(&out, Value::Null, -32700, &format!("parse error: {e}"));
                continue;
            }
        };

        let id = msg.get("id").cloned();
        let method = msg.get("method").and_then(Value::as_str).unwrap_or_default();
        let params = msg.get("params").cloned().unwrap_or(Value::Null);

        if method.is_empty() {
            continue;
        }
        log_line(&root, &format!("<- {method}"));

        // 没有 id 的是通知：执行但不回包，回包会污染会话
        let Some(id) = id else {
            let _ = rpc::dispatch(method, &params, &state, &out, &root);
            continue;
        };

        if method == "shutdown" || method == "plugin/shutdown" {
            let _ = emit_result(&out, id, json!({ "ok": true }));
            break;
        }

        match rpc::dispatch(method, &params, &state, &out, &root) {
            Ok(result) => emit_result(&out, id, result),
            Err((code, message)) => emit_error(&out, id, code, &message),
        }
    }

    // 宿主关掉 stdin 就等于插件被卸载：走之前把屏幕还回去
    let saved = state.saved.lock().unwrap().clone();
    restore_all(&saved);
    clear_backup();
    log_line(&root, "stdin closed, screen restored");
}

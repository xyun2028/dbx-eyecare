//! 方法分发。
//!
//! 握手是第一步：`plugin/initialize` 不过，后面所有调用都会被宿主拒绝。

use std::sync::Arc;

use serde_json::{json, Value};

use crate::gamma;
use crate::model::{self, minutes_now, now_secs, resolve, Config, PRESET_CUSTOM};
use crate::protocol::{log_line, SharedWriter};
use crate::state::{apply_now, AppState};

fn plugin_identity() -> (String, String) {
    // 从可执行文件往上找 manifest.json，别把 id / version 写死在代码里
    if let Ok(exe) = std::env::current_exe() {
        let mut dir = exe.parent().map(|p| p.to_path_buf());
        for _ in 0..6 {
            let Some(current) = dir else { break };
            let candidate = current.join("manifest.json");
            if let Ok(text) = std::fs::read_to_string(&candidate) {
                if let Ok(value) = serde_json::from_str::<Value>(&text) {
                    let id = value.get("id").and_then(Value::as_str).unwrap_or_default();
                    let version = value.get("version").and_then(Value::as_str).unwrap_or_default();
                    if !id.is_empty() {
                        return (id.to_string(), version.to_string());
                    }
                }
            }
            dir = current.parent().map(|p| p.to_path_buf());
        }
    }
    ("com.eyecare.dbx".to_string(), "0.1.0".to_string())
}

fn state_payload(state: &AppState) -> Value {
    let cfg = state.config();
    let displays = gamma::list();
    let effective = resolve(&cfg, minutes_now());

    json!({
        "enabled": cfg.enabled,
        "preset": cfg.preset,
        "kelvin": cfg.kelvin,
        "brightness": cfg.brightness,
        "color": cfg.color,
        "target": cfg.target,
        "presets": model::presets(),
        "displays": displays,
        "effective": effective,
        "schedule": cfg.schedule,
        "break": {
            "enabled": cfg.break_plan.enabled,
            "workMinutes": cfg.break_plan.work_minutes,
            "restSeconds": cfg.break_plan.rest_seconds,
            "remainingSeconds": cfg.break_plan.remaining(),
        },
        "platform": std::env::consts::OS,
        "mock": false,
    })
}

fn parse_color(value: &Value) -> Option<[u8; 3]> {
    let arr = value.as_array()?;
    if arr.len() != 3 {
        return None;
    }
    let mut out = [0u8; 3];
    for (i, item) in arr.iter().enumerate() {
        out[i] = item.as_u64()?.min(255) as u8;
    }
    Some(out)
}

pub fn dispatch(
    method: &str,
    params: &Value,
    state: &Arc<AppState>,
    _out: &SharedWriter,
    data_root: &std::path::Path,
) -> Result<Value, (i32, String)> {
    if method == "plugin/initialize" || method == "initialize" {
        let offered: Vec<i64> = params
            .get("host")
            .and_then(|host| host.get("protocolVersions"))
            .and_then(Value::as_array)
            .map(|list| list.iter().filter_map(Value::as_i64).collect())
            .unwrap_or_default();

        let (id, version) = plugin_identity();
        log_line(
            data_root,
            &format!("plugin/initialize offered={offered:?} identity={id}@{version}"),
        );

        if !offered.is_empty() && !offered.contains(&1) {
            return Err((-32001, "DBX 与插件没有共同的协议版本".to_string()));
        }

        return Ok(json!({
            "protocolVersion": 1,
            "capabilities": ["workbench"],
            "plugin": { "id": id, "version": version },
        }));
    }

    match method {
        "eyecare/ping" => Ok(json!({
            "ok": true,
            "version": env!("CARGO_PKG_VERSION"),
            "platform": std::env::consts::OS,
        })),

        "eyecare/state" => Ok(state_payload(state)),

        "eyecare/config" => {
            let mut cfg: Config = state.config();

            if let Some(v) = params.get("enabled").and_then(Value::as_bool) {
                cfg.enabled = v;
            }
            if let Some(v) = params.get("preset").and_then(Value::as_str) {
                cfg.preset = v.to_string();
            }
            if let Some(v) = params.get("kelvin").and_then(Value::as_u64) {
                cfg.kelvin = (v as u32).clamp(1000, 6500);
            }
            if let Some(v) = params.get("brightness").and_then(Value::as_f64) {
                cfg.brightness = v.clamp(0.1, 1.0);
            }
            if let Some(v) = params.get("color") {
                cfg.color = if v.is_null() { None } else { parse_color(v) };
            }
            if let Some(v) = params.get("target").and_then(Value::as_str) {
                cfg.target = v.to_string();
            }
            if let Some(s) = params.get("schedule").and_then(Value::as_object) {
                if let Some(v) = s.get("enabled").and_then(Value::as_bool) {
                    cfg.schedule.enabled = v;
                }
                if let Some(v) = s.get("dayKelvin").and_then(Value::as_u64) {
                    cfg.schedule.day_kelvin = (v as u32).clamp(1000, 6500);
                }
                if let Some(v) = s.get("nightKelvin").and_then(Value::as_u64) {
                    cfg.schedule.night_kelvin = (v as u32).clamp(1000, 6500);
                }
                if let Some(v) = s.get("dayStart").and_then(Value::as_u64) {
                    cfg.schedule.day_start = (v as u32).min(1439);
                }
                if let Some(v) = s.get("nightStart").and_then(Value::as_u64) {
                    cfg.schedule.night_start = (v as u32).min(1439);
                }
                if let Some(v) = s.get("transition").and_then(Value::as_u64) {
                    cfg.schedule.transition = (v as u32).min(180);
                }
            }

            *state.config.lock().unwrap() = cfg.clone();
            state.persist();
            apply_now(state, &cfg);
            Ok(state_payload(state))
        }

        // 选一个预设：把预设里的参数抄进配置，界面上一行代码都不用懂预设怎么算
        "eyecare/preset" => {
            let id = params
                .get("id")
                .and_then(Value::as_str)
                .ok_or((-32602, "缺少参数 id".to_string()))?;

            let mut cfg = state.config();
            let preset = model::presets()
                .into_iter()
                .find(|item| item.id == id)
                .ok_or((-32602, format!("没有这个预设：{id}")))?;

            cfg.preset = preset.id.clone();
            cfg.enabled = true;
            if let Some(kelvin) = preset.kelvin {
                cfg.kelvin = kelvin;
            }
            if preset.color.is_some() {
                cfg.color = preset.color;
            }
            if preset.color.is_none() {
                cfg.color = None;
            }

            *state.config.lock().unwrap() = cfg.clone();
            state.persist();
            apply_now(state, &cfg);
            Ok(state_payload(state))
        }

        // 自定义色温：拖动滑杆时用，自动切到 custom 模式
        "eyecare/tint" => {
            let mut cfg = state.config();
            cfg.preset = PRESET_CUSTOM.to_string();
            cfg.enabled = true;
            cfg.color = None;
            if let Some(v) = params.get("kelvin").and_then(Value::as_u64) {
                cfg.kelvin = (v as u32).clamp(1000, 6500);
            }
            if let Some(v) = params.get("brightness").and_then(Value::as_f64) {
                cfg.brightness = v.clamp(0.1, 1.0);
            }
            if let Some(v) = params.get("color") {
                cfg.color = if v.is_null() { None } else { parse_color(v) };
            }

            *state.config.lock().unwrap() = cfg.clone();
            state.persist();
            apply_now(state, &cfg);
            Ok(state_payload(state))
        }

        // 20-20-20 计时：开关、时长、跳过这一次
        "eyecare/break" => {
            let mut cfg = state.config();

            if let Some(v) = params.get("enabled").and_then(Value::as_bool) {
                cfg.break_plan.enabled = v;
                if v {
                    cfg.break_plan.arm();
                } else {
                    cfg.break_plan.disarm();
                }
            }
            if let Some(v) = params.get("workMinutes").and_then(Value::as_u64) {
                cfg.break_plan.work_minutes = (v as u32).clamp(1, 180);
                // 时长变了，重新计时
                cfg.break_plan.disarm();
                cfg.break_plan.arm();
            }
            if let Some(v) = params.get("restSeconds").and_then(Value::as_u64) {
                cfg.break_plan.rest_seconds = (v as u32).clamp(5, 600);
            }
            if params.get("action").and_then(Value::as_str) == Some("skip") {
                cfg.break_plan.due_at =
                    Some(now_secs() + cfg.break_plan.work_minutes.max(1) as u64 * 60);
            }

            cfg.break_plan.arm();
            *state.config.lock().unwrap() = cfg.clone();
            state.persist();
            Ok(state_payload(state))
        }

        other => Err((-32601, format!("未知方法：{other}"))),
    }
}

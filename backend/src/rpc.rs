//! 方法分发。
//!
//! 握手是第一步：`plugin/initialize` 不过，后面所有调用都会被宿主拒绝。

use std::sync::Arc;

use serde_json::{json, Value};

use crate::gamma;
use crate::model::{
    self, all_presets, new_custom_id, Anchor, Config, CustomPreset, Moment, ScheduleMode,
    KELVIN_MAX, KELVIN_MIN, PRESET_CUSTOM,
};
use crate::protocol::{log_line, SharedWriter};
use crate::state::{apply_now, rest_active, AppState};
use crate::sun;

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

fn parse_points(value: &Value) -> Vec<Anchor> {
    let Some(arr) = value.as_array() else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|item| {
            let at = item.get("at")?.as_u64()?;
            let kelvin = item.get("kelvin")?.as_u64()?;
            Some(Anchor {
                at: (at as u32).min(1439),
                kelvin: (kelvin as u32).clamp(KELVIN_MIN, KELVIN_MAX),
            })
        })
        .collect()
}

/// 曲线当前长什么样、今天太阳几点出来。界面画图要用，所以一起塞进 schedule。
fn schedule_payload(cfg: &Config, moment: &Moment) -> Value {
    let mut value = serde_json::to_value(&cfg.schedule).unwrap_or_else(|_| json!({}));

    let points = cfg.schedule.effective_points(moment);
    let sun_times = sun::sun_times(
        moment.day_of_year,
        cfg.schedule.lat,
        cfg.schedule.lon,
        moment.utc_offset_min,
    );

    if let Some(obj) = value.as_object_mut() {
        obj.insert(
            "effectivePoints".to_string(),
            serde_json::to_value(points).unwrap_or(Value::Null),
        );
        obj.insert(
            "sunTimes".to_string(),
            match sun_times {
                Some(t) => json!({ "sunrise": t.sunrise, "sunset": t.sunset }),
                // 极昼极夜时没有日出日落，界面据此不显示时刻
                None => Value::Null,
            },
        );
    }

    value
}

fn state_payload(state: &AppState) -> Value {
    let cfg = state.config();
    let moments = Moment::now();
    let displays = gamma::list();
    let effective = model::resolve(&cfg, &moments);
    // 平台能不能真的改屏幕，由 gamma 层判断：Windows 总是可以，
    // Linux 要看是不是 Wayland，macOS 要看有没有图形会话。
    let support = gamma::capability();
    let stats = state.stats.lock().unwrap().view();

    json!({
        "enabled": cfg.enabled,
        "preset": cfg.preset,
        "kelvin": cfg.kelvin,
        "brightness": cfg.brightness,
        "color": cfg.color,
        "target": cfg.target,
        "presets": all_presets(&cfg.custom_presets),
        "displays": displays,
        "effective": effective,
        "schedule": schedule_payload(&cfg, &moments),
        "break": {
            "enabled": cfg.break_plan.enabled,
            "workMinutes": cfg.break_plan.work_minutes,
            "restSeconds": cfg.break_plan.rest_seconds,
            "dimScreen": cfg.break_plan.dim_screen,
            "remainingSeconds": cfg.break_plan.remaining(),
            "resting": rest_active(state),
        },
        "stats": stats,
        "platform": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "support": support,
        "mock": false,
    })
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
                cfg.kelvin = (v as u32).clamp(KELVIN_MIN, KELVIN_MAX);
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
                if let Some(v) = s.get("mode").and_then(Value::as_str) {
                    cfg.schedule.mode = match v {
                        "sun" => ScheduleMode::Sun,
                        _ => ScheduleMode::Clock,
                    };
                }
                if let Some(v) = s.get("points") {
                    let points = parse_points(v);
                    // 空曲线会让换算失去基准，这种情况保留原曲线
                    if !points.is_empty() {
                        cfg.schedule.points = points;
                    }
                }
                if let Some(v) = s.get("weekendPoints") {
                    cfg.schedule.weekend_points = parse_points(v);
                }
                // 下面两个是老版本的两档参数，sun 模式下仍然在用
                if let Some(v) = s.get("dayKelvin").and_then(Value::as_u64) {
                    cfg.schedule.day_kelvin = (v as u32).clamp(KELVIN_MIN, KELVIN_MAX);
                }
                if let Some(v) = s.get("nightKelvin").and_then(Value::as_u64) {
                    cfg.schedule.night_kelvin = (v as u32).clamp(KELVIN_MIN, KELVIN_MAX);
                }
                if let Some(v) = s.get("transition").and_then(Value::as_u64) {
                    cfg.schedule.transition = (v as u32).min(180);
                }
                if let Some(v) = s.get("lat").and_then(Value::as_f64) {
                    cfg.schedule.lat = v.clamp(-90.0, 90.0);
                }
                if let Some(v) = s.get("lon").and_then(Value::as_f64) {
                    cfg.schedule.lon = v.clamp(-180.0, 180.0);
                }
                if let Some(v) = s.get("sunMargin").and_then(Value::as_u64) {
                    cfg.schedule.sun_margin = (v as u32).min(180);
                }
                cfg.schedule.normalize();
            }

            cfg.normalize();
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

            // 先看是不是用户自己存的
            if let Some(user) = cfg.find_custom(id).cloned() {
                cfg.preset = user.id.clone();
                cfg.enabled = true;
                if let Some(kelvin) = user.kelvin {
                    cfg.kelvin = kelvin.clamp(KELVIN_MIN, KELVIN_MAX);
                }
                cfg.color = user.color;
                cfg.brightness = user.brightness.clamp(0.1, 1.0);

                *state.config.lock().unwrap() = cfg.clone();
                state.persist();
                apply_now(state, &cfg);
                return Ok(state_payload(state));
            }

            let preset = model::builtin_presets()
                .into_iter()
                .find(|item| item.id == id)
                .ok_or((-32602, format!("没有这个预设：{id}")))?;

            cfg.preset = preset.id.clone();
            cfg.enabled = true;
            if let Some(kelvin) = preset.kelvin {
                cfg.kelvin = kelvin;
            }
            cfg.color = preset.color;

            *state.config.lock().unwrap() = cfg.clone();
            state.persist();
            apply_now(state, &cfg);
            Ok(state_payload(state))
        }

        // 把当前这套色温和亮度存成一个预设
        "eyecare/preset.save" => {
            let mut cfg = state.config();

            if cfg.custom_presets.len() >= 24 {
                return Err((-32000, "自定义预设最多 24 个，先删几个再用".to_string()));
            }

            let given = params
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string);

            let name = given.unwrap_or_else(|| {
                // 没给名字时按当前色温生成一个，比「未命名」有用
                if cfg.color.is_some() {
                    format!("滤色 {}", cfg.custom_presets.len() + 1)
                } else {
                    format!("{}K", cfg.kelvin)
                }
            });

            let id = new_custom_id(&cfg.custom_presets);
            cfg.custom_presets.push(CustomPreset {
                id: id.clone(),
                name,
                kelvin: if cfg.color.is_some() { None } else { Some(cfg.kelvin) },
                color: cfg.color,
                brightness: cfg.brightness,
            });

            cfg.preset = id;
            cfg.enabled = true;

            *state.config.lock().unwrap() = cfg.clone();
            state.persist();
            Ok(state_payload(state))
        }

        "eyecare/preset.rename" => {
            let id = params
                .get("id")
                .and_then(Value::as_str)
                .ok_or((-32602, "缺少参数 id".to_string()))?;
            let name = params
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or((-32602, "名字不能为空".to_string()))?;

            let mut cfg = state.config();
            let Some(item) = cfg.custom_presets.iter_mut().find(|p| p.id == id) else {
                return Err((-32602, format!("没有这个自定义预设：{id}")));
            };
            item.name = name.chars().take(24).collect();

            *state.config.lock().unwrap() = cfg.clone();
            state.persist();
            Ok(state_payload(state))
        }

        "eyecare/preset.delete" => {
            let id = params
                .get("id")
                .and_then(Value::as_str)
                .ok_or((-32602, "缺少参数 id".to_string()))?;

            let mut cfg = state.config();
            let before = cfg.custom_presets.len();
            cfg.custom_presets.retain(|p| p.id != id);
            if cfg.custom_presets.len() == before {
                return Err((-32602, format!("没有这个自定义预设：{id}")));
            }

            // 删掉的正好是当前在用的，退回「办公」，别让界面停在一个不存在的预设上
            if cfg.preset == id {
                cfg.preset = "office".to_string();
                cfg.kelvin = 4500;
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
                cfg.kelvin = (v as u32).clamp(KELVIN_MIN, KELVIN_MAX);
            }
            if let Some(v) = params.get("brightness").and_then(Value::as_f64) {
                cfg.brightness = v.clamp(0.1, 1.0);
            }
            if let Some(v) = params.get("color") {
                cfg.color = if v.is_null() { None } else { parse_color(v) };
            }

            cfg.normalize();
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
            if let Some(v) = params.get("dimScreen").and_then(Value::as_bool) {
                cfg.break_plan.dim_screen = v;
            }
            if params.get("action").and_then(Value::as_str) == Some("skip") {
                cfg.break_plan.due_at =
                    Some(model::now_secs() + cfg.break_plan.work_minutes.max(1) as u64 * 60);
            }
            // 手动跳过时也把正在进行的休息提示收掉，否则屏幕还停在暖色上
            if params.get("action").and_then(Value::as_str) == Some("endRest") {
                *state.rest_until.lock().unwrap() = None;
                apply_now(state, &cfg);
            }

            cfg.break_plan.arm();
            *state.config.lock().unwrap() = cfg.clone();
            state.persist();
            Ok(state_payload(state))
        }

        other => Err((-32601, format!("未知方法：{other}"))),
    }
}

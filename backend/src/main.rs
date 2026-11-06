//! Eye眼 · DBX 插件侧车。
//!
//! 形态：DBX 启动的子进程，stdin/stdout 上行分隔 JSON-RPC 2.0。
//! 这个文件只做三件事：起状态、跑后台循环、读一行交给 `rpc::dispatch`。

mod color;
mod gamma;
mod insight;
mod model;
mod presence;
mod protocol;
mod rpc;
mod state;
mod stats;
mod sun;

use std::io::BufRead;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use crate::protocol::{emit_error, emit_event, emit_result, log_line, SharedWriter};
use crate::state::{
    apply_now, apply_tick, begin_rest, clear_backup, data_root, gamma_conflict_id, restore_all,
    settle_rest, AppState, RestPending,
};

/// 后台循环的间隔。改屏、计时、统计都按这个粒度推进。
const TICK: u64 = 5;
/// 统计文件多久落一次盘。每次心跳都写文件没有必要。
const STATS_FLUSH_TICKS: u64 = 12;

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

    // 后台循环：跟随曲线变化、防系统重置 Gamma 表、到点提醒休息、记用眼时长
    {
        let state = Arc::clone(&state);
        let out = Arc::clone(&out);
        let mut ticks: u64 = 0;
        let mut conflict_seen = false;
        let mut last_tick = model::now_secs();

        std::thread::spawn(move || loop {
            // 休息时的压暗只有几秒，整拍睡会把这段压暗拖到下一拍（最坏多出 TICK 秒）。
            // 有正在进行的休息就只睡到它结束的那一刻，让配置的秒数说到做到。
            let nap = {
                let mut guard = state.rest_until.lock().unwrap();
                match *guard {
                    Some(end) => {
                        let remain = end.saturating_sub(model::now_secs());
                        if remain == 0 {
                            // 已经过期：就地清掉。留着会让下面的睡眠一直是 1 秒，
                            // 投屏或全屏期间就成了每秒一次的循环
                            *guard = None;
                            TICK
                        } else {
                            TICK.min(remain)
                        }
                    }
                    None => TICK,
                }
            };
            std::thread::sleep(Duration::from_secs(nap));

            let now = model::now_secs();
            // 睡眠可能被切短，暂停计时只能按真实经过的秒数往后推，不能一律加 TICK
            let elapsed = now.saturating_sub(last_tick).max(1);
            last_tick = now;
            ticks += 1;
            let activity = presence::probe();
            let cfg = state.config();
            let rule = &cfg.presence;

            let idle = rule.idle_aware && activity.is_idle();
            let fullscreen = rule.fullscreen_pause && activity.fullscreen;
            let presentation = now < rule.presentation_until;
            // 「人不在 / 全屏内容 / 演示」这三种情况都不该弹休息提醒。
            // 全屏和演示还会暂停写屏，避免在投屏或游戏里反复改 Gamma 表。
            let away = idle || fullscreen || presentation;
            let pause_writing = fullscreen || presentation;

            // 用眼时长只在护眼开着、且人确实在电脑前时累加。
            // 全屏看视频和投屏演示仍然算用眼，不排除。
            state.stats.lock().unwrap().tick(now, cfg.enabled && !idle);

            // 先结算上一次休息：到点不记，确认停手了才记
            settle_rest(&state, now, activity.idle_secs, rule.real_rest);

            // 暂停期间把到期时间往后推，等效于计时冻结。
            // 只改内存，不每次写盘；到刷新统计的那一拍顺手落一次。
            if away {
                let mut guard = state.config.lock().unwrap();
                if let Some(due) = guard.break_plan.due_at.as_mut() {
                    *due = due.saturating_add(elapsed);
                }
            }
            let cfg = state.config();

            if cfg.enabled && !pause_writing {
                // 后台循环走节流版本：目标表没变就不重复写 Gamma 表，
                // 曲线的缓慢过渡也压到 20 秒一次，屏幕上看不出台阶
                apply_tick(&state, &cfg);
            }

            let conflict = gamma_conflict_id(&state);
            if conflict.is_some() && !conflict_seen {
                emit_event(
                    &out,
                    "evt/gamma.conflict",
                    json!({ "display": conflict.clone(), "at": now }),
                );
            }
            conflict_seen = conflict.is_some();

            if cfg.break_plan.enabled && !away {
                if let Some(due) = cfg.break_plan.due_at {
                    if now >= due {
                        let rest = cfg.break_plan.rest_seconds.max(1) as u64;
                        let dim = cfg
                            .break_plan
                            .dim_seconds
                            .clamp(model::DIM_SECONDS_MIN, model::DIM_SECONDS_MAX)
                            as u64;

                        emit_event(
                            &out,
                            "evt/break.due",
                            json!({
                                "restSeconds": rest,
                                "dimSeconds": dim,
                                "dimScreen": cfg.break_plan.dim_screen,
                                "at": now,
                            }),
                        );

                        // 面板可能根本没人看着，所以提醒要落到屏幕上：
                        // 这一段时间的屏幕会比平时更暖更暗，想忽略都难。
                        // 时长只取 dim_seconds，跟休息时长解耦——压 20 秒太碍事。
                        if cfg.break_plan.dim_screen && cfg.enabled {
                            begin_rest(&state, dim);
                            apply_now(&state, &cfg);
                        }

                        if rule.real_rest && activity.idle_supported() {
                            *state.rest_pending.lock().unwrap() = Some(RestPending {
                                until: now + rest,
                                required_secs: rest,
                            });
                        } else {
                            state.stats.lock().unwrap().took_break();
                        }

                        let mut next = cfg.clone();
                        next.break_plan.due_at =
                            Some(now + cfg.break_plan.work_minutes.max(1) as u64 * 60);
                        *state.config.lock().unwrap() = next;
                        state.persist();
                    }
                }
            }

            if ticks % STATS_FLUSH_TICKS == 0 {
                state.persist_stats();
                if away {
                    state.persist();
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

    // 收尾：统计落盘，再把屏幕还回去。
    // 宿主如果直接杀进程，这两步不会执行，屏幕会停在当前颜色上；
    // 下次启动时 `AppState::load` 会发现遗留的备份并先还原。
    state.persist_stats();
    let saved = state.saved.lock().unwrap().clone();
    restore_all(&saved);
    clear_backup();
    log_line(&root, "stdin closed, screen restored");
}

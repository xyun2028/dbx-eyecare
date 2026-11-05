//! 插件的数据模型：预设、时段规则、护眼计时，以及「现在该用什么颜色」的换算。

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

pub const PRESET_CUSTOM: &str = "custom";

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 一天中的分钟数，带小数，用于平滑过渡。
pub fn minutes_now() -> f64 {
    use chrono::Timelike;
    let now = chrono::Local::now();
    now.hour() as f64 * 60.0 + now.minute() as f64 + now.second() as f64 / 60.0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Preset {
    pub id: String,
    pub name: String,
    pub hint: String,
    pub kelvin: Option<u32>,
    pub color: Option<[u8; 3]>,
    pub swatch: String,
}

fn p(id: &str, name: &str, hint: &str, kelvin: Option<u32>, color: Option<[u8; 3]>, swatch: &str) -> Preset {
    Preset {
        id: id.to_string(),
        name: name.to_string(),
        hint: hint.to_string(),
        kelvin,
        color,
        swatch: swatch.to_string(),
    }
}

pub fn presets() -> Vec<Preset> {
    vec![
        p("off", "原色", "不做调整", Some(6500), None, "#d7e2f4"),
        p("soft", "轻度", "轻微偏暖", Some(5500), None, "#f5e3c8"),
        p("office", "办公", "长时间看代码", Some(4500), None, "#ffd096"),
        p("night", "夜间", "晚上值班", Some(3400), None, "#ffb173"),
        p("sleep", "睡前", "睡前半小时", Some(2700), None, "#ff8f4f"),
        p("green", "豆沙绿", "老式护眼绿", None, Some([199, 237, 204]), "#a9ddb7"),
        p("paper", "纸质暖黄", "护眼纸色", None, Some([250, 235, 210]), "#f0d29f"),
    ]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Schedule {
    pub enabled: bool,
    pub day_kelvin: u32,
    pub night_kelvin: u32,
    /// 白天开始时刻，距零点的分钟数
    pub day_start: u32,
    pub night_start: u32,
    /// 过渡时长，分钟。到点突变眼睛会不适，这里做线性过渡
    pub transition: u32,
}

impl Default for Schedule {
    fn default() -> Self {
        Self {
            enabled: false,
            day_kelvin: 5500,
            night_kelvin: 3000,
            day_start: 7 * 60,
            night_start: 19 * 60,
            transition: 60,
        }
    }
}

fn lerp(from: f64, to: f64, t: f64) -> f64 {
    from + (to - from) * t.clamp(0.0, 1.0)
}

impl Schedule {
    pub fn kelvin_at(&self, minutes: f64) -> u32 {
        let day_k = self.day_kelvin as f64;
        let night_k = self.night_kelvin as f64;
        let tr = self.transition.max(1) as f64;
        let day_s = self.day_start as f64;
        let night_s = self.night_start as f64;
        let up_end = day_s + tr;
        let down_start = night_s - tr;

        let value = if minutes >= up_end && minutes < down_start {
            day_k
        } else if minutes >= day_s && minutes < up_end {
            lerp(night_k, day_k, (minutes - day_s) / tr)
        } else if minutes >= down_start && minutes < night_s {
            lerp(day_k, night_k, (minutes - down_start) / tr)
        } else {
            night_k
        };

        value.round().clamp(1000.0, 6500.0) as u32
    }
}

/// 20-20-20：每工作 20 分钟，看 20 英尺外 20 秒。
/// 这里只管「什么时候该提醒」，不强制锁屏。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BreakPlan {
    pub enabled: bool,
    pub work_minutes: u32,
    pub rest_seconds: u32,
    /// 下次提醒的时间戳。None 表示计时暂停或还没开始
    #[serde(default)]
    pub due_at: Option<u64>,
}

impl Default for BreakPlan {
    fn default() -> Self {
        Self {
            enabled: true,
            work_minutes: 20,
            rest_seconds: 20,
            due_at: None,
        }
    }
}

impl BreakPlan {
    /// 确保计时已经启动。开启开关或改完时长后调用。
    pub fn arm(&mut self) {
        if self.enabled && self.due_at.is_none() {
            self.due_at = Some(now_secs() + self.work_minutes.max(1) as u64 * 60);
        }
    }

    pub fn disarm(&mut self) {
        self.due_at = None;
    }

    pub fn remaining(&self) -> Option<u64> {
        self.due_at.map(|due| due.saturating_sub(now_secs()))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub enabled: bool,
    pub preset: String,
    pub kelvin: u32,
    pub brightness: f64,
    pub color: Option<[u8; 3]>,
    /// "all" 或具体显示器 id
    pub target: String,
    pub schedule: Schedule,
    #[serde(default)]
    pub break_plan: BreakPlan,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: true,
            preset: "office".to_string(),
            kelvin: 4500,
            brightness: 1.0,
            color: None,
            target: "all".to_string(),
            schedule: Schedule::default(),
            break_plan: BreakPlan::default(),
        }
    }
}

/// 最终要下发给屏幕的东西。
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Tint {
    Kelvin { kelvin: u32 },
    Rgb { rgb: [u8; 3] },
}

/// 综合开关 / 自动 / 预设 / 自定义，算出此刻该用什么。None 表示应恢复原色。
pub fn resolve(cfg: &Config, minutes: f64) -> Option<Tint> {
    if !cfg.enabled {
        return None;
    }

    if cfg.schedule.enabled {
        return Some(Tint::Kelvin {
            kelvin: cfg.schedule.kelvin_at(minutes),
        });
    }

    if cfg.preset == PRESET_CUSTOM {
        return Some(match cfg.color {
            Some(rgb) => Tint::Rgb { rgb },
            None => Tint::Kelvin { kelvin: cfg.kelvin },
        });
    }

    if let Some(item) = presets().into_iter().find(|item| item.id == cfg.preset) {
        if let Some(rgb) = item.color {
            return Some(Tint::Rgb { rgb });
        }
        if let Some(kelvin) = item.kelvin {
            return Some(Tint::Kelvin { kelvin });
        }
    }

    Some(Tint::Kelvin { kelvin: cfg.kelvin })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 白天和深夜取到两端值() {
        let s = Schedule::default();
        assert_eq!(s.kelvin_at(12.0 * 60.0), 5500);
        assert_eq!(s.kelvin_at(23.0 * 60.0), 3000);
    }

    #[test]
    fn 过渡区间落在两端之间() {
        let s = Schedule::default();
        let mid = s.kelvin_at(7.5 * 60.0);
        assert!(mid > 3000 && mid < 5500, "实际 {mid}");
    }

    #[test]
    fn 关闭时不调整() {
        let mut cfg = Config::default();
        cfg.enabled = false;
        assert!(resolve(&cfg, 12.0 * 60.0).is_none());
    }

    #[test]
    fn 自动模式优先于预设() {
        let mut cfg = Config::default();
        cfg.preset = "night".to_string();
        cfg.schedule.enabled = true;
        match resolve(&cfg, 12.0 * 60.0) {
            Some(Tint::Kelvin { kelvin }) => assert_eq!(kelvin, 5500),
            other => panic!("应为白天色温，实际 {other:?}"),
        }
    }

    #[test]
    fn 计时器按工作分钟数设定到期时间() {
        let mut plan = BreakPlan::default();
        plan.arm();
        let remaining = plan.remaining().unwrap();
        assert!(remaining > 19 * 60 && remaining <= 20 * 60, "实际 {remaining}");
        plan.disarm();
        assert!(plan.remaining().is_none());
    }
}

//! 用眼建议：从按钟点累计的记录里找出「最容易一口气看很久」的时段。
//!
//! 这里只**生成建议**，不改配置。真正写入锚点要等界面上点「应用」，
//! 而且整个分析功能由 `Config.insight.enabled` 开关控制，默认关。
//!
//! 数据只来自本地 stats.json：每天的 24 个钟点槽，没有窗口标题、没有应用名、
//! 没有按键内容。升级前的记录没有钟点槽，会自动等攒够新数据再给结论。

use serde::Serialize;

use crate::model::{Anchor, Config, Moment, KELVIN_MAX, KELVIN_MIN, MAX_ANCHORS};
use crate::stats::Stats;

/// 至少要有这么多天带钟点数据，结论才不像是巧合。
const MIN_DAYS: usize = 3;
/// 最多回看这么多天。
const LOOKBACK_DAYS: usize = 14;
/// 建议比当前时段再暖这么多 K。
const WARMER_BY: u32 = 500;
/// 已经存在的锚点离建议时刻这么近，就不再重复加。
const NEAR_MINUTES: i32 = 20;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Insight {
    /// 一句人话结论
    pub summary: String,
    /// 建议锚点时刻（从零点算起的分钟）
    pub at: u32,
    /// 建议色温
    pub kelvin: u32,
    /// 能不能点「应用」
    pub can_apply: bool,
    /// 这次分析用了几天的记录
    pub days: usize,
}

fn hhmm(minutes: u32) -> String {
    let m = minutes % 1440;
    format!("{:02}:{:02}", m / 60, m % 60)
}

pub fn analyze(stats: &Stats, cfg: &Config, moment: &Moment) -> Insight {
    let rows: Vec<_> = stats
        .days()
        .iter()
        .rev()
        .filter(|(_, day)| day.hours.len() == 24 && day.hours.iter().any(|v| *v > 0))
        .take(LOOKBACK_DAYS)
        .collect();

    let empty = Insight {
        summary: String::new(),
        at: 0,
        kelvin: cfg.kelvin,
        can_apply: false,
        days: rows.len(),
    };

    if rows.len() < MIN_DAYS {
        return Insight {
            summary: format!(
                "再攒 {} 天带时段的用眼记录，就能看出你在哪个时段最容易连着看很久。",
                MIN_DAYS.saturating_sub(rows.len())
            ),
            ..empty
        };
    }

    // 把每天的钟点槽加总，再看哪一段连续两小时最密
    let mut totals = [0u64; 24];
    for (_, day) in &rows {
        for (h, value) in day.hours.iter().take(24).enumerate() {
            totals[h] += *value as u64;
        }
    }

    let mut best_hour = 0usize;
    let mut best_sum = 0u64;
    for h in 0..24 {
        let sum = totals[h] + totals[(h + 1) % 24];
        if sum > best_sum {
            best_sum = sum;
            best_hour = h;
        }
    }

    let days = rows.len();
    let minutes_per_hour = (best_sum as f64 / (days as f64 * 2.0 * 60.0)).round() as u32;
    let at = ((best_hour as u32) * 60 + 30) % 1440;

    let current = if cfg.schedule.enabled {
        cfg.schedule
            .kelvin_at(&Moment {
                minutes: at as f64,
                ..*moment
            })
            .round() as u32
    } else {
        cfg.kelvin
    };
    let kelvin = current
        .saturating_sub(WARMER_BY)
        .clamp(KELVIN_MIN, KELVIN_MAX);

    let near = cfg.schedule.effective_points(moment).iter().any(|p| {
        let d = p.at as i32 - at as i32;
        d.abs() <= NEAR_MINUTES || d.abs() >= 1440 - NEAR_MINUTES
    });
    let full = cfg.schedule.points.len() >= MAX_ANCHORS;
    let can_apply = !near && !full && cfg.schedule.mode != crate::model::ScheduleMode::Sun;

    let reason = if cfg.schedule.mode == crate::model::ScheduleMode::Sun {
        "日出日落模式的锚点由太阳算出来，不能手动加。"
    } else if near {
        "这个时段已经有锚点，直接拖动它改色温就行。"
    } else if full {
        "曲线上的锚点已经到上限了，先删一个再加。"
    } else {
        "可以在这里加一个更暖的锚点。"
    };

    Insight {
        summary: format!(
            "最近 {} 天，{}–{} 平均每小时看了 {} 分钟，是一天里最密的一段。{}",
            days,
            hhmm(best_hour as u32 * 60),
            hhmm(((best_hour as u32 + 2) * 60) % 1440),
            minutes_per_hour,
            reason
        ),
        at,
        kelvin,
        can_apply,
        days,
    }
}

/// 把建议写进工作日曲线。同一时刻附近已有锚点时改成更新它，避免叠出重复点。
pub fn apply_to(cfg: &mut Config, at: u32, kelvin: u32) -> bool {
    if cfg.schedule.mode == crate::model::ScheduleMode::Sun {
        return false;
    }

    let at = at.min(1439);
    let kelvin = kelvin.clamp(KELVIN_MIN, KELVIN_MAX);

    if let Some(existing) = cfg
        .schedule
        .points
        .iter_mut()
        .find(|p| (p.at as i32 - at as i32).abs() <= NEAR_MINUTES)
    {
        existing.kelvin = kelvin;
    } else {
        if cfg.schedule.points.len() >= MAX_ANCHORS {
            return false;
        }
        cfg.schedule.points.push(Anchor { at, kelvin });
    }

    cfg.schedule.normalize();
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::DayEntry;

    fn stats_with(days: usize, busy_hour: usize) -> Stats {
        let mut stats = Stats::default();
        for i in 0..days {
            let mut hours = vec![0u32; 24];
            hours[busy_hour] = 3000;
            hours[(busy_hour + 1) % 24] = 2400;
            stats.days.insert(
                format!("2026-10-{:02}", i + 1),
                DayEntry {
                    seconds: 5400,
                    breaks: 2,
                    longest_streak_seconds: 3600,
                    hours,
                },
            );
        }
        stats
    }

    #[test]
    fn 数据不够时不给建议() {
        let cfg = Config::default();
        let insight = analyze(&stats_with(2, 21), &cfg, &Moment::weekday_at(12.0 * 60.0));
        assert!(!insight.can_apply);
        assert!(insight.summary.contains("再攒"));
    }

    #[test]
    fn 找到最密的时段并给出更暖的锚点() {
        let cfg = Config::default();
        let insight = analyze(&stats_with(5, 21), &cfg, &Moment::weekday_at(12.0 * 60.0));
        assert!(insight.can_apply);
        assert_eq!(insight.at, 21 * 60 + 30);
        assert_eq!(insight.days, 5);
        assert!(insight.kelvin < cfg.kelvin);
    }

    #[test]
    fn 建议可以写进曲线且不叠重复点() {
        let mut cfg = Config::default();
        let at = 15 * 60 + 30;
        assert!(apply_to(&mut cfg, at, 3600));
        assert!(cfg.schedule.points.iter().any(|p| p.at == at && p.kelvin == 3600));

        let count = cfg.schedule.points.len();
        assert!(apply_to(&mut cfg, at + 5, 3400));
        assert_eq!(cfg.schedule.points.len(), count, "附近的点应被更新而不是新增");
    }

    #[test]
    fn 到上限时拒绝再加() {
        let mut cfg = Config::default();
        cfg.schedule.points = (0..MAX_ANCHORS)
            .map(|i| Anchor { at: i as u32 * 60, kelvin: 4000 })
            .collect();
        assert!(!apply_to(&mut cfg, 23 * 60, 3000));
    }
}

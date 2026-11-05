//! 用眼记录。
//!
//! 只记三件事：今天一共亮了多久、休息了几次、这一轮连续看了多久。
//! 目的是让用户回头能看到「今天确实盯了七个多小时」，而不是做一块数据看板。
//!
//! 一条原则：间隔明显超常（电脑睡眠、合盖）的时间不算用眼。
//! 否则早上打开面板会看到昨天累计的十来个小时。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 相邻两次心跳超过这个秒数，就认为中间没在用电脑，整段跳过。
const MAX_GAP_SECS: u64 = 120;

/// 只保留最近这么多天。
const KEEP_DAYS: usize = 30;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DayEntry {
    /// 累计亮屏秒数（插件在调整状态下的时长）
    #[serde(default)]
    pub seconds: u64,
    #[serde(default)]
    pub breaks: u32,
    /// 这一天里最长的一段连续用眼
    #[serde(default)]
    pub longest_streak_seconds: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Stats {
    #[serde(default)]
    pub days: BTreeMap<String, DayEntry>,
    /// 当前这一轮已经连续看了多久，休息后清零
    #[serde(default)]
    pub streak_seconds: u64,
    /// 上次心跳的时间戳，用来算增量
    #[serde(default)]
    pub last_tick: u64,
}

/// 给界面的视图。`week` 固定七行，缺的那天补零，画柱状图时不会缺格。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatsView {
    pub today: DayEntry,
    pub streak_seconds: u64,
    pub week: Vec<WeekRow>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WeekRow {
    pub date: String,
    pub seconds: u64,
    pub breaks: u32,
}

fn today_key() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

fn stats_path(root: &Path) -> PathBuf {
    root.join("stats.json")
}

impl Stats {
    pub fn load(root: &Path) -> Self {
        let mut stats: Stats = fs::read_to_string(stats_path(root))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        stats.prune();
        stats
    }

    pub fn save(&self, root: &Path) {
        let _ = fs::create_dir_all(root);
        if let Ok(text) = serde_json::to_string_pretty(self) {
            let _ = fs::write(stats_path(root), text);
        }
    }

    /// 心跳。每几秒调一次，累加两次之间的时长。
    ///
    /// `active` 为 false（护眼关着）时不累加，但要把 `last_tick` 推上去，
    /// 否则重新打开时会一口气把关掉的那段时间也算进来。
    pub fn tick(&mut self, now: u64, active: bool) {
        if self.last_tick == 0 {
            self.last_tick = now;
            return;
        }

        let gap = now.saturating_sub(self.last_tick);
        self.last_tick = now;

        // 中间断了一大段（睡眠、休眠、插件被挂起），不补算，连续记录也断开
        if gap > MAX_GAP_SECS {
            self.streak_seconds = 0;
            return;
        }
        if !active {
            return;
        }

        let entry = self.days.entry(today_key()).or_default();
        entry.seconds += gap;
        self.streak_seconds += gap;
        if self.streak_seconds > entry.longest_streak_seconds {
            entry.longest_streak_seconds = self.streak_seconds;
        }
    }

    /// 记一次休息。连续用眼从这一刻重新算。
    pub fn took_break(&mut self) {
        let entry = self.days.entry(today_key()).or_default();
        entry.breaks += 1;
        self.streak_seconds = 0;
    }

    pub fn view(&self) -> StatsView {
        let today_key = today_key();
        let today = self.days.get(&today_key).cloned().unwrap_or_default();

        let mut week = Vec::with_capacity(7);
        let now = chrono::Local::now();
        for back in (0..7).rev() {
            let day = now - chrono::Duration::days(back as i64);
            let key = day.format("%Y-%m-%d").to_string();
            let entry = self.days.get(&key);
            week.push(WeekRow {
                date: key,
                seconds: entry.map(|e| e.seconds).unwrap_or(0),
                breaks: entry.map(|e| e.breaks).unwrap_or(0),
            });
        }

        StatsView {
            today,
            streak_seconds: self.streak_seconds,
            week,
        }
    }

    /// 丢掉太老的记录，避免文件越滚越大。
    fn prune(&mut self) {
        if self.days.len() <= KEEP_DAYS {
            return;
        }
        let cutoff = (chrono::Local::now() - chrono::Duration::days(KEEP_DAYS as i64))
            .format("%Y-%m-%d")
            .to_string();
        self.days.retain(|key, _| key.as_str() >= cutoff.as_str());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(stats: &Stats) -> DayEntry {
        stats.days.get(&today_key()).cloned().unwrap_or_default()
    }

    #[test]
    fn 两次心跳之间会被记进今天() {
        let mut stats = Stats::default();
        stats.tick(1_000, true);
        stats.tick(1_005, true);
        assert_eq!(entry(&stats).seconds, 5);
        assert_eq!(stats.streak_seconds, 5);
    }

    #[test]
    fn 关掉护眼时不计时() {
        let mut stats = Stats::default();
        stats.tick(1_000, true);
        stats.tick(1_005, false);
        assert_eq!(entry(&stats).seconds, 0);
    }

    #[test]
    fn 睡眠留下的长间隔不补算() {
        let mut stats = Stats::default();
        stats.tick(1_000, true);
        stats.tick(1_005, true);
        stats.streak_seconds = 600;

        // 合盖三个小时
        stats.tick(1_005 + 3 * 3600, true);
        assert_eq!(entry(&stats).seconds, 5, "整段睡眠不该算成用眼");
        assert_eq!(stats.streak_seconds, 0, "回来之后连续记录要断掉");
    }

    #[test]
    fn 休息后连续用眼清零但总数保留() {
        let mut stats = Stats::default();
        stats.tick(1_000, true);
        stats.tick(1_020, true);
        stats.took_break();

        assert_eq!(stats.streak_seconds, 0);
        let today = entry(&stats);
        assert_eq!(today.breaks, 1);
        assert_eq!(today.seconds, 20);
    }

    #[test]
    fn 最长连续用眼取峰值() {
        let mut stats = Stats::default();
        stats.tick(1_000, true);
        stats.tick(1_100, true);
        stats.took_break();
        stats.tick(1_100, true);
        stats.tick(1_120, true);

        assert_eq!(entry(&stats).longest_streak_seconds, 100);
        assert_eq!(stats.streak_seconds, 20);
    }

    #[test]
    fn 周视图固定七行() {
        let stats = Stats::default();
        let view = stats.view();
        assert_eq!(view.week.len(), 7);
        assert_eq!(view.week[6].date, today_key());
    }
}

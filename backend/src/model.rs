//! 插件的数据模型：预设、24 小时色温曲线、护眼计时，以及「现在该用什么颜色」的换算。
//!
//! 0.2 之前只有「白天一档、夜间一档」。现在改成一条由多个锚点组成的曲线：
//! 每个锚点是一个钟点加一个色温，锚点之间保持前一个色温，临近下一个锚点时才
//! 用 `transition` 分钟做线性过渡。这样既能表达「上班 5200K、深夜 3000K」这种
//! 多段变化，又不会让整条曲线一直在渐变、屏幕看着一直变。
//!
//! 老配置里没有 `points`，`normalize` 会用旧的 day/night 两档补出来，
//! 所以升级不会丢掉用户原来的设置。

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::color::kelvin_to_rgb;
use crate::sun;

pub const PRESET_CUSTOM: &str = "custom";
/// 自定义预设的 id 前缀。内置预设不用这个前缀，两者不会撞。
pub const USER_PREFIX: &str = "user:";

/// 色温允许的范围，界面滑杆和曲线都按这个取值。
pub const KELVIN_MIN: u32 = 1000;
pub const KELVIN_MAX: u32 = 6500;

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/* ------------------------------------------------------------------ 当前时刻 */

/// 换算色温时需要的「现在的样子」。
///
/// 单独拆出来是因为它要同时回答三个问题：现在几点、今天是不是周末、
/// 今天是今年第几天（日出日落要用）。直接读系统时间会让单测没法固定时间。
#[derive(Debug, Clone, Copy)]
pub struct Moment {
    /// 一天中的分钟数，带小数
    pub minutes: f64,
    pub is_weekend: bool,
    pub day_of_year: u32,
    /// 当地时区相对 UTC 的分钟数，北京时间是 480
    pub utc_offset_min: i32,
}

impl Moment {
    pub fn now() -> Self {
        use chrono::{Datelike, Timelike};
        let now = chrono::Local::now();
        Self {
            minutes: now.hour() as f64 * 60.0 + now.minute() as f64 + now.second() as f64 / 60.0,
            // num_days_from_monday 里周六是 5、周日是 6
            is_weekend: now.weekday().num_days_from_monday() >= 5,
            day_of_year: now.ordinal(),
            utc_offset_min: now.offset().local_minus_utc() / 60,
        }
    }

    /// 测试用：固定成一个工作日的某个时刻，时区按北京时间。
    #[cfg(test)]
    pub fn weekday_at(minutes: f64) -> Self {
        Self {
            minutes,
            is_weekend: false,
            day_of_year: 172,
            utc_offset_min: 480,
        }
    }

    #[cfg(test)]
    pub fn weekend_at(minutes: f64) -> Self {
        Self {
            is_weekend: true,
            ..Self::weekday_at(minutes)
        }
    }

    #[cfg(test)]
    pub fn on_day(mut self, day_of_year: u32) -> Self {
        self.day_of_year = day_of_year;
        self
    }
}

/* ---------------------------------------------------------------------- 预设 */

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Preset {
    pub id: String,
    pub name: String,
    pub hint: String,
    pub kelvin: Option<u32>,
    pub color: Option<[u8; 3]>,
    pub swatch: String,
    /// true 表示用户自己存的，界面上可以改名和删除
    #[serde(default)]
    pub custom: bool,
}

fn p(id: &str, name: &str, hint: &str, kelvin: Option<u32>, color: Option<[u8; 3]>) -> Preset {
    Preset {
        id: id.to_string(),
        name: name.to_string(),
        hint: hint.to_string(),
        kelvin,
        color,
        swatch: swatch_for(kelvin, color, 1.0),
        custom: false,
    }
}

/// 内置预设。顺序就是界面上的顺序。
pub fn builtin_presets() -> Vec<Preset> {
    vec![
        p("off", "原色", "不做调整", Some(6500), None),
        p("soft", "轻度", "轻微偏暖", Some(5500), None),
        p("office", "办公", "长时间看代码", Some(4500), None),
        p("night", "夜间", "晚上值班", Some(3400), None),
        p("sleep", "睡前", "睡前半小时", Some(2700), None),
        p("green", "豆沙绿", "老式护眼绿", None, Some([199, 237, 204])),
        p("paper", "纸质暖黄", "护眼纸色", None, Some([250, 235, 210])),
    ]
}

/// 色块颜色。色温预设按黑体辐射色算，滤色预设直接用用户的颜色。
///
/// 乘 0.92 是因为满亮度的白色在深色界面上几乎看不出色差，压一点更容易分辨。
fn swatch_for(kelvin: Option<u32>, color: Option<[u8; 3]>, factor: f64) -> String {
    let (r, g, b) = match color {
        Some(rgb) => (
            rgb[0] as f64 / 255.0,
            rgb[1] as f64 / 255.0,
            rgb[2] as f64 / 255.0,
        ),
        None => kelvin_to_rgb(kelvin.unwrap_or(6500)),
    };
    let to = |v: f64| (v * 255.0 * factor * 0.92).round().clamp(0.0, 255.0) as u8;
    format!("#{:02x}{:02x}{:02x}", to(r), to(g), to(b))
}

/// 用户存下来的预设。比内置的多一个亮度，因为亮度和色温经常是一起调的。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomPreset {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub kelvin: Option<u32>,
    #[serde(default)]
    pub color: Option<[u8; 3]>,
    #[serde(default = "default_brightness")]
    pub brightness: f64,
}

impl CustomPreset {
    pub fn to_preset(&self) -> Preset {
        Preset {
            id: self.id.clone(),
            name: self.name.clone(),
            hint: "自定义".to_string(),
            kelvin: self.kelvin,
            color: self.color,
            swatch: swatch_for(self.kelvin, self.color, self.brightness.min(1.0)),
            custom: true,
        }
    }
}

pub fn new_custom_id(existing: &[CustomPreset]) -> String {
    let mut seed = now_secs();
    loop {
        let id = format!("{USER_PREFIX}{seed}");
        if !existing.iter().any(|p| p.id == id) {
            return id;
        }
        seed += 1;
    }
}

/// 内置 + 自定义，界面直接拿这个列表渲染。
pub fn all_presets(custom: &[CustomPreset]) -> Vec<Preset> {
    let mut out = builtin_presets();
    out.extend(custom.iter().map(CustomPreset::to_preset));
    out
}

fn default_brightness() -> f64 {
    1.0
}

/* ------------------------------------------------------------------ 曲线锚点 */

/// 曲线上的一个点：从零点算起的分钟数 + 那一刻的目标色温。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Anchor {
    pub at: u32,
    pub kelvin: u32,
}

/// 按钟点算，还是跟着当地的日出日落走。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ScheduleMode {
    #[default]
    Clock,
    Sun,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Schedule {
    pub enabled: bool,
    #[serde(default)]
    pub mode: ScheduleMode,
    /// 白天色温。`mode = sun` 时用这一档当白天的目标值。
    pub day_kelvin: u32,
    /// 夜间色温。`mode = sun` 时用这一档当夜间的目标值。
    pub night_kelvin: u32,
    /// 白天开始时刻，距零点的分钟数。只在迁移旧配置时用到。
    pub day_start: u32,
    /// 夜间开始时刻。只在迁移旧配置时用到。
    pub night_start: u32,
    /// 过渡时长，分钟。到点突变眼睛会不适，这里做线性过渡。
    pub transition: u32,
    /// 工作日曲线，至少两个点。
    #[serde(default)]
    pub points: Vec<Anchor>,
    /// 周末曲线。空表示跟工作日一样。
    #[serde(default)]
    pub weekend_points: Vec<Anchor>,
    /// 观察点纬度，北纬为正。默认深圳。
    #[serde(default = "default_lat")]
    pub lat: f64,
    /// 观察点经度，东经为正。
    #[serde(default = "default_lon")]
    pub lon: f64,
    /// 日出日落前后各留多少分钟做过渡。
    #[serde(default = "default_sun_margin")]
    pub sun_margin: u32,
}

fn default_lat() -> f64 {
    22.55
}
fn default_lon() -> f64 {
    114.06
}
fn default_sun_margin() -> u32 {
    40
}

impl Default for Schedule {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: ScheduleMode::Clock,
            day_kelvin: 5500,
            night_kelvin: 3000,
            day_start: 7 * 60,
            night_start: 19 * 60,
            transition: 60,
            // 新装用户直接给一条五段的曲线，比两档更能说明「这东西可以细调」
            points: vec![
                Anchor { at: 7 * 60, kelvin: 5500 },
                Anchor { at: 9 * 60, kelvin: 5200 },
                Anchor { at: 14 * 60, kelvin: 5000 },
                Anchor { at: 18 * 60 + 30, kelvin: 4000 },
                Anchor { at: 22 * 60 + 30, kelvin: 3200 },
            ],
            weekend_points: Vec::new(),
            lat: default_lat(),
            lon: default_lon(),
            sun_margin: default_sun_margin(),
        }
    }
}

impl Schedule {
    /// 把老配置补齐成新形状，并顺手把锚点排好序。
    ///
    /// 读一次配置就调用一次，之后所有换算都只认 `points`，
    /// 不再出现「两个字段说两套话」的情况。
    pub fn normalize(&mut self) {
        if self.points.is_empty() {
            self.points = vec![
                Anchor {
                    at: self.day_start,
                    kelvin: self.day_kelvin,
                },
                Anchor {
                    at: self.night_start,
                    kelvin: self.night_kelvin,
                },
            ];
        }

        sort_points(&mut self.points);
        sort_points(&mut self.weekend_points);

        // 时间点重复会让插值算出 0 长度的区间，去重时留第一个
        self.points.dedup_by_key(|a| a.at);
        self.weekend_points.dedup_by_key(|a| a.at);

        self.lat = self.lat.clamp(-90.0, 90.0);
        self.lon = self.lon.clamp(-180.0, 180.0);
        self.sun_margin = self.sun_margin.min(180);
        self.transition = self.transition.min(180);
    }

    /// 这一刻该用多少色温。
    pub fn kelvin_at(&self, moment: &Moment) -> f64 {
        let points = self.points_for(moment);
        sample(&points, moment.minutes, self.transition)
    }

    fn points_for(&self, moment: &Moment) -> Vec<Anchor> {
        match self.mode {
            ScheduleMode::Sun => self.sun_points(moment),
            ScheduleMode::Clock => {
                if moment.is_weekend && !self.weekend_points.is_empty() {
                    self.weekend_points.clone()
                } else {
                    self.points.clone()
                }
            }
        }
    }

    /// 日出前后暖起来、日落前后暖下去，四个点交给统一的插值处理。
    fn sun_points(&self, moment: &Moment) -> Vec<Anchor> {
        let (sunrise, sunset) = match sun::sun_times(
            moment.day_of_year,
            self.lat,
            self.lon,
            moment.utc_offset_min,
        ) {
            Some(t) => (t.sunrise, t.sunset),
            // 极昼极夜没有日出日落，退回一个保守的固定时刻，
            // 总比让屏幕在一天里保持同一个颜色强
            None => (6.0 * 60.0, 18.0 * 60.0),
        };

        let margin = self.sun_margin as f64;
        let wrap = |m: f64| -> u32 {
            let v = ((m % 1440.0) + 1440.0) % 1440.0;
            (v.round() as u32) % 1440
        };

        vec![
            Anchor {
                at: wrap(sunrise - margin),
                kelvin: self.night_kelvin,
            },
            Anchor {
                at: wrap(sunrise + margin),
                kelvin: self.day_kelvin,
            },
            Anchor {
                at: wrap(sunset - margin),
                kelvin: self.day_kelvin,
            },
            Anchor {
                at: wrap(sunset + margin),
                kelvin: self.night_kelvin,
            },
        ]
    }

    /// 现在的曲线长什么样，给界面画图用。
    /// 日出模式返回的是今天算出来的四个点，不是配置里存的那条。
    pub fn effective_points(&self, moment: &Moment) -> Vec<Anchor> {
        let mut points = self.points_for(moment);
        sort_points(&mut points);
        points.dedup_by_key(|a| a.at);
        points
    }
}

fn sort_points(points: &mut [Anchor]) {
    points.sort_by_key(|a| a.at);
}

fn lerp(from: f64, to: f64, t: f64) -> f64 {
    from + (to - from) * t.clamp(0.0, 1.0)
}

/// 在锚点序列上取值。
///
/// 规则是「保持 + 过渡」而不是「一路线性」：从当前锚点开始一直保持它的色温，
/// 直到距离下一个锚点还剩 `transition` 分钟时才开始变。整条曲线一直在渐变的话，
/// 屏幕颜色一天里都在缓慢变化，反而让人分不清自己到底调了什么。
///
/// 锚点按一天 24 小时首尾相接，所以最后一个点到第一个点之间也是连续的一段。
fn sample(raw: &[Anchor], minutes: f64, transition: u32) -> f64 {
    let mut points = raw.to_vec();
    sort_points(&mut points);
    points.dedup_by_key(|a| a.at);

    if points.is_empty() {
        return KELVIN_MAX as f64;
    }
    if points.len() == 1 {
        return points[0].kelvin as f64;
    }

    let n = points.len();
    let m = ((minutes % 1440.0) + 1440.0) % 1440.0;

    // 找最后一个不晚于当前时刻的锚点；都比当前晚就落在最后一个点之后
    let mut idx = n - 1;
    for (i, point) in points.iter().enumerate() {
        if point.at as f64 <= m {
            idx = i;
        } else {
            break;
        }
    }

    let cur = points[idx];
    let next = points[(idx + 1) % n];

    // 跨零点时把「下一个」展开到第二天，这样区间长度始终是正的
    let cur_at = cur.at as f64;
    let next_at = if idx + 1 == n {
        next.at as f64 + 1440.0
    } else {
        next.at as f64
    };
    let into = if m >= cur_at {
        m - cur_at
    } else {
        m + 1440.0 - cur_at
    };

    let span = (next_at - cur_at).max(0.001);
    let tr = (transition as f64).min(span);

    if tr <= 0.0 || into < span - tr {
        return cur.kelvin as f64;
    }

    let t = (into - (span - tr)) / tr;
    lerp(cur.kelvin as f64, next.kelvin as f64, t)
}

/* ------------------------------------------------------------------ 护眼计时 */

/// 20-20-20：每工作 20 分钟，看 20 英尺外 20 秒。
/// 这里只管「什么时候该提醒」，不强制锁屏。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BreakPlan {
    pub enabled: bool,
    pub work_minutes: u32,
    pub rest_seconds: u32,
    /// 到点时把屏幕短暂调暖压暗，让提醒跑到屏幕上去，而不只是待在面板里
    #[serde(default = "default_true")]
    pub dim_screen: bool,
    /// 下次提醒的时间戳。None 表示计时暂停或还没开始
    #[serde(default)]
    pub due_at: Option<u64>,
}

fn default_true() -> bool {
    true
}

impl Default for BreakPlan {
    fn default() -> Self {
        Self {
            enabled: true,
            work_minutes: 20,
            rest_seconds: 20,
            dim_screen: true,
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

/* -------------------------------------------------------------------- 配置 */

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
    #[serde(default)]
    pub custom_presets: Vec<CustomPreset>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: true,
            // 首次装好时保持原色。一打开就把屏幕染暖，用户会以为是装坏了，
            // 也分不清到底是插件改的还是别的东西改的
            preset: "off".to_string(),
            kelvin: 6500,
            brightness: 1.0,
            color: None,
            target: "all".to_string(),
            schedule: Schedule::default(),
            break_plan: BreakPlan::default(),
            custom_presets: Vec::new(),
        }
    }
}

impl Config {
    pub fn normalize(&mut self) {
        self.schedule.normalize();
        self.kelvin = self.kelvin.clamp(KELVIN_MIN, KELVIN_MAX);
        self.brightness = self.brightness.clamp(0.1, 1.0);
    }

    pub fn find_custom(&self, id: &str) -> Option<&CustomPreset> {
        self.custom_presets.iter().find(|p| p.id == id)
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
pub fn resolve(cfg: &Config, moment: &Moment) -> Option<Tint> {
    if !cfg.enabled {
        return None;
    }

    // 自动模式优先：开了「按时间/日出日落」就不看预设
    if cfg.schedule.enabled {
        let kelvin = cfg
            .schedule
            .kelvin_at(moment)
            .round()
            .clamp(KELVIN_MIN as f64, KELVIN_MAX as f64) as u32;
        return Some(Tint::Kelvin { kelvin });
    }

    if cfg.preset == PRESET_CUSTOM {
        return Some(match cfg.color {
            Some(rgb) => Tint::Rgb { rgb },
            None => Tint::Kelvin { kelvin: cfg.kelvin },
        });
    }

    if let Some(user) = cfg.find_custom(&cfg.preset) {
        return Some(match user.color {
            Some(rgb) => Tint::Rgb { rgb },
            None => Tint::Kelvin {
                kelvin: user.kelvin.unwrap_or(cfg.kelvin),
            },
        });
    }

    if let Some(item) = builtin_presets().into_iter().find(|p| p.id == cfg.preset) {
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
    fn 老配置只有两档时补成两个锚点() {
        let mut schedule = Schedule::default();
        schedule.points.clear();
        schedule.day_start = 7 * 60;
        schedule.night_start = 19 * 60;
        schedule.day_kelvin = 5500;
        schedule.night_kelvin = 3000;

        schedule.normalize();

        assert_eq!(schedule.points.len(), 2);
        assert_eq!(schedule.points[0], Anchor { at: 420, kelvin: 5500 });
        assert_eq!(schedule.points[1], Anchor { at: 1140, kelvin: 3000 });
    }

    #[test]
    fn 白天和深夜取到两端值() {
        let s = Schedule::default();
        // 12:00 落在 9:00 那个锚点的平台上，保持 5200
        assert_eq!(s.kelvin_at(&Moment::weekday_at(12.0 * 60.0)), 5200.0);
        assert_eq!(s.kelvin_at(&Moment::weekday_at(23.5 * 60.0)), 3200.0);
    }

    #[test]
    fn 过渡只在临近下一个锚点前发生() {
        let s = Schedule::default();
        // 9:00 是 5200，下一个锚点 14:00，过渡 60 分钟
        assert_eq!(s.kelvin_at(&Moment::weekday_at(12.0 * 60.0)), 5200.0);
        assert_eq!(s.kelvin_at(&Moment::weekday_at(13.0 * 60.0)), 5200.0);
        let mid = s.kelvin_at(&Moment::weekday_at(13.5 * 60.0));
        assert!(mid > 5000.0 && mid < 5200.0, "过渡中途应在两档之间，实际 {mid}");
    }

    #[test]
    fn 跨零点不会断档() {
        let s = Schedule::default();
        // 22:30 是最后一个锚点，下一个是次日 7:00
        let late = s.kelvin_at(&Moment::weekday_at(23.0 * 60.0));
        let early = s.kelvin_at(&Moment::weekday_at(1.0 * 60.0));
        assert_eq!(late, 3200.0);
        assert_eq!(early, 3200.0);

        // 次日 6:30 已经进入 7:00 前的过渡带
        let dawn = s.kelvin_at(&Moment::weekday_at(6.5 * 60.0));
        assert!(dawn > 3200.0 && dawn < 5500.0, "实际 {dawn}");
    }

    #[test]
    fn 周末曲线只在周末生效() {
        let mut s = Schedule::default();
        s.weekend_points = vec![
            Anchor { at: 9 * 60, kelvin: 6000 },
            Anchor { at: 23 * 60, kelvin: 3800 },
        ];

        assert_eq!(s.kelvin_at(&Moment::weekday_at(12.0 * 60.0)), 5200.0);
        assert_eq!(s.kelvin_at(&Moment::weekend_at(12.0 * 60.0)), 6000.0);
    }

    #[test]
    fn 周末曲线为空时沿用工作日() {
        let s = Schedule::default();
        assert_eq!(s.kelvin_at(&Moment::weekday_at(12.0 * 60.0)), 5200.0);
        assert_eq!(s.kelvin_at(&Moment::weekend_at(12.0 * 60.0)), 5200.0);
    }

    #[test]
    fn 日出模式白天用白天的色温() {
        let mut s = Schedule::default();
        s.mode = ScheduleMode::Sun;
        s.day_kelvin = 5600;
        s.night_kelvin = 2900;

        let noon = Moment::weekday_at(12.0 * 60.0).on_day(172);
        assert_eq!(s.kelvin_at(&noon), 5600.0);

        let night = Moment::weekday_at(23.5 * 60.0).on_day(172);
        assert_eq!(s.kelvin_at(&night), 2900.0);
    }

    #[test]
    fn 日出模式给出四个锚点() {
        let mut s = Schedule::default();
        s.mode = ScheduleMode::Sun;
        let points = s.effective_points(&Moment::weekday_at(12.0 * 60.0).on_day(172));
        assert_eq!(points.len(), 4);
        assert_eq!(points[0].kelvin, s.night_kelvin);
        assert!(points[1].at < points[2].at, "日出应早于日落");
    }

    #[test]
    fn 关闭时不调整() {
        let mut cfg = Config::default();
        cfg.enabled = false;
        assert!(resolve(&cfg, &Moment::weekday_at(12.0 * 60.0)).is_none());
    }

    #[test]
    fn 自动模式优先于预设() {
        let mut cfg = Config::default();
        cfg.preset = "night".to_string();
        cfg.schedule.enabled = true;
        match resolve(&cfg, &Moment::weekday_at(12.0 * 60.0)) {
            Some(Tint::Kelvin { kelvin }) => assert_eq!(kelvin, 5200),
            other => panic!("应为白天色温，实际 {other:?}"),
        }
    }

    #[test]
    fn 自定义预设能被解析() {
        let mut cfg = Config::default();
        cfg.preset = "user:42".to_string();
        cfg.custom_presets.push(CustomPreset {
            id: "user:42".to_string(),
            name: "写代码".to_string(),
            kelvin: Some(4300),
            color: None,
            brightness: 0.8,
        });

        match resolve(&cfg, &Moment::weekday_at(12.0 * 60.0)) {
            Some(Tint::Kelvin { kelvin }) => assert_eq!(kelvin, 4300),
            other => panic!("实际 {other:?}"),
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

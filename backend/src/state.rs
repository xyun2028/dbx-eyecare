//! 插件状态、持久化，以及「把配置真正写到屏幕上」。

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::color::{build_gamma_ramp, build_gamma_ramp_rgb};
use crate::gamma::{self, Ramp};
use crate::model::{self, resolve_for, Config, Moment, ScreenConfig, Tint};
use crate::stats::Stats;

/// 旧格式：首行版本标记，之后每行 `屏名,768 个数字`。
const BACKUP_HEADER_V1: &str = "eyecare-dbx-v1";
/// 新格式：首行版本标记，之后一行 JSON。
/// 改成 JSON 是因为表长不再固定——Windows 是 256，X11 可能是 1024。
const BACKUP_HEADER_V2: &str = "eyecare-dbx-v2";

/// 休息提示时把色温压到当前的这个比例，亮度再乘下面那个系数。
/// 用相对值而不是固定色温，是因为用户本来就在 2700K 时，
/// 一个固定的 1900K 几乎看不出变化，压三成才有「屏幕变了」的感觉。
const REST_KELVIN_RATIO: f64 = 0.55;
const REST_BRIGHTNESS_RATIO: f64 = 0.7;
/// 滤色模式下没有色温可压，退回这个暖色。
const REST_FALLBACK_KELVIN: u32 = 1800;

/// 宿主注入的数据目录；没有宿主时退回临时目录，方便本地直接跑。
pub fn data_root() -> PathBuf {
    std::env::var("DBX_PLUGIN_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir().join("eyecare-dbx"))
}

fn config_path() -> PathBuf {
    data_root().join("config.json")
}

fn backup_path() -> PathBuf {
    data_root().join("original_ramp.txt")
}

/// 读回的表和上次自己写的表，逐点允许这么大的误差。
/// macOS 走 float、X11 会重采样，读回的值差一两个点是正常的；
/// 真正的「被别的软件改了」通常整条曲线都不一样，差值远大于这个数。
const RAMP_TOLERANCE: i32 = 160;

/// 同一块屏两次写表的最小间隔。后台循环 5 秒一拍，不节流的话一小时 720 次，
/// 一部分核显驱动会因此轻微抖动，日志也会跟着写爆。
///
/// 20 秒在曲线过渡期大约对应不到 10K 的色温变化，肉眼看不出台阶；
/// 平台期则完全不重复写。
const MIN_WRITE_SECS: u64 = 20;
/// 目标表一直没变，也隔这么久重写一次。兜住那些读回检查发现不了的漂移
/// （把 Gamma 表悄悄改回去又不触发整条曲线差异的情况）。
const IDLE_REWRITE_SECS: u64 = 300;

/// 一次休息要等确认，不能到点就记一笔。
#[derive(Debug, Clone, Copy)]
pub struct RestPending {
    /// 休息结束的时间戳
    pub until: u64,
    /// 要观察到至少这么长的连续空闲，才算真的停手
    pub required_secs: u64,
}

pub struct AppState {
    pub config: Mutex<Config>,
    /// 改屏之前拍下的原始表。退出或异常时需要它把屏幕还回去
    pub saved: Mutex<Vec<(String, Ramp)>>,
    /// 休息提示截止的时间戳。有值且还没到期时，写屏走「休息色」
    pub rest_until: Mutex<Option<u64>>,
    pub stats: Mutex<Stats>,
    /// 每块屏最近一次由我们写进去的表。用来发现别人动过 Gamma 表。
    pub last_written: Mutex<HashMap<String, Ramp>>,
    /// 检测到冲突后记下屏 id，写屏整轮暂停，等用户点「重新接管」。
    pub gamma_conflict: Mutex<Option<String>>,
    /// 休息结束前不急着记 breaks，等确认用户真的停手了再记。
    pub rest_pending: Mutex<Option<RestPending>>,
    /// 上次真正写屏的时刻，给后台循环的写屏节流用。
    pub last_apply_at: Mutex<u64>,
    /// 上次写屏时屏幕是不是在「休息压暗」状态。
    /// 这个状态只有几秒宽，切换必须立刻生效，不能被节流挡住。
    pub last_resting: Mutex<bool>,
}

impl AppState {
    pub fn load() -> Self {
        let mut cfg: Config = fs::read_to_string(config_path())
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();

        // 旧配置只有 day/night 两档，这里补成锚点；之后所有逻辑只认锚点
        cfg.normalize();

        // 计时开关开着就立刻开始算，否则界面会一直显示「未开始」
        cfg.break_plan.arm();

        let state = Self {
            config: Mutex::new(cfg),
            saved: Mutex::new(Vec::new()),
            rest_until: Mutex::new(None),
            stats: Mutex::new(Stats::load(&data_root())),
            last_written: Mutex::new(HashMap::new()),
            gamma_conflict: Mutex::new(None),
            rest_pending: Mutex::new(None),
            last_apply_at: Mutex::new(0),
            last_resting: Mutex::new(false),
        };

        // 上次如果不是干净退出，先把屏幕救回来
        let leftover = load_backup();
        if !leftover.is_empty() {
            restore_all(&leftover);
            clear_backup();
        }

        let saved = snapshot();
        let _ = save_backup(&saved);
        *state.saved.lock().unwrap() = saved;

        state
    }

    pub fn persist(&self) {
        let cfg = self.config.lock().unwrap().clone();
        if let Some(dir) = config_path().parent() {
            let _ = fs::create_dir_all(dir);
        }
        if let Ok(text) = serde_json::to_string_pretty(&cfg) {
            let _ = fs::write(config_path(), text);
        }
    }

    pub fn persist_stats(&self) {
        self.stats.lock().unwrap().save(&data_root());
    }

    pub fn config(&self) -> Config {
        self.config.lock().unwrap().clone()
    }
}

/// 开始一次休息提示。到点后由主循环调用。
pub fn begin_rest(state: &AppState, seconds: u64) {
    *state.rest_until.lock().unwrap() = Some(model::now_secs() + seconds.max(1));
}

/// 休息提示是否还在生效。过期的顺手清掉。
pub fn rest_active(state: &AppState) -> bool {
    let mut guard = state.rest_until.lock().unwrap();
    match *guard {
        Some(until) if model::now_secs() < until => true,
        Some(_) => {
            *guard = None;
            false
        }
        None => false,
    }
}

pub fn snapshot() -> Vec<(String, Ramp)> {
    gamma::list()
        .into_iter()
        .filter_map(|d| gamma::read_ramp(&d.id).map(|r| (d.id, r)))
        .collect()
}

pub fn restore_all(saved: &[(String, Ramp)]) {
    for (id, ramp) in saved {
        // 备份也可能本身就是偏的（上一轮遗留），统一走同一套判定
        let base = baseline_for(Some(ramp));
        let ok = gamma::write_ramp(id, &base);
        // 上次被强杀时的还原路径没有别的地方留痕，这里补一行，
        // 出问题时能从日志里看出到底有没有救回来
        crate::protocol::log_line(
            &data_root(),
            &format!("restore-backup {} ok={ok} peak={}", id, base.peak(0)),
        );
    }
}

#[derive(Serialize, Deserialize)]
struct BackupFile {
    displays: Vec<BackupEntry>,
}

#[derive(Serialize, Deserialize)]
struct BackupEntry {
    id: String,
    #[serde(flatten)]
    ramp: Ramp,
}

/// 把内存里的备份拼成文件内容。单独拎出来是为了能直接测往返，
/// 不用真的去写磁盘。
fn backup_text(saved: &[(String, Ramp)]) -> Option<String> {
    let file = BackupFile {
        displays: saved
            .iter()
            .map(|(id, ramp)| BackupEntry {
                id: id.clone(),
                ramp: ramp.clone(),
            })
            .collect(),
    };

    let body = serde_json::to_string(&file).ok()?;
    Some(format!("{BACKUP_HEADER_V2}\n{body}\n"))
}

fn save_backup(saved: &[(String, Ramp)]) -> std::io::Result<()> {
    let path = backup_path();
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }

    let text = backup_text(saved).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "备份序列化失败")
    })?;
    fs::write(path, text)
}

pub fn load_backup() -> Vec<(String, Ramp)> {
    match fs::read_to_string(backup_path()) {
        Ok(text) => parse_backup(&text),
        Err(_) => Vec::new(),
    }
}

fn parse_backup(text: &str) -> Vec<(String, Ramp)> {
    let mut lines = text.lines();
    let header = lines.next().unwrap_or_default().trim();

    match header {
        BACKUP_HEADER_V2 => {
            let body: String = lines.collect::<Vec<_>>().join("\n");
            let Ok(file) = serde_json::from_str::<BackupFile>(&body) else {
                return Vec::new();
            };
            file.displays
                .into_iter()
                .filter(|entry| !entry.id.is_empty() && entry.ramp.is_valid())
                .map(|entry| (entry.id, entry.ramp))
                .collect()
        }

        // 升级前留下的文件必须还能读，否则用户更新到多平台版本后，
        // 屏幕会一直停在上一轮写进去的偏色上。
        BACKUP_HEADER_V1 => {
            let mut out = Vec::new();
            for line in lines {
                let mut parts = line.split(',');
                let Some(id) = parts.next() else { continue };
                if id.is_empty() {
                    continue;
                }

                let mut flat = [0u16; 768];
                let mut count = 0;
                for (i, part) in parts.enumerate().take(768) {
                    match part.parse::<u16>() {
                        Ok(value) => {
                            flat[i] = value;
                            count += 1;
                        }
                        Err(_) => break,
                    }
                }

                if count == 768 {
                    if let Some(ramp) = Ramp::from_flat(&flat) {
                        out.push((id.to_string(), ramp));
                    }
                }
            }
            out
        }

        _ => Vec::new(),
    }
}

pub fn clear_backup() {
    let _ = fs::remove_file(backup_path());
}

/// 「关闭 / 恢复原色」时到底该写什么。
///
/// 直接用启动时抓到的表是不行的：如果那一刻屏幕已经被染过色（比如上一轮
/// 自己写的，或者别的护眼软件在跑），抓到的「原色」本身就是偏的，写回去
/// 等于没关——表现就是开关点了没反应。
///
/// 判据：真正的原始表或校色配置，三个通道的峰值都是 65535；被降色温或滤色
/// 压过的表，峰值一定低于 65535。所以峰值为满才敢沿用，否则写线性表。
fn baseline_for(saved: Option<&Ramp>) -> Ramp {
    match saved {
        Some(ramp) if ramp.is_unpolluted() => ramp.clone(),
        _ => Ramp::identity(256),
    }
}

/// 休息提示要把屏幕变成什么样。
///
/// 在原来的颜色上再往暖处压一档，而不是换成一个固定值：
/// 固定值在用户本来就很暖的时候看不出来，在用户很白的时候又太突兀。
fn rest_tint(tint: Tint, brightness: f64) -> (Tint, f64) {
    let kelvin = match tint {
        Tint::Kelvin { kelvin } => (kelvin as f64 * REST_KELVIN_RATIO).round() as u32,
        Tint::Rgb { .. } => REST_FALLBACK_KELVIN,
    };
    (
        Tint::Kelvin {
            kelvin: kelvin.clamp(model::KELVIN_MIN, model::KELVIN_MAX),
        },
        (brightness * REST_BRIGHTNESS_RATIO).clamp(0.1, 1.0),
    )
}

/// 读回的表和上次自己写的是不是同一条。
/// 只抽 16 个点比对，够发现「被别的软件整条改掉」，又不会被转换误差误伤。
fn ramps_close(actual: &Ramp, expected: &Ramp) -> bool {
    if !actual.is_valid() || !expected.is_valid() {
        return true;
    }
    let scaled = actual.resample(expected.len);
    let step = (scaled.len / 16).max(1);
    for ch in 0..3 {
        let a = scaled.channel(ch);
        let b = expected.channel(ch);
        for i in (0..scaled.len).step_by(step) {
            if (a[i] as i32 - b[i] as i32).abs() > RAMP_TOLERANCE {
                return false;
            }
        }
    }
    true
}

/// 别人（f.lux、Night Shift、显卡驱动、系统自身）动过 Gamma 表时，
/// 不默默写回去打架。暂停整轮写屏，把冲突交给界面说明。
fn detect_conflict(state: &AppState, displays: &[gamma::DisplayInfo]) -> bool {
    let written = state.last_written.lock().unwrap();
    for display in displays {
        // HDR 开启时系统本来就会绕过 GDI 表，读回不一致属于预期，不算冲突
        if display.hdr == Some(true) {
            continue;
        }
        let Some(expected) = written.get(&display.id) else {
            continue;
        };
        if let Some(actual) = gamma::read_ramp(&display.id) {
            if !ramps_close(&actual, expected) {
                *state.gamma_conflict.lock().unwrap() = Some(display.id.clone());
                crate::protocol::log_line(
                    &data_root(),
                    &format!("gamma conflict on {}，暂停写屏等用户接管", display.id),
                );
                return true;
            }
        }
    }
    false
}

pub fn gamma_conflict_id(state: &AppState) -> Option<String> {
    state.gamma_conflict.lock().unwrap().clone()
}

/// 「重新接管」：清掉冲突标记和基线，下一轮重新写一次。
pub fn resume_gamma(state: &AppState) {
    *state.gamma_conflict.lock().unwrap() = None;
    state.last_written.lock().unwrap().clear();
}

/// 休息结束后结算：只有这段时间确实没碰键鼠，才记一次「休息了」。
///
/// 判据用 `idle_secs`：它是从最后一次输入算起的连续秒数，休息期间只要
/// 动过鼠标就会被清零，结束时自然小于要求的时长。平台查不到空闲时
/// 退回旧行为，宁可多记一次，也不要让人少收到提醒。
pub fn settle_rest(state: &AppState, now: u64, idle_secs: Option<u64>, real_rest: bool) -> bool {
    let pending = {
        let guard = state.rest_pending.lock().unwrap();
        let Some(item) = *guard else { return false };
        if now < item.until {
            return false;
        }
        item
    };

    let confirmed = match (real_rest, idle_secs) {
        (true, Some(idle)) => idle >= pending.required_secs.saturating_mul(80) / 100,
        _ => true,
    };

    *state.rest_pending.lock().unwrap() = None;
    if confirmed {
        state.stats.lock().unwrap().took_break();
    }
    confirmed
}

/// 把当前配置写到屏幕上，立刻生效。
///
/// 用户改配置、启动首帧、休息压暗的开始与结束都走这条，不受后台节流影响。
/// 每块屏各算各的：没参与的、单独关掉的，都写回原始值。
pub fn apply_now(state: &AppState, cfg: &Config) {
    apply_inner(state, cfg, true);
}

/// 后台循环专用的写屏入口。
///
/// 和 `apply_now` 的唯一区别是「允许跳过」：目标表和上次写进去的一样、
/// 或者离上次写屏还不够久，就不写。以前每 5 秒无条件重设一次 LUT，
/// 曲线在平台期根本没变，屏幕却被反复改，日志也跟着写爆。
pub fn apply_tick(state: &AppState, cfg: &Config) {
    apply_inner(state, cfg, false);
}

/// 这一次要不要真的写屏。
///
/// - `force`：用户操作、休息状态切换，一律写
/// - 距上次写屏不到 `MIN_WRITE_SECS`：跳过，避免过渡期每拍都写
/// - 目标表和上次一致：只有隔了 `IDLE_REWRITE_SECS` 才保底重写
fn should_write(prev: Option<&Ramp>, next: &Ramp, since_last: u64, force: bool) -> bool {
    if force {
        return true;
    }
    if since_last < MIN_WRITE_SECS {
        return false;
    }
    if prev != Some(next) {
        return true;
    }
    since_last >= IDLE_REWRITE_SECS
}

fn apply_inner(state: &AppState, cfg: &Config, force: bool) {
    let displays = gamma::list();
    if displays.is_empty() {
        let support = gamma::capability();
        crate::protocol::log_line(
            &data_root(),
            &format!(
                "apply: 没有可用的屏幕 session={} reason={}",
                support.session,
                support.reason.unwrap_or_default()
            ),
        );
        return;
    }

    // 关闭护眼时必须能还原屏幕，不能被冲突标记挡住。
    // 顺手清掉基线，重新打开时当作全新一轮写屏。
    // 还原本身也不受节流限制：用户点了关闭，屏幕就该马上回来。
    let mut force = force;
    if !cfg.enabled {
        *state.gamma_conflict.lock().unwrap() = None;
        state.last_written.lock().unwrap().clear();
        force = true;
    } else if gamma_conflict_id(state).is_some() || detect_conflict(state, &displays) {
        return;
    }

    // 配置里的 target 可能指向另一台机器的屏，或者已经拔掉的屏。
    // 匹配不上就当全部处理，否则用户看到的是「开关是开的，但屏幕没变」。
    let target_all = cfg.target == "all" || !displays.iter().any(|d| d.id == cfg.target);
    if target_all && cfg.target != "all" {
        crate::protocol::log_line(
            &data_root(),
            &format!("target {} 不在当前显示器列表里，本次按全部屏幕处理", cfg.target),
        );
    }

    let now = model::now_secs();
    let moment = Moment::now();
    let resting = rest_active(state);

    // 压暗窗口只有几秒宽，开始和结束都必须越过节流立刻落到屏幕上，
    // 否则那条提醒会被节流整个吞掉，用户什么都看不到
    let rest_changed = {
        let mut guard = state.last_resting.lock().unwrap();
        let changed = *guard != resting;
        *guard = resting;
        changed
    };
    let force = force || rest_changed;

    let since_last = now.saturating_sub(*state.last_apply_at.lock().unwrap());

    let saved = state.saved.lock().unwrap();
    let mut written = state.last_written.lock().unwrap();
    let mut wrote_any = false;

    for display in &displays {
        let screen: ScreenConfig = cfg.screen_for(&display.id);
        let participating = target_all || cfg.target == display.id;

        let tint = if participating {
            resolve_for(cfg, &screen, &moment)
        } else {
            None
        };
        let mut brightness = cfg.brightness_for(&screen);
        let tint = tint.map(|base| {
            if resting {
                let (t, b) = rest_tint(base, brightness);
                brightness = b;
                t
            } else {
                base
            }
        });

        let flat: Vec<u16> = match tint {
            Some(Tint::Kelvin { kelvin }) => build_gamma_ramp(kelvin, brightness).to_vec(),
            Some(Tint::Rgb { rgb }) => build_gamma_ramp_rgb(rgb, brightness).to_vec(),
            None => baseline_for(
                saved
                    .iter()
                    .find(|(id, _)| id == &display.id)
                    .map(|(_, ramp)| ramp),
            )
            .to_flat(),
        };
        // 算法固定产出 256 个点，目标平台需要多长由 gamma 层去重采样
        let ramp = Ramp::from_flat(&flat).unwrap_or_else(|| Ramp::identity(256));

        // 目标没变就不写。这一条同时解决两件事：不再每 5 秒重设一次 LUT，
        // 也不会再每拍写一行日志——那行日志以前一天能滚出几万条。
        let needed = {
            let prev = written.get(&display.id);
            should_write(prev, &ramp, since_last, force)
        };
        if !needed {
            continue;
        }

        let ok = gamma::write_ramp(&display.id, &ramp);
        if ok {
            written.insert(display.id.clone(), ramp.clone());
            wrote_any = true;
        }
        crate::protocol::log_line(
            &data_root(),
            &format!(
                "apply {} tint={tint:?} ok={ok} resting={resting} peak={} len={} screen={:?}",
                display.id,
                ramp.peak(0),
                ramp.len,
                screen.mode
            ),
        );
    }

    drop(written);

    // 只有真的写了才推进节流时间。一直没写时 since_last 会继续增长，
    // 最终触发上面那条保底重写
    if wrote_any {
        *state.last_apply_at.lock().unwrap() = now;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 被染过色的备份不会被当成原色写回去() {
        let mut ramp = Ramp::identity(256);
        ramp.r[255] = 51143;
        ramp.g[255] = 60909;
        ramp.b[255] = 52428;

        let base = baseline_for(Some(&ramp));
        assert_eq!(base.peak(0), 65535, "峰值不满，应判定为污染并改写线性表");
        assert_eq!(base.peak(1), 65535);
        assert_eq!(base.peak(2), 65535);
    }

    #[test]
    fn 峰值为满的表原样沿用() {
        let ramp = Ramp::identity(256);
        assert_eq!(baseline_for(Some(&ramp)), ramp);
    }

    #[test]
    fn 没有备份时写线性表() {
        assert_eq!(baseline_for(None), Ramp::identity(256));
    }

    #[test]
    fn 能读回旧版备份() {
        let mut line = String::from(r"\\.\DISPLAY1");
        for i in 0..768 {
            line.push(',');
            line.push_str(&(i * 85 % 65536).to_string());
        }
        let text = format!("{BACKUP_HEADER_V1}\n{line}\n");

        let parsed = parse_backup(&text);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].0, r"\\.\DISPLAY1");
        assert_eq!(parsed[0].1.len, 256);
    }

    #[test]
    fn 新备份带着表长和三个通道() {
        let mut ramp = Ramp::identity(1024);
        ramp.b[0] = 7;
        let saved = vec![("eDP-1".to_string(), ramp)];

        let text = backup_text(&saved).expect("应该能序列化");
        let parsed = parse_backup(&text);

        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].0, "eDP-1");
        assert_eq!(parsed[0].1.len, 1024);
        assert_eq!(parsed[0].1.b[0], 7);
    }

    #[test]
    fn 备份写入后能原样读回() {
        let saved = vec![
            (r"\\.\DISPLAY1".to_string(), Ramp::identity(256)),
            ("DP-1".to_string(), Ramp::identity(1024).resample(512)),
        ];
        let text = backup_text(&saved).unwrap();
        let parsed = parse_backup(&text);

        assert_eq!(parsed, saved, "落盘再读回不应丢长度或通道");
    }

    #[test]
    fn 坏掉的备份不会当成有效数据() {
        let text = format!("{BACKUP_HEADER_V2}\n{{\"displays\":[{{\"id\":\"x\",\"len\":4,\"r\":[1],\"g\":[1],\"b\":[1]}}]}}");
        assert!(parse_backup(&text).is_empty());
    }

    #[test]
    fn 休息提示比当前颜色更暖更暗() {
        let (tint, brightness) = rest_tint(Tint::Kelvin { kelvin: 4500 }, 1.0);
        match tint {
            Tint::Kelvin { kelvin } => {
                assert_eq!(kelvin, 2475, "4500 压到五成半");
                assert!(kelvin < 4500);
            }
            other => panic!("实际 {other:?}"),
        }
        assert!(brightness < 0.8, "亮度应压下去，实际 {brightness}");
    }

    #[test]
    fn 休息提示不会低于最低色温() {
        let (tint, _) = rest_tint(Tint::Kelvin { kelvin: 1000 }, 1.0);
        match tint {
            Tint::Kelvin { kelvin } => assert_eq!(kelvin, model::KELVIN_MIN),
            other => panic!("实际 {other:?}"),
        }
    }

    #[test]
    fn 滤色模式下休息提示退回暖色() {
        let (tint, _) = rest_tint(Tint::Rgb { rgb: [199, 237, 204] }, 1.0);
        match tint {
            Tint::Kelvin { kelvin } => assert_eq!(kelvin, REST_FALLBACK_KELVIN),
            other => panic!("实际 {other:?}"),
        }
    }

    #[test]
    fn 转换带来的小误差不算抢桌() {
        let expected = Ramp::identity(256);
        let mut actual = expected.clone();
        actual.b[100] = actual.b[100].saturating_add(3);
        assert!(ramps_close(&actual, &expected));
    }

    #[test]
    fn 整条曲线被改过会被识别为冲突() {
        let expected = Ramp::identity(256);
        let mut actual = expected.clone();
        for v in actual.b.iter_mut() {
            *v = (*v as u32 / 2) as u16;
        }
        assert!(!ramps_close(&actual, &expected));
    }

    fn test_state() -> AppState {
        AppState {
            config: Mutex::new(Config::default()),
            saved: Mutex::new(Vec::new()),
            rest_until: Mutex::new(None),
            stats: Mutex::new(Stats::default()),
            last_written: Mutex::new(HashMap::new()),
            gamma_conflict: Mutex::new(None),
            rest_pending: Mutex::new(None),
            last_apply_at: Mutex::new(0),
            last_resting: Mutex::new(false),
        }
    }

    #[test]
    fn 目标表没变且不到保底时间就不写() {
        let ramp = Ramp::identity(256);
        assert!(!should_write(
            Some(&ramp),
            &ramp,
            IDLE_REWRITE_SECS - 1,
            false
        ));
        assert!(
            should_write(Some(&ramp), &ramp, IDLE_REWRITE_SECS, false),
            "隔够时间要保底重写一次"
        );
    }

    #[test]
    fn 目标表变了才写() {
        let old = Ramp::identity(256);
        let mut next = Ramp::identity(256);
        next.b[200] = next.b[200].saturating_sub(1000);

        assert!(should_write(Some(&old), &next, MIN_WRITE_SECS, false));
        assert!(
            !should_write(Some(&old), &next, MIN_WRITE_SECS - 1, false),
            "过渡期也要守住最小间隔，不然还是每拍都写"
        );
    }

    #[test]
    fn 用户操作不受节流限制() {
        let ramp = Ramp::identity(256);
        assert!(should_write(Some(&ramp), &ramp, 0, true));
    }

    #[test]
    fn 第一次写屏不会被判成重复() {
        let ramp = Ramp::identity(256);
        assert!(should_write(None, &ramp, MIN_WRITE_SECS, false));
    }

    #[test]
    fn 休息期间还在用电脑就不记这次休息() {
        let state = test_state();
        *state.rest_pending.lock().unwrap() = Some(RestPending { until: 100, required_secs: 20 });

        assert!(!settle_rest(&state, 90, Some(10), true), "没到结束时间不结算");
        assert!(!settle_rest(&state, 100, Some(2), true), "还在动键鼠");
        assert_eq!(state.stats.lock().unwrap().view().today.breaks, 0);
    }

    #[test]
    fn 确实停手了才记一次休息() {
        let state = test_state();
        *state.rest_pending.lock().unwrap() = Some(RestPending { until: 100, required_secs: 20 });

        assert!(settle_rest(&state, 100, Some(20), true));
        assert_eq!(state.stats.lock().unwrap().view().today.breaks, 1);
        assert!(!settle_rest(&state, 200, Some(30), true), "一次休息只记一笔");
    }

    #[test]
    fn 平台查不到空闲时按旧行为记休息() {
        let state = test_state();
        *state.rest_pending.lock().unwrap() = Some(RestPending { until: 100, required_secs: 20 });
        assert!(settle_rest(&state, 100, None, true));
        assert_eq!(state.stats.lock().unwrap().view().today.breaks, 1);
    }
}

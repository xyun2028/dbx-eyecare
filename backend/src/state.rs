//! 插件状态、持久化，以及「把配置真正写到屏幕上」。

use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::color::{build_gamma_ramp, build_gamma_ramp_rgb};
use crate::gamma::{self, Ramp};
use crate::model::{resolve, Config, Tint};

/// 旧格式：首行版本标记，之后每行 `屏名,768 个数字`。
const BACKUP_HEADER_V1: &str = "eyecare-dbx-v1";
/// 新格式：首行版本标记，之后一行 JSON。
/// 改成 JSON 是因为表长不再固定——Windows 是 256，X11 可能是 1024。
const BACKUP_HEADER_V2: &str = "eyecare-dbx-v2";

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

pub struct AppState {
    pub config: Mutex<Config>,
    /// 改屏之前拍下的原始表。退出或异常时需要它把屏幕还回去
    pub saved: Mutex<Vec<(String, Ramp)>>,
}

impl AppState {
    pub fn load() -> Self {
        let mut cfg: Config = fs::read_to_string(config_path())
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();

        // 计时开关开着就立刻开始算，否则界面会一直显示「未开始」
        cfg.break_plan.arm();

        let state = Self {
            config: Mutex::new(cfg),
            saved: Mutex::new(Vec::new()),
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

    pub fn config(&self) -> Config {
        self.config.lock().unwrap().clone()
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

/// 把当前配置写到屏幕上。
/// 目标是单块屏时，其它屏写回原始值，避免出现「只改了一块、其它还留着上次的颜色」。
pub fn apply_now(state: &AppState, cfg: &Config) {
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

    // 配置里的 target 可能指向另一台机器的屏，或者已经拔掉的屏。
    // 匹配不上就当全部处理，否则用户看到的是「开关是开的，但屏幕没变」。
    let target_all = cfg.target == "all" || !displays.iter().any(|d| d.id == cfg.target);
    if target_all && cfg.target != "all" {
        crate::protocol::log_line(
            &data_root(),
            &format!("target {} 不在当前显示器列表里，本次按全部屏幕处理", cfg.target),
        );
    }

    let saved = state.saved.lock().unwrap();

    let Some(tint) = resolve(cfg, crate::model::minutes_now()) else {
        for display in &displays {
            let base = saved
                .iter()
                .find(|(id, _)| id == &display.id)
                .map(|(_, ramp)| ramp);
            let ramp = baseline_for(base);
            let ok = gamma::write_ramp(&display.id, &ramp);
            crate::protocol::log_line(
                &data_root(),
                &format!("restore {} ok={ok} peak={}", display.id, ramp.peak(0)),
            );
        }
        return;
    };

    let brightness = cfg.brightness.clamp(0.1, 1.0);
    for display in displays {
        let targeted = target_all || cfg.target == display.id;
        if targeted {
            let ramp = match tint {
                Tint::Kelvin { kelvin } => build_gamma_ramp(kelvin, brightness),
                Tint::Rgb { rgb } => build_gamma_ramp_rgb(rgb, brightness),
            };
            // 算法固定产出 256 个点，目标平台需要多长由 gamma 层去重采样
            let ramp = Ramp::from_flat(&ramp).unwrap_or_else(|| Ramp::identity(256));
            let ok = gamma::write_ramp(&display.id, &ramp);
            crate::protocol::log_line(
                &data_root(),
                &format!(
                    "apply {} tint={tint:?} ok={ok} peak={} len={}",
                    display.id,
                    ramp.peak(0),
                    ramp.len
                ),
            );
        } else if let Some((_, original)) = saved.iter().find(|(id, _)| id == &display.id) {
            gamma::write_ramp(&display.id, &baseline_for(Some(original)));
        }
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
}

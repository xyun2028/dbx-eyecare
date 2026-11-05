//! 插件状态、持久化，以及「把配置真正写到屏幕上」。

use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use crate::color::{build_gamma_ramp, build_gamma_ramp_rgb};
use crate::gamma;
use crate::model::{resolve, Config, Tint};

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
    pub saved: Mutex<Vec<(String, [u16; 768])>>,
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

pub fn snapshot() -> Vec<(String, [u16; 768])> {
    gamma::list()
        .into_iter()
        .filter_map(|d| gamma::read_ramp(&d.id).map(|r| (d.id, r)))
        .collect()
}

pub fn restore_all(saved: &[(String, [u16; 768])]) {
    for (id, ramp) in saved {
        // 备份也可能本身就是偏的（上一轮遗留），统一走同一套判定
        gamma::write_ramp(id, &baseline_for(Some(ramp)));
    }
}

fn save_backup(saved: &[(String, [u16; 768])]) -> std::io::Result<()> {
    let path = backup_path();
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut text = String::from("eyecare-dbx-v1\n");
    for (id, ramp) in saved {
        text.push_str(id);
        for v in ramp {
            text.push(',');
            text.push_str(&v.to_string());
        }
        text.push('\n');
    }
    fs::write(path, text)
}

pub fn load_backup() -> Vec<(String, [u16; 768])> {
    let Ok(text) = fs::read_to_string(backup_path()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in text.lines().skip(1) {
        let mut parts = line.split(',');
        let Some(id) = parts.next() else { continue };
        if id.is_empty() {
            continue;
        }
        let mut ramp = [0u16; 768];
        let mut count = 0;
        for (i, part) in parts.enumerate().take(768) {
            match part.parse::<u16>() {
                Ok(v) => {
                    ramp[i] = v;
                    count += 1;
                }
                Err(_) => break,
            }
        }
        if count == 768 {
            out.push((id.to_string(), ramp));
        }
    }
    out
}

pub fn clear_backup() {
    let _ = fs::remove_file(backup_path());
}

/// 不做任何调整时的线性表。
fn identity_ramp() -> [u16; 768] {
    let mut ramp = [0u16; 768];
    for ch in 0..3 {
        for i in 0..256usize {
            ramp[ch * 256 + i] = (i as f64 / 255.0 * 65535.0).round() as u16;
        }
    }
    ramp
}

/// 「关闭 / 恢复原色」时到底该写什么。
///
/// 直接用启动时抓到的表是不行的：如果那一刻屏幕已经被染过色（比如上一轮
/// 自己写的，或者别的护眼软件在跑），抓到的「原色」本身就是偏的，写回去
/// 等于没关——表现就是开关点了没反应。
///
/// 判据：真正的原始表或校色配置，三个通道的峰值都是 65535；被降色温或滤色
/// 压过的表，峰值一定低于 65535。所以峰值为满才敢沿用，否则写线性表。
fn baseline_for(saved: Option<&[u16; 768]>) -> [u16; 768] {
    match saved {
        Some(ramp) if ramp[255] == 65535 && ramp[511] == 65535 && ramp[767] == 65535 => *ramp,
        _ => identity_ramp(),
    }
}

/// 把当前配置写到屏幕上。
/// 目标是单块屏时，其它屏写回原始值，避免出现「只改了一块、其它还留着上次的颜色」。
pub fn apply_now(state: &AppState, cfg: &Config) {
    let displays = gamma::list();
    if displays.is_empty() {
        crate::protocol::log_line(&data_root(), "apply: 没有枚举到显示器");
        return;
    }

    let saved = state.saved.lock().unwrap();

    let Some(tint) = resolve(cfg, crate::model::minutes_now()) else {
        for display in &displays {
            let base = saved.iter().find(|(id, _)| id == &display.id).map(|(_, ramps)| ramps);
            let ramp = baseline_for(base);
            let ok = gamma::write_ramp(&display.id, &ramp);
            crate::protocol::log_line(
                &data_root(),
                &format!("restore {} ok={ok} peak={}", display.id, ramp[255]),
            );
        }
        return;
    };

    let brightness = cfg.brightness.clamp(0.1, 1.0);
    for display in displays {
        let targeted = cfg.target == "all" || cfg.target == display.id;
        if targeted {
            let ramp = match tint {
                Tint::Kelvin { kelvin } => build_gamma_ramp(kelvin, brightness),
                Tint::Rgb { rgb } => build_gamma_ramp_rgb(rgb, brightness),
            };
            let ok = gamma::write_ramp(&display.id, &ramp);
            crate::protocol::log_line(
                &data_root(),
                &format!("apply {} tint={tint:?} ok={ok} peak={}", display.id, ramp[255]),
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
        let mut ramp = identity_ramp();
        ramp[255] = 51143;
        ramp[511] = 60909;
        ramp[767] = 52428;

        let base = baseline_for(Some(&ramp));
        assert_eq!(base[255], 65535, "峰值不满，应判定为污染并改写线性表");
        assert_eq!(base[511], 65535);
    }

    #[test]
    fn 峰值为满的表原样沿用() {
        let ramp = identity_ramp();
        assert_eq!(baseline_for(Some(&ramp)), ramp);
    }

    #[test]
    fn 没有备份时写线性表() {
        assert_eq!(baseline_for(None), identity_ramp());
    }
}

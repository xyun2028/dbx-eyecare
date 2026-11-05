//! 屏幕访问层。
//!
//! 三家平台的差异只有两点：表有多长、数值是什么类型。
//!
//! - Windows：固定 3×256 个 `WORD`，0..=65535
//! - Linux/X11：长度由 crtc 的 gamma size 决定，常见 256，也可能是 1024、4096
//! - macOS：长度自定，数值是 0.0..=1.0 的 float
//!
//! 所以内部统一用 [`Ramp`]：三个通道等长、每个点是 0..=65535 的整数。
//! 写屏时由各平台自己转：Windows 原样，X11 重采样到 crtc 的长度，
//! macOS 除以 65535。色温算法只需要关心「0 到 1 的曲线长什么样」，
//! 不用管目标平台用多少位表示。

use serde::{Deserialize, Serialize};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
mod unsupported;

#[cfg(target_os = "linux")]
pub use linux::{capability, list, read_ramp, write_ramp};
#[cfg(target_os = "macos")]
pub use macos::{capability, list, read_ramp, write_ramp};
#[cfg(target_os = "windows")]
pub use windows::{capability, list, read_ramp, write_ramp};
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub use unsupported::{capability, list, read_ramp, write_ramp};

#[derive(Debug, Clone, Serialize)]
pub struct DisplayInfo {
    /// 平台内稳定标识：Windows 是 `\\.\DISPLAY1`，macOS 是显示器的数字 id，
    /// Linux 是 RandR 的 output 名（如 `eDP-1`）
    pub id: String,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub primary: bool,
    /// None 表示查不到，不能当成「没开 HDR」
    pub hdr: Option<bool>,
}

/// 当前平台到底能不能改屏幕，以及不能改的原因。
///
/// 前端拿这个字段决定提示什么。不要把「改不了」伪装成「没有显示器」，
/// 用户会以为是自己没接屏。
#[derive(Debug, Clone, Serialize)]
pub struct Capability {
    pub available: bool,
    /// windows / macos / x11 / wayland / headless / unsupported
    pub session: &'static str,
    /// 给人看的一句话，available 时通常为 None
    pub reason: Option<String>,
}

impl Capability {
    pub fn ok(session: &'static str) -> Self {
        Self {
            available: true,
            session,
            reason: None,
        }
    }

    /// 只有 Linux/macOS 会用到；Windows 上不会出现「改不了」的情况。
    #[allow(dead_code)]
    pub fn no(session: &'static str, reason: impl Into<String>) -> Self {
        Self {
            available: false,
            session,
            reason: Some(reason.into()),
        }
    }
}

/// 一条 Gamma 查找表：三个通道等长，每个点 0..=65535。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ramp {
    pub len: usize,
    pub r: Vec<u16>,
    pub g: Vec<u16>,
    pub b: Vec<u16>,
}

impl Ramp {
    /// 不做任何调整的线性表。
    pub fn identity(len: usize) -> Self {
        let len = len.max(2);
        let channel: Vec<u16> = (0..len)
            .map(|i| (i as f64 / (len - 1) as f64 * 65535.0).round() as u16)
            .collect();
        Self {
            len,
            r: channel.clone(),
            g: channel.clone(),
            b: channel,
        }
    }

    /// 从「红 256 + 绿 256 + 蓝 256」的连续数组转过来，
    /// 也就是 `color.rs` 产出的那套格式。
    pub fn from_flat(flat: &[u16]) -> Option<Self> {
        if flat.len() % 3 != 0 || flat.is_empty() {
            return None;
        }
        let len = flat.len() / 3;
        Some(Self {
            len,
            r: flat[..len].to_vec(),
            g: flat[len..len * 2].to_vec(),
            b: flat[len * 2..].to_vec(),
        })
    }

    /// 按通道顺序拼成一个连续数组，长度是 `len * 3`。
    /// 只有 Windows 的 GDI 接口需要这个连续布局。
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub fn to_flat(&self) -> Vec<u16> {
        let mut out = Vec::with_capacity(self.len * 3);
        out.extend_from_slice(&self.r);
        out.extend_from_slice(&self.g);
        out.extend_from_slice(&self.b);
        out
    }

    /// 三个通道等长、至少两个点，才算一条能写的表。
    /// 从磁盘读回来的备份要先过这一关，否则写屏时可能越界。
    pub fn is_valid(&self) -> bool {
        self.len >= 2
            && self.r.len() == self.len
            && self.g.len() == self.len
            && self.b.len() == self.len
    }

    pub fn channel(&self, index: usize) -> &[u16] {
        match index {
            0 => &self.r,
            1 => &self.g,
            _ => &self.b,
        }
    }

    /// 某个通道最亮的那个点。用来判断这条表有没有被压过色。
    pub fn peak(&self, index: usize) -> u16 {
        self.channel(index).iter().copied().max().unwrap_or(0)
    }

    /// 三个通道的峰值都是满的，才算没被染过色。
    pub fn is_unpolluted(&self) -> bool {
        (0..3).all(|ch| self.peak(ch) == 65535)
    }

    /// 线性重采样到指定长度。
    ///
    /// 色温曲线本身是连续的，256 个点足够描述它；但 X11 要求写入的表
    /// 长度和 crtc 的 gamma size 完全一致，所以这里要能变长。
    pub fn resample(&self, len: usize) -> Self {
        let len = len.max(2);
        Self {
            len,
            r: resample_channel(&self.r, len),
            g: resample_channel(&self.g, len),
            b: resample_channel(&self.b, len),
        }
    }
}

fn resample_channel(src: &[u16], len: usize) -> Vec<u16> {
    if src.len() == len {
        return src.to_vec();
    }
    if src.len() < 2 {
        return vec![src.first().copied().unwrap_or(0); len];
    }
    (0..len)
        .map(|i| {
            let pos = i as f64 * (src.len() - 1) as f64 / (len - 1) as f64;
            let low = pos.floor() as usize;
            let high = (low + 1).min(src.len() - 1);
            let t = pos - low as f64;
            let value = src[low] as f64 + (src[high] as f64 - src[low] as f64) * t;
            value.round().clamp(0.0, 65535.0) as u16
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 线性表首尾取到两端() {
        let ramp = Ramp::identity(256);
        assert_eq!(ramp.r[0], 0);
        assert_eq!(ramp.r[255], 65535);
        assert!(ramp.is_unpolluted());
    }

    #[test]
    fn 连续数组可以来回转() {
        let flat: Vec<u16> = (0..768).map(|i| (i % 65536) as u16).collect();
        let ramp = Ramp::from_flat(&flat).expect("768 个点应该能拆成三通道");
        assert_eq!(ramp.len, 256);
        assert_eq!(ramp.to_flat(), flat);
    }

    #[test]
    fn 长度不是三的倍数会被拒() {
        assert!(Ramp::from_flat(&[1, 2, 3, 4]).is_none());
        assert!(Ramp::from_flat(&[]).is_none());
    }

    #[test]
    fn 重采样保持两端与单调() {
        let ramp = Ramp::identity(256).resample(1024);
        assert_eq!(ramp.len, 1024);
        assert_eq!(ramp.r[0], 0);
        assert_eq!(ramp.r[1023], 65535);
        assert!(ramp.r.windows(2).all(|pair| pair[0] <= pair[1]));
    }

    #[test]
    fn 重采样到原长度不改变内容() {
        let ramp = Ramp::identity(256);
        assert_eq!(ramp.resample(256), ramp);
    }

    #[test]
    fn 峰值不满的表判为被污染() {
        let mut ramp = Ramp::identity(256);
        ramp.b[255] = 50000;
        assert!(!ramp.is_unpolluted());
    }
}

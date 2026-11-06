//! 在场感知：人有多久没碰键鼠、前台窗口是不是全屏。
//!
//! 侧车每 5 秒问一次，只拿「距今多少秒没输入」这个数，不记录按过什么键、
//! 不读窗口标题、不联网。查不到就返回 `None`，调用方按「人在」处理，
//! 绝不因为检测失败就把用户当成离开。
//!
//! 平台分派方式和 gamma 层一致：三份实现各自编译，剩下的一律降级为空实现。

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
mod unsupported;

#[cfg(target_os = "windows")]
pub use windows::probe;
#[cfg(target_os = "macos")]
pub use macos::probe;
#[cfg(target_os = "linux")]
pub use linux::probe;
#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
pub use unsupported::probe;

/// 超过这个秒数没输入，就认为人暂时不在电脑前。
pub const IDLE_THRESHOLD_SECS: u64 = 120;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Activity {
    /// 距上次键鼠输入的秒数。`None` 表示当前平台查不到。
    pub idle_secs: Option<u64>,
    /// 前台窗口是否铺满整块屏幕（全屏游戏、播放器、演示）。
    pub fullscreen: bool,
}

impl Activity {
    /// 只有确实探测到了、而且超过阈值，才算离开。
    pub fn is_idle(&self) -> bool {
        self.idle_secs
            .map(|secs| secs >= IDLE_THRESHOLD_SECS)
            .unwrap_or(false)
    }

    /// 这个平台能不能查到输入空闲。前端据此说明为什么计时没有暂停。
    pub fn idle_supported(&self) -> bool {
        self.idle_secs.is_some()
    }
}

/// 用于界面展示的一句话，把「检测不到」和「检测到了、人在」区分开。
pub fn capability_note() -> &'static str {
    "只读取距上次键鼠输入的秒数，不记录按键内容、不读窗口标题、不联网。"
}

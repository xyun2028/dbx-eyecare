//! macOS：用 CoreGraphics 的事件源时间戳拿空闲时长。
//!
//! `CGEventSourceSecondsSinceLastEventType` 读的是 HID 系统态下「距上次输入
//! 多少秒」，不需要辅助功能权限，也不接收任何按键内容。和 gamma/macos.rs
//! 一样直接声明 FFI，不引入第三方 crate。
//!
//! 全屏判定暂时不实现：那需要 AppKit 的 NSApplication 或遍历窗口列表，
//! 在没有真机可验证的情况下不做，probe 会如实返回 false。

use super::Activity;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventSourceSecondsSinceLastEventType(source: i32, event_type: u32) -> f64;
}

/// CGEventSourceStateID：HID 系统态。
const HID_SYSTEM_STATE: i32 = 1;
/// CGEventType：任意输入事件。
const ANY_INPUT_EVENT: u32 = u32::MAX;

pub fn probe() -> Activity {
    let secs = unsafe { CGEventSourceSecondsSinceLastEventType(HID_SYSTEM_STATE, ANY_INPUT_EVENT) };
    Activity {
        idle_secs: if secs.is_finite() && secs >= 0.0 {
            Some(secs as u64)
        } else {
            None
        },
        // 未实现，按「不是全屏」处理，绝不误伤提醒
        fullscreen: false,
    }
}

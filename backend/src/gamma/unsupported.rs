//! 还没做屏幕实现的平台（BSD 等）。
//!
//! 编得过、跑得起来，只是不会真的改屏幕。宿主会拿到 `available: false`，
//! 界面上会直接说清楚，而不是让用户对着一个没反应的开关发呆。

use super::{Capability, DisplayInfo, Ramp};

pub fn capability() -> Capability {
    Capability::no(
        "unsupported",
        format!(
            "{} 上还没有实现屏幕色温调整，这个平台的版本只提供提醒功能。",
            std::env::consts::OS
        ),
    )
}

pub fn list() -> Vec<DisplayInfo> {
    Vec::new()
}

pub fn read_ramp(_id: &str) -> Option<Ramp> {
    None
}

pub fn write_ramp(_id: &str, _ramp: &Ramp) -> bool {
    false
}

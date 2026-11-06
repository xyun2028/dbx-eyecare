//! 其它平台：查不到在场信息，一律按「人在、不是全屏」返回。
//! 检测失败不能让用户少收到提醒，这是这一层的底线。

use super::Activity;

pub fn probe() -> Activity {
    Activity::default()
}

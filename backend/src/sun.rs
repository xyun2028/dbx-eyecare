//! 本地计算日出日落。
//!
//! 插件没有网络权限，也不该为了知道太阳几点落山去申请一个。所以这里用
//! NOAA 的简化太阳位置公式，只要有经纬度就能算，输入只有三个：一年中的第几天、
//! 经纬度、当地时区。误差在一两分钟以内，对「几点开始变暖」这件事完全够用。
//!
//! 返回的是**当地钟点**（从零点起的分钟数，带小数），不是 UTC。
//! 极昼极夜时没有日出日落，返回 `None`，调用方退回固定时刻。

use std::f64::consts::PI;

/// 当天日出、日落的当地钟点（分钟，0..1440）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SunTimes {
    pub sunrise: f64,
    pub sunset: f64,
}

/// 把结果从 UTC 分钟换成当地分钟，并折回一天之内。
fn to_local(minutes: f64, utc_offset_min: i32) -> f64 {
    let shifted = minutes + utc_offset_min as f64;
    ((shifted % 1440.0) + 1440.0) % 1440.0
}

/// 太阳赤纬（弧度）与均时差（分钟）。
fn solar_position(day_of_year: f64) -> (f64, f64) {
    let g = 2.0 * PI / 365.0 * (day_of_year - 1.0);

    let eqtime = 229.18
        * (0.000_075 + 0.001_868 * g.cos()
            - 0.032_077 * g.sin()
            - 0.014_615 * (2.0 * g).cos()
            - 0.040_849 * (2.0 * g).sin());

    let decl = 0.006_918 - 0.399_912 * g.cos()
        + 0.070_257 * g.sin()
        - 0.006_758 * (2.0 * g).cos()
        + 0.000_907 * (2.0 * g).sin()
        - 0.002_697 * (3.0 * g).cos()
        + 0.001_480 * (3.0 * g).sin();

    (decl, eqtime)
}

/// 算某一天的日出日落。
///
/// `lat` / `lon` 用十进制度，东经和北纬为正；`utc_offset_min` 是当地时区
/// 相对 UTC 的分钟数（北京时间是 480）。
///
/// 极圈内某些日子太阳整天不落或整天不出，此时返回 `None`。
pub fn sun_times(
    day_of_year: u32,
    lat: f64,
    lon: f64,
    utc_offset_min: i32,
) -> Option<SunTimes> {
    if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
        return None;
    }

    let lat = lat.to_radians();
    let (decl, eqtime) = solar_position(day_of_year.clamp(1, 366) as f64);

    // 日出日落用 90.833° 天顶角，比 90° 多出的那点是大气折射和日面半径
    let zenith = 90.833_f64.to_radians();
    let cos_ha = zenith.cos() / (lat.cos() * decl.cos()) - lat.tan() * decl.tan();

    // 超出 [-1, 1] 说明当天太阳压根不出或不落
    if !(-1.0..=1.0).contains(&cos_ha) {
        return None;
    }

    let ha = cos_ha.acos().to_degrees();

    // 每度经度对应 4 分钟；东经为正，所以经度越大日出越早
    let sunrise_utc = 720.0 - 4.0 * (lon + ha) - eqtime;
    let sunset_utc = 720.0 - 4.0 * (lon - ha) - eqtime;

    Some(SunTimes {
        sunrise: to_local(sunrise_utc, utc_offset_min),
        sunset: to_local(sunset_utc, utc_offset_min),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minutes_of(clock: f64) -> (u32, u32) {
        let h = clock / 60.0;
        let hour = h.floor() as u32;
        let minute = ((h - hour as f64) * 60.0).round() as u32;
        (hour, minute)
    }

    #[test]
    fn 深圳夏至接近五点四十日出() {
        // 2026-06-21 是第 172 天；深圳 114.06E, 22.55N，UTC+8
        let t = sun_times(172, 22.55, 114.06, 480).expect("深圳不会极夜");
        let (h, m) = minutes_of(t.sunrise);
        assert_eq!(h, 5, "日出应在 5 点，实际 {h}:{m:02}");

        let (h, m) = minutes_of(t.sunset);
        assert_eq!(h, 19, "日落应在 19 点，实际 {h}:{m:02}");
        assert!(m >= 5 && m <= 25, "夏至日落约 19:12，实际 {h}:{m:02}");
    }

    #[test]
    fn 深圳冬至白天明显短于夏至() {
        // 2026-12-21 是第 355 天
        let summer = sun_times(172, 22.55, 114.06, 480).unwrap();
        let winter = sun_times(355, 22.55, 114.06, 480).unwrap();

        let mut summer_len = summer.sunset - summer.sunrise;
        let mut winter_len = winter.sunset - winter.sunrise;
        if summer_len < 0.0 {
            summer_len += 1440.0;
        }
        if winter_len < 0.0 {
            winter_len += 1440.0;
        }

        assert!(
            summer_len - winter_len > 120.0,
            "夏至白天应比冬至长两小时以上：夏 {summer_len} 冬 {winter_len}"
        );
    }

    #[test]
    fn 北京冬至七点半后日出() {
        // 2026-12-21，北京 116.4E, 39.9N
        let t = sun_times(355, 39.9, 116.4, 480).unwrap();
        let (h, m) = minutes_of(t.sunrise);
        assert_eq!(h, 7, "北京冬至约 07:33 日出，实际 {h}:{m:02}");
        assert!(m >= 20, "分钟数应接近 33，实际 {m}");
    }

    #[test]
    fn 极昼没有日出日落() {
        // 北纬 80 度夏至，太阳整天在地平线上
        assert!(sun_times(172, 80.0, 0.0, 0).is_none());
    }

    #[test]
    fn 极夜也没有() {
        assert!(sun_times(355, 80.0, 0.0, 0).is_none());
    }

    #[test]
    fn 结果总落在一天之内() {
        for day in [1u32, 80, 172, 266, 355] {
            let t = sun_times(day, 22.55, 114.06, 480).unwrap();
            assert!((0.0..1440.0).contains(&t.sunrise), "第 {day} 天日出越界");
            assert!((0.0..1440.0).contains(&t.sunset), "第 {day} 天日落越界");
        }
    }

    #[test]
    fn 非法经纬度返回空() {
        assert!(sun_times(172, 91.0, 0.0, 0).is_none());
        assert!(sun_times(172, 0.0, 181.0, 0).is_none());
    }
}

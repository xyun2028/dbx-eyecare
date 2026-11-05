//! macOS 的屏幕访问：CoreGraphics 的 transfer table。
//!
//! 没有引入任何 crate，直接声明要用的几个函数。它们的签名十几年没变过：
//! 显示器用 `CGDirectDisplayID`（u32）标识，表里的数值是 0.0..=1.0 的 float，
//! 长度自定（系统默认 256）。
//!
//! 为什么不用 AppKit 或 CoreDisplay：那些要么太重，要么是私有 API。
//! CGSetDisplayTransferByTable 是公开接口，够用。

use std::os::raw::c_int;

use super::{Capability, DisplayInfo, Ramp};

type CGDirectDisplayID = u32;

/// 同时接 32 块屏已经远超实际，超过这个数系统会截断。
const MAX_DISPLAYS: u32 = 32;

/// 系统默认的采样点数。读不到时按这个长度兜底。
const DEFAULT_SAMPLES: u32 = 256;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct CGSize {
    width: f64,
    height: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct CGRect {
    origin: CGPoint,
    size: CGSize,
}

// 64 位 macOS 上 CGFloat 就是 f64，Intel 和 Apple Silicon 都一样。
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGMainDisplayID() -> CGDirectDisplayID;
    fn CGGetActiveDisplayList(
        max_displays: u32,
        displays: *mut CGDirectDisplayID,
        display_count: *mut u32,
    ) -> c_int;
    fn CGDisplayBounds(display: CGDirectDisplayID) -> CGRect;
    fn CGDisplayIsMain(display: CGDirectDisplayID) -> c_int;
    fn CGDisplayIsBuiltin(display: CGDirectDisplayID) -> c_int;
    fn CGGetDisplayTransferByTable(
        display: CGDirectDisplayID,
        capacity: u32,
        red: *mut f32,
        green: *mut f32,
        blue: *mut f32,
        sample_count: *mut u32,
    ) -> c_int;
    fn CGSetDisplayTransferByTable(
        display: CGDirectDisplayID,
        sample_count: u32,
        red: *const f32,
        green: *const f32,
        blue: *const f32,
    ) -> c_int;
}

const K_CG_ERROR_SUCCESS: c_int = 0;

pub fn capability() -> Capability {
    // 没有主屏通常意味着跑在 SSH、无图形会话或 CI 里，这时候写表也不会有人看到
    if unsafe { CGMainDisplayID() } == 0 {
        return Capability::no("headless", "当前会话没有可用的图形显示，色温调整不会生效。");
    }
    Capability::ok("macos")
}

fn parse_id(id: &str) -> Option<CGDirectDisplayID> {
    id.parse::<u32>().ok().filter(|value| *value != 0)
}

pub fn list() -> Vec<DisplayInfo> {
    let mut ids = vec![0u32; MAX_DISPLAYS as usize];
    let mut count: u32 = 0;
    let err = unsafe { CGGetActiveDisplayList(MAX_DISPLAYS, ids.as_mut_ptr(), &mut count) };
    if err != K_CG_ERROR_SUCCESS {
        return Vec::new();
    }
    ids.truncate(count as usize);

    // 主屏放最前，和界面上的排列顺序一致
    ids.sort_by_key(|id| unsafe { CGDisplayIsMain(*id) == 0 });

    let mut out = Vec::with_capacity(ids.len());
    for (index, id) in ids.into_iter().enumerate() {
        let builtin = unsafe { CGDisplayIsBuiltin(id) } != 0;
        let primary = unsafe { CGDisplayIsMain(id) } != 0;
        let bounds = unsafe { CGDisplayBounds(id) };

        let name = if builtin {
            "内建显示器".to_string()
        } else if primary {
            "主显示器".to_string()
        } else {
            format!("显示器 {}", index + 1)
        };

        out.push(DisplayInfo {
            // macOS 没有公开的显示器名字接口，数字 id 本身是稳定的，就用它
            id: id.to_string(),
            name,
            width: bounds.size.width.round().max(0.0) as u32,
            height: bounds.size.height.round().max(0.0) as u32,
            primary,
            // 公开 API 查不到「是否 HDR/EDR」，这里只能如实说不知道
            hdr: None,
        });
    }
    out
}

fn to_samples(channel: &[u16]) -> Vec<f32> {
    channel.iter().map(|v| *v as f32 / 65535.0).collect()
}

fn from_samples(samples: &[f32]) -> Vec<u16> {
    samples
        .iter()
        .map(|v| (v.clamp(0.0, 1.0) * 65535.0).round() as u16)
        .collect()
}

pub fn read_ramp(id: &str) -> Option<Ramp> {
    let display = parse_id(id)?;

    // 绝大多数机器是 256 个点；个别采集卡给 1024，长度不够会直接失败，
    // 所以失败一次就换更大的缓冲再试。
    for capacity in [DEFAULT_SAMPLES, 1024] {
        let mut red = vec![0f32; capacity as usize];
        let mut green = vec![0f32; capacity as usize];
        let mut blue = vec![0f32; capacity as usize];
        let mut count: u32 = 0;

        let err = unsafe {
            CGGetDisplayTransferByTable(
                display,
                capacity,
                red.as_mut_ptr(),
                green.as_mut_ptr(),
                blue.as_mut_ptr(),
                &mut count,
            )
        };

        if err != K_CG_ERROR_SUCCESS || count == 0 || count > capacity {
            continue;
        }

        let len = count as usize;
        red.truncate(len);
        green.truncate(len);
        blue.truncate(len);

        return Some(Ramp {
            len,
            r: from_samples(&red),
            g: from_samples(&green),
            b: from_samples(&blue),
        });
    }

    None
}

pub fn write_ramp(id: &str, ramp: &Ramp) -> bool {
    let Some(display) = parse_id(id) else {
        return false;
    };

    let table = ramp.resample(DEFAULT_SAMPLES as usize);
    let red = to_samples(&table.r);
    let green = to_samples(&table.g);
    let blue = to_samples(&table.b);

    let err = unsafe {
        CGSetDisplayTransferByTable(
            display,
            table.len as u32,
            red.as_ptr(),
            green.as_ptr(),
            blue.as_ptr(),
        )
    };
    err == K_CG_ERROR_SUCCESS
}

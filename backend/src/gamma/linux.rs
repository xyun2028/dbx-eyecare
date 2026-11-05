//! Linux 的屏幕访问：X11 + RandR。
//!
//! 用纯 Rust 的 x11rb，不链接 libX11/libXrandr。
//! 这样交叉编译 arm64/armv7 时不需要目标平台的开发包和 sysroot，
//! 打出来的二进制也不依赖编译机的库路径。
//!
//! RandR 的 Gamma 是挂在 crtc 上的，不是挂在 output 上。
//! 所以对外用 output 名（`eDP-1`、`HDMI-1`）当 id，内部再翻译成 crtc。
//!
//! Wayland 没有对应的公开协议：`wlr-gamma-control` 只有 wlroots 系合成器支持，
//! GNOME/KDE 都不给第三方改。这里如实降级，不假装成功。

use x11rb::connection::Connection as _;
use x11rb::protocol::randr::{Connection as RandrConnection, ConnectionExt as _};
use x11rb::rust_connection::RustConnection;

use super::{Capability, DisplayInfo, Ramp};

fn wayland_session() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some()
        || std::env::var("XDG_SESSION_TYPE")
            .map(|value| value.eq_ignore_ascii_case("wayland"))
            .unwrap_or(false)
}

fn connect() -> Option<(RustConnection, u32)> {
    let (conn, screen_num) = x11rb::connect(None).ok()?;
    let root = conn.setup().roots.get(screen_num)?.root;
    Some((conn, root))
}

pub fn capability() -> Capability {
    let wayland = wayland_session();

    match connect() {
        // XWayland 能连上，但改的是 XWayland 那份假的表，原生 Wayland 窗口不受影响。
        // 这种情况必须说清楚，否则用户会以为插件坏了。
        Some(_) if wayland => Capability::no(
            "wayland",
            "当前是 Wayland 会话。Wayland 不允许应用直接改 Gamma 表，只有 XWayland 里的窗口会变色，桌面本身不变。要完整生效，请登录 X11 会话。",
        ),
        Some(_) => Capability::ok("x11"),
        None if wayland => Capability::no(
            "wayland",
            "当前是 Wayland 会话，且没有可用的 X 显示。这个功能需要 X11 会话，Wayland 下无法改 Gamma 表。",
        ),
        None => Capability::no(
            "headless",
            "没有连上 X 显示（DISPLAY 未设置或连接失败），色温调整不会生效。",
        ),
    }
}

/// 一次查询里顺带把 output 和它的 crtc 都取出来，避免重复往返。
struct OutputEntry {
    output: u32,
    crtc: u32,
    name: String,
    width: u32,
    height: u32,
}

fn collect(conn: &RustConnection, root: u32) -> Vec<OutputEntry> {
    let resources = match conn
        .randr_get_screen_resources_current(root)
        .ok()
        .and_then(|cookie| cookie.reply().ok())
    {
        Some(reply) => reply,
        None => return Vec::new(),
    };

    let mut entries = Vec::new();
    for output in resources.outputs {
        let info = match conn
            .randr_get_output_info(output, resources.config_timestamp)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
        {
            Some(reply) => reply,
            None => continue,
        };

        // 只有真的接了屏、并且绑定了 crtc 的 output 才有 gamma 可调
        if info.connection != RandrConnection::CONNECTED || info.crtc == 0 {
            continue;
        }

        let (width, height) = match conn
            .randr_get_crtc_info(info.crtc, resources.config_timestamp)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
        {
            Some(crtc) => (u32::from(crtc.width), u32::from(crtc.height)),
            None => (0, 0),
        };

        entries.push(OutputEntry {
            output,
            crtc: info.crtc,
            name: String::from_utf8_lossy(&info.name).to_string(),
            width,
            height,
        });
    }
    entries
}

fn primary_output(conn: &RustConnection, root: u32) -> u32 {
    conn.randr_get_output_primary(root)
        .ok()
        .and_then(|cookie| cookie.reply().ok())
        .map(|reply| reply.output)
        .unwrap_or(0)
}

pub fn list() -> Vec<DisplayInfo> {
    let Some((conn, root)) = connect() else {
        return Vec::new();
    };

    let primary = primary_output(&conn, root);
    let mut entries = collect(&conn, root);
    // 主屏放最前，和界面上的排列顺序一致
    entries.sort_by_key(|entry| entry.output != primary);

    entries
        .into_iter()
        .map(|entry| DisplayInfo {
            // output 名比 XID 稳定：重启后 XID 会变，名字一般不变
            id: entry.name.clone(),
            name: entry.name,
            width: entry.width,
            height: entry.height,
            primary: entry.output == primary,
            // X11 没有查询 HDR 的标准方式
            hdr: None,
        })
        .collect()
}

/// 把 output 名翻译成 crtc。找不到就说明屏被拔了或名字对不上。
fn crtc_for(conn: &RustConnection, root: u32, id: &str) -> Option<u32> {
    collect(conn, root)
        .into_iter()
        .find(|entry| entry.name == id)
        .map(|entry| entry.crtc)
}

pub fn read_ramp(id: &str) -> Option<Ramp> {
    let (conn, root) = connect()?;
    let crtc = crtc_for(&conn, root, id)?;

    let gamma = conn
        .randr_get_crtc_gamma(crtc)
        .ok()
        .and_then(|cookie| cookie.reply().ok())?;

    if gamma.size() == 0 {
        return None;
    }

    Some(Ramp {
        len: gamma.size() as usize,
        r: gamma.red,
        g: gamma.green,
        b: gamma.blue,
    })
}

pub fn write_ramp(id: &str, ramp: &Ramp) -> bool {
    let Some((conn, root)) = connect() else {
        return false;
    };
    let Some(crtc) = crtc_for(&conn, root, id) else {
        return false;
    };

    // 表长必须和 crtc 的 gamma size 完全一致，否则 X 会返回 BadMatch。
    // x11rb 从切片长度推 size，所以这里先读一次真实长度再重采样。
    let size = match conn
        .randr_get_crtc_gamma(crtc)
        .ok()
        .and_then(|cookie| cookie.reply().ok())
    {
        Some(reply) if reply.size() > 0 => reply.size() as usize,
        _ => return false,
    };

    let table = ramp.resample(size);
    conn.randr_set_crtc_gamma(crtc, &table.r, &table.g, &table.b)
        .ok()
        .and_then(|cookie| cookie.check().ok())
        .is_some()
}

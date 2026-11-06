//! Linux/X11：screensaver 扩展读空闲，EWMH 属性判全屏。
//!
//! 两者都只做查询：不启动屏保、不改窗口状态、不订阅事件。
//! Wayland 会话里连不上 X（或连上的是 XWayland），查询失败就返回「人在」，
//! 走原来的固定计时，不会因为检测不到而停掉提醒。

use x11rb::connection::Connection as _;
use x11rb::protocol::screensaver::ConnectionExt as ScreensaverExt;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as XprotoExt};
use x11rb::rust_connection::RustConnection;

use super::Activity;

fn connect() -> Option<(RustConnection, u32)> {
    let (conn, screen_num) = x11rb::connect(None).ok()?;
    let root = conn.setup().roots.get(screen_num)?.root;
    Some((conn, root))
}

fn atom(conn: &RustConnection, name: &[u8]) -> Option<u32> {
    conn.intern_atom(false, name)
        .ok()
        .and_then(|cookie| cookie.reply().ok())
        .map(|reply| reply.atom)
        .filter(|value| *value != 0)
}

fn u32_values(bytes: &[u8], format: u8) -> Vec<u32> {
    if format != 32 {
        return Vec::new();
    }
    bytes
        .chunks_exact(4)
        .map(|chunk| u32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}

fn idle_secs(conn: &RustConnection, root: u32) -> Option<u64> {
    let reply = conn
        .screensaver_query_info(root)
        .ok()
        .and_then(|cookie| cookie.reply().ok())?;
    Some(reply.ms_since_user_input as u64 / 1000)
}

/// 读 `_NET_ACTIVE_WINDOW` → 该窗口的 `_NET_WM_STATE` 里有没有
/// `_NET_WM_STATE_FULLSCREEN`。合成器不提供这些属性就当不是全屏。
fn foreground_fullscreen(conn: &RustConnection, root: u32) -> bool {
    let (Some(active_atom), Some(state_atom), Some(full_atom)) = (
        atom(conn, b"_NET_ACTIVE_WINDOW"),
        atom(conn, b"_NET_WM_STATE"),
        atom(conn, b"_NET_WM_STATE_FULLSCREEN"),
    ) else {
        return false;
    };

    let active = conn
        .get_property(false, root, active_atom, AtomEnum::ATOM, 0, 1)
        .ok()
        .and_then(|cookie| cookie.reply().ok())
        .and_then(|reply| u32_values(&reply.value, reply.format).into_iter().next())
        .unwrap_or(0);
    if active == 0 {
        return false;
    }

    let states = conn
        .get_property(false, active, state_atom, AtomEnum::ATOM, 0, 32)
        .ok()
        .and_then(|cookie| cookie.reply().ok())
        .map(|reply| u32_values(&reply.value, reply.format))
        .unwrap_or_default();

    states.contains(&full_atom)
}

pub fn probe() -> Activity {
    let Some((conn, root)) = connect() else {
        return Activity::default();
    };
    Activity {
        idle_secs: idle_secs(&conn, root),
        fullscreen: foreground_fullscreen(&conn, root),
    }
}

//! Windows：GetLastInputInfo 拿空闲时长，前台窗口矩形与所在显示器矩形比对判全屏。

use std::mem::{size_of, zeroed};

use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, MONITORINFOEXW, MONITOR_DEFAULTTONEAREST,
};
use windows_sys::Win32::System::SystemInformation::GetTickCount64;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetShellWindow, GetWindowRect,
};

use super::Activity;

/// 只取 32 位 tick 的低位：LASTINPUTINFO.dwTime 是 GetTickCount 的 u32，
/// 机器连续开机超过 49.7 天会回绕，用 wrapping_sub 才不会算出几亿秒的空闲。
fn idle_secs() -> Option<u64> {
    unsafe {
        let mut info = LASTINPUTINFO {
            cbSize: size_of::<LASTINPUTINFO>() as u32,
            dwTime: 0,
        };
        if GetLastInputInfo(&mut info) == 0 {
            return None;
        }
        let now = GetTickCount64() as u32;
        Some(now.wrapping_sub(info.dwTime) as u64 / 1000)
    }
}

/// 前台窗口盖满它所在的那块显示器，就当作全屏内容。
///
/// 不读窗口标题、不取进程名——这里只需要一个矩形。桌面和任务栏会被排除，
/// 否则壁纸本身就会被判成全屏，插件会一直不提醒。
fn foreground_fullscreen() -> bool {
    unsafe {
        let hwnd = GetForegroundWindow();
        let shell = GetShellWindow();
        if hwnd.is_null() || hwnd == shell {
            return false;
        }

        let mut window: RECT = zeroed();
        if GetWindowRect(hwnd, &mut window) == 0 {
            return false;
        }

        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        if monitor.is_null() {
            return false;
        }

        let mut info: MONITORINFOEXW = zeroed();
        info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        if GetMonitorInfoW(monitor, &mut info as *mut _ as *mut _) == 0 {
            return false;
        }
        let screen = info.monitorInfo.rcMonitor;

        // 留 2 像素容差：有些播放器边框会差一两像素
        window.left <= screen.left + 2
            && window.top <= screen.top + 2
            && window.right >= screen.right - 2
            && window.bottom >= screen.bottom - 2
    }
}

pub fn probe() -> Activity {
    Activity {
        idle_secs: idle_secs(),
        fullscreen: foreground_fullscreen(),
    }
}

//! 屏幕访问层。Windows 之外的平台先留空实现，保证能编译。

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct DisplayInfo {
    /// 系统标识，Windows 下形如 `\\.\DISPLAY1`
    pub id: String,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub primary: bool,
    /// None 表示查不到，不能当成「没开 HDR」
    pub hdr: Option<bool>,
}

#[cfg(windows)]
mod imp {
    use std::collections::HashMap;
    use std::ffi::c_void;
    use std::mem::size_of;
    use std::ptr::{null, null_mut};

    use windows_sys::Win32::Devices::Display::{
        DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes, QueryDisplayConfig,
        DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO, DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
        DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO, DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO,
        DISPLAYCONFIG_SOURCE_DEVICE_NAME, QDC_ONLY_ACTIVE_PATHS,
    };
    use windows_sys::Win32::Graphics::Gdi::{
        CreateDCW, DeleteDC, EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFOEXW,
    };
    use windows_sys::Win32::UI::ColorSystem::{GetDeviceGammaRamp, SetDeviceGammaRamp};

    use super::DisplayInfo;

    const MONITORINFOF_PRIMARY: u32 = 1;

    fn utf16_to_string(buf: &[u16]) -> String {
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..len])
    }

    fn to_wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// 每块屏单独取 DC。用桌面 DC 会一次改掉所有屏幕。
    fn open_dc(device: &str) -> Option<HDC> {
        let wide = to_wide(device);
        let hdc = unsafe { CreateDCW(wide.as_ptr(), null(), null(), null()) };
        if hdc.is_null() {
            None
        } else {
            Some(hdc)
        }
    }

    struct MonitorCtx {
        list: Vec<(String, u32, u32, bool)>,
    }

    unsafe extern "system" fn monitor_cb(
        hmon: HMONITOR,
        _hdc: HDC,
        _rect: *mut windows_sys::Win32::Foundation::RECT,
        lparam: windows_sys::Win32::Foundation::LPARAM,
    ) -> i32 {
        let ctx = &mut *(lparam as *mut MonitorCtx);
        let mut info: MONITORINFOEXW = std::mem::zeroed();
        info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;

        if GetMonitorInfoW(hmon, &mut info as *mut _ as *mut _) != 0 {
            let id = utf16_to_string(&info.szDevice);
            if !id.is_empty() {
                let rc = info.monitorInfo.rcMonitor;
                ctx.list.push((
                    id,
                    (rc.right - rc.left) as u32,
                    (rc.bottom - rc.top) as u32,
                    info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
                ));
            }
        }
        1
    }

    fn enumerate() -> Vec<(String, u32, u32, bool)> {
        let mut ctx = MonitorCtx { list: Vec::new() };
        unsafe {
            EnumDisplayMonitors(
                null_mut(),
                null(),
                Some(monitor_cb),
                &mut ctx as *mut _ as windows_sys::Win32::Foundation::LPARAM,
            );
        }
        ctx.list
    }

    /// 每块屏的 HDR 状态，键是 GDI 设备名。
    ///
    /// 同一路径上，target 侧给高级色彩信息，source 侧给 GDI 名字，两边配对才认得出来。
    fn hdr_map() -> HashMap<String, bool> {
        let mut map = HashMap::new();
        unsafe {
            let mut path_count: u32 = 0;
            let mut mode_count: u32 = 0;
            if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut path_count, &mut mode_count) != 0 {
                return map;
            }

            let mut paths: Vec<DISPLAYCONFIG_PATH_INFO> =
                vec![std::mem::zeroed(); path_count as usize];
            let mut modes: Vec<DISPLAYCONFIG_MODE_INFO> =
                vec![std::mem::zeroed(); mode_count as usize];

            if QueryDisplayConfig(
                QDC_ONLY_ACTIVE_PATHS,
                &mut path_count,
                paths.as_mut_ptr(),
                &mut mode_count,
                modes.as_mut_ptr(),
                null_mut(),
            ) != 0
            {
                return map;
            }

            for path in paths.iter().take(path_count as usize) {
                let mut source: DISPLAYCONFIG_SOURCE_DEVICE_NAME = std::mem::zeroed();
                source.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME;
                source.header.size = size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32;
                source.header.adapterId = path.sourceInfo.adapterId;
                source.header.id = path.sourceInfo.id;
                if DisplayConfigGetDeviceInfo(&mut source.header) != 0 {
                    continue;
                }
                let gdi_name = utf16_to_string(&source.viewGdiDeviceName);
                if gdi_name.is_empty() {
                    continue;
                }

                let mut color: DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO = std::mem::zeroed();
                color.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO;
                color.header.size = size_of::<DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO>() as u32;
                color.header.adapterId = path.targetInfo.adapterId;
                color.header.id = path.targetInfo.id;
                if DisplayConfigGetDeviceInfo(&mut color.header) != 0 {
                    continue;
                }

                // 第 0 位：支持高级色彩；第 1 位：当前已启用
                let bits = color.Anonymous.value;
                if bits & 1 != 0 {
                    map.insert(gdi_name, bits & 2 != 0);
                }
            }
        }
        map
    }

    pub fn list() -> Vec<DisplayInfo> {
        let hdr = hdr_map();
        enumerate()
            .into_iter()
            .map(|(id, width, height, primary)| {
                let short = id.trim_start_matches(r"\\.\").to_string();
                DisplayInfo {
                    hdr: hdr.get(&id).copied(),
                    name: if short.is_empty() { id.clone() } else { short },
                    id,
                    width,
                    height,
                    primary,
                }
            })
            .collect()
    }

    pub fn read_ramp(id: &str) -> Option<[u16; 768]> {
        let hdc = open_dc(id)?;
        let mut buf = [0u16; 768];
        let ok = unsafe { GetDeviceGammaRamp(hdc, buf.as_mut_ptr() as *mut c_void) } != 0;
        unsafe { DeleteDC(hdc) };
        if ok {
            Some(buf)
        } else {
            None
        }
    }

    pub fn write_ramp(id: &str, ramp: &[u16; 768]) -> bool {
        let Some(hdc) = open_dc(id) else {
            return false;
        };
        let ok = unsafe { SetDeviceGammaRamp(hdc, ramp.as_ptr() as *const c_void) } != 0;
        unsafe { DeleteDC(hdc) };
        ok
    }
}

#[cfg(not(windows))]
mod imp {
    use super::DisplayInfo;

    pub fn list() -> Vec<DisplayInfo> {
        Vec::new()
    }

    pub fn read_ramp(_id: &str) -> Option<[u16; 768]> {
        None
    }

    pub fn write_ramp(_id: &str, _ramp: &[u16; 768]) -> bool {
        false
    }
}

pub use imp::{list, read_ramp, write_ramp};

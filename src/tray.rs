// Tray icon and context menu. Menu ids are shared with App::menu_command:
// 10 capture, 5/6 export/import, 7 re-register hotkey, 8 autostart,
// 11 about, 9 quit. TaskbarCreated re-registration and the one-time
// balloon are unchanged from 3.3.

use std::ffi::c_void;

use crate::win;

const ICO: &[u8] = include_bytes!("../assets/app.ico");

pub fn make_icon() -> win::HICON {
    unsafe {
        if ICO.len() < 6 { return fallback_icon(); }
        let count = u16::from_le_bytes([ICO[4], ICO[5]]) as usize;
        let mut best: Option<(u32, u32, u8, u8)> = None;
        for i in 0..count {
            let off = 6 + i * 16;
            if off + 16 > ICO.len() { break; }
            let w = ICO[off];
            let h = ICO[off + 1];
            let size = u32::from_le_bytes([ICO[off + 8], ICO[off + 9], ICO[off + 10], ICO[off + 11]]);
            let image_off = u32::from_le_bytes([ICO[off + 12], ICO[off + 13], ICO[off + 14], ICO[off + 15]]);
            if (image_off as usize) + (size as usize) > ICO.len() { continue; }
            // Prefer the 16x16 frame for the notification area.
            if w == 16 || h == 16 {
                best = Some((size, image_off, w, h));
                break;
            }
            if best.is_none() { best = Some((size, image_off, w, h)); }
        }
        if let Some((size, off, w, h)) = best {
            let mut bits = ICO[off as usize..(off + size) as usize].to_vec();
            let icon = win::CreateIconFromResourceEx(bits.as_mut_ptr(), size, 1, 0x0003_0000,
                if w == 0 { 16 } else { w as i32 }, if h == 0 { 16 } else { h as i32 }, 0);
            if !icon.is_null() { return icon; }
        }
        fallback_icon()
    }
}

/// Pick the ICO frame closest to (but not smaller than) `target` pixels.
/// The tray wants 16px, the title logo wants 32px, and downscaling a 256px
/// frame through GDI+ loses the crisp 1px outlines, so pick the real frame.
pub fn icon_at(target: i32) -> win::HICON {
    unsafe {
        if ICO.len() < 6 { return fallback_icon(); }
        let count = u16::from_le_bytes([ICO[4], ICO[5]]) as usize;
        let mut best: Option<(i32, u32, u32)> = None; // (frame size, image size, offset)
        for i in 0..count {
            let off = 6 + i * 16;
            if off + 16 > ICO.len() { break; }
            let w = if ICO[off] == 0 { 256 } else { ICO[off] as i32 };
            let h = if ICO[off + 1] == 0 { 256 } else { ICO[off + 1] as i32 };
            let size = u32::from_le_bytes([ICO[off + 8], ICO[off + 9], ICO[off + 10], ICO[off + 11]]);
            let image_off = u32::from_le_bytes([ICO[off + 12], ICO[off + 13], ICO[off + 14], ICO[off + 15]]);
            if (image_off as usize) + (size as usize) > ICO.len() { continue; }
            let frame = w.min(h);
            if frame < target { continue; }
            // Smallest frame that still covers the target keeps the edges sharp.
            if best.map_or(true, |(f, _, _)| frame < f) {
                best = Some((frame, size, image_off));
            }
        }
        if let Some((frame, size, off)) = best {
            let mut bits = ICO[off as usize..(off + size) as usize].to_vec();
            let icon = win::CreateIconFromResourceEx(bits.as_mut_ptr(), size, 1, 0x0003_0000,
                frame, frame, 0);
            if !icon.is_null() { return icon; }
        }
        make_icon()
    }
}

fn fallback_icon() -> win::HICON {
    unsafe { win::LoadIconW(std::ptr::null_mut(), win::IDI_APPLICATION as *const u16) }
}

fn base_data(hwnd: win::HWND, icon: win::HICON) -> win::NOTIFYICONDATAW {
    let mut n: win::NOTIFYICONDATAW = Default::default();
    n.cbSize = std::mem::size_of::<win::NOTIFYICONDATAW>() as u32;
    n.hwnd = hwnd;
    n.uID = 1;
    n.hIcon = icon;
    let tip = win::wide("TT酱 · 双击 Ctrl 唤起");
    let len = tip.len().min(128);
    n.szTip[..len].copy_from_slice(&tip[..len]);
    n
}

pub fn add(hwnd: win::HWND, icon: win::HICON) -> bool {
    unsafe {
        let mut n = base_data(hwnd, icon);
        n.uFlags = win::NIF_MESSAGE | win::NIF_ICON | win::NIF_TIP;
        n.uCallbackMessage = win::WM_APP_TRAY;
        let ok = win::Shell_NotifyIconW(win::NIM_ADD, &mut n);
        if ok == 0 {
            win::log(&format!("Shell_NotifyIcon NIM_ADD 失败 err={}", win::GetLastError()));
        }
        ok != 0
    }
}

pub fn remove(hwnd: win::HWND, icon: win::HICON) {
    unsafe {
        let mut n = base_data(hwnd, icon);
        win::Shell_NotifyIconW(win::NIM_DELETE, &mut n);
    }
}

pub fn balloon(hwnd: win::HWND, icon: win::HICON) {
    unsafe {
        let mut n = base_data(hwnd, icon);
        n.uFlags = win::NIF_INFO;
        n.dwInfoFlags = win::NIIF_INFO;
        let title = win::wide("TT酱仍在运行");
        let body = win::wide("双击 Ctrl 随时唤起窗口；右键托盘图标可退出。");
        let a = title.len().min(64);
        let b = body.len().min(256);
        n.szInfoTitle[..a].copy_from_slice(&title[..a]);
        n.szInfo[..b].copy_from_slice(&body[..b]);
        win::Shell_NotifyIconW(win::NIM_MODIFY, &mut n);
    }
}

/// Rectangle the shell actually assigned to our tray icon, in screen pixels.
/// None when the icon is not registered or the shell declines (Win7+ API).
pub fn icon_rect(hwnd: win::HWND) -> Option<win::RECT> {
    unsafe {
        let pii = win::NOTIFYICONIDENTIFIER {
            cbSize: std::mem::size_of::<win::NOTIFYICONIDENTIFIER>() as u32,
            hWnd: hwnd,
            uID: 1,
            guidItem: win::Guid::default(),
        };
        let mut rc: win::RECT = Default::default();
        let hr = win::Shell_NotifyIconGetRect(&pii, &mut rc);
        if hr == 0 && rc.right > rc.left && rc.bottom > rc.top { return Some(rc); }
        win::log(&format!("Shell_NotifyIconGetRect 失败 hr=0x{:08X}", hr));
        None
    }
}

/// Anchor for the context menu: the tray icon's own rectangle, not the mouse.
/// Anchoring at the cursor put the menu's top edge on the icon, so the whole
/// menu ran downhill into the taskbar. Anchoring on the icon's far corner and
/// growing back toward the desktop keeps it clear of the screen edge no matter
/// where the taskbar sits.
pub fn menu_anchor(hwnd: win::HWND) -> (i32, i32, u32) {
    unsafe {
        let rc = icon_rect(hwnd);
        let mut pt: win::POINT = Default::default();
        win::GetCursorPos(&mut pt);
        let (cx, cy) = match rc {
            Some(r) => ((r.left + r.right) / 2, (r.top + r.bottom) / 2),
            None => (pt.x, pt.y),
        };
        let sw = win::GetSystemMetrics(win::SM_CXSCREEN).max(1);
        let sh = win::GetSystemMetrics(win::SM_CYSCREEN).max(1);
        let mut flags = 0;
        let x = if cx > sw / 2 {
            flags |= win::TPM_RIGHTALIGN; // grow leftward
            rc.map_or(pt.x, |r| r.right)
        } else {
            rc.map_or(pt.x, |r| r.left)
        };
        let y = if cy > sh / 2 {
            flags |= win::TPM_BOTTOMALIGN; // grow upward, off the taskbar
            rc.map_or(pt.y - 12, |r| r.top - 6)
        } else {
            rc.map_or(pt.y + 12, |r| r.bottom + 6)
        };
        (x, y, flags)
    }
}

/// Returns the selected command id, or 0 when dismissed.
pub fn show_menu(hwnd: win::HWND, hotkey_text: &str, autostart: bool) -> i32 {
    unsafe {
        let menu = win::CreatePopupMenu();
        if menu.is_null() { return 0; }
        let shot = win::wide(&format!("截图  ({})", hotkey_text));
        win::AppendMenuW(menu, win::MF_STRING, 10, shot.as_ptr());
        win::AppendMenuW(menu, win::MF_SEPARATOR, 0, std::ptr::null());
        for (id, text) in [(5usize, "导出快捷方式…"), (6, "导入快捷方式…")] {
            let t = win::wide(text);
            win::AppendMenuW(menu, win::MF_STRING, id, t.as_ptr());
        }
        win::AppendMenuW(menu, win::MF_SEPARATOR, 0, std::ptr::null());
        let t = win::wide("重新注册全局热键（双击 Ctrl 失效时）");
        win::AppendMenuW(menu, win::MF_STRING, 7, t.as_ptr());
        win::AppendMenuW(menu, win::MF_SEPARATOR, 0, std::ptr::null());
        let t = win::wide("开机自动启动");
        win::AppendMenuW(menu, win::MF_STRING | if autostart { win::MF_CHECKED } else { 0 }, 8, t.as_ptr());
        win::AppendMenuW(menu, win::MF_SEPARATOR, 0, std::ptr::null());
        let t = win::wide("关于 TT酱…");
        win::AppendMenuW(menu, win::MF_STRING, 11, t.as_ptr());
        win::AppendMenuW(menu, win::MF_SEPARATOR, 0, std::ptr::null());
        let t = win::wide("退出");
        win::AppendMenuW(menu, win::MF_STRING, 9, t.as_ptr());

        let (x, y, align) = menu_anchor(hwnd);
        win::SetForegroundWindow(hwnd);
        let cmd = win::TrackPopupMenu(menu,
            win::TPM_RETURNCMD | win::TPM_RIGHTBUTTON | win::TPM_NONOTIFY | align,
            x, y, 0, hwnd, std::ptr::null());
        win::PostMessageW(hwnd, win::WM_NULL, 0, 0);
        win::DestroyMenu(menu);
        cmd as i32
    }
}

// ---- HKCU auto-start -----------------------------------------------------
const HKEY_CURRENT_USER: *mut c_void = 0x8000_0001usize as *mut c_void;
const KEY_READ: u32 = 0x0002_0019;
const KEY_WRITE: u32 = 0x0002_0006;
const REG_SZ: u32 = 1;

pub fn autostart_enabled() -> bool {
    unsafe {
        let sub = win::wide("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
        let name = win::wide("TTJiang");
        let mut key: *mut c_void = std::ptr::null_mut();
        if win::RegOpenKeyExW(HKEY_CURRENT_USER, sub.as_ptr(), 0, KEY_READ, &mut key) != 0 {
            return false;
        }
        let mut buf = [0u8; 2048];
        let mut len = 2048u32;
        let ok = win::RegQueryValueExW(key, name.as_ptr(), std::ptr::null_mut(),
            std::ptr::null_mut(), buf.as_mut_ptr(), &mut len);
        win::RegCloseKey(key);
        ok == 0 && len > 2
    }
}

pub fn set_autostart(on: bool) {
    unsafe {
        let sub = win::wide("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
        let name = win::wide("TTJiang");
        let mut key: *mut c_void = std::ptr::null_mut();
        if win::RegOpenKeyExW(HKEY_CURRENT_USER, sub.as_ptr(), 0, KEY_WRITE, &mut key) != 0 {
            return;
        }
        if on {
            let value = win::wide(&format!("\"{}\"", win::exe_path()));
            win::RegSetValueExW(key, name.as_ptr(), 0, REG_SZ,
                value.as_ptr() as *const u8, (value.len() * 2) as u32);
        } else {
            win::RegDeleteValueW(key, name.as_ptr());
        }
        win::RegCloseKey(key);
    }
}

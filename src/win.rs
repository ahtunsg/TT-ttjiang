// Hand-written Win32 bindings. No windows-sys / winapi crate: the whole
// resident process is one Rust binary with zero third-party dependencies,
// which keeps the deployment contract identical to the old net48 build
#![allow(non_snake_case)]

use std::collections::HashMap;
use std::ffi::c_void;
use std::fs::OpenOptions;
use std::io::Write;
use std::sync::{Mutex, OnceLock};

use crate::types::Rect;

pub type HWND = *mut c_void;
pub type HDC = *mut c_void;
pub type HGDIOBJ = *mut c_void;
pub type HBITMAP = *mut c_void;
pub type HICON = *mut c_void;
pub type HCURSOR = *mut c_void;
pub type HINSTANCE = *mut c_void;
pub type HMENU = *mut c_void;
pub type HBRUSH = *mut c_void;
pub type HPEN = *mut c_void;
pub type HFONT = *mut c_void;
pub type HANDLE = *mut c_void;
pub type HGLOBAL = *mut c_void;
pub type WPARAM = usize;
pub type LPARAM = isize;
pub type LRESULT = isize;
pub type WNDPROC = Option<unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT>;
pub type HOOKPROC = Option<unsafe extern "system" fn(i32, WPARAM, LPARAM) -> LRESULT>;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct POINT { pub x: i32, pub y: i32 }

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct RECT { pub left: i32, pub top: i32, pub right: i32, pub bottom: i32 }

/// Answer for WM_GETMINMAXINFO. Reporting the same value for the min and max
/// tracking size is how a window that must not be resized (Aero Snap included)
/// states its one and only size.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct MINMAXINFO {
    pub ptReserved: POINT,
    pub ptMaxSize: POINT,
    pub ptMaxPosition: POINT,
    pub ptMinTrackSize: POINT,
    pub ptMaxTrackSize: POINT,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct MONITORINFO {
    pub cbSize: u32,
    pub rcMonitor: RECT,
    pub rcWork: RECT,
    pub dwFlags: u32,
}

#[repr(C)]
pub struct MSG {
    pub hwnd: HWND,
    pub message: u32,
    pub wParam: WPARAM,
    pub lParam: LPARAM,
    pub time: u32,
    pub pt: POINT,
    pub lPrivate: u32,
}
impl Default for MSG {
    fn default() -> Self { unsafe { std::mem::zeroed() } }
}

#[repr(C)]
pub struct WNDCLASSEXW {
    pub cbSize: u32,
    pub style: u32,
    pub lpfnWndProc: WNDPROC,
    pub cbClsExtra: i32,
    pub cbWndExtra: i32,
    pub hInstance: HINSTANCE,
    pub hIcon: HICON,
    pub hCursor: HCURSOR,
    pub hbrBackground: HBRUSH,
    pub lpszMenuName: *const u16,
    pub lpszClassName: *const u16,
    pub hIconSm: HICON,
}

#[repr(C)]
pub struct BITMAPINFOHEADER {
    pub biSize: u32,
    pub biWidth: i32,
    pub biHeight: i32,
    pub biPlanes: u16,
    pub biBitCount: u16,
    pub biCompression: u32,
    pub biSizeImage: u32,
    pub biXPelsPerMeter: i32,
    pub biYPelsPerMeter: i32,
    pub biClrUsed: u32,
    pub biClrImportant: u32,
}

#[repr(C)]
pub struct PAINTSTRUCT {
    pub hdc: HDC,
    pub fErase: i32,
    pub rcPaint: RECT,
    pub fRestore: i32,
    pub fIncUpdate: i32,
    pub rgbReserved: [u8; 32],
}
impl Default for PAINTSTRUCT {
    fn default() -> Self { unsafe { std::mem::zeroed() } }
}

/// Only the TME_LEAVE shape is used, so the struct stays in its 32-bit form.
#[repr(C)]
pub struct TRACKMOUSEEVENT {
    pub cbSize: u32,
    pub dwFlags: u32,
    pub hwndTrack: HWND,
    pub dwHoverTime: u32,
}

#[repr(C)]
pub struct Guid { pub d1: u32, pub d2: u16, pub d3: u16, pub d4: [u8; 8] }
impl Default for Guid {
    fn default() -> Self { Guid { d1: 0, d2: 0, d3: 0, d4: [0; 8] } }
}

#[repr(C)]
pub struct NOTIFYICONDATAW {
    pub cbSize: u32,
    pub hwnd: HWND,
    pub uID: u32,
    pub uFlags: u32,
    pub uCallbackMessage: u32,
    pub hIcon: HICON,
    pub szTip: [u16; 128],
    pub dwState: u32,
    pub dwStateMask: u32,
    pub szInfo: [u16; 256],
    pub uVersionOrTimeout: u32,
    pub szInfoTitle: [u16; 64],
    pub dwInfoFlags: u32,
    pub guidItem: Guid,
    pub hBalloonIcon: HICON,
}
impl Default for NOTIFYICONDATAW {
    fn default() -> Self { unsafe { std::mem::zeroed() } }
}

/// Identifies one tray icon so the shell can report where it actually sits.
/// Needed to hang the context menu off the icon instead of the cursor: the
/// cursor sits on the icon's face, so anchoring the menu there pushes it down
/// against the taskbar.
#[repr(C)]
pub struct NOTIFYICONIDENTIFIER {
    pub cbSize: u32,
    pub hWnd: HWND,
    pub uID: u32,
    pub guidItem: Guid,
}
impl Default for NOTIFYICONIDENTIFIER {
    fn default() -> Self { unsafe { std::mem::zeroed() } }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct BLENDFUNCTION {
    pub BlendOp: u8,
    pub BlendFlags: u8,
    pub SourceConstantAlpha: u8,
    pub AlphaFormat: u8,
}

#[repr(C)]
pub struct KBDLLHOOKSTRUCT {
    pub vkCode: u32,
    pub scanCode: u32,
    pub flags: u32,
    pub time: u32,
    pub dwExtraInfo: usize,
}

/// SHFILEINFO from shellapi.h. Field order and the two inline string buffers
/// must match exactly: SHGetFileInfoW writes 340 wide chars past the header.
#[repr(C)]
pub struct SHFILEINFO {
    pub hIcon: HICON,
    pub iIcon: i32,
    pub dwAttributes: u32,
    pub szDisplayName: [u16; 260],
    pub szTypeName: [u16; 80],
}
impl Default for SHFILEINFO {
    fn default() -> Self { unsafe { std::mem::zeroed() } }
}

// ---- constants -----------------------------------------------------------
pub const WS_POPUP: u32 = 0x8000_0000;
pub const WS_VISIBLE: u32 = 0x1000_0000;
/// Caption + system menu + a minimise box, but no WS_THICKFRAME: the launcher
/// derives its size from the screen, so a draggable edge would only break the
/// fixed 8x4 grid.
pub const WS_CAPTION: u32 = 0x00C0_0000;
pub const WS_SYSMENU: u32 = 0x0008_0000;
pub const WS_MINIMIZEBOX: u32 = 0x0002_0000;
pub const WS_EX_TOPMOST: u32 = 0x0000_0008;
pub const WS_EX_TOOLWINDOW: u32 = 0x0000_0080;
pub const WS_EX_NOACTIVATE: u32 = 0x0800_0000;
pub const WS_CHILD: u32 = 0x4000_0000;
/// Keeps the parent's off-screen blit out of the child control's rectangle.
/// Without it the search field is painted over on every frame and flickers.
pub const WS_CLIPCHILDREN: u32 = 0x0200_0000;
pub const ES_LEFT: u32 = 0x0000;
pub const ES_AUTOHSCROLL: u32 = 0x0080;
pub const SWP_NOSIZE: u32 = 0x0001;
pub const SWP_NOMOVE: u32 = 0x0002;
pub const SWP_NOZORDER: u32 = 0x0004;
pub const SWP_NOACTIVATE: u32 = 0x0010;
pub const SWP_SHOWWINDOW: u32 = 0x0040;
pub const SW_HIDE: i32 = 0;
pub const SW_SHOW: i32 = 5;
pub const SW_SHOWNOACTIVATE: i32 = 4;
pub const SW_RESTORE: i32 = 9;
pub const SRCCOPY: u32 = 0x00CC_0020;
pub const CAPTUREBLT: u32 = 0x4000_0000;
pub const SM_XVIRTUALSCREEN: i32 = 76;
pub const SM_YVIRTUALSCREEN: i32 = 77;
pub const SM_CXVIRTUALSCREEN: i32 = 78;
pub const SM_CYVIRTUALSCREEN: i32 = 79;
pub const SM_CXSCREEN: i32 = 0;
pub const SM_CYSCREEN: i32 = 1;
pub const SM_CYCAPTION: i32 = 4;
pub const SPI_GETWORKAREA: u32 = 0x0030;
// Z-order pseudo-handles. Cast through isize because they are negative, which
// a plain `as` on a constant expression would refuse.
pub const HWND_TOPMOST: HWND = -1isize as HWND;
pub const HWND_NOTOPMOST: HWND = -2isize as HWND;
pub const MONITOR_DEFAULTTOPRIMARY: u32 = 0x0000_0001;
pub const WM_PAINT: u32 = 0x000F;
pub const WM_ERASEBKGND: u32 = 0x0014;
pub const WM_DESTROY: u32 = 0x0002;
pub const WM_LBUTTONDOWN: u32 = 0x0201;
pub const WM_LBUTTONUP: u32 = 0x0202;
pub const WM_LBUTTONDBLCLK: u32 = 0x0203;
pub const WM_RBUTTONDOWN: u32 = 0x0204;
pub const WM_RBUTTONUP: u32 = 0x0205;
pub const WM_MOUSEMOVE: u32 = 0x0200;
pub const WM_MOUSELEAVE: u32 = 0x02A3;
pub const WM_CAPTURECHANGED: u32 = 0x0215;
pub const TME_LEAVE: u32 = 0x0000_0002;
pub const WM_MOUSEWHEEL: u32 = 0x020A;
pub const WM_SETCURSOR: u32 = 0x0020;
pub const IDC_HAND: usize = 32649;
pub const IDC_ARROW: usize = 32512;
pub const IDC_IBEAM: usize = 32513;
pub const IDC_SIZEALL: usize = 32646;
pub const HTCLIENT: i32 = 1;
pub const WM_KEYDOWN: u32 = 0x0100;
pub const WM_KEYUP: u32 = 0x0101;
/// A translated character, sent to the focused control.
pub const WM_CHAR: u32 = 0x0102;
pub const WM_SYSKEYDOWN: u32 = 0x0104;
pub const WM_TIMER: u32 = 0x0113;
pub const WM_SYSCOMMAND: u32 = 0x0112;
pub const WM_GETMINMAXINFO: u32 = 0x0024;
/// Low four bits of the wParam are reserved by the system; mask them off
/// before comparing a system command.
pub const SC_MASK: usize = 0xFFF0;
pub const SC_MAXIMIZE: usize = 0xF030;
pub const WM_HOTKEY: u32 = 0x0312;
pub const WM_CONTEXTMENU: u32 = 0x007B;
pub const WM_DPICHANGED: u32 = 0x02E0;
pub const WM_APP: u32 = 0x8000;
pub const WM_COMMAND: u32 = 0x0111;
pub const WM_SETFONT: u32 = 0x0030;
pub const WM_SETFOCUS: u32 = 0x0007;
pub const WM_CTLCOLOREDIT: u32 = 0x0133;
/// `WM_COMMAND` notification: the text of an EDIT control changed.
pub const EN_CHANGE: u32 = 0x0300;
pub const EM_SETSEL: u32 = 0x00B1;
/// Paste the clipboard into an EDIT; what Ctrl+V reaches once translated.
pub const WM_PASTE: u32 = 0x0302;
pub const EM_SETMARGINS: u32 = 0x00D3;
pub const EC_LEFTMARGIN: usize = 0x0001;
pub const EC_RIGHTMARGIN: usize = 0x0002;
/// Vista and later; the placeholder text an empty field shows.
pub const EM_SETCUEBANNER: u32 = 0x1501;
pub const WM_CLOSE: u32 = 0x0010;
pub const WM_SETICON: u32 = 0x0080;
pub const ICON_SMALL: usize = 0;
pub const ICON_BIG: usize = 1;
pub const WM_NULL: u32 = 0x0000;
pub const WM_APP_TRAY: u32 = WM_APP + 1;
pub const WM_APP_OCR_DONE: u32 = WM_APP + 2;
pub const MOD_ALT: u32 = 1;
pub const MOD_CONTROL: u32 = 2;
pub const MOD_SHIFT: u32 = 4;
pub const MOD_WIN: u32 = 8;
pub const VK_1: u32 = 0x31;
pub const VK_ESCAPE: u32 = 0x1B;
pub const VK_BACK: u32 = 0x08;
pub const VK_RETURN: u32 = 0x0D;
pub const VK_CONTROL: u32 = 0x11;
pub const VK_MENU: u32 = 0x12;
pub const VK_F4: u32 = 0x73;
pub const KEYEVENTF_KEYUP: u32 = 0x0002;
pub const VK_LCONTROL: u32 = 0xA2;
pub const VK_RCONTROL: u32 = 0xA3;
pub const VK_F1: u32 = 0x70;
pub const VK_F24: u32 = 0x87;
pub const IDC_CROSS: usize = 32515;
pub const IDI_APPLICATION: usize = 32512;
pub const GWLP_USERDATA: i32 = -21;
pub const GWLP_WNDPROC: i32 = -4;
pub const GMEM_MOVEABLE: u32 = 0x0002;
pub const GMEM_ZEROINIT: u32 = 0x0040;
pub const CF_UNICODETEXT: u32 = 13;
/// `GetStockObject` index for the process-wide white brush; no handle to free.
pub const WHITE_BRUSH: i32 = 0;
pub const CF_DIB: u32 = 8;
pub const NIM_ADD: u32 = 0;
pub const NIM_MODIFY: u32 = 1;
pub const NIM_DELETE: u32 = 2;
pub const NIF_MESSAGE: u32 = 1;
pub const NIF_ICON: u32 = 2;
pub const NIF_TIP: u32 = 4;
pub const NIF_INFO: u32 = 0x10;
pub const NIIF_INFO: u32 = 1;
pub const MF_STRING: u32 = 0;
pub const MF_SEPARATOR: u32 = 0x800;
pub const MF_CHECKED: u32 = 8;
pub const TPM_RETURNCMD: u32 = 0x0100;
pub const TPM_RIGHTBUTTON: u32 = 0x0002;
pub const TPM_NONOTIFY: u32 = 0x0010;
pub const TPM_RIGHTALIGN: u32 = 0x0008;
pub const TPM_BOTTOMALIGN: u32 = 0x0020;
pub const WH_KEYBOARD_LL: i32 = 13;
pub const HC_ACTION: i32 = 0;
pub const ERROR_ALREADY_EXISTS: u32 = 183;
pub const TRANSPARENT: i32 = 1;
pub const DT_CENTER: u32 = 1;
pub const DT_RIGHT: u32 = 2;
pub const DT_VCENTER: u32 = 4;
pub const DT_SINGLELINE: u32 = 0x20;
pub const DT_NOCLIP: u32 = 0x100;
pub const DT_NOPREFIX: u32 = 0x0800;
pub const DT_END_ELLIPSIS: u32 = 0x8000;
pub const INVALID_HANDLE_VALUE: isize = -1;

// ---- imports -------------------------------------------------------------
#[link(name = "kernel32")]
extern "system" {
    pub fn GetModuleHandleW(name: *const u16) -> HINSTANCE;
    pub fn GetModuleFileNameW(inst: HINSTANCE, buf: *mut u16, len: u32) -> u32;
    pub fn GetLastError() -> u32;
    pub fn SetDllDirectoryW(path: *const u16) -> i32;
    pub fn CreateMutexW(attrs: *mut c_void, initial: i32, name: *const u16) -> HANDLE;
    pub fn ReleaseMutex(h: HANDLE) -> i32;
    pub fn CloseHandle(h: HANDLE) -> i32;
    pub fn CreateEventW(attrs: *mut c_void, manual: i32, initial: i32, name: *const u16) -> HANDLE;
    pub fn SetEvent(h: HANDLE) -> i32;
    pub fn WaitForSingleObject(h: HANDLE, ms: u32) -> u32;
    pub fn GlobalAlloc(flags: u32, bytes: usize) -> HGLOBAL;
    pub fn GlobalLock(h: HGLOBAL) -> *mut c_void;
    pub fn GlobalUnlock(h: HGLOBAL) -> i32;
    pub fn GlobalFree(h: HGLOBAL) -> *mut c_void;
    pub fn Sleep(ms: u32);
    pub fn GetTickCount64() -> u64;
    pub fn GetEnvironmentVariableW(name: *const u16, buf: *mut u16, len: u32) -> u32;
    pub fn SetProcessDPIAware() -> i32;
    pub fn GetCurrentThreadId() -> u32;
    pub fn CreateDirectoryW(path: *const u16, attrs: *mut c_void) -> i32;
    pub fn GetFileAttributesW(path: *const u16) -> u32;
    pub fn CreateProcessW(
        app: *const u16, cmd: *mut u16, pa: *mut c_void, ta: *mut c_void,
        inherit: i32, flags: u32, env: *mut c_void, dir: *const u16,
        si: *mut c_void, pi: *mut c_void,
    ) -> i32;
}

#[link(name = "user32")]
extern "system" {
    pub fn RegisterClassExW(cls: *const WNDCLASSEXW) -> u16;
    pub fn CreateWindowExW(
        ex: u32, cls: *const u16, title: *const u16, style: u32,
        x: i32, y: i32, w: i32, h: i32,
        parent: HWND, menu: HMENU, inst: HINSTANCE, param: *mut c_void,
    ) -> HWND;
    pub fn DestroyWindow(h: HWND) -> i32;
    pub fn ShowWindow(h: HWND, cmd: i32) -> i32;
    pub fn SetForegroundWindow(h: HWND) -> i32;
    pub fn SetFocus(h: HWND) -> HWND;
    pub fn GetFocus() -> HWND;
    pub fn DefWindowProcW(h: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT;
    pub fn GetMessageW(msg: *mut MSG, h: HWND, min: u32, max: u32) -> i32;
    pub fn PeekMessageW(msg: *mut MSG, h: HWND, min: u32, max: u32, remove: u32) -> i32;
    pub fn TranslateMessage(msg: *const MSG) -> i32;
    pub fn DispatchMessageW(msg: *const MSG) -> LRESULT;
    pub fn PostQuitMessage(code: i32);
    pub fn PostMessageW(h: HWND, msg: u32, w: WPARAM, l: LPARAM) -> i32;
    pub fn SendMessageW(h: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT;
    pub fn BeginPaint(h: HWND, ps: *mut PAINTSTRUCT) -> HDC;
    pub fn EndPaint(h: HWND, ps: *const PAINTSTRUCT) -> i32;
    pub fn GetDC(h: HWND) -> HDC;
    pub fn ReleaseDC(h: HWND, dc: HDC) -> i32;
    pub fn InvalidateRect(h: HWND, rc: *const RECT, erase: i32) -> i32;
    pub fn UpdateWindow(h: HWND) -> i32;
    pub fn GetCursorPos(p: *mut POINT) -> i32;
    pub fn TrackMouseEvent(tme: *mut TRACKMOUSEEVENT) -> i32;
    pub fn SetCursor(h: HCURSOR) -> HCURSOR;
    pub fn LoadCursorW(inst: HINSTANCE, id: *const u16) -> HCURSOR;
    pub fn SetCapture(h: HWND) -> HWND;
    pub fn ReleaseCapture() -> i32;
    pub fn RegisterHotKey(h: HWND, id: i32, mods: u32, vk: u32) -> i32;
    pub fn UnregisterHotKey(h: HWND, id: i32) -> i32;
    pub fn RegisterWindowMessageW(name: *const u16) -> u32;
    pub fn SetTimer(h: HWND, id: usize, ms: u32, proc: *mut c_void) -> usize;
    pub fn KillTimer(h: HWND, id: usize) -> i32;
    pub fn GetSystemMetrics(index: i32) -> i32;
    pub fn SystemParametersInfoW(action: u32, p1: u32, p2: *mut c_void, p3: u32) -> i32;
    pub fn GetClientRect(h: HWND, rc: *mut RECT) -> i32;
    pub fn FillRect(dc: HDC, rc: *const RECT, brush: HBRUSH) -> i32;
    pub fn SetWindowPos(h: HWND, after: HWND, x: i32, y: i32, w: i32, h: i32, flags: u32) -> i32;
    pub fn GetWindowRect(h: HWND, rc: *mut RECT) -> i32;
    pub fn AdjustWindowRectEx(rc: *mut RECT, style: u32, menu: i32, ex: u32) -> i32;
    pub fn PrintWindow(h: HWND, dc: HDC, flags: u32) -> i32;
    pub fn MonitorFromPoint(pt: POINT, flags: u32) -> *mut c_void;
    pub fn MonitorFromWindow(h: HWND, flags: u32) -> *mut c_void;
    pub fn GetMonitorInfoW(mon: *mut c_void, mi: *mut MONITORINFO) -> i32;
    pub fn BringWindowToTop(h: HWND) -> i32;
    pub fn SetActiveWindow(h: HWND) -> HWND;
    pub fn GetActiveWindow() -> HWND;
    pub fn IsZoomed(h: HWND) -> i32;
    pub fn AttachThreadInput(attach: u32, to: u32, on: i32) -> i32;
    pub fn GetWindowThreadProcessId(h: HWND, pid: *mut u32) -> u32;
    pub fn TrackPopupMenu(menu: HMENU, flags: u32, x: i32, y: i32, reserved: i32, h: HWND, rc: *const RECT) -> i32;
    pub fn CreatePopupMenu() -> HMENU;
    pub fn AppendMenuW(menu: HMENU, flags: u32, id: usize, text: *const u16) -> i32;
    pub fn DestroyMenu(menu: HMENU) -> i32;
    pub fn MessageBoxW(h: HWND, text: *const u16, caption: *const u16, flags: u32) -> i32;
    pub fn LoadIconW(inst: HINSTANCE, id: *const u16) -> HICON;
    pub fn CreateIconFromResourceEx(bits: *mut u8, size: u32, icon: i32, version: u32, cx: i32, cy: i32, flags: u32) -> HICON;
    pub fn DestroyIcon(h: HICON) -> i32;
    pub fn SetWindowsHookExW(id: i32, proc: HOOKPROC, inst: HINSTANCE, thread: u32) -> HANDLE;
    pub fn UnhookWindowsHookEx(h: HANDLE) -> i32;
    pub fn CallNextHookEx(h: HANDLE, code: i32, w: WPARAM, l: LPARAM) -> LRESULT;
    pub fn GetForegroundWindow() -> HWND;
    pub fn SetWindowLongPtrW(h: HWND, index: i32, value: isize) -> isize;
    pub fn GetWindowLongPtrW(h: HWND, index: i32) -> isize;
    pub fn CallWindowProcW(old: WNDPROC, h: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT;
    pub fn SetWindowTextW(h: HWND, text: *const u16) -> i32;
    pub fn GetWindowTextW(h: HWND, buf: *mut u16, max: i32) -> i32;
    pub fn GetWindowTextLengthW(h: HWND) -> i32;
    pub fn IsWindow(h: HWND) -> i32;
    pub fn GetParent(h: HWND) -> HWND;
    pub fn OpenClipboard(h: HWND) -> i32;
    pub fn CloseClipboard() -> i32;
    pub fn EmptyClipboard() -> i32;
    pub fn SetClipboardData(fmt: u32, data: *mut c_void) -> *mut c_void;
    pub fn IsWindowVisible(h: HWND) -> i32;
    pub fn IsChild(parent: HWND, h: HWND) -> i32;
    pub fn IsIconic(h: HWND) -> i32;
    pub fn SetCursorPos(x: i32, y: i32) -> i32;
    pub fn ScreenToClient(h: HWND, p: *mut POINT) -> i32;
    pub fn SetProcessDpiAwarenessContext(value: *mut c_void) -> i32;
    pub fn GetKeyState(vk: i32) -> i16;
    pub fn keybd_event(vk: u8, scan: u8, flags: u32, extra: usize);
    pub fn PrivateExtractIconsW(file: *const u16, index: i32, cx: i32, cy: i32,
                                icons: *mut HICON, ids: *mut u32, count: u32, flags: u32) -> u32;
    // Icon resources. Reading the group icon first is the only way to learn
    // which frame sizes a file really carries: asking PrivateExtractIcons for
    // a size that is not there just returns a stretched (blurry) copy.
    pub fn LoadLibraryExW(file: *const u16, h: HWND, flags: u32) -> HMODULE;
    pub fn FreeLibrary(m: HMODULE) -> i32;
    pub fn FindResourceW(m: HMODULE, name: *const u16, kind: *const u16) -> HRSRC;
    pub fn LoadResource(m: HMODULE, r: HRSRC) -> HGLOBAL;
    pub fn LockResource(h: HGLOBAL) -> *mut c_void;
    pub fn SizeofResource(m: HMODULE, r: HRSRC) -> u32;
    pub fn EnumResourceNamesW(m: HMODULE, kind: *const u16,
                              cb: Option<unsafe extern "system" fn(HMODULE, *const u16, *const u16, isize) -> i32>,
                              lparam: isize) -> i32;
    pub fn LoadImageW(m: HMODULE, name: *const u16, kind: u32,
                      cx: i32, cy: i32, flags: u32) -> HANDLE;
}

pub type HMODULE = *mut c_void;
pub type HRSRC = *mut c_void;

/// `LoadLibraryEx` flag: map the file as resource data only, never run it.
pub const LOAD_LIBRARY_AS_DATAFILE: u32 = 0x0000_0002;
/// `LoadImage` "type" for icons.
pub const IMAGE_ICON: u32 = 1;
/// `FindResourceW` name for resource type 14 (RT_GROUP_ICON).
pub const RT_GROUP_ICON: usize = 14;
/// `FindResourceW` name for resource type 3 (RT_ICON).
pub const RT_ICON: usize = 3;
/// `LoadImage` flag: keep the frame's own pixels, no monochrome conversion.
pub const LR_DEFAULTCOLOR: u32 = 0;

#[link(name = "gdi32")]
extern "system" {
    pub fn CreateCompatibleDC(dc: HDC) -> HDC;
    pub fn CreateDIBSection(dc: HDC, bmi: *const BITMAPINFOHEADER, usage: u32, bits: *mut *mut c_void, section: *mut c_void, offset: u32) -> HBITMAP;
    pub fn SelectObject(dc: HDC, obj: HGDIOBJ) -> HGDIOBJ;
    pub fn DeleteObject(obj: HGDIOBJ) -> i32;
    pub fn DeleteDC(dc: HDC) -> i32;
    pub fn BitBlt(dst: HDC, x: i32, y: i32, w: i32, h: i32, src: HDC, sx: i32, sy: i32, op: u32) -> i32;
    pub fn CreateSolidBrush(color: u32) -> HBRUSH;
    pub fn CreatePen(style: i32, width: i32, color: u32) -> HPEN;
    pub fn Rectangle(dc: HDC, l: i32, t: i32, r: i32, b: i32) -> i32;
    pub fn RoundRect(dc: HDC, l: i32, t: i32, r: i32, b: i32, rw: i32, rh: i32) -> i32;
    pub fn SetBkMode(dc: HDC, mode: i32) -> i32;
    pub fn SetBkColor(dc: HDC, color: u32) -> u32;
    pub fn SetTextColor(dc: HDC, color: u32) -> u32;
    pub fn TextOutW(dc: HDC, x: i32, y: i32, text: *const u16, len: i32) -> i32;
    pub fn DrawTextW(dc: HDC, text: *const u16, len: i32, rc: *mut RECT, flags: u32) -> i32;
    pub fn CreateFontW(h: i32, w: i32, esc: i32, orient: i32, weight: i32,
        italic: u32, underline: u32, strike: u32, charset: u32,
        out_prec: u32, clip_prec: u32, quality: u32, pitch: u32, face: *const u16) -> HFONT;
    pub fn GetStockObject(index: i32) -> HGDIOBJ;
    pub fn MoveToEx(dc: HDC, x: i32, y: i32, old: *mut POINT) -> i32;
    pub fn LineTo(dc: HDC, x: i32, y: i32) -> i32;
    pub fn GdiFlush() -> i32;
}

#[link(name = "shell32")]
extern "system" {
    pub fn Shell_NotifyIconW(action: u32, data: *mut NOTIFYICONDATAW) -> i32;
    pub fn Shell_NotifyIconGetRect(pii: *const NOTIFYICONIDENTIFIER, rc: *mut RECT) -> i32;
    pub fn ShellExecuteW(h: HWND, verb: *const u16, file: *const u16, args: *const u16, dir: *const u16, show: i32) -> *mut c_void;
    pub fn ExtractIconExW(file: *const u16, index: i32, large: *mut HICON, small: *mut HICON, count: u32) -> u32;
    pub fn SHGetFileInfoW(path: *const u16, attrs: u32, sfi: *mut SHFILEINFO, cb: u32, flags: u32) -> usize;
}

#[link(name = "msimg32")]
extern "system" {
    pub fn AlphaBlend(dst: HDC, x: i32, y: i32, w: i32, h: i32,
                      src: HDC, sx: i32, sy: i32, sw: i32, sh: i32, f: BLENDFUNCTION) -> i32;
}

#[link(name = "advapi32")]
extern "system" {
    pub fn RegOpenKeyExW(root: *mut c_void, sub: *const u16, opts: u32, access: u32, out: *mut *mut c_void) -> i32;
    pub fn RegSetValueExW(key: *mut c_void, name: *const u16, reserved: u32, ty: u32, data: *const u8, len: u32) -> i32;
    pub fn RegQueryValueExW(key: *mut c_void, name: *const u16, reserved: *mut u32, ty: *mut u32, data: *mut u8, len: *mut u32) -> i32;
    pub fn RegDeleteValueW(key: *mut c_void, name: *const u16) -> i32;
    pub fn RegCloseKey(key: *mut c_void) -> i32;
}

// ---- registry reads shared by auto-start and the browser lookup -----------
pub const HKEY_CURRENT_USER_KEY: *mut c_void = 0x8000_0001usize as *mut c_void;
pub const HKEY_CLASSES_ROOT_KEY: *mut c_void = 0x8000_0000usize as *mut c_void;
pub const KEY_READ_ACCESS: u32 = 0x0002_0019;

/// Read a REG_SZ (or REG_EXPAND_SZ, taken verbatim) value.
pub fn reg_read_string(root: *mut c_void, sub: &str, name: &str) -> Option<String> {
    unsafe {
        let subw = wide(sub);
        let namew = wide(name);
        let mut key: *mut c_void = std::ptr::null_mut();
        if RegOpenKeyExW(root, subw.as_ptr(), 0, KEY_READ_ACCESS, &mut key) != 0 {
            return None;
        }
        let mut buf = vec![0u16; 1024];
        let mut bytes = (buf.len() * 2) as u32;
        let ok = RegQueryValueExW(key, namew.as_ptr(), std::ptr::null_mut(),
            std::ptr::null_mut(), buf.as_mut_ptr() as *mut u8, &mut bytes);
        RegCloseKey(key);
        if ok != 0 { return None; }
        let n = (bytes as usize / 2).saturating_sub(1).min(buf.len());
        let s = String::from_utf16_lossy(&buf[..n]).trim().to_string();
        if s.is_empty() { None } else { Some(s) }
    }
}

/// `HKCR\<progid>\shell\open\command`, tried in both 64- and 32-bit views:
/// an Opera or Firefox install that only wrote the 32-bit view is still found.
pub fn reg_read_progid_command(progid: &str) -> Option<String> {
    const KEY_WOW64_64KEY: u32 = 0x0100;
    const KEY_WOW64_32KEY: u32 = 0x0200;
    let sub = wide(&format!("{}\\shell\\open\\command", progid));
    for view in [KEY_WOW64_64KEY | KEY_READ_ACCESS, KEY_WOW64_32KEY | KEY_READ_ACCESS] {
        let got = unsafe {
            let mut key: *mut c_void = std::ptr::null_mut();
            if RegOpenKeyExW(HKEY_CLASSES_ROOT_KEY, sub.as_ptr(), 0, view, &mut key) != 0 {
                continue;
            }
            let mut buf = vec![0u16; 1024];
            let mut bytes = (buf.len() * 2) as u32;
            let ok = RegQueryValueExW(key, std::ptr::null(), std::ptr::null_mut(),
                std::ptr::null_mut(), buf.as_mut_ptr() as *mut u8, &mut bytes);
            RegCloseKey(key);
            if ok != 0 { continue; }
            let n = (bytes as usize / 2).saturating_sub(1).min(buf.len());
            String::from_utf16_lossy(&buf[..n]).trim().to_string()
        };
        if !got.is_empty() { return Some(got); }
    }
    None
}

// ---- helpers -------------------------------------------------------------
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub unsafe fn wide_ptr(v: &Vec<u16>) -> *const u16 {
    v.as_ptr()
}

pub unsafe fn from_wide_ptr(p: *const u16) -> String {
    if p.is_null() { return String::new(); }
    let mut n = 0usize;
    while *p.add(n) != 0 { n += 1; }
    String::from_utf16_lossy(std::slice::from_raw_parts(p, n))
}

pub fn rgb(r: u8, g: u8, b: u8) -> u32 {
    (r as u32) | ((g as u32) << 8) | ((b as u32) << 16)
}

/// Current text of a window or control, as a `String`.
///
/// Two passes: the first sizes the buffer, the second copies. The text can grow
/// between the two calls, so a short read is retried rather than truncated.
pub fn window_text(h: HWND) -> String {
    if h.is_null() { return String::new(); }
    unsafe {
        for _ in 0..4 {
            let len = GetWindowTextLengthW(h);
            if len <= 0 { return String::new(); }
            let mut buf = vec![0u16; len as usize + 1];
            let got = GetWindowTextW(h, buf.as_mut_ptr(), buf.len() as i32);
            if got <= 0 { return String::new(); }
            if got < buf.len() as i32 {
                return String::from_utf16_lossy(&buf[..got as usize]);
            }
        }
    }
    String::new()
}

pub fn exe_path() -> String {
    unsafe {
        let mut buf = [0u16; 1024];
        let n = GetModuleFileNameW(std::ptr::null_mut(), buf.as_mut_ptr(), 1024);
        if n == 0 { return String::new(); }
        String::from_utf16_lossy(&buf[..n as usize])
    }
}

pub fn exe_dir() -> String {
    let p = exe_path();
    match p.rfind(['\\', '/']) {
        Some(i) => p[..i].to_string(),
        None => ".".to_string(),
    }
}

pub fn env(name: &str) -> String {
    unsafe {
        let n = wide(name);
        let mut buf = [0u16; 1024];
        let got = GetEnvironmentVariableW(n.as_ptr(), buf.as_mut_ptr(), 1024);
        if got == 0 || got >= 1024 { return String::new(); }
        String::from_utf16_lossy(&buf[..got as usize])
    }
}

pub fn virtual_screen() -> Rect {
    unsafe {
        let x = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let y = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let mut w = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let mut h = GetSystemMetrics(SM_CYVIRTUALSCREEN);
        if w <= 0 || h <= 0 {
            w = GetSystemMetrics(SM_CXSCREEN);
            h = GetSystemMetrics(SM_CYSCREEN);
            return Rect::new(0, 0, w.max(1), h.max(1));
        }
        Rect::new(x, y, w.max(1), h.max(1))
    }
}

pub fn work_area() -> Rect {
    unsafe {
        let mut r: RECT = Default::default();
        if SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut r as *mut _ as *mut c_void, 0) != 0 {
            return Rect::new(r.left, r.top, r.right - r.left, r.bottom - r.top);
        }
    }
    virtual_screen()
}

/// Usable rectangle of the primary monitor, in physical pixels.
///
/// `virtual_screen()` spans every monitor, so a window centred on it lands on
/// the seam between two displays — with a secondary screen on the left the
/// centre sits at a negative x and the window is split across the bezel. The
/// launcher and its dialogs therefore centre on the primary monitor only.
pub fn primary_work_area() -> Rect {
    unsafe {
        let mon = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        if !mon.is_null() {
            let mut mi = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            if GetMonitorInfoW(mon, &mut mi) != 0 {
                let (w, h) = (mi.rcWork.right - mi.rcWork.left, mi.rcWork.bottom - mi.rcWork.top);
                if w > 0 && h > 0 {
                    return Rect::new(mi.rcWork.left, mi.rcWork.top, w, h);
                }
            }
        }
    }
    // SPI_GETWORKAREA is also primary-only, so this fallback keeps the same
    // meaning rather than silently reverting to the whole desktop.
    work_area()
}

/// Top-left corner that centres a `w`x`h` window on the primary monitor and
/// keeps it wholly on that screen even when the window is larger than the work
/// area.
pub fn center_on_primary(w: i32, h: i32) -> (i32, i32) {
    center_on_primary_below(w, h, 0)
}

/// Same as `center_on_primary`, then shifted down by `pct` percent of the work
/// area height. The shift is clamped to the work area, so a window that already
/// fills the screen just stays put instead of hanging off the bottom edge.
pub fn center_on_primary_below(w: i32, h: i32, pct: i32) -> (i32, i32) {
    let wa = primary_work_area();
    let x = wa.x + ((wa.w - w) / 2).max(0);
    let centred = wa.y + ((wa.h - h) / 2).max(0);
    let lowest = wa.y + (wa.h - h).max(0);
    let y = (centred + wa.h * pct / 100).clamp(wa.y, lowest.max(wa.y));
    (x, y)
}

/// Re-centre an existing window on the primary monitor without touching its
/// size or z-order.
pub fn center_window(h: HWND) -> i32 {
    center_window_below(h, 0)
}

/// `center_window`, with the same downward shift as `center_on_primary_below`.
pub fn center_window_below(h: HWND, pct: i32) -> i32 {
    unsafe {
        let mut rc: RECT = Default::default();
        if GetWindowRect(h, &mut rc) == 0 { return 0; }
        let (w, hgt) = (rc.right - rc.left, rc.bottom - rc.top);
        if w <= 0 || hgt <= 0 { return 0; }
        let (x, y) = center_on_primary_below(w, hgt, pct);
        if x == rc.left && y == rc.top { return 0; }
        SetWindowPos(h, std::ptr::null_mut::<c_void>(), x, y, 0, 0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE)
    }
}

/// Put a window in front of everything and give it the keyboard.
///
/// A background process is routinely denied `SetForegroundWindow`, which is why
/// the panel occasionally came up behind the app the user was working in. The
/// topmost round-trip re-orders the window above every normal window without
/// flashing on screen, and attaching to the current foreground thread lifts the
/// foreground lock for the duration of the call so typing lands in our window
/// rather than the one we just covered.
pub fn raise_window(h: HWND) {
    unsafe {
        SetWindowPos(h, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        SetWindowPos(h, HWND_NOTOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        BringWindowToTop(h);
        take_foreground(h);
    }
}

/// Give a window the keyboard without touching its z-order.
///
/// A background process is denied `SetForegroundWindow`, and the screenshot
/// overlay is always created that way: the user presses Ctrl+1 while another
/// app owns the foreground, so a bare call leaves the overlay painted but deaf —
/// Esc and the action-bar hotkeys go to whatever is underneath. Attaching to the
/// foreground thread lifts the lock for the duration of the call, which is the
/// only documented way to take the keyboard without user input.
pub fn take_foreground(h: HWND) {
    unsafe {
        let fg = GetForegroundWindow();
        let me = GetCurrentThreadId();
        let other = if fg.is_null() { 0 } else { GetWindowThreadProcessId(fg, std::ptr::null_mut()) };
        if other != 0 && other != me {
            AttachThreadInput(me, other, 1);
            SetForegroundWindow(h);
            AttachThreadInput(me, other, 0);
        } else {
            SetForegroundWindow(h);
        }
        SetActiveWindow(h);
        SetFocus(h);
    }
}

/// Physical-pixel DPI awareness. All geometry in this program is in physical
/// pixels (GetSystemMetrics / GetCursorPos / BitBlt), so PMv2 is correct; the
/// old build declared it in a manifest but never handled WM_DPICHANGED.
pub fn set_dpi_awareness() {
    unsafe {
        // DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2 == (void*)-4
        if SetProcessDpiAwarenessContext(-4isize as *mut c_void) == 0 {
            SetProcessDPIAware();
        }
    }
}

pub fn log(line: &str) {
    let path = format!("{}\\ttjiang.log", exe_dir());
    if let Ok(meta) = std::fs::metadata(&path) {
        if meta.len() > 1024 * 1024 {
            let _ = std::fs::rename(&path, format!("{}\\ttjiang.old.log", exe_dir()));
        }
    }
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs()).unwrap_or(0);
        let _ = writeln!(f, "[{}] {}", ts, line);
    }
}

pub fn message_box(text: &str, caption: &str, flags: u32) -> i32 {
    message_box_result(std::ptr::null_mut(), text, caption, flags)
}

/// MessageBox with an owner window, used by dialogs so it stays modal to TT酱.
pub fn message_box_result(parent: HWND, text: &str, caption: &str, flags: u32) -> i32 {
    unsafe {
        let t = wide(text);
        let c = wide(caption);
        MessageBoxW(parent, t.as_ptr(), c.as_ptr(), flags)
    }
}

pub fn make_font(size: i32, bold: bool) -> HFONT {
    unsafe {
        let face = wide("Microsoft YaHei UI");
        CreateFontW(-size, 0, 0, 0, if bold { 700 } else { 400 }, 0, 0, 0,
            1, 0, 0, 2, 0, face.as_ptr())
    }
}

/// A paint font, created once per (size, bold) and kept for the process.
///
/// `CreateFontW` runs a font match every call, and the tile grid used to ask
/// for one per tile per repaint — sixty-odd matches for a single frame, and a
/// frame is produced for every hover change. The size set is tiny and fixed, so
/// the handles are cached instead. Callers must not delete what this returns.
pub fn font(size: i32, bold: bool) -> HFONT {
    static FONTS: OnceLock<Mutex<HashMap<(i32, bool), isize>>> = OnceLock::new();
    let cache = FONTS.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(m) = cache.lock() {
        if let Some(h) = m.get(&(size, bold)) { return *h as HFONT; }
    }
    let h = make_font(size, bold);
    if let Ok(mut m) = cache.lock() {
        m.insert((size, bold), h as isize);
    }
    h
}

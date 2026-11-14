// Native Win32 dialogs replacing the 3.3 WPF windows. No WPF, no COM UI
// framework, no crates.io dependency: the shortcut editor uses plain
// EDIT/BUTTON/STATIC children plus the standard common dialogs for
// file/folder/save/import picking, while Settings and About are painted by
// card.rs in the launcher's own visual language.

use std::ffi::c_void;
use std::sync::atomic::{AtomicIsize, Ordering};

use crate::card::{self, Card, Tone};
use crate::profile;
use crate::win;

pub const CLASS_DIALOG: &str = "TTJiangDialog4";
pub const CLASS_SETTINGS: &str = "TTJiangSettings4";

/// Dialog windows must NOT go through app.rs's wnd_proc: creating a window
/// synchronously dispatches WM_CREATE back into that proc, which would form a
/// second &mut App while menu_command() still owns one. This proc only
/// forwards control commands to the host's message loop.
pub const WM_APP_DIALOG_CMD: u32 = win::WM_APP + 5;

static DIALOG_HOST: AtomicIsize = AtomicIsize::new(0);

pub fn set_host(h: win::HWND) { DIALOG_HOST.store(h as isize, Ordering::SeqCst); }

const WM_CTLCOLORSTATIC: u32 = 0x0138;
const TRANSPARENT: i32 = 1;
const NULL_BRUSH: i32 = 5;

pub unsafe extern "system" fn dialog_proc(
    hwnd: win::HWND, msg: u32, w: win::WPARAM, l: win::LPARAM,
) -> win::LRESULT {
    // STATIC controls default to a COLOR_BTNFACE fill, which shows up as grey
    // stripes across the white dialog body. Paint their background transparent
    // so labels sit flush on the window.
    if msg == WM_CTLCOLORSTATIC {
        win::SetBkMode(w as win::HDC, TRANSPARENT);
        return win::GetStockObject(NULL_BRUSH) as win::LRESULT;
    }
    // Settings and About paint themselves; everything else in the dialog
    // family (the shortcut editor) keeps native controls.
    if card::handle(hwnd, msg, w, l) { return 0; }
    if msg == win::WM_COMMAND || msg == win::WM_CLOSE {
        // A BUTTON reports a press as BN_CLICKED, which is 0. An EDIT control
        // also sends WM_COMMAND, but puts an EN_* code in the high word; taking
        // only the low word as the id made those look like a button press, so
        // merely creating a text field fired whichever command shared its id
        // (it opened the export dialog twice, once per EN_UPDATE/EN_CHANGE).
        // Drop anything that is not a click before dispatching.
        if msg == win::WM_COMMAND && ((w >> 16) & 0xFFFF) != 0 {
            return 0;
        }
        let id = if msg == win::WM_CLOSE { ID_CANCEL } else { (w & 0xFFFF) as i32 };
        let host = DIALOG_HOST.load(Ordering::SeqCst) as win::HWND;
        if !host.is_null() {
            win::PostMessageW(host, WM_APP_DIALOG_CMD, id as usize, l);
        }
        return 0;
    }
    win::DefWindowProcW(hwnd, msg, w, l)
}

// Control ids, shared with the WM_COMMAND handler in app.rs.
// These must be unique: the handler dispatches on the id alone, so two
// controls sharing one id both fire the same command.
pub const ID_NAME: i32 = 101;
pub const ID_PATH: i32 = 102;
pub const ID_ARGS: i32 = 103;
pub const ID_PICK_FILE: i32 = 105;
pub const ID_PICK_FOLDER: i32 = 106;
pub const ID_OK: i32 = 107;
pub const ID_CANCEL: i32 = 108;
pub const ID_EXPORT: i32 = 109;
pub const ID_IMPORT: i32 = 110;
pub const ID_START_INDEX: i32 = 111;
/// Optional tile picture, and its browse/clear buttons. Kept above the
/// settings-window ids that used to occupy 109-111.
pub const ID_ICON: i32 = 113;
pub const ID_PICK_ICON: i32 = 114;
pub const ID_CLEAR_ICON: i32 = 115;

/// Project home page, shown in the About window.
pub const PROJECT_URL: &str = "https://gitee.com/ryansg/TT-ttjiang";

const WS_CHILD: u32 = 0x4000_0000;
const WS_VISIBLE: u32 = 0x1000_0000;
const WS_BORDER: u32 = 0x0080_0000;
const WS_TABSTOP: u32 = 0x0001_0000;
const ES_AUTOHSCROLL: u32 = 0x0080;
const BS_PUSHBUTTON: u32 = 0x0000;
const BS_DEFPUSHBUTTON: u32 = 0x0001;
const WM_SETFONT: u32 = 0x0030;
const WM_GETTEXTLENGTH: u32 = 0x000E;
const WM_GETTEXT: u32 = 0x000D;
const WM_SETTEXT: u32 = 0x000C;
const MB_OK: u32 = 0x0000;
const MB_YESNO: u32 = 0x0004;
const IDYES: i32 = 6;

#[link(name = "user32")]
extern "system" {
    fn GetWindowTextW(h: win::HWND, buf: *mut u16, max: i32) -> i32;
    fn GetWindowTextLengthW(h: win::HWND) -> i32;
    fn SetWindowTextW(h: win::HWND, text: *const u16) -> i32;
    fn EnableWindow(h: win::HWND, enable: i32) -> i32;
    fn GetParent(h: win::HWND) -> win::HWND;
}

#[link(name = "comdlg32")]
extern "system" {
    fn GetOpenFileNameW(ofn: *mut OPENFILENAMEW) -> i32;
    fn GetSaveFileNameW(ofn: *mut OPENFILENAMEW) -> i32;
}

#[link(name = "shell32")]
extern "system" {
    fn SHBrowseForFolderW(bi: *const BROWSEINFOW) -> *mut c_void;
    fn SHGetPathFromIDListW(pidl: *mut c_void, out: *mut u16) -> i32;
}

#[link(name = "ole32")]
extern "system" {
    fn CoInitializeEx(reserved: *mut c_void, coinit: u32) -> i32;
    fn CoUninitialize();
    fn CoTaskMemFree(p: *mut c_void);
}

#[repr(C)]
struct OPENFILENAMEW {
    l_struct_size: u32,
    hwnd_owner: win::HWND,
    h_instance: *mut c_void,
    lpstr_filter: *const u16,
    lpstr_custom_filter: *mut u16,
    n_max_cust_filter: u32,
    n_filter_index: u32,
    lpstr_file: *mut u16,
    n_max_file: u32,
    lpstr_file_title: *mut u16,
    n_max_file_title: u32,
    lpstr_initial_dir: *const u16,
    lpstr_title: *const u16,
    flags: u32,
    n_file_offset: u16,
    n_file_extension: u16,
    lpstr_def_ext: *const u16,
    l_cust_data: isize,
    lpfn_hook: *mut c_void,
    lp_template_name: *const u16,
    pv_reserved: *mut c_void,
    dw_reserved: u32,
    flags_ex: u32,
}

#[repr(C)]
struct BROWSEINFOW {
    hwnd_owner: win::HWND,
    pidl_root: *const c_void,
    display_name: *mut u16,
    title: *const u16,
    flags: u32,
    callback: *mut c_void,
    l_param: isize,
    image: i32,
}

const OFN_EXPLORER: u32 = 0x0008_0000;
const OFN_FILEMUSTEXIST: u32 = 0x0000_1000;
const OFN_PATHMUSTEXIST: u32 = 0x0000_0800;
const OFN_HIDEREADONLY: u32 = 0x0000_0004;
const OFN_OVERWRITEPROMPT: u32 = 0x0000_0002;
const OFN_NOCHANGEDIR: u32 = 0x0000_0008;
const BIF_RETURNONLYFSDIRS: u32 = 0x0001;
const BIF_EDITBOX: u32 = 0x0010;
const BIF_NEWDIALOGSTYLE: u32 = 0x0040;

fn ctl(parent: win::HWND, class: &str, text: &str, id: i32,
       x: i32, y: i32, w: i32, h: i32, extra: u32, font: win::HFONT) -> win::HWND {
    unsafe {
        let c = win::wide(class);
        let t = win::wide(text);
        let hwnd = win::CreateWindowExW(
            0, c.as_ptr(), t.as_ptr(), WS_CHILD | WS_VISIBLE | WS_TABSTOP | extra,
            x, y, w, h, parent, id as win::HMENU,
            win::GetModuleHandleW(std::ptr::null()), std::ptr::null_mut(),
        );
        if !hwnd.is_null() && !font.is_null() {
            win::SendMessageW(hwnd, WM_SETFONT, font as usize, 1);
        }
        hwnd
    }
}

fn label(parent: win::HWND, text: &str, x: i32, y: i32, w: i32, font: win::HFONT) -> win::HWND {
    unsafe {
        let c = win::wide("STATIC");
        let t = win::wide(text);
        let h = win::CreateWindowExW(0, c.as_ptr(), t.as_ptr(), WS_CHILD | WS_VISIBLE,
            x, y, w, 22, parent, 0 as win::HMENU,
            win::GetModuleHandleW(std::ptr::null()), std::ptr::null_mut());
        if !h.is_null() && !font.is_null() { win::SendMessageW(h, WM_SETFONT, font as usize, 1); }
        h
    }
}

pub fn control_text(h: win::HWND) -> String {
    if h.is_null() { return String::new(); }
    unsafe {
        let n = GetWindowTextLengthW(h);
        if n <= 0 { return String::new(); }
        let mut buf: Vec<u16> = vec![0u16; (n + 1) as usize];
        let got = GetWindowTextW(h, buf.as_mut_ptr(), buf.len() as i32);
        if got <= 0 { return String::new(); }
        String::from_utf16_lossy(&buf[..got as usize])
    }
}

pub fn set_control_text(h: win::HWND, text: &str) {
    if h.is_null() { return; }
    let t = win::wide(text);
    unsafe { SetWindowTextW(h, t.as_ptr()); }
}

/// Read the shortcut fields from control handles, so callers do not have to
/// hold an immutable borrow of the dialog while mutating app state.
pub fn read_shortcut(name: win::HWND, path: win::HWND, args: win::HWND,
                     icon: win::HWND, pos: i32) -> profile::Shortcut {
    profile::Shortcut {
        name: control_text(name).trim().to_string(),
        path: control_text(path).trim().to_string(),
        args: control_text(args).trim().to_string(),
        pos,
        icon: control_text(icon).trim().to_string(),
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum DialogKind { Add, Edit(usize), Settings, About }

pub struct Dialog {
    pub hwnd: win::HWND,
    pub kind: DialogKind,
    pub name: win::HWND,
    pub path: win::HWND,
    pub args: win::HWND,
    pub icon: win::HWND,
    /// Grid slot of the shortcut being edited, so saving does not reset it.
    /// -1 for a new shortcut.
    pub pos: i32,
    pub font: win::HFONT,
}

const CARD_W: u32 = 0x8000_0000 | 0x00C0_0000 | 0x0008_0000; // WS_POPUP|WS_CAPTION|WS_SYSMENU
const CARD_EX: u32 = 0x0000_0001; // WS_EX_DLGMODALFRAME

/// Windows the launcher paints itself. Their size below is the client area;
/// the frame is added by AdjustWindowRectEx.
pub(crate) fn card_client_size(kind: DialogKind) -> Option<(i32, i32)> {
    match kind {
        DialogKind::Settings => Some((560, 372)),
        DialogKind::About => Some((460, 236)),
        _ => None,
    }
}

/// Content of the two painted windows, laid out on the same 8px grid the
/// launcher uses: 12px card inset, 32px text margin, 34px controls.
pub(crate) fn build_card(kind: DialogKind, service_text: &str) -> Card {
    let mut c = Card::new();
    match kind {
        DialogKind::Settings => {
            c.text("索引服务", 32, 24, 14, true, Tone::Ink);
            c.text(service_text, 32, 50, 12, false, Tone::Dim);
            c.button(ID_START_INDEX, "启动内置索引服务", 32, 78, 180, 34, true);
            c.rule(136);
            c.text("搜索", 32, 148, 14, true, Tone::Ink);
            c.text("显示全部匹配，不设条数上限；滚动时按需加载。", 32, 174, 12, false, Tone::Dim);
            c.rule(206);
            c.text("快捷方式", 32, 218, 14, true, Tone::Ink);
            c.button(ID_EXPORT, "导出…", 32, 246, 104, 34, false);
            c.button(ID_IMPORT, "导入…", 148, 246, 104, 34, false);
            // Footer: a rule keeps Close from floating in empty space and
            // matches how the launcher separates its status line.
            c.rule(296);
            c.button(ID_CANCEL, "关闭", 424, 310, 104, 34, false);
        }
        _ => {
            // Cargo forces three components to stay semver-legal, but a
            // trailing ".0" reads as noise where the version is checked.
            let ver = format!("TT酱 {}", env!("CARGO_PKG_VERSION").trim_end_matches(".0"));
            c.text(&ver, 32, 22, 20, true, Tone::Ink);
            c.text("常驻托盘的快速启动面板", 32, 54, 12, false, Tone::Dim);
            c.rule(82);
            c.text("双击 Ctrl 唤起窗口 · Ctrl+1 截图识别文字与表格", 32, 94, 12, false, Tone::Ink);
            c.text("项目源", 32, 120, 12, false, Tone::Dim);
            c.text(PROJECT_URL, 86, 120, 12, false, Tone::Accent);
            c.text("MIT License © 2026 ahtunsg", 32, 152, 11, false, Tone::Faint);
            c.button(ID_CANCEL, "关闭", 324, 168, 104, 34, true);
        }
    }
    c
}

impl Dialog {
    pub fn create(parent: win::HWND, kind: DialogKind, class: &[u16],
                  initial: Option<&profile::Shortcut>, service_text: &str) -> Result<Dialog, String> {
        let font = win::make_font(14, false);
        // Editing keeps the tile's slot; without this, saving an edited
        // shortcut sends it to the first free cell and it jumps.
        let pos = initial.map(|s| s.pos).unwrap_or(-1);
        let title = match kind {
            DialogKind::Settings => "设置",
            DialogKind::About => "关于 TT酱",
            _ => "快捷方式",
        };
        // The painted windows are sized by their client area; the shortcut
        // editor keeps the 3.3 outer size so its control grid stays put.
        let (w, h) = match card_client_size(kind) {
            Some((cw, ch)) => unsafe {
                let mut rc = win::RECT { left: 0, top: 0, right: cw, bottom: ch };
                win::AdjustWindowRectEx(&mut rc, CARD_W, 0, CARD_EX);
                (rc.right - rc.left, rc.bottom - rc.top)
            },
            // Taller than 3.3's editor: a hint line saying the path box also
            // takes a web address, plus an optional picture row, because a
            // link's site icon cannot be read from a browser cache.
            None => (556, 344),
        };
        unsafe {
            let t = win::wide(title);
            // Primary monitor, same as the panel. Centring on the virtual
            // desktop threw these windows onto the seam between displays and
            // left part of them off-screen.
            let (dx, dy) = win::center_on_primary(w, h);
            let hwnd = win::CreateWindowExW(
                CARD_EX,
                class.as_ptr(), t.as_ptr(),
                CARD_W,
                dx, dy, w, h,
                parent, std::ptr::null_mut(),
                win::GetModuleHandleW(std::ptr::null()), std::ptr::null_mut(),
            );
            if hwnd.is_null() {
                return Err(format!("创建窗口失败 err={}", win::GetLastError()));
            }
            let mut d = Dialog {
                hwnd, kind, name: std::ptr::null_mut(), path: std::ptr::null_mut(),
                args: std::ptr::null_mut(), icon: std::ptr::null_mut(), pos, font,
            };
            if card_client_size(kind).is_some() {
                card::attach(build_card(kind, service_text), hwnd);
            } else {
                d.build_shortcut(initial);
            }
            win::ShowWindow(hwnd, win::SW_SHOW);
            // The parent may be hidden when the dialog is opened from the tray;
            // an owned window of a hidden owner is easy for the shell to leave
            // behind, so raise it explicitly instead of trusting activation.
            win::raise_window(hwnd);
            Ok(d)
        }
    }

    unsafe fn build_shortcut(&mut self, initial: Option<&profile::Shortcut>) {
        let p = self.hwnd;
        let f = self.font;
        label(p, "名称", 24, 24, 60, f);
        self.name = ctl(p, "EDIT", "", ID_NAME, 90, 20, 430, 26, WS_BORDER | ES_AUTOHSCROLL, f);
        // One box takes both a program path and a web address: a URL in here is
        // recognised on save and opens in the default browser, so the editor
        // needs no type switch — only a hint that the box accepts more than
        // files. The label stays "路径" so the field keeps its familiar name.
        label(p, "路径", 24, 62, 60, f);
        self.path = ctl(p, "EDIT", "", ID_PATH, 90, 58, 300, 26, WS_BORDER | ES_AUTOHSCROLL, f);
        ctl(p, "BUTTON", "文件", ID_PICK_FILE, 396, 58, 58, 26, BS_PUSHBUTTON, f);
        ctl(p, "BUTTON", "目录", ID_PICK_FOLDER, 460, 58, 60, 26, BS_PUSHBUTTON, f);
        label(p, "参数", 24, 100, 60, f);
        self.args = ctl(p, "EDIT", "", ID_ARGS, 90, 96, 430, 26, WS_BORDER | ES_AUTOHSCROLL, f);
        // Optional picture. Web links have no icon on disk to read and a
        // browser's favicon cache is a locked database, so this box is the only
        // way a link tile shows a real logo instead of its domain initial.
        label(p, "图标", 24, 138, 60, f);
        self.icon = ctl(p, "EDIT", "", ID_ICON, 90, 134, 300, 26, WS_BORDER | ES_AUTOHSCROLL, f);
        ctl(p, "BUTTON", "图片", ID_PICK_ICON, 396, 134, 58, 26, BS_PUSHBUTTON, f);
        ctl(p, "BUTTON", "清除", ID_CLEAR_ICON, 460, 134, 60, 26, BS_PUSHBUTTON, f);
        ctl(p, "BUTTON", "确定", ID_OK, 336, 212, 88, 32, BS_DEFPUSHBUTTON, f);
        ctl(p, "BUTTON", "取消", ID_CANCEL, 434, 212, 88, 32, BS_PUSHBUTTON, f);
        // One line, deliberately short: a STATIC is one line tall here, so
        // anything wider than the box is silently clipped mid-sentence.
        label(p, "提示：路径也能填网址；取不到网站图标时显示首字母，可用「图片」自选",
            24, 165, 510, f);
        if let Some(s) = initial {
            set_control_text(self.name, &s.name);
            set_control_text(self.path, &s.path);
            set_control_text(self.args, &s.args);
            set_control_text(self.icon, &s.icon);
        }
        win::SetFocus(self.name);
    }

    pub fn read(&self) -> profile::Shortcut {
        read_shortcut(self.name, self.path, self.args, self.icon, self.pos)
    }

    pub fn close(&mut self) {
        unsafe {
            if !self.hwnd.is_null() { win::DestroyWindow(self.hwnd); self.hwnd = std::ptr::null_mut(); }
            if !self.font.is_null() { win::DeleteObject(self.font); self.font = std::ptr::null_mut(); }
        }
    }
}

fn com_guard() -> bool {
    // Common dialogs need an STA apartment on this thread. The main UI thread
    // is already pumping messages; initialising here is idempotent enough for
    // our purposes and S_FALSE still means COM is usable.
    unsafe { CoInitializeEx(std::ptr::null_mut(), 2) >= 0 }
}

fn com_done(initialized: bool) {
    if initialized { unsafe { CoUninitialize(); } }
}

fn ofn(owner: win::HWND, file_buf: &mut Vec<u16>, title: &[u16], filter: &[u16],
       def_ext: &[u16], flags: u32) -> OPENFILENAMEW {
    OPENFILENAMEW {
        l_struct_size: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwnd_owner: owner,
        h_instance: std::ptr::null_mut(),
        lpstr_filter: filter.as_ptr(),
        lpstr_custom_filter: std::ptr::null_mut(),
        n_max_cust_filter: 0,
        n_filter_index: 1,
        lpstr_file: file_buf.as_mut_ptr(),
        n_max_file: file_buf.len() as u32,
        lpstr_file_title: std::ptr::null_mut(),
        n_max_file_title: 0,
        lpstr_initial_dir: std::ptr::null(),
        lpstr_title: title.as_ptr(),
        flags: OFN_EXPLORER | OFN_HIDEREADONLY | OFN_NOCHANGEDIR | flags,
        n_file_offset: 0,
        n_file_extension: 0,
        lpstr_def_ext: def_ext.as_ptr(),
        l_cust_data: 0,
        lpfn_hook: std::ptr::null_mut(),
        lp_template_name: std::ptr::null(),
        pv_reserved: std::ptr::null_mut(),
        dw_reserved: 0,
        flags_ex: 0,
    }
}

fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

pub fn pick_file(owner: win::HWND) -> Option<String> {
    let ok = com_guard();
    let filter: Vec<u16> = "所有文件\0*.*\0\0".encode_utf16().collect();
    let title: Vec<u16> = "选择文件\0".encode_utf16().collect();
    let ext: Vec<u16> = "\0".encode_utf16().collect();
    let mut buf: Vec<u16> = vec![0u16; 32768];
    let mut o = ofn(owner, &mut buf, &title, &filter, &ext,
        OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST);
    let r = unsafe { GetOpenFileNameW(&mut o) };
    com_done(ok);
    if r == 0 { None } else { Some(from_wide(&buf)) }
}

pub fn pick_folder(owner: win::HWND) -> Option<String> {
    let ok = com_guard();
    let title: Vec<u16> = "选择文件夹\0".encode_utf16().collect();
    let mut display: Vec<u16> = vec![0u16; 260];
    let bi = BROWSEINFOW {
        hwnd_owner: owner,
        pidl_root: std::ptr::null(),
        display_name: display.as_mut_ptr(),
        title: title.as_ptr(),
        flags: BIF_RETURNONLYFSDIRS | BIF_EDITBOX | BIF_NEWDIALOGSTYLE,
        callback: std::ptr::null_mut(),
        l_param: 0,
        image: 0,
    };
    let pidl = unsafe { SHBrowseForFolderW(&bi) };
    let out = if pidl.is_null() {
        None
    } else {
        let mut path: Vec<u16> = vec![0u16; 32768];
        let r = unsafe { SHGetPathFromIDListW(pidl, path.as_mut_ptr()) };
        unsafe { CoTaskMemFree(pidl); }
        if r != 0 { Some(from_wide(&path)) } else { None }
    };
    com_done(ok);
    out
}

pub fn pick_save(owner: win::HWND, default_name: &str) -> Option<String> {
    let ok = com_guard();
    let filter: Vec<u16> = "快捷方式配置 (*.json)\0*.json\0所有文件 (*.*)\0*.*\0\0".encode_utf16().collect();
    let title: Vec<u16> = "导出快捷方式\0".encode_utf16().collect();
    let ext: Vec<u16> = "json\0".encode_utf16().collect();
    let mut buf: Vec<u16> = vec![0u16; 32768];
    let init: Vec<u16> = default_name.encode_utf16().collect();
    for (i, &c) in init.iter().enumerate() {
        if i + 1 < buf.len() { buf[i] = c; }
    }
    let mut o = ofn(owner, &mut buf, &title, &filter, &ext,
        OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST);
    let r = unsafe { GetSaveFileNameW(&mut o) };
    com_done(ok);
    if r == 0 { None } else { Some(from_wide(&buf)) }
}

/// Save-as for a screenshot. PNG only — the format stays fixed — but the
/// dialog opens in the folder the previous shot went to, so a plain save is
/// still two clicks.
pub fn pick_png_save(owner: win::HWND, initial_dir: &str, default_name: &str) -> Option<String> {
    let ok = com_guard();
    let filter: Vec<u16> = "PNG 图片 (*.png)\0*.png\0\0".encode_utf16().collect();
    let title: Vec<u16> = "保存截图\0".encode_utf16().collect();
    let ext: Vec<u16> = "png\0".encode_utf16().collect();
    let dir: Vec<u16> = format!("{}\0", initial_dir).encode_utf16().collect();
    let mut buf: Vec<u16> = vec![0u16; 32768];
    for (i, c) in default_name.encode_utf16().enumerate() {
        if i + 1 < buf.len() { buf[i] = c; }
    }
    let mut o = ofn(owner, &mut buf, &title, &filter, &ext,
        OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST);
    // Must outlive the call: the dialog reads the path while it runs.
    o.lpstr_initial_dir = dir.as_ptr();
    let r = unsafe { GetSaveFileNameW(&mut o) };
    com_done(ok);
    if r == 0 { None } else { Some(from_wide(&buf)) }
}

pub fn pick_import(owner: win::HWND) -> Option<String> {
    let ok = com_guard();
    let filter: Vec<u16> = "快捷方式配置 (*.json)\0*.json\0所有文件 (*.*)\0*.*\0\0".encode_utf16().collect();
    let title: Vec<u16> = "导入快捷方式\0".encode_utf16().collect();
    let ext: Vec<u16> = "json\0".encode_utf16().collect();
    let mut buf: Vec<u16> = vec![0u16; 32768];
    let mut o = ofn(owner, &mut buf, &title, &filter, &ext,
        OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST);
    let r = unsafe { GetOpenFileNameW(&mut o) };
    com_done(ok);
    if r == 0 { None } else { Some(from_wide(&buf)) }
}

pub fn confirm(parent: win::HWND, text: &str, caption: &str) -> bool {
    win::message_box_result(parent, text, caption, MB_YESNO) == IDYES
}

pub fn info(parent: win::HWND, text: &str, caption: &str) {
    win::message_box_result(parent, text, caption, MB_OK);
}

pub fn set_enabled(h: win::HWND, on: bool) {
    if !h.is_null() { unsafe { EnableWindow(h, if on { 1 } else { 0 }); } }
}

pub fn parent_of(h: win::HWND) -> win::HWND {
    unsafe { GetParent(h) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every control id must be distinct. `app::on_dialog_command` dispatches
    /// on the id alone, so a duplicate silently wires one control to another
    /// command — a collision on 109 once made creating the icon field open the
    /// export dialog, twice, because an EDIT sends EN_UPDATE and EN_CHANGE.
    #[test]
    fn control_ids_are_unique() {
        let ids = [
            ("name", ID_NAME), ("path", ID_PATH), ("args", ID_ARGS),
            ("pick_file", ID_PICK_FILE),
            ("pick_folder", ID_PICK_FOLDER), ("ok", ID_OK), ("cancel", ID_CANCEL),
            ("export", ID_EXPORT), ("import", ID_IMPORT),
            ("start_index", ID_START_INDEX),
            ("icon", ID_ICON), ("pick_icon", ID_PICK_ICON), ("clear_icon", ID_CLEAR_ICON),
        ];
        for (i, (a, x)) in ids.iter().enumerate() {
            for (b, y) in ids.iter().skip(i + 1) {
                assert_ne!(x, y, "{} and {} share id {}", a, b, x);
            }
        }
    }
}

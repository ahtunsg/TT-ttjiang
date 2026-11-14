// Application shell: single instance, message loop, tray, hotkeys and the
// screenshot/OCR orchestration. All windows share one WNDPROC and are
// dispatched by HWND; there is no WPF dispatcher or mixed-rendering pump.

use std::cell::Cell;
use std::sync::atomic::{AtomicIsize, AtomicU64, Ordering};

use crate::capture;
use crate::config::Config;
use crate::dialogs::{self, Dialog};
use crate::everything;
use crate::image::{self, Image};
use crate::overlay::{self, ActionBar, Overlay, Toast};
use crate::paddle::{self, PaddleOcr};
use crate::profile::{self, Shortcut};
use crate::searchbox::{self, Field as SearchField};
use crate::tray;
use crate::types::{OcrResult, Rect};
use crate::win;
use crate::layout;

const TIMER_HOTKEY_RETRY: usize = 2;
const TIMER_TRAY_RETRY: usize = 3;
const TIMER_CAPTURE_DELAY: usize = 4;
const TIMER_SEARCH_DEBOUNCE: usize = 5;
const WM_APP_ACTIVATE: u32 = win::WM_APP + 3;
const WM_APP_SEARCH_DONE: u32 = win::WM_APP + 4;
const WM_APP_CANCEL_CAPTURE: u32 = win::WM_APP + 6;
// Upper bound for the search box. The field shows roughly 30 characters, so a
// paste longer than this is a mis-aimed Ctrl+V rather than a query worth
// running against the index.
const SEARCH_MAX_CHARS: usize = 200;
const WS_OVERLAPPEDWINDOW: u32 = 0x00CF_0000;
const IDC_ARROW: usize = 32512;

static HOST_HWND: AtomicIsize = AtomicIsize::new(0);
static LAST_CTRL: AtomicU64 = AtomicU64::new(0);
static CTRL_TICK: AtomicU64 = AtomicU64::new(0);

thread_local! {
    static APP_PTR: Cell<*mut App> = Cell::new(std::ptr::null_mut());
}

/// Borrow the live `App` for the window/control procedures that receive no
/// context pointer. The pointer is only set while the message loop is running,
/// and those procedures are called from it, so no `&mut App` is live.
pub(crate) fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP_PTR.with(|p| {
        let ptr = p.get();
        if ptr.is_null() { None } else { Some(f(unsafe { &mut *ptr })) }
    })
}

pub struct SearchDone {
    pub seq: u64,
    pub paths: Vec<String>,
    pub error: Option<String>,
}

pub struct App {
    pub host: win::HWND,
    pub main: win::HWND,
    /// The search field. A real EDIT child: it owns the caret, selection, IME
    /// composition and paste, and reports every edit through `EN_CHANGE`.
    pub search: SearchField,
    pub overlay: Option<Overlay>,
    pub bar: Option<ActionBar>,
    pub toast: Option<Toast>,
    pub cls_host: Vec<u16>,
    pub cls_main: Vec<u16>,
    pub cls_overlay: Vec<u16>,
    pub cls_bar: Vec<u16>,
    pub cls_toast: Vec<u16>,
    pub icon: win::HICON,
    pub taskbar_msg: u32,
    pub tray_added: bool,
    pub tray_retries: u32,
    pub hotkey_ok: bool,
    pub hook: win::HANDLE,
    pub config: Config,
    pub screen: Option<Image>,
    pub selection: Rect,
    pub vs: Rect,
    pub ocr: *mut PaddleOcr,
    pub ocr_busy: bool,
    pub pending_action: i32,
    pub shortcuts: Vec<Shortcut>,
    pub settings: profile::Settings,
    pub query: String,
    /// Set while the field text is being rewritten from `EN_CHANGE`, so the
    /// truncating `SetWindowText` does not re-enter the handler.
    pub syncing_query: bool,
    pub search_hits: Vec<String>,
    pub search_error: Option<String>,
    pub search_busy: bool,
    pub search_seq: u64,
    pub dx_result: String,
    pub dialog: Option<Dialog>,
    pub cls_dialog: Vec<u16>,
    pub cls_settings: Vec<u16>,
    pub logo: Option<Image>,
    pub hover_tile: i32,
    pub hover_row: i32,
    /// The "+ 添加" cell is not a shortcut, so it needs its own hover flag.
    pub hover_add: bool,
    /// First visible result row; keeps long Everything lists reachable.
    pub scroll_rows: i32,
    /// Drag-to-reorder state for the shortcut grid. `drag_from` is -1 when
    /// idle. `drag_active` only flips past a small movement threshold, so a
    /// plain click still opens the shortcut instead of being read as a drag.
    pub drag_from: i32,
    pub drag_active: bool,
    pub drag_cur: (i32, i32),
    pub drag_offset: (i32, i32),
    /// True while a TME_LEAVE request is outstanding for the main window.
    pub tracking_leave: bool,
    pub balloon_shown: bool,
    pub quit: bool,
}

pub struct OcrDone {
    pub text: String,
    pub lines: i32,
    pub cols: i32,
    pub language: String,
    pub engine: String,
    pub error: Option<String>,
}

/// A panic used to end the process with nothing on screen: windows_subsystem
/// is "windows", there is no console, and panic=abort skips unwinding. Log the
/// message, show it once, then let the abort proceed.
fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let msg = info.payload();
        let text = if let Some(s) = msg.downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = msg.downcast_ref::<String>() {
            s.clone()
        } else {
            "未知错误".to_string()
        };
        let loc = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_default();
        win::log(&format!("panic at {}: {}", loc, text));
        win::message_box(
            &format!(
                "TT酱遇到内部错误，即将退出。\n\n{}
位置：{}\n\n日志：{}\nttjiang.log",
                text,
                loc,
                win::exe_dir()
            ),
            "TT酱 - 内部错误",
            0x10,
        );
    }));
}

pub fn run() {
    install_panic_hook();
    unsafe {
        win::set_dpi_awareness();
        let args: Vec<String> = std::env::args().skip(1).collect();
        if !args.is_empty() && args[0] == "--ocr-check" {
            let code = run_ocr_check(&args);
            std::process::exit(code);
        }
        if !args.is_empty() && args[0] == "--capture-smoke" {
            let code = run_capture_smoke(&args);
            std::process::exit(code);
        }
        if !args.is_empty() && args[0] == "--selftest" {
            let code = run_selftest(&args);
            std::process::exit(code);
        }

        if !args.is_empty() && args[0] == "--icon-dump" {
            // <path> <out.png> [size] — writes one shortcut icon so the
            // extraction/resample quality can be judged on its own instead of
            // through a 37px tile in a screenshot.
            let src = args.get(1).cloned().unwrap_or_default();
            let out = args.get(2).cloned().unwrap_or_else(|| "icon.png".into());
            let size: i32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(256);
            println!("{}", image::icon_source_report(&src, size));
            match image::icon_for_path(&src, size) {
                Some(img) => {
                    let code = match capture::save_png_as(&img, &out) { Ok(_) => 0, Err(_) => 1 };
                    std::process::exit(code);
                }
                None => std::process::exit(2),
            }
        }

        // Off-screen renderers never reach the tray or the hotkeys, so they
        // must not fight a running instance over the single-instance mutex —
        // that would silently wake the user's window and write no snapshot.
        let headless = args.first()
            .map(|a| a == "--ui-smoke" || a == "--overlay-shot" || a == "--about-shot"
                || a == "--dialog-shot" || a == "--tray-anchor" || a == "--show-test"
                || a == "--click-test" || a == "--dialog-screen" || a == "--panel-screen"
                || a == "--add-click-test" || a == "--icon-dump" || a == "--paste-test"
                || a == "--esc-test" || a == "--key-test" || a == "--field-shot")
            .unwrap_or(false);

        // Single instance, same mutex/event names as 3.3.
        let mname = win::wide("Local\\TTJiang_SingleInstance_v1");
        let mutex = if headless {
            std::ptr::null_mut()
        } else {
            win::CreateMutexW(std::ptr::null_mut(), 0, mname.as_ptr())
        };
        if !mutex.is_null() && win::GetLastError() == win::ERROR_ALREADY_EXISTS {
            let ename = win::wide("Local\\TTJiang_Activate_v1");
            let ev = win::CreateEventW(std::ptr::null_mut(), 0, 0, ename.as_ptr());
            for _ in 0..24 {
                if !ev.is_null() { win::SetEvent(ev); }
                win::Sleep(60);
            }
            if !ev.is_null() { win::CloseHandle(ev); }
            win::CloseHandle(mutex);
            return;
        }

        let proc: win::WNDPROC = Some(wnd_proc);
        let cls_host = register(overlay::CLASS_HOST, proc);
        let cls_main = register(overlay::CLASS_MAIN, proc);
        let cls_overlay = register(overlay::CLASS_OVERLAY, proc);
        let cls_bar = register(overlay::CLASS_BAR, proc);
        let cls_toast = register(overlay::CLASS_TOAST, proc);
        let dlg_proc: win::WNDPROC = Some(dialogs::dialog_proc);
        let cls_dialog = register(dialogs::CLASS_DIALOG, dlg_proc);
        let cls_settings = register(dialogs::CLASS_SETTINGS, dlg_proc);

        let inst = win::GetModuleHandleW(std::ptr::null());
        let title = win::wide("TT酱");
        let host = win::CreateWindowExW(0, cls_host.as_ptr(), title.as_ptr(), win::WS_POPUP,
            0, 0, 1, 1, std::ptr::null_mut(), std::ptr::null_mut(), inst, std::ptr::null_mut());
        if host.is_null() {
            win::message_box("无法创建消息窗口，TT酱将退出。", "TT酱", 0x10);
            return;
        }
        HOST_HWND.store(host as isize, Ordering::SeqCst);
        dialogs::set_host(host);

        // Centre on the primary monitor, not on the union of all monitors: with
        // a second screen attached the virtual desktop's centre sits on the
        // seam, so the panel used to open half on each display.
        //
        // The size is a fixed share of the primary work area and the frame has
        // no resizing border, so an 8x4 grid is always the same 8x4 grid.
        // CreateWindowExW wants the outer size; the painter works in client
        // coordinates, so convert once here instead of scattering the frame
        // width over the layout maths.
        let (cw, chh) = panel_client_size();
        let mut wrc = win::RECT { left: 0, top: 0, right: cw, bottom: chh };
        win::AdjustWindowRectEx(&mut wrc, PANEL_STYLE, 0, 0);
        let (mw, mh) = (wrc.right - wrc.left, wrc.bottom - wrc.top);
        let (mx, my) = win::center_on_primary_below(mw, mh, PANEL_DROP_PCT);
        let main = win::CreateWindowExW(0, cls_main.as_ptr(), title.as_ptr(), PANEL_STYLE,
            mx, my, mw, mh,
            std::ptr::null_mut(), std::ptr::null_mut(), inst, std::ptr::null_mut());

        // The search field is a real EDIT child of the panel. Its rectangle
        // comes from the same geometry the painter uses for the white box, so
        // the frame and the control cannot drift apart.
        let search = {
            let mut crc: win::RECT = Default::default();
            win::GetClientRect(main, &mut crc);
            let (l, t, r, _) = search_rect(&crc);
            let x = l + 48;
            // A single-line EDIT draws its text from the top of its client
            // area, it does not centre it. The control is therefore only as
            // tall as one line of 15px text, and placed where the painted
            // version centred it inside the 46px box.
            let y = t + 12;
            searchbox::create(main, inst, (x, y, (r - 14) - x, 22))
        };

        let icon = tray::make_icon();
        // Title bar, Alt+Tab and the taskbar button read WM_SETICON; without it
        // a class icon of null makes Windows fall back to the file icon, which
        // used to be blank. Set both sizes from the real ICO frames so they
        // stay sharp instead of being resampled by the shell.
        for (which, px) in [(win::ICON_BIG, 32), (win::ICON_SMALL, 16)] {
            let h = tray::icon_at(px);
            if !h.is_null() {
                win::SendMessageW(main, win::WM_SETICON, which, h as isize);
            }
        }
        // The 3.3 window shows the brand mark inside the search box. Take the
        // real 32px ICO frame instead of scaling the 256px one: GDI+ downscale
        // blurs the 1px outline that makes the logo readable at 30px.
        let logo = {
            let h = tray::icon_at(32);
            let img = if h.is_null() { None } else { Image::from_hicon(h, 30, 30) };
            if img.is_none() { win::log("品牌图标加载失败，标题区使用文字兜底"); }
            if !h.is_null() { win::DestroyIcon(h); }
            img
        };
        // Still needed for the screenshot path: grabbing has to cover every
        // monitor, unlike the launcher itself.
        let vs = win::virtual_screen();
        let (mut shortcuts, settings) = profile::load_all();
        normalize_slots(&mut shortcuts);
        let mut app = App {
            host, main,
            search,
            overlay: None, bar: None, toast: None,
            cls_host, cls_main, cls_overlay, cls_bar, cls_toast,
            cls_dialog, cls_settings,
            icon,
            taskbar_msg: win::RegisterWindowMessageW(win::wide("TaskbarCreated").as_ptr()),
            tray_added: false, tray_retries: 0,
            hotkey_ok: false,
            hook: std::ptr::null_mut(),
            config: Config::load(),
            screen: None, selection: Rect::new(0, 0, 0, 0), vs,
            ocr: std::ptr::null_mut(), ocr_busy: false, pending_action: -1,
            shortcuts, settings, query: String::new(), syncing_query: false,
            search_hits: Vec::new(), search_error: None,
            search_busy: false, search_seq: 0, dx_result: String::new(),
            dialog: None,
            logo,
            hover_tile: -1, hover_row: -1, hover_add: false, scroll_rows: 0,
            drag_from: -1, drag_active: false, drag_cur: (0, 0), drag_offset: (0, 0),
            tracking_leave: false,
            balloon_shown: false, quit: false,
        };

        APP_PTR.with(|p| p.set(&mut app as *mut App));
        win::log("TT酱 4.3 (Rust) 启动");
        win::log(&format!("内置文件搜索组件: {}", if everything::components_present() {
            "齐备".to_string()
        } else {
            "缺失（首次文件搜索会提示重新解压完整压缩包）".to_string()
        }));

        if !args.is_empty() && args[0] == "--ui-smoke" {
            let out = args.get(1).cloned().unwrap_or_else(|| "ui.png".into());
            // Optional extra arguments each switch a render state on, so a
            // combination such as "gaps hoveradd" is reviewable in one shot.
            for a in args.iter().skip(2) {
                if a.starts_with("rows") {
                    // rows / rows40: sample hits, plus an optional scroll
                    // offset as the fourth argument, so the wheel path is
                    // reviewable without a desktop.
                    let n: usize = a[4..].parse().unwrap_or(4);
                    app.query = "报告".into();
                    app.search_hits = (0..n)
                        .map(|i| {
                            format!(
                                "C://Users//Ryan//Documents//2026//季度经营报告_{:02}.docx",
                                i + 1
                            )
                        })
                        .collect();
                    if let Some(v) = args.get(3) {
                        app.scroll_rows = v.parse().unwrap_or(0);
                    }
                } else if a == "full" {
                    // 31 shortcuts: the board is full and the + must land on the
                    // last cell without pushing anything off the panel.
                    let names = ["记事本", "计算器", "画图", "终端", "设置", "任务管理器",
                                 "资源管理器", "命令提示符", "控制面板", "写字板", "截图工具",
                                 "系统信息", "注册表", "服务", "设备管理器", "磁盘管理",
                                 "事件查看器", "性能监视器", "远程桌面", "剪贴板", "时钟",
                                 "天气", "相机", "日历", "邮件", "音乐", "视频", "地图",
                                 "商店", "浏览器", "解压"];
                    app.shortcuts = names.iter().enumerate().map(|(i, n)| profile::Shortcut {
                        name: (*n).into(),
                        path: format!("C:\\Windows\\System32\\{}{}.exe", "app", i),
                        pos: -1,
                        ..Default::default()
                    }).collect();
                } else if a == "gaps" {
                    // gaps: free placement with holes on purpose — items pinned
                    // to slots 0/2/5/6, so the "drop anywhere, leave a gap"
                    // layout is reviewable without editing the real profile.
                    let mk = |name: &str, exe: &str, pos: i32| profile::Shortcut {
                        name: name.into(),
                        path: format!("C:\\Windows\\System32\\{}", exe),
                        pos,
                        ..Default::default()
                    };
                    app.shortcuts = vec![
                        mk("记事本", "notepad.exe", 0),
                        mk("计算器", "calc.exe", 2),
                        mk("画图", "mspaint.exe", 5),
                        mk("终端", "cmd.exe", 6),
                    ];
                } else if a == "hoveradd" {
                    // The + cell has its own hover state; review it the same way.
                    app.hover_add = true;
                } else if a.starts_with("hover") {
                    // hover1: paints one tile with its hover plate, so the
                    // state that changes on every mouse move is reviewable
                    // without a desktop.
                    app.hover_tile = a.get(5..).and_then(|v| v.parse().ok()).unwrap_or(0);
                } else if a.starts_with("drag") {
                    // drag1to4: renders a reorder in flight — source slot
                    // empty, drop target outlined, lifted tile under the
                    // cursor. Format: drag<from>to<to>.
                    let rest = &a[4..];
                    let (from_s, to_s) = rest.split_once("to").unwrap_or((rest, "0"));
                    let from: i32 = from_s.parse().unwrap_or(0);
                    let to: usize = to_s.parse().unwrap_or(0);
                    let mut rc: win::RECT = Default::default();
                    win::GetClientRect(app.main, &mut rc);
                    let (dl, dt) = tile_xy(&rc, to);
                    let (tw, th) = tile_size(&rc);
                    app.drag_from = from;
                    app.drag_active = from >= 0;
                    app.drag_offset = (tw / 2, th / 2);
                    app.drag_cur = (dl + tw / 2, dt + th / 2);
                }
            }
            let code = render_main_snapshot(&app, &out);
            std::process::exit(code);
        }

        if !args.is_empty() && args[0] == "--panel-screen" {
            // Real desktop pixels of the panel, frame included. The off-screen
            // renderer only knows the client area, so it cannot show whether
            // the fixed frame clips the board or whether the window really is
            // the size the screen asked for.
            let out = args.get(1).cloned().unwrap_or_else(|| "panel-screen.png".into());
            app.show_main();
            let mut m: win::MSG = Default::default();
            let t0 = std::time::Instant::now();
            while t0.elapsed() < std::time::Duration::from_millis(450) {
                while win::PeekMessageW(&mut m, std::ptr::null_mut(), 0, 0, 1) > 0 {
                    win::TranslateMessage(&m);
                    win::DispatchMessageW(&m);
                }
                win::Sleep(10);
            }
            let code = match capture::grab_virtual_screen() {
                Ok(shot) => {
                    let mut rc: win::RECT = Default::default();
                    win::GetWindowRect(app.main, &mut rc);
                    let vs = win::virtual_screen();
                    let r = Rect::new(rc.left - 8, rc.top - 8,
                        rc.right - rc.left + 16, rc.bottom - rc.top + 16);
                    let cropped = capture::crop(&shot, r, vs);
                    match capture::save_png_as(&cropped, &out) { Ok(_) => 0, Err(_) => 1 }
                }
                Err(_) => 1,
            };
            std::process::exit(code);
        }

        if !args.is_empty() && args[0] == "--show-test" {
            // Shows the real panel and reports whether it actually ended up in
            // front and where. The double-Ctrl path had no automated check at
            // all, which is how "sometimes it opens behind the other app" got
            // through.
            let out = args.get(1).cloned().unwrap_or_else(|| "show.json".into());
            app.show_main();
            let mut m: win::MSG = Default::default();
            let t0 = std::time::Instant::now();
            while t0.elapsed() < std::time::Duration::from_millis(400) {
                while win::PeekMessageW(&mut m, std::ptr::null_mut(), 0, 0, 1) > 0 {
                    win::TranslateMessage(&m);
                    win::DispatchMessageW(&m);
                }
                win::Sleep(10);
            }
            unsafe {
                let mut rc: win::RECT = Default::default();
                win::GetWindowRect(app.main, &mut rc);
                let fg = win::IsChild(app.main, win::GetForegroundWindow()) != 0
                    || win::GetForegroundWindow() == app.main;
                let body = format!(
                    "{{\n  \"foreground\": {},\n  \"visible\": {},\n  \"rect\": [{}, {}, {}, {}],\n  \
                     \"primaryWorkArea\": [{}, {}, {}, {}]\n}}\n",
                    fg, win::IsWindowVisible(app.main) != 0,
                    rc.left, rc.top, rc.right - rc.left, rc.bottom - rc.top,
                    win::primary_work_area().x, win::primary_work_area().y,
                    win::primary_work_area().w, win::primary_work_area().h);
                let _ = std::fs::write(&out, body);
            }
            std::process::exit(0);
        }

        if !args.is_empty() && args[0] == "--dialog-screen" {
            // PrintWindow fills the non-client frame with whatever the theme
            // compositor would have drawn, which is not what the user sees.
            // This grabs the real desktop pixels of the window's rectangle.
            let kind = args.get(1).cloned().unwrap_or_else(|| "settings".into());
            let out = args.get(2).cloned().unwrap_or_else(|| "screen.png".into());
            let (dkind, cls) = match kind.as_str() {
                "settings" => (dialogs::DialogKind::Settings, app.cls_settings.clone()),
                _ => (dialogs::DialogKind::About, app.cls_dialog.clone()),
            };
            let service = if dkind == dialogs::DialogKind::Settings {
                everything::service_state()
            } else {
                String::new()
            };
            let mut dlg = match Dialog::create(std::ptr::null_mut(), dkind, &cls, None, &service) {
                Ok(d) => d,
                Err(e) => { win::log(&format!("dialog screen 失败: {}", e)); std::process::exit(1); }
            };
            let mut m: win::MSG = Default::default();
            let t0 = std::time::Instant::now();
            while t0.elapsed() < std::time::Duration::from_millis(400) {
                while win::PeekMessageW(&mut m, std::ptr::null_mut(), 0, 0, 1) > 0 {
                    win::TranslateMessage(&m);
                    win::DispatchMessageW(&m);
                }
                win::Sleep(10);
            }
            let code = match capture::grab_virtual_screen() {
                Ok(shot) => {
                    let mut rc: win::RECT = Default::default();
                    unsafe { win::GetWindowRect(dlg.hwnd, &mut rc); }
                    let vs = win::virtual_screen();
                    let r = Rect::new(rc.left - 8, rc.top - 8,
                        rc.right - rc.left + 16, rc.bottom - rc.top + 16);
                    let cropped = capture::crop(&shot, r, vs);
                    match capture::save_png_as(&cropped, &out) {
                        Ok(_) => 0,
                        Err(_) => 1,
                    }
                }
                Err(_) => 1,
            };
            dlg.close();
            std::process::exit(code);
        }

        if !args.is_empty() && args[0] == "--add-click-test" {
            // The + is now a grid cell, not a footer button. Nothing else proves
            // that clicking that cell still opens the add dialog, and a stale
            // hit box would be invisible in a screenshot.
            let out = args.get(1).cloned().unwrap_or_else(|| "addclick.json".into());
            app.show_main();
            let mut m: win::MSG = Default::default();
            let pump = |ms: u64, m: &mut win::MSG| {
                let t0 = std::time::Instant::now();
                while t0.elapsed() < std::time::Duration::from_millis(ms) {
                    while win::PeekMessageW(m, std::ptr::null_mut(), 0, 0, 1) > 0 {
                        win::TranslateMessage(m);
                        win::DispatchMessageW(m);
                    }
                    win::Sleep(8);
                }
            };
            pump(200, &mut m);
            let mut cell = false;
            if let Some(r) = add_tile_rect(&app, &{
                let mut rc: win::RECT = Default::default();
                win::GetClientRect(app.main, &mut rc);
                rc
            }) {
                let (x, y) = ((r.0 + r.2) / 2, (r.1 + r.3) / 2);
                let lp = ((y as isize) << 16) | (x as isize & 0xFFFF);
                unsafe {
                    win::SendMessageW(app.main, win::WM_LBUTTONDOWN, 1, lp);
                    win::SendMessageW(app.main, win::WM_LBUTTONUP, 0, lp);
                }
                cell = true;
            }
            pump(260, &mut m);
            let dialog = app.dialog.is_some();
            let _ = std::fs::write(&out, format!(
                "{{\n  \"addCell\": {},\n  \"dialogOpened\": {}\n}}\n", cell, dialog));
            std::process::exit(if cell && dialog { 0 } else { 1 });
        }

        if !args.is_empty() && args[0] == "--paste-test" {
            // The paste itself is the EDIT class's own behaviour, so this sends
            // `WM_PASTE` to the focused window instead of injecting Ctrl+V: a
            // diagnostic process cannot reliably own the desktop foreground,
            // and an injected chord that misses lands in whatever is foreground
            // (the terminal, during a test run).
            let out = args.get(1).cloned().unwrap_or_else(|| "paste.json".into());
            let png = args.get(2).cloned();
            let sample = "C:\\Users\\Ryan\\Documents\\季度 报告.txt\r\n第二行";
            let wrote = capture::copy_text(sample).is_ok();
            app.show_main();
            let mut m: win::MSG = Default::default();
            let t0 = std::time::Instant::now();
            while t0.elapsed() < std::time::Duration::from_millis(220) {
                while win::PeekMessageW(&mut m, std::ptr::null_mut(), 0, 0, 1) > 0 {
                    win::TranslateMessage(&m);
                    win::DispatchMessageW(&m);
                }
                win::Sleep(8);
            }
            win::SendMessageW(win::GetFocus(), win::WM_PASTE, 0, 0);
            let got = app.query.clone();
            // A single-line EDIT keeps the first line of a multi-line paste and
            // drops the rest, which is the control's own rule; recorded here as
            // the expected value rather than assumed.
            let want = sample.split("\r\n").next().unwrap_or("").to_string();
            let ok = got == want;
            let _ = std::fs::write(&out, format!(
                "{{\n  \"clipboard\": {},\n  \"pasted\": {},\n  \"matches\": {},\n  \"got\": {:?},\n  \"want\": {:?}\n}}\n",
                wrote, !got.is_empty(), ok, got, want));
            if let Some(p) = png.as_deref() { capture_window_png(app.main, p); }
            std::process::exit(if ok { 0 } else { 1 });
        }

        if !args.is_empty() && args[0] == "--field-shot" {
            // The off-screen renderer cannot show a child control, so the empty
            // field — placeholder, caret, margins — has to be captured from the
            // real window.
            let out = args.get(1).cloned().unwrap_or_else(|| "field.png".into());
            app.show_main();
            let mut m: win::MSG = Default::default();
            let t0 = std::time::Instant::now();
            while t0.elapsed() < std::time::Duration::from_millis(320) {
                while win::PeekMessageW(&mut m, std::ptr::null_mut(), 0, 0, 1) > 0 {
                    win::TranslateMessage(&m);
                    win::DispatchMessageW(&m);
                }
                win::Sleep(8);
            }
            let code = capture_window_png(app.main, &out);
            std::process::exit(code);
        }

        if !args.is_empty() && args[0] == "--key-test" {
            // The field is a real control now, so the question is no longer
            // "did the character reach the query" but "did it reach the field":
            // typing, backspace, Esc-clears and Esc-closes all travel different
            // paths once a child owns the caret.
            let out = args.get(1).cloned().unwrap_or_else(|| "keys.json".into());
            let png = args.get(2).cloned();
            app.show_main();
            let mut m: win::MSG = Default::default();
            let pump = |ms: u64, m: &mut win::MSG| {
                let t0 = std::time::Instant::now();
                while t0.elapsed() < std::time::Duration::from_millis(ms) {
                    while win::PeekMessageW(m, std::ptr::null_mut(), 0, 0, 1) > 0 {
                        win::TranslateMessage(m);
                        win::DispatchMessageW(m);
                    }
                    win::Sleep(8);
                }
            };
            pump(220, &mut m);
            // Report, do not assume: a process started from a terminal may not
            // be allowed to take the foreground, and without it `GetFocus`
            // answers for the wrong queue.
            win::take_foreground(app.main);
            pump(120, &mut m);
            let (focus, fg) = (win::GetFocus(), win::GetForegroundWindow());
            let focused = focus == app.search.hwnd;
            let foreground = fg == app.main;

            // Characters go to whatever owns the caret, which is the point of
            // the test: sending them to `GetFocus()` rather than to the edit
            // handle would silently pass even with the routing broken.
            let type_str = |s: &str| {
                for ch in s.chars() {
                    win::SendMessageW(win::GetFocus(), win::WM_CHAR, ch as usize, 1);
                }
            };
            type_str("abc");
            pump(120, &mut m);
            let typed = searchbox::text(app.search.hwnd);
            let typed_query = app.query.clone();
            win::SendMessageW(win::GetFocus(), win::WM_CHAR, win::VK_BACK as usize, 1);
            pump(80, &mut m);
            let after_backspace = searchbox::text(app.search.hwnd);
            // Snapshot with text and caret in the field: that is the state the
            // old painted box could not produce at all.
            if let Some(p) = png.as_deref() { capture_window_png(app.main, p); }

            // Esc is posted to the focused window rather than injected: a
            // diagnostic process does not reliably own the desktop foreground,
            // so a real keypress can land in the terminal instead (that is what
            // `foreground` records). Delivery to the field is already proven by
            // the typing above; what is under test here is the handler.
            let press_esc = || {
                win::PostMessageW(win::GetFocus(), win::WM_KEYDOWN, win::VK_ESCAPE as usize, 0);
            };
            // First Esc empties the field, the panel stays up.
            press_esc();
            pump(160, &mut m);
            let cleared = searchbox::text(app.search.hwnd).is_empty();
            let still_open = win::IsWindowVisible(app.main) != 0;
            // Second Esc, on an empty field, collapses the panel.
            press_esc();
            pump(160, &mut m);
            let hidden = win::IsWindowVisible(app.main) == 0;

            // Reopening must hand back an empty field with the caret in it, the
            // state the hotkey path depends on.
            app.show_main();
            pump(200, &mut m);
            let reopened_empty = searchbox::text(app.search.hwnd).is_empty()
                && win::GetFocus() == app.search.hwnd;

            // Past the 200-character cap: the field, not the query struct, is
            // what has to be cut, otherwise the caret keeps running ahead of
            // the text that is being searched.
            type_str(&"x".repeat(SEARCH_MAX_CHARS + 10));
            pump(120, &mut m);
            let clamped = app.query.chars().count() == SEARCH_MAX_CHARS
                && searchbox::text(app.search.hwnd).chars().count() == SEARCH_MAX_CHARS;

            let ok = focused && typed == "abc" && typed_query == "abc"
                && after_backspace == "ab" && cleared && still_open && hidden
                && reopened_empty && clamped;
            let _ = std::fs::write(&out, format!(
                "{{\n  \"focusedField\": {},\n  \"foreground\": {},\n  \"typed\": {:?},\n  \"query\": {:?},\n  \"afterBackspace\": {:?},\n  \"escCleared\": {},\n  \"escKeptPanel\": {},\n  \"escHidPanel\": {},\n  \"reopenedEmpty\": {},\n  \"clampedTo200\": {}\n}}\n",
                focused, foreground, typed, typed_query, after_backspace, cleared, still_open, hidden,
                reopened_empty, clamped));
            std::process::exit(if ok { 0 } else { 1 });
        }

        if !args.is_empty() && args[0] == "--esc-test" {
            // Esc has to reach the overlay from a real keyboard, and the overlay
            // is created while some other app owns the foreground. Report both
            // facts instead of assuming the window took focus just because it
            // asked for it.
            let out = args.get(1).cloned().unwrap_or_else(|| "esc.json".into());
            // The real keyboard hook is installed further down, after these
            // diagnostics return, so install one here: the second round below
            // is exactly about the hook's fallback path.
            let _hook = unsafe {
                win::SetWindowsHookExW(win::WH_KEYBOARD_LL, Some(keyboard_proc),
                    win::GetModuleHandleW(std::ptr::null()), 0)
            };
            let hook_ok = !_hook.is_null();
            let mut m: win::MSG = Default::default();
            let pump = |ms: u64, m: &mut win::MSG| {
                let t0 = std::time::Instant::now();
                while t0.elapsed() < std::time::Duration::from_millis(ms) {
                    while win::PeekMessageW(m, std::ptr::null_mut(), 0, 0, 1) > 0 {
                        win::TranslateMessage(m);
                        win::DispatchMessageW(m);
                    }
                    win::Sleep(8);
                }
            };
            app.grab_and_show();
            pump(300, &mut m);
            let ov = app.overlay.as_ref().map(|o| o.hwnd).unwrap_or(std::ptr::null_mut());
            let (fg, active) = unsafe { (win::GetForegroundWindow(), win::GetActiveWindow()) };
            let has_focus = !ov.is_null() && (fg == ov || active == ov);
            unsafe {
                win::keybd_event(0x1B, 0, 0, 0);
                win::keybd_event(0x1B, 0, win::KEYEVENTF_KEYUP, 0);
            }
            pump(250, &mut m);
            let alive = app.overlay.is_some();
            let first = !alive;

            // Second round: the overlay is up, then something takes the
            // foreground back — a click on another window, a popup, or in this
            // case our own panel, which is the cheapest way to reproduce it.
            // Esc must still cancel, and it must not leak into the panel
            // (whose Esc handler would hide it).
            app.grab_and_show();
            pump(220, &mut m);
            app.show_main();
            pump(80, &mut m);
            let stolen = unsafe {
                win::GetForegroundWindow() != ov
            };
            unsafe {
                win::keybd_event(0x1B, 0, 0, 0);
                win::keybd_event(0x1B, 0, win::KEYEVENTF_KEYUP, 0);
            }
            pump(220, &mut m);
            let still_open = app.overlay.is_some();
            let panel_visible = unsafe { win::IsWindowVisible(app.main) } != 0;
            let second = !still_open && panel_visible;

            // Third round: the frame the user is most likely looking at when
            // they hit Esc — a selection is made, the overlay is gone and the
            // action bar is up. The bar carries Esc / Enter / 1-4, and until now
            // it never owned the keyboard.
            app.grab_and_show();
            pump(200, &mut m);
            let ov3 = app.overlay.as_ref().map(|o| o.hwnd).unwrap_or(std::ptr::null_mut());
            let lp = |x: i32, y: i32| ((y as isize) << 16) | (x as isize & 0xFFFF);
            unsafe {
                win::SendMessageW(ov3, win::WM_LBUTTONDOWN, 1, lp(100, 100));
                win::SendMessageW(ov3, win::WM_MOUSEMOVE, 1, lp(400, 300));
                win::SendMessageW(ov3, win::WM_LBUTTONUP, 0, lp(400, 300));
            }
            pump(250, &mut m);
            let bar_up = app.bar.is_some();
            unsafe {
                win::keybd_event(0x1B, 0, 0, 0);
                win::keybd_event(0x1B, 0, win::KEYEVENTF_KEYUP, 0);
            }
            pump(220, &mut m);
            let third = bar_up && app.bar.is_none() && app.overlay.is_none();

            let _ = std::fs::write(&out, format!(
                "{{\n  \"hook\": {},\n  \"overlay\": {},\n  \"foregroundIsOverlay\": {},\n  \"focused\": {},\n  \"escClosed\": {},\n  \"stolenForeground\": {},\n  \"escClosedAfterTheft\": {},\n  \"panelStillVisible\": {},\n  \"selectionMade\": {},\n  \"escClosedBar\": {}\n}}\n",
                hook_ok, !ov.is_null(), fg == ov, has_focus, first, stolen, !still_open, panel_visible,
                bar_up, third));
            app.cancel_capture();
            std::process::exit(if first && second && third { 0 } else { 1 });
        }

        if !args.is_empty() && args[0] == "--click-test" {
            // The painted dialogs have no native BUTTON, so nothing else
            // proves that a click still turns into a command. Drive one with
            // real mouse messages and report whether the window closed.
            let out = args.get(1).cloned().unwrap_or_else(|| "click.json".into());
            app.show_main();
            app.open_settings_dialog();
            let mut m: win::MSG = Default::default();
            let pump = |ms: u64, m: &mut win::MSG| {
                let t0 = std::time::Instant::now();
                while t0.elapsed() < std::time::Duration::from_millis(ms) {
                    while win::PeekMessageW(m, std::ptr::null_mut(), 0, 0, 1) > 0 {
                        win::TranslateMessage(m);
                        win::DispatchMessageW(m);
                    }
                    win::Sleep(8);
                }
            };
            pump(160, &mut m);
            let mut opened = false;
            let mut clicked = false;
            if let Some(d) = app.dialog.as_ref() {
                opened = true;
                let hwnd = d.hwnd;
                if let Some((x, y)) = crate::card::button_center(hwnd, dialogs::ID_CANCEL) {
                    let lp = ((y as isize) << 16) | (x as isize & 0xFFFF);
                    unsafe {
                        win::SendMessageW(hwnd, win::WM_LBUTTONDOWN, 1, lp);
                        win::SendMessageW(hwnd, win::WM_LBUTTONUP, 0, lp);
                    }
                    clicked = true;
                }
            }
            pump(200, &mut m);
            let closed = app.dialog.is_none();
            let _ = std::fs::write(&out, format!(
                "{{\n  \"opened\": {},\n  \"clickedClose\": {},\n  \"closed\": {}\n}}\n",
                opened, clicked, closed));
            std::process::exit(if opened && clicked && closed { 0 } else { 1 });
        }

        if !args.is_empty() && args[0] == "--overlay-shot" {
            let out = args.get(1).cloned().unwrap_or_else(|| "overlay.png".into());
            let code = capture_overlay_snapshot(&mut app, &out);
            std::process::exit(code);
        }

        if !args.is_empty() && (args[0] == "--about-shot" || args[0] == "--dialog-shot") {
            // --about-shot <png>                 : the About window
            // --dialog-shot <about|settings|add> <png> : any dialog, on demand
            let (kind, out) = if args[0] == "--dialog-shot" {
                let k = args.get(1).cloned().unwrap_or_else(|| "settings".into());
                (k.clone(), args.get(2).cloned().unwrap_or_else(|| format!("{}.png", k)))
            } else {
                ("about".to_string(), args.get(1).cloned().unwrap_or_else(|| "about.png".into()))
            };
            let (dkind, cls, service) = match kind.as_str() {
                "settings" => (dialogs::DialogKind::Settings, app.cls_settings.clone(),
                               everything::service_state()),
                "add" => (dialogs::DialogKind::Add, app.cls_dialog.clone(), String::new()),
                _ => (dialogs::DialogKind::About, app.cls_dialog.clone(), String::new()),
            };
            let mut dlg = match Dialog::create(std::ptr::null_mut(), dkind, &cls, None, &service) {
                Ok(d) => d,
                Err(e) => {
                    win::log(&format!("{} 快照创建窗口失败: {}", kind, e));
                    std::process::exit(1);
                }
            };
            // The child controls have never painted at this point, so a bare
            // PrintWindow comes back as an empty dialog body. Give them one
            // pass through the queue first.
            win::UpdateWindow(dlg.hwnd);
            let mut m: win::MSG = Default::default();
            let t0 = std::time::Instant::now();
            while t0.elapsed() < std::time::Duration::from_millis(150) {
                while win::PeekMessageW(&mut m, std::ptr::null_mut(), 0, 0, 1) > 0 {
                    win::TranslateMessage(&m);
                    win::DispatchMessageW(&m);
                }
                win::Sleep(8);
            }
            dump_layout(&out, app.main, dlg.hwnd);
            let code = capture_window_png(dlg.hwnd, &out);
            dlg.close();
            std::process::exit(code);
        }

        if !args.is_empty() && args[0] == "--tray-anchor" {
            // Reports where Windows actually put our icon and where the menu
            // will hang off it. The NOTIFYICONIDENTIFIER layout is the only
            // silent failure here: a wrong cbSize makes the shell answer
            // E_INVALIDARG and we would fall back to the cursor invisibly.
            let out = args.get(1).cloned().unwrap_or_else(|| "tray-anchor.json".into());
            let added = tray::add(host, icon);
            let mut m: win::MSG = Default::default();
            let t0 = std::time::Instant::now();
            while t0.elapsed() < std::time::Duration::from_millis(300) {
                while win::PeekMessageW(&mut m, std::ptr::null_mut(), 0, 0, 1) > 0 {
                    win::TranslateMessage(&m);
                    win::DispatchMessageW(&m);
                }
                win::Sleep(8);
            }
            let rc = tray::icon_rect(host);
            let (x, y, flags) = tray::menu_anchor(host);
            let sw = win::GetSystemMetrics(win::SM_CXSCREEN);
            let sh = win::GetSystemMetrics(win::SM_CYSCREEN);
            let json = format!(
                "{{\"added\":{},\"screen\":\"{}x{}\",\"icon\":\"{}\",\"anchor\":{{\"x\":{},\"y\":{},\"bottomAlign\":{},\"rightAlign\":{}}},\"structSize\":{}}}",
                added, sw, sh,
                rc.map(|r| format!("{},{},{},{}", r.left, r.top, r.right, r.bottom))
                    .unwrap_or_else(|| "-".into()),
                x, y,
                flags & win::TPM_BOTTOMALIGN != 0,
                flags & win::TPM_RIGHTALIGN != 0,
                std::mem::size_of::<win::NOTIFYICONIDENTIFIER>()
            );
            let _ = std::fs::write(&out, json);
            tray::remove(host, icon);
            std::process::exit(if added && rc.is_some() { 0 } else { 1 });
        }

        app.tray_added = tray::add(host, icon);
        if !app.tray_added {
            win::SetTimer(host, TIMER_TRAY_RETRY, 2000, std::ptr::null_mut());
        }
        let (mods, vk) = app.config.parse_hotkey();
        app.hotkey_ok = win::RegisterHotKey(host, 1, mods, vk) != 0;
        if !app.hotkey_ok {
            win::log("RegisterHotKey 失败，启动 3s 重试");
            win::SetTimer(host, TIMER_HOTKEY_RETRY, 3000, std::ptr::null_mut());
        }
        app.hook = win::SetWindowsHookExW(win::WH_KEYBOARD_LL, Some(keyboard_proc), inst, 0);
        // Worth one line in the log: when the hotkey "does nothing", the first
        // question is whether the hook exists or whether input never arrives.
        win::log(&format!("键盘钩子: {:?} (err={})", app.hook as usize, win::GetLastError()));
        // A KEYUP swallowed while no hook was installed would strand the flag
        // high and silently disable double-Ctrl for the rest of the session.
        CTRL_TICK.store(0, Ordering::SeqCst);

        // Load the v6 tiny engine once, on this thread. Same files as 3.3.
        match paddle::find_runtime_dir() {
            Ok(dir) => match PaddleOcr::new(&dir) {
                Ok(engine) => {
                    let boxed = Box::new(engine);
                    app.ocr = Box::into_raw(boxed);
                    let eng = &*app.ocr;
                    eng.warmup();
                    win::log(&format!("PP-OCRv6 tiny 就绪: {}", dir));
                }
                Err(e) => {
                    win::log(&format!("本地 OCR 加载失败: {}", e));
                    // Silent failure is the worst outcome: the tray appears,
                    // shortcuts open the UI, and only OCR quietly does nothing
                    // until the user gives up. Say it once, up front, and name
                    // the missing piece instead of a stack trace.
                    win::message_box(
                        &format!(
                            "OCR 组件加载失败，截图识别功能将不可用。\n\n目录：{}\n原因：{}\n\n请确认 ocr 子目录里同时存在 det.onnx、rec.onnx、keys.txt、\nonnxruntime.dll 与 VC 运行库（vcruntime140.dll 等）。\n重新解压完整压缩包可修复。\n\n详细日志：{}\nttjiang.log",
                            dir,
                            e,
                            win::exe_dir()
                        ),
                        "TT酱 - OCR 不可用",
                        0x30,
                    );
                }
            },
            Err(e) => {
                win::log(&format!("ocr 目录不可用: {}", e));
                win::message_box(
                    &format!(
                        "找不到 OCR 运行库目录，截图识别功能将不可用。\n\n原因：{}\n\nocr 文件夹需要与 TT酱.exe 放在同一目录下，\n重新解压完整压缩包可修复。\n\n详细日志：{}\nttjiang.log",
                        e,
                        win::exe_dir()
                    ),
                    "TT酱 - OCR 不可用",
                    0x30,
                );
            }
        }

        loop {
            let mut msg: win::MSG = Default::default();
            let r = win::GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0);
            if r <= 0 { break; }
            win::TranslateMessage(&msg);
            win::DispatchMessageW(&msg);
            if app.quit { break; }
        }

        app.cleanup();
        APP_PTR.with(|p| p.set(std::ptr::null_mut()));
        if !mutex.is_null() { win::ReleaseMutex(mutex); win::CloseHandle(mutex); }
    }
}

impl App {
    fn cleanup(&mut self) {
        unsafe {
            if let Some(mut o) = self.overlay.take() { o.close(); }
            if let Some(mut b) = self.bar.take() { b.close(); }
            if let Some(mut t) = self.toast.take() { t.close(); }
            if let Some(mut d) = self.dialog.take() { d.close(); }
            tray::remove(self.host, self.icon);
            if !self.icon.is_null() { win::DestroyIcon(self.icon); }
            if !self.hook.is_null() { win::UnhookWindowsHookEx(self.hook); }
            win::UnregisterHotKey(self.host, 1);
            win::KillTimer(self.host, TIMER_HOTKEY_RETRY);
            win::KillTimer(self.host, TIMER_TRAY_RETRY);
            win::KillTimer(self.host, TIMER_CAPTURE_DELAY);
        }
        win::log("TT酱 退出");
    }

    fn toast(&mut self, text: &str, ok: bool, ms: u32) {
        if !self.config.show_toast { return; }
        if self.toast.is_none() {
            if let Ok(t) = Toast::new(&self.cls_toast) { self.toast = Some(t); }
        }
        if let Some(t) = self.toast.as_mut() { t.show(text, ok, ms); }
    }

    fn show_main(&mut self) {
        unsafe {
            if self.main.is_null() { return; }
            // Clearing the control first: its EN_CHANGE is what resets `query`,
            // so the state below stays consistent with what is on screen.
            searchbox::set_text(self.search.hwnd, "");
            self.query.clear();
            self.search_hits.clear();
            self.search_error = None;
            self.dx_result.clear();
            self.hover_tile = -1;
            self.hover_row = -1;
            self.hover_add = false;
            win::KillTimer(self.host, TIMER_SEARCH_DEBOUNCE);
            // A minimised or maximised window has to be restored first: the
            // panel is a fixed-size launcher, and moving a maximised window
            // does not change what the user sees.
            if win::IsIconic(self.main) != 0 || win::IsZoomed(self.main) != 0 {
                win::ShowWindow(self.main, win::SW_RESTORE);
            }
            win::ShowWindow(self.main, win::SW_SHOW);
            // Always open where the user expects it, even if the window was
            // dragged somewhere else last time.
            win::center_window_below(self.main, PANEL_DROP_PCT);
            win::raise_window(self.main);
            // raise_window also focuses the frame; hand the caret to the field
            // so the first keystroke lands where the user is looking.
            self.focus_search();
            win::InvalidateRect(self.main, std::ptr::null(), 0);
        }
    }

    fn hide_main(&mut self) {
        unsafe { if !self.main.is_null() { win::ShowWindow(self.main, win::SW_HIDE); } }
    }

    /// Drop a shortcut on a grid slot and persist it at once. A position that
    /// only survived until the next launch would read as a bug, not a feature.
    fn move_shortcut(&mut self, item: usize, slot: usize) {
        let before = slot_map_of(&self.shortcuts);
        place_at(&mut self.shortcuts, item, slot);
        let after = slot_map_of(&self.shortcuts);
        if before == after { return; }   // dropped back on its own cell
        self.hover_tile = -1;
        self.hover_add = false;
        match profile::save_all(&self.shortcuts, &self.settings) {
            Ok(_) => self.toast(&format!("已放到第 {} 格", slot + 1), true, 1400),
            Err(e) => self.toast(&format!("位置已调整，但写盘失败：{}", e), false, 3000),
        }
    }

    // Double-Ctrl toggles, the way the Windows key toggles the Start menu.
    // Deliberately unconditional: SetForegroundWindow is often refused for a
    // background process, so "hide only when focused" left the window
    // impossible to close. A visible window is collapsed, a minimised one is
    // restored, a hidden one is shown.
    fn toggle_main(&mut self) {
        unsafe {
            if self.main.is_null() { return; }
            if win::IsWindowVisible(self.main) != 0 && win::IsIconic(self.main) == 0 {
                self.hide_main();
                return;
            }
        }
        self.show_main();
    }

    fn begin_capture(&mut self) {
        if self.overlay.is_some() || self.screen.is_some() { return; }
        self.hide_main();
        // Give DWM 140ms to settle, same as the C# build.
        unsafe { win::SetTimer(self.host, TIMER_CAPTURE_DELAY, 140, std::ptr::null_mut()); }
        win::log("Ctrl+1 -> 140ms 后抓屏");
    }

    fn grab_and_show(&mut self) {
        unsafe { win::KillTimer(self.host, TIMER_CAPTURE_DELAY); }
        let img = match capture::grab_virtual_screen() {
            Ok(i) => i,
            Err(e) => { self.toast(&format!("截图失败：{}", e), false, 3600); return; }
        };
        self.vs = win::virtual_screen();
        match Overlay::new(img.clone(), self.vs, &self.cls_overlay) {
            Ok(ov) => {
                self.screen = Some(img);
                self.overlay = Some(ov);
                self.overlay.as_ref().unwrap().show();
            }
            Err(e) => { win::log(&format!("遮罩创建失败: {}", e)); self.toast(&format!("截图失败：{}", e), false, 3600); }
        }
    }

    fn cancel_capture(&mut self) {
        if let Some(mut o) = self.overlay.take() { o.close(); }
        if let Some(mut b) = self.bar.take() { b.close(); }
        self.screen = None;
    }

    fn on_selected(&mut self, screen_rect: Rect) {
        self.selection = screen_rect;
        if screen_rect.w < 8 || screen_rect.h < 8 {
            self.cancel_capture();
            return;
        }
        if let Some(mut o) = self.overlay.take() { o.close(); }
        match ActionBar::new(&self.cls_bar) {
            Ok(bar) => {
                bar.place_near(screen_rect);
                self.bar = Some(bar);
            }
            Err(e) => { self.toast(&format!("动作条失败：{}", e), false, 3000); self.cancel_capture(); }
        }
    }

    fn on_action(&mut self, action: i32) {
        if action == overlay::ACTION_CANCEL || action < 0 {
            self.cancel_capture();
            if let Some(t) = self.toast.as_mut() { t.hide(); }
            return;
        }
        let screen = match self.screen.as_ref() {
            Some(s) => s.clone(),
            None => { self.cancel_capture(); return; }
        };
        let img = capture::crop(&screen, self.selection, self.vs);
        let mut done = false;
        match action {
            overlay::ACTION_IMAGE => {
                match capture::copy_image(&img) {
                    Ok(_) => self.toast(&format!("图片已复制到剪贴板  ({} × {})", img.w, img.h), true, 2000),
                    Err(e) => self.toast(&e, false, 3000),
                }
                done = true;
            }
            overlay::ACTION_SAVE => {
                // The action bar is a topmost window, so a modal save dialog
                // would open behind it. Close the capture UI first, then ask
                // where the file should go.
                if let Some(mut b) = self.bar.take() { b.close(); }
                let dir = self.config.effective_save_dir();
                let name = capture::stamp_name();
                if let Some(path) = dialogs::pick_png_save(self.host, &dir, &name) {
                    match capture::save_png_as(&img, &path) {
                        Ok(_) => {
                            // Remember the chosen folder so the next save opens
                            // there instead of making the user navigate again.
                            if let Some(parent) = std::path::Path::new(&path).parent() {
                                let p = parent.to_string_lossy().into_owned();
                                if !p.is_empty() && p != self.config.save_dir {
                                    self.config.save_dir = p;
                                    self.config.save();
                                }
                            }
                            let shown = std::path::Path::new(&path)
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_else(|| path.clone());
                            self.toast(&format!("已保存：{}", shown), true, 2400);
                        }
                        Err(e) => self.toast(&e, false, 3000),
                    }
                } else {
                    // Cancelling must not look like a successful save.
                    self.toast("已取消保存", false, 1600);
                }
                done = true;
            }
            overlay::ACTION_TABLE | overlay::ACTION_TEXT => {
                if self.ocr_busy {
                    self.toast("上一次识别还在进行中…", false, 1800);
                    return;
                }
                if self.ocr.is_null() {
                    self.toast("缺少 ocr 文件夹：det.onnx、rec.onnx、keys.txt 和 onnxruntime.dll 需要与 TT酱.exe 放在同一目录下的 ocr 子目录里。请解压完整压缩包后再运行。", false, 5000);
                    return;
                }
                self.pending_action = action;
                self.ocr_busy = true;
                self.toast("正在离线识别…", true, 3000);
                // Pass the engine as an address, not a raw pointer: Rust's
                // disjoint closure capture would otherwise grab only the
                // *mut field and reject it as !Send.
                let engine_addr = self.ocr as usize;
                let host_id = self.host as usize;
                std::thread::spawn(move || {
                    let eng = unsafe { &*(engine_addr as *const PaddleOcr) };
                    let outcome: Result<OcrResult, String> = eng.recognize(&img);
                    let result = match outcome {
                        Ok(res) => {
                            let (text, cols, lines) = if ptr_action_table(action) {
                                let tsv = layout::to_tsv(&res);
                                let cols = layout::column_count(&tsv);
                                let lines = layout::line_count(&tsv);
                                (tsv, cols, lines)
                            } else {
                                let txt = layout::to_plain_text(&res);
                                let lines = layout::line_count(&txt);
                                (txt, 0, lines)
                            };
                            OcrDone {
                                text, lines, cols,
                                language: res.language.clone(),
                                engine: res.engine.clone(),
                                error: None,
                            }
                        }
                        Err(e) => OcrDone {
                            text: String::new(), lines: 0, cols: 0,
                            language: String::new(), engine: String::new(),
                            error: Some(e),
                        },
                    };
                    let raw = Box::into_raw(Box::new(result));
                    unsafe {
                        win::PostMessageW(host_id as win::HWND, win::WM_APP_OCR_DONE, action as usize, raw as win::LPARAM);
                    }
                });
                return;
            }
            _ => {}
        }
        if done {
            if let Some(mut b) = self.bar.take() { b.close(); }
            self.screen = None;
        }
    }

    fn on_ocr_done(&mut self, action: i32, done: OcrDone) {
        self.ocr_busy = false;
        if let Some(e) = &done.error {
            self.toast(&format!("识别失败：{}", e), false, 3600);
        } else if done.text.trim().is_empty() {
            self.toast("选区内未识别到文字", false, 2600);
        } else {
            if !done.language.is_empty() && done.language != self.config.recent_language {
                self.config.recent_language = done.language.clone();
                self.config.save();
            }
            match capture::copy_text(&done.text) {
                Ok(_) => {
                    if action == overlay::ACTION_TABLE {
                        self.toast(&format!("表格已复制：可粘贴进 Excel / WPS（{} 列）", done.cols), true, 2400);
                    } else {
                        self.toast(&format!("文字已复制到剪贴板（{} 行）", done.lines), true, 2200);
                    }
                }
                Err(e) => self.toast(&e, false, 3000),
            }
        }
        if let Some(mut b) = self.bar.take() { b.close(); }
        self.screen = None;
    }

    fn menu_command(&mut self, cmd: i32) {
        match cmd {
            10 => self.begin_capture(),
            5 => self.export_profile(),
            6 => self.import_profile(),
            7 => {
                unsafe {
                    if !self.hook.is_null() { win::UnhookWindowsHookEx(self.hook); }
                    self.hook = win::SetWindowsHookExW(win::WH_KEYBOARD_LL, Some(keyboard_proc),
                        win::GetModuleHandleW(std::ptr::null()), 0);
                    CTRL_TICK.store(0, Ordering::SeqCst);
                }
                self.toast(if self.hook.is_null() { "全局热键重注册失败" } else { "已重新注册全局热键" },
                    !self.hook.is_null(), 2200);
            }
            8 => {
                let on = !tray::autostart_enabled();
                tray::set_autostart(on);
                self.toast(if on { "已开启开机自动启动" } else { "已关闭开机自动启动" }, true, 2200);
            }
            11 => self.open_about_dialog(),
            9 => { self.quit = true; unsafe { win::PostQuitMessage(0); } }
            _ => {}
        }
    }

    // ---- launcher windows -------------------------------------------------

    fn open_shortcut_dialog(&mut self, index: Option<usize>) {
        if self.dialog.is_some() {
            dialogs::info(self.main, "请先关闭已打开的窗口。", "TT酱");
            return;
        }
        let cls = self.cls_dialog.clone();
        let initial = index.and_then(|i| self.shortcuts.get(i)).cloned();
        let seed = initial.unwrap_or_default();
        let kind = match index { Some(i) => dialogs::DialogKind::Edit(i), None => dialogs::DialogKind::Add };
        match Dialog::create(self.main, kind, &cls, Some(&seed), "") {
            Ok(d) => self.dialog = Some(d),
            Err(e) => self.toast(&format!("无法打开窗口：{}", e), false, 2600),
        }
    }

    fn open_settings_dialog(&mut self) {
        if self.dialog.is_some() {
            dialogs::info(self.main, "请先关闭已打开的窗口。", "TT酱");
            return;
        }
        let cls = self.cls_settings.clone();
        let state = everything::service_state();
        match Dialog::create(self.main, dialogs::DialogKind::Settings, &cls, None, &state) {
            Ok(d) => self.dialog = Some(d),
            Err(e) => self.toast(&format!("无法打开设置：{}", e), false, 2600),
        }
    }

    fn open_about_dialog(&mut self) {
        if self.dialog.is_some() {
            dialogs::info(self.main, "请先关闭已打开的窗口。", "TT酱");
            return;
        }
        let cls = self.cls_dialog.clone();
        // No owner: the About window must come up even when the launcher is
        // hidden, which is the normal state when it is opened from the tray.
        match Dialog::create(std::ptr::null_mut(), dialogs::DialogKind::About, &cls, None, "") {
            Ok(d) => self.dialog = Some(d),
            Err(e) => self.toast(&format!("无法打开窗口：{}", e), false, 2600),
        }
    }

    fn close_dialog(&mut self) {
        if let Some(mut d) = self.dialog.take() { d.close(); }
    }

    fn refresh_main(&self) {
        if !self.main.is_null() {
            unsafe { win::InvalidateRect(self.main, std::ptr::null(), 0); }
        }
    }

    fn on_dialog_command(&mut self, id: i32) {
        // Copy plain handles out so no immutable borrow of `self` is held
        // while mutating shortcuts / profiles below.
        let (kind, hname, hpath, hargs, hicon, pos, hwnd) = match self.dialog.as_ref() {
            Some(d) => (d.kind, d.name, d.path, d.args, d.icon, d.pos, d.hwnd),
            None => return,
        };
        match id {
            dialogs::ID_CANCEL => self.close_dialog(),
            dialogs::ID_OK => {
                let s = dialogs::read_shortcut(hname, hpath, hargs, hicon, pos);
                if s.name.is_empty() {
                    dialogs::info(hwnd, "请填写名称。", "TT酱");
                    unsafe { win::SetFocus(hname); }
                    return;
                }
                if s.path.is_empty() {
                    dialogs::info(hwnd, "请填写路径（或网址），也可以用「文件 / 目录」按钮选择。", "TT酱");
                    unsafe { win::SetFocus(hpath); }
                    return;
                }
                match kind {
                    dialogs::DialogKind::Edit(i) if i < self.shortcuts.len() => self.shortcuts[i] = s,
                    _ => self.shortcuts.push(s),
                }
                if let Err(e) = profile::save_all(&self.shortcuts, &self.settings) {
                    self.toast(&format!("保存失败：{}", e), false, 3000);
                }
                self.close_dialog();
                self.refresh_main();
                self.toast("快捷方式已保存", true, 2000);
            }
            dialogs::ID_PICK_FILE => {
                if let Some(p) = dialogs::pick_file(hwnd) {
                    dialogs::set_control_text(hpath, &p);
                    if dialogs::control_text(hname).trim().is_empty() {
                        let stem = std::path::Path::new(&p)
                            .file_stem().map(|x| x.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        dialogs::set_control_text(hname, &stem);
                    }
                }
            }
            dialogs::ID_PICK_FOLDER => {
                if let Some(p) = dialogs::pick_folder(hwnd) {
                    dialogs::set_control_text(hpath, &p);
                }
            }
            dialogs::ID_PICK_ICON => {
                if let Some(p) = dialogs::pick_file(hwnd) {
                    dialogs::set_control_text(hicon, &p);
                }
            }
            dialogs::ID_CLEAR_ICON => dialogs::set_control_text(hicon, ""),
            dialogs::ID_EXPORT => self.export_profile(),
            dialogs::ID_IMPORT => self.import_profile(),
            dialogs::ID_START_INDEX => {
                let r = if everything::is_client_running() {
                    Ok(())
                } else {
                    everything::start_client().or_else(|_| everything::install_service()
                        .and_then(|_| everything::start_client()))
                };
                match r {
                    Ok(_) => dialogs::info(hwnd, "内置索引服务已启动，稍等几秒后文件搜索即可使用。", "TT酱"),
                    Err(e) => dialogs::info(hwnd, &format!("启动失败：{}", e), "TT酱"),
                }
            }
            _ => {}
        }
    }

    fn export_profile(&mut self) {
        let name = profile::default_export_name();
        if let Some(path) = dialogs::pick_save(self.main, &name) {
            match profile::export_to(&path, &self.shortcuts) {
                Ok(_) => self.toast("快捷方式已导出", true, 2200),
                Err(e) => dialogs::info(self.main, &format!("导出失败：{}", e), "TT酱"),
            }
        }
    }

    fn import_profile(&mut self) {
        let path = match dialogs::pick_import(self.main) { Some(p) => p, None => return };
        let items = match profile::import_from(&path) {
            Ok(v) => v,
            Err(e) => { dialogs::info(self.main, &e, "TT酱"); return; }
        };
        let count = items.len();
        if !self.shortcuts.is_empty()
            && !dialogs::confirm(self.main,
                &format!("导入会替换当前的 {} 个快捷方式，继续？", self.shortcuts.len()), "TT酱")
        { return; }
        let backup = profile::backup();
        self.shortcuts = items;
        match profile::save_all(&self.shortcuts, &self.settings) {
            Ok(_) => {
                self.refresh_main();
                let suffix = backup.map(|b| format!("原配置已备份为 {}", b)).unwrap_or_default();
                dialogs::info(self.main, &format!("已导入 {} 个快捷方式。{}", count, suffix), "TT酱");
            }
            Err(e) => dialogs::info(self.main, &format!("导入失败：{}", e), "TT酱"),
        }
    }

    // ---- file search ------------------------------------------------------

    fn schedule_search(&mut self) {
        unsafe { win::SetTimer(self.host, TIMER_SEARCH_DEBOUNCE, 300, std::ptr::null_mut()); }
    }

    /// Put the caret back in the field. The panel has exactly one focusable
    /// child, so every path that could leave the keyboard on the frame comes
    /// back here instead.
    fn focus_search(&self) {
        let h = self.search.hwnd;
        if h.is_null() || unsafe { win::IsWindow(h) } == 0 { return; }
        unsafe { win::SetFocus(h); }
    }

    /// Keys the panel owns, as opposed to the ones the text field owns.
    ///
    /// Called from the field's window procedure — that is where they arrive
    /// while the caret is in the box — and from the panel's own `WM_KEYDOWN` as
    /// a fallback. `GetKeyState` rather than `GetAsyncKeyState`: it reports the
    /// modifier state at the moment the message was taken from the queue, which
    /// is what the chord meant even if the handler runs late.
    pub(crate) fn on_search_key(&mut self, vk: u32) -> bool {
        let ctrl = unsafe { win::GetKeyState(win::VK_CONTROL as i32) } < 0;
        let alt = unsafe { win::GetKeyState(win::VK_MENU as i32) } < 0;
        match vk {
            win::VK_ESCAPE => {
                if self.query.is_empty() {
                    self.hide_main();
                } else {
                    // Clearing the control is the same event as the user
                    // deleting the text, so EN_CHANGE resets the results.
                    searchbox::set_text(self.search.hwnd, "");
                }
                true
            }
            win::VK_RETURN => {
                if !self.dx_result.is_empty() {
                    let _ = capture::copy_text(&self.dx_result);
                    self.toast("已复制大写金额", true, 1800);
                } else {
                    let rows = main_rows(self);
                    open_row(self, rows.first().map(|r| &r.0));
                }
                true
            }
            _ if ctrl && vk == 0xBC => { self.open_settings_dialog(); true }        // Ctrl+,
            _ if ctrl && vk == 0x51 => {                                            // Ctrl+Q
                self.quit = true;
                unsafe { win::PostQuitMessage(0) };
                true
            }
            _ if ctrl && vk == 0x46 => { searchbox::select_all(self.search.hwnd); true } // Ctrl+F
            _ if alt && vk == win::VK_F4 => { self.hide_main(); true }              // Alt+F4
            _ => false,
        }
    }

    /// `EN_CHANGE`: the control is the single source of truth for `query`.
    ///
    /// Nothing else writes the query behind the field's back, which is why the
    /// old character-by-character bookkeeping (caret, backspace, paste folding)
    /// is gone: the control already did all of it.
    fn on_query_changed(&mut self) {
        if self.syncing_query { return; }
        let h = self.search.hwnd;
        let mut text = searchbox::text(h);
        if text.chars().count() > SEARCH_MAX_CHARS {
            let cut: String = text.chars().take(SEARCH_MAX_CHARS).collect();
            // Writing back re-enters this handler synchronously; the flag stops
            // the second pass from scheduling a search of the cut text twice.
            self.syncing_query = true;
            searchbox::set_text(h, &cut);
            self.syncing_query = false;
            searchbox::caret_to_end(h);
            self.toast(&format!("搜索内容过长，只取前 {} 字", SEARCH_MAX_CHARS), false, 1800);
            text = cut;
        }
        if text == self.query { return; }
        self.query = text;
        self.search_hits.clear();
        self.search_error = None;
        self.dx_result.clear();
        self.scroll_rows = 0;
        self.schedule_search();
        self.refresh_main();
    }

    fn start_search(&mut self) {
        unsafe { win::KillTimer(self.host, TIMER_SEARCH_DEBOUNCE); }
        let q = self.query.trim().to_string();
        self.dx_result = to_chinese_amount(&q).unwrap_or_default();
        if q.is_empty() {
            self.search_hits.clear();
            self.search_error = None;
            self.search_busy = false;
            self.refresh_main();
            return;
        }
        // "dx 12345" is a local conversion, no need to hit the index service.
        if !self.dx_result.is_empty() {
            self.search_hits.clear();
            self.search_error = None;
            self.search_busy = false;
            self.refresh_main();
            return;
        }
        self.search_seq = self.search_seq.wrapping_add(1);
        let seq = self.search_seq;
        self.search_busy = true;
        let host = self.host as usize;
        let opts = self.settings.options();
        std::thread::spawn(move || {
            let out = everything::query(&q, opts);
            let done = Box::new(SearchDone { seq, paths: out.paths, error: out.error });
            unsafe {
                win::PostMessageW(host as win::HWND, WM_APP_SEARCH_DONE, seq as usize,
                    Box::into_raw(done) as win::LPARAM);
            }
        });
        self.refresh_main();
    }

    fn on_search_done(&mut self, raw: *mut SearchDone) {
        if raw.is_null() { return; }
        let done = unsafe { *Box::from_raw(raw) };
        if done.seq != self.search_seq { return; } // stale query, ignore
        self.search_hits = done.paths;
        self.scroll_rows = 0;
        self.search_error = done.error;
        self.search_busy = false;
        self.refresh_main();
    }
}

// "dx 12345" -> Chinese uppercase amount, same helper mode as 3.3.
const CN_DIGITS: [&str; 10] = ["零", "壹", "贰", "叁", "肆", "伍", "陆", "柒", "捌", "玖"];

/// Is a screenshot overlay or its action bar on screen right now? Read through
/// the raw App pointer: the keyboard hook runs on the message-loop thread, where
/// no `&mut App` is live, and the question has to be answered without a borrow.
fn capture_ui_open() -> bool {
    let ptr = APP_PTR.with(|c| c.get());
    if ptr.is_null() { return false; }
    let app = unsafe { &*(ptr as *const App) };
    app.overlay.is_some() || app.bar.is_some()
}

fn to_chinese_amount(query: &str) -> Option<String> {
    let s = query.trim();
    let lower = s.to_lowercase();
    let rest = lower.strip_prefix("dx")?;
    let rest = rest.trim();
    if rest.is_empty() { return None; }
    let cleaned: String = rest.chars().filter(|c| *c != ',' && !c.is_whitespace()).collect();
    let negative = cleaned.starts_with('-');
    let body = cleaned.trim_start_matches('-');
    if body.is_empty() || body.len() > 16
        || !body.chars().all(|c| c.is_ascii_digit() || c == '.') { return None; }
    let mut parts = body.splitn(2, '.');
    let int_part = parts.next().unwrap_or("");
    let frac = parts.next().unwrap_or("");
    if int_part.is_empty() || int_part.len() > 12 || frac.len() > 2 { return None; }
    let int_val: u64 = int_part.parse().ok()?;

    let small = ["", "拾", "佰", "仟"];
    let big = ["", "万", "亿", "万亿"];
    let digits: Vec<usize> = int_val.to_string().bytes().map(|b| (b - b'0') as usize).collect();
    let len = digits.len();
    let mut out = String::new();
    let mut pending_zero = false;
    for i in 0..len {
        let p = len - 1 - i;
        let sec = p / 4;
        let idx = p % 4;
        if digits[i] == 0 {
            pending_zero = true;
        } else {
            if pending_zero && !out.is_empty() { out.push('零'); }
            pending_zero = false;
            out.push_str(CN_DIGITS[digits[i]]);
            out.push_str(small[idx]);
        }
        if idx == 0 && sec > 0 {
            out.push_str(big[sec.min(3)]);
            pending_zero = false;
        }
    }
    if out.is_empty() { out.push('零'); }
    out.push('元');

    let mut cents = frac.to_string();
    while cents.len() < 2 { cents.push('0'); }
    let cb = cents.as_bytes();
    let jiao = (cb[0] - b'0') as usize;
    let fen = (cb[1] - b'0') as usize;
    if jiao == 0 && fen == 0 {
        out.push('整');
    } else {
        if jiao != 0 { out.push_str(CN_DIGITS[jiao]); out.push('角'); }
        else if int_val != 0 { out.push('零'); }
        if fen != 0 { out.push_str(CN_DIGITS[fen]); out.push('分'); }
    }
    if negative { out.insert(0, '负'); }
    Some(out)
}

fn ptr_action_table(action: i32) -> bool { action == overlay::ACTION_TABLE }

fn open_with_shell(path: &str) {
    unsafe {
        let p = win::wide(path);
        let verb = win::wide("open");
        win::ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), p.as_ptr(),
            std::ptr::null(), std::ptr::null(), 5);
    }
}

unsafe fn register(name: &str, proc: win::WNDPROC) -> Vec<u16> {
    let cls = win::wide(name);
    let wc = win::WNDCLASSEXW {
        cbSize: std::mem::size_of::<win::WNDCLASSEXW>() as u32,
        style: 0,
        lpfnWndProc: proc,
        cbClsExtra: 0, cbWndExtra: 0,
        hInstance: win::GetModuleHandleW(std::ptr::null()),
        hIcon: std::ptr::null_mut(),
        hCursor: win::LoadCursorW(std::ptr::null_mut(), IDC_ARROW as *const u16),
        hbrBackground: std::ptr::null_mut(),
        lpszMenuName: std::ptr::null(),
        lpszClassName: cls.as_ptr(),
        hIconSm: std::ptr::null_mut(),
    };
    let atom = win::RegisterClassExW(&wc);
    if atom == 0 {
        win::log(&format!("RegisterClassExW({}) 失败 err={}", name, win::GetLastError()));
    }
    cls
}

unsafe extern "system" fn keyboard_proc(code: i32, w: win::WPARAM, l: win::LPARAM) -> win::LRESULT {
    if code == win::HC_ACTION {
        let kb = &*(l as *const win::KBDLLHOOKSTRUCT);
        let vk = kb.vkCode;
        // Esc must end a capture no matter who owns the foreground. Ctrl+1 is
        // pressed while another app is in front, and if that app takes the
        // keyboard back (a click, a popup of its own) the overlay stops hearing
        // keys entirely — then Esc is the only way out short of aiming at the
        // bar's tiny cancel button. Swallowed so the app underneath does not
        // also react to it.
        if w as u32 == win::WM_KEYDOWN && vk == win::VK_ESCAPE && capture_ui_open() {
            let host = HOST_HWND.load(Ordering::SeqCst) as win::HWND;
            if !host.is_null() { win::PostMessageW(host, WM_APP_CANCEL_CAPTURE, 0, 0); }
            return 1;
        }
        let is_ctrl = vk == win::VK_CONTROL || vk == win::VK_LCONTROL || vk == win::VK_RCONTROL;
        if w as u32 == win::WM_KEYUP {
            if is_ctrl {
                CTRL_TICK.store(0, Ordering::SeqCst);
            }
            return win::CallNextHookEx(std::ptr::null_mut(), code, w, l);
        }
        if w as u32 == win::WM_KEYDOWN {
            if is_ctrl {
                let now = win::GetTickCount64();
                // Auto-repeat sends KEYDOWN every ~30ms while the key is held,
                // which the interval test below would read as a double tap.
                // Only the up-to-down transition counts. The 3s bound resets a
                // flag stranded by a KEYUP the hook never saw.
                let down_at = CTRL_TICK.load(Ordering::SeqCst);
                if down_at != 0 && now.saturating_sub(down_at) < 3000 {
                    return win::CallNextHookEx(std::ptr::null_mut(), code, w, l);
                }
                CTRL_TICK.store(now, Ordering::SeqCst);
                let last = LAST_CTRL.load(Ordering::SeqCst);
                if now.saturating_sub(last) <= 600 {
                    LAST_CTRL.store(0, Ordering::SeqCst);
                    let host = HOST_HWND.load(Ordering::SeqCst) as win::HWND;
                    if !host.is_null() { win::PostMessageW(host, WM_APP_ACTIVATE, 0, 0); }
                } else {
                    LAST_CTRL.store(now, Ordering::SeqCst);
                }
            } else {
                LAST_CTRL.store(0, Ordering::SeqCst);
            }
        }
    }
    win::CallNextHookEx(std::ptr::null_mut(), code, w, l)
}

unsafe extern "system" fn wnd_proc(hwnd: win::HWND, msg: u32, w: win::WPARAM, l: win::LPARAM) -> win::LRESULT {
    let ptr = APP_PTR.with(|c| c.get());
    if ptr.is_null() {
        return win::DefWindowProcW(hwnd, msg, w, l);
    }
    let app = &mut *ptr;
    let is_host = hwnd == app.host;
    let is_main = hwnd == app.main;
    let is_overlay = app.overlay.as_ref().map(|o| o.hwnd == hwnd).unwrap_or(false);
    let is_bar = app.bar.as_ref().map(|b| b.hwnd == hwnd).unwrap_or(false);
    let is_toast = app.toast.as_ref().map(|t| t.hwnd == hwnd).unwrap_or(false);

    if is_host {
        match msg {
            win::WM_HOTKEY if w == 1 => app.begin_capture(),
            win::WM_TIMER if w == TIMER_CAPTURE_DELAY => app.grab_and_show(),
            win::WM_TIMER if w == TIMER_SEARCH_DEBOUNCE => app.start_search(),
            win::WM_TIMER if w == TIMER_HOTKEY_RETRY => {
                let (mods, vk) = app.config.parse_hotkey();
                if win::RegisterHotKey(app.host, 1, mods, vk) != 0 {
                    app.hotkey_ok = true;
                    win::KillTimer(app.host, TIMER_HOTKEY_RETRY);
                    app.toast(&format!("已接管截图快捷键 {}", app.config.hotkey_text()), true, 2200);
                }
            }
            win::WM_TIMER if w == TIMER_TRAY_RETRY => {
                app.tray_retries += 1;
                if tray::add(app.host, app.icon) || app.tray_retries >= 10 {
                    app.tray_added = true;
                    win::KillTimer(app.host, TIMER_TRAY_RETRY);
                }
            }
            win::WM_APP_TRAY => {
                let ev = (l & 0xFFFF) as u32;
                if ev == win::WM_LBUTTONDBLCLK {
                    app.show_main();
                } else if ev == win::WM_RBUTTONUP || ev == win::WM_CONTEXTMENU {
                    let cmd = tray::show_menu(app.host, &app.config.hotkey_text(), tray::autostart_enabled());
                    if cmd != 0 { app.menu_command(cmd); }
                }
            }
            WM_APP_ACTIVATE => app.toggle_main(),
            WM_APP_CANCEL_CAPTURE => app.cancel_capture(),
            dialogs::WM_APP_DIALOG_CMD => app.on_dialog_command(w as i32),
            WM_APP_SEARCH_DONE => app.on_search_done(l as *mut SearchDone),
            win::WM_APP_OCR_DONE => {
                let raw = l as *mut OcrDone;
                if !raw.is_null() {
                    let done = *Box::from_raw(raw);
                    app.on_ocr_done(w as i32, done);
                }
            }
            _ => {
                if msg == app.taskbar_msg {
                    tray::remove(app.host, app.icon);
                    app.tray_added = tray::add(app.host, app.icon);
                    if !app.tray_added {
                        app.tray_retries = 0;
                        win::SetTimer(app.host, TIMER_TRAY_RETRY, 2000, std::ptr::null_mut());
                    }
                    win::log("TaskbarCreated -> 重建托盘图标");
                }
            }
        }
        if !app.balloon_shown && app.tray_added {
            app.balloon_shown = true;
            // 4.3: 首次托盘气泡与操作提示浮层共用同一个开关，默认都不弹。
            if app.config.show_toast {
                tray::balloon(app.host, app.icon);
            }
        }
        return win::DefWindowProcW(hwnd, msg, w, l);
    }

    if is_toast {
        if msg == win::WM_TIMER && w == 1 {
            if let Some(t) = app.toast.as_mut() { t.hide(); }
            return 0;
        }
        if msg == win::WM_PAINT {
            let mut ps: win::PAINTSTRUCT = Default::default();
            let dc = win::BeginPaint(hwnd, &mut ps);
            if !dc.is_null() {
                if let Some(t) = app.toast.as_ref() { t.paint(dc); }
                win::EndPaint(hwnd, &ps);
            }
            return 0;
        }
        if msg == win::WM_ERASEBKGND { return 1; }
        return win::DefWindowProcW(hwnd, msg, w, l);
    }

    if is_overlay {
        match msg {
            win::WM_PAINT => {
                let mut ps: win::PAINTSTRUCT = Default::default();
                let dc = win::BeginPaint(hwnd, &mut ps);
                if !dc.is_null() {
                    if let Some(o) = app.overlay.as_ref() { o.paint(dc); }
                    win::EndPaint(hwnd, &ps);
                }
                return 0;
            }
            win::WM_ERASEBKGND => return 1,
            win::WM_LBUTTONDOWN => {
                if let Some(o) = app.overlay.as_mut() {
                    let x = (l & 0xFFFF) as i16 as i32;
                    let y = ((l >> 16) & 0xFFFF) as i16 as i32;
                    o.start = (x, y);
                    o.rect = Rect::new(x, y, 0, 0);
                    o.has_rect = false;
                    o.dragging = true;
                    o.redraw();
                }
                win::SetCapture(hwnd);
                return 0;
            }
            win::WM_SETCURSOR => {
                let mut pt = win::POINT { x: 0, y: 0 };
                win::GetCursorPos(&mut pt);
                win::ScreenToClient(hwnd, &mut pt);
                let mut rc: win::RECT = Default::default();
                win::GetClientRect(hwnd, &mut rc);
                let over = tile_hit(app, &rc, pt.x, pt.y).is_some()
                    || row_hit(app, &rc, pt.x, pt.y).is_some()
                    || add_tile_rect(app, &rc).map_or(false, |r| inside(r, pt.x, pt.y))
                    || inside(gear_button_rect(&rc), pt.x, pt.y);
                if over {
                    win::SetCursor(win::LoadCursorW(std::ptr::null_mut(), win::IDC_HAND as *const u16));
                    return 1;
                }
            }
            win::WM_MOUSEMOVE => {
                if let Some(o) = app.overlay.as_mut() {
                    if o.dragging {
                        let now = win::GetTickCount64();
                        if now.saturating_sub(o.last_draw) >= 16 {
                            o.last_draw = now;
                            let x = (l & 0xFFFF) as i16 as i32;
                            let y = ((l >> 16) & 0xFFFF) as i16 as i32;
                            o.rect = Rect::from_points(o.start.0, o.start.1, x, y).clamp(o.w, o.h);
                            o.has_rect = o.rect.w > 1 && o.rect.h > 1;
                            o.redraw();
                        }
                    }
                }
                return 0;
            }
            win::WM_LBUTTONUP => {
                win::ReleaseCapture();
                let sel = if let Some(o) = app.overlay.as_mut() {
                    o.dragging = false;
                    o.redraw();
                    if o.has_rect && o.rect.w >= 8 && o.rect.h >= 8 { Some(o.screen_rect()) } else { None }
                } else { None };
                if let Some(r) = sel { app.on_selected(r); }
                return 0;
            }
            win::WM_RBUTTONUP => { app.cancel_capture(); return 0; }
            win::WM_KEYDOWN if w as u32 == win::VK_ESCAPE => { app.cancel_capture(); return 0; }
            win::WM_DPICHANGED => {
                // Geometry is already physical; force a full repaint so a DPI
                // change on a secondary monitor can never leave a stale frame.
                if let Some(o) = app.overlay.as_mut() { o.redraw(); }
                return 0;
            }
            _ => {}
        }
        return win::DefWindowProcW(hwnd, msg, w, l);
    }

    if is_bar {
        match msg {
            win::WM_PAINT => {
                let mut ps: win::PAINTSTRUCT = Default::default();
                let dc = win::BeginPaint(hwnd, &mut ps);
                if !dc.is_null() {
                    if let Some(b) = app.bar.as_ref() { b.paint(dc); }
                    win::EndPaint(hwnd, &ps);
                }
                return 0;
            }
            win::WM_ERASEBKGND => return 1,
            win::WM_MOUSEMOVE => {
                let x = (l & 0xFFFF) as i16 as i32;
                let y = ((l >> 16) & 0xFFFF) as i16 as i32;
                let hover = app.bar.as_ref().map(|b| b.hit(x, y)).unwrap_or(-1);
                if let Some(b) = app.bar.as_mut() {
                    if b.hover != hover { b.hover = hover; win::InvalidateRect(hwnd, std::ptr::null(), 0); }
                }
                return 0;
            }
            win::WM_LBUTTONUP => {
                let x = (l & 0xFFFF) as i16 as i32;
                let y = ((l >> 16) & 0xFFFF) as i16 as i32;
                let a = app.bar.as_ref().map(|b| b.hit(x, y)).unwrap_or(-1);
                app.on_action(a);
                return 0;
            }
            win::WM_KEYDOWN => {
                let a = match w as u32 {
                    win::VK_ESCAPE => overlay::ACTION_CANCEL,
                    win::VK_RETURN => overlay::ACTION_IMAGE,
                    0x31 => overlay::ACTION_IMAGE,
                    0x32 => overlay::ACTION_SAVE,
                    0x33 => overlay::ACTION_TABLE,
                    0x34 => overlay::ACTION_TEXT,
                    _ => -2,
                };
                if a != -2 { app.on_action(a); }
                return 0;
            }
            _ => {}
        }
        return win::DefWindowProcW(hwnd, msg, w, l);
    }

    if is_main {
        match msg {
            win::WM_PAINT => {
                let mut ps: win::PAINTSTRUCT = Default::default();
                let dc = win::BeginPaint(hwnd, &mut ps);
                if !dc.is_null() {
                    let mut rc: win::RECT = Default::default();
                    win::GetClientRect(hwnd, &mut rc);
                    if !paint_main_buffered(dc, app, &rc) {
                        paint_main(dc, app);
                    }
                    win::EndPaint(hwnd, &ps);
                }
                return 0;
            }
            win::WM_ERASEBKGND => return 1,
            win::WM_SETCURSOR if (l & 0xFFFF) as i32 == win::HTCLIENT => {
                // Over the control the EDIT class sets its own I-beam; this
                // covers the padding and logo around it, which are ours.
                let mut pt = win::POINT { x: 0, y: 0 };
                win::GetCursorPos(&mut pt);
                win::ScreenToClient(hwnd, &mut pt);
                if app.drag_active {
                    // Mid-reorder: the move cursor is the only honest signal
                    // that the tile is being carried rather than opened.
                    win::SetCursor(win::LoadCursorW(std::ptr::null_mut(), win::IDC_SIZEALL as *const u16));
                    return 1;
                }
                let mut rc: win::RECT = Default::default();
                win::GetClientRect(hwnd, &mut rc);
                let over_item = tile_hit(app, &rc, pt.x, pt.y).is_some()
                    || row_hit(app, &rc, pt.x, pt.y).is_some()
                    || add_tile_rect(app, &rc).map_or(false, |r| inside(r, pt.x, pt.y))
                    || inside(gear_button_rect(&rc), pt.x, pt.y);
                if let Some(id) = cursor_target(&rc, pt.x, pt.y, over_item) {
                    win::SetCursor(win::LoadCursorW(std::ptr::null_mut(), id as *const u16));
                    return 1;
                }
            }
            win::WM_CLOSE => { app.hide_main(); return 0; }
            win::WM_GETMINMAXINFO => {
                // One size only. Aero Snap (Win+Left, dragging the caption to a
                // screen edge) resizes even a caption-only window, so the limit
                // has to be stated here rather than trusted to the style bits.
                let p = l as *mut win::MINMAXINFO;
                let mut wr: win::RECT = Default::default();
                win::GetWindowRect(hwnd, &mut wr);
                let (ww, wh) = (wr.right - wr.left, wr.bottom - wr.top);
                if !p.is_null() && ww > 0 && wh > 0 {
                    (*p).ptMinTrackSize = win::POINT { x: ww, y: wh };
                    (*p).ptMaxTrackSize = win::POINT { x: ww, y: wh };
                    return 0;
                }
            }
            win::WM_SYSCOMMAND if (w & win::SC_MASK) == win::SC_MAXIMIZE => {
                // The panel's size is derived from the screen, so maximising it
                // would only stretch the 8x4 board. Swallow the command that a
                // title-bar double-click also sends.
                return 0;
            }
            // Typing, paste, selection and IME all belong to the search field;
            // nothing on the panel turns characters into a query any more.
            win::WM_SETFOCUS => {
                // Clicking the board, the logo or the frame must never leave the
                // panel without a caret: the only focusable child is the field.
                app.focus_search();
                return 0;
            }
            win::WM_CTLCOLOREDIT => {
                // The field sits inside the white rounded box the painter draws,
                // so it has to use the same white and the same text colour.
                let dc = w as win::HDC;
                win::SetTextColor(dc, win::rgb(0x1F, 0x1F, 0x1F));
                win::SetBkColor(dc, win::rgb(0xFF, 0xFF, 0xFF));
                return win::GetStockObject(win::WHITE_BRUSH) as isize;
            }
            win::WM_COMMAND
                if (w & 0xFFFF) == searchbox::ID
                    && (((w >> 16) & 0xFFFF) as u32) == win::EN_CHANGE =>
            {
                app.on_query_changed();
                return 0;
            }
            win::WM_KEYDOWN => {
                // Fallback: with the field focused these never arrive here, but
                // a panel that lost focus to a child it did not create still has
                // to answer Esc.
                if app.on_search_key(w as u32) { return 0; }
            }
            win::WM_LBUTTONDOWN => {
                let x = (l & 0xFFFF) as i16 as i32;
                let y = ((l >> 16) & 0xFFFF) as i16 as i32;
                let mut rc: win::RECT = Default::default();
                win::GetClientRect(hwnd, &mut rc);
                if inside(gear_button_rect(&rc), x, y) {
                    app.open_settings_dialog();
                    return 0;
                }
                // The + tile sits in a grid cell, so it has to be tested before
                // the tile hit test: it is drawn inside the board, not above it.
                if add_tile_rect(app, &rc).map_or(false, |r| inside(r, x, y)) {
                    app.open_shortcut_dialog(None);
                    return 0;
                }
                if let Some(i) = tile_hit(app, &rc, x, y) {
                    // Hold the press instead of opening right away: it only
                    // becomes a drag once the pointer actually moves. A plain
                    // click still opens on button-up, below.
                    let (tl, tt) = tile_xy(&rc, item_slot(app, i));
                    app.drag_from = i as i32;
                    app.drag_active = false;
                    app.drag_cur = (x, y);
                    app.drag_offset = (x - tl, y - tt);
                    win::SetCapture(hwnd);
                    return 0;
                }
                if let Some(i) = row_hit(app, &rc, x, y) {
                    let kind = main_rows(app)[i].0.clone();
                    open_row(app, Some(&kind));
                }
                return 0;
            }
            win::WM_LBUTTONUP => {
                if app.drag_from >= 0 {
                    let x = (l & 0xFFFF) as i16 as i32;
                    let y = ((l >> 16) & 0xFFFF) as i16 as i32;
                    let from = app.drag_from as usize;
                    let dragged = app.drag_active;
                    win::ReleaseCapture();
                    app.drag_from = -1;
                    app.drag_active = false;
                    let mut rc: win::RECT = Default::default();
                    win::GetClientRect(hwnd, &mut rc);
                    if dragged {
                        app.move_shortcut(from, tile_drop_slot(&rc, x, y));
                        // The lifted tile has to disappear from under the
                        // cursor even when the drop slot did not change.
                        win::InvalidateRect(hwnd, std::ptr::null(), 0);
                    } else if from < app.shortcuts.len() {
                        open_row(app, Some(&RowKind::Shortcut(from)));
                    }
                }
                return 0;
            }
            win::WM_CAPTURECHANGED => {
                // Alt+Tab or another window stealing the mouse would otherwise
                // leave a tile "held" forever, and the next click would act on
                // a stale index.
                if app.drag_from >= 0 {
                    app.drag_from = -1;
                    app.drag_active = false;
                    win::InvalidateRect(hwnd, std::ptr::null(), 0);
                }
                return 0;
            }
            win::WM_RBUTTONDOWN => {
                let x = (l & 0xFFFF) as i16 as i32;
                let y = ((l >> 16) & 0xFFFF) as i16 as i32;
                let mut rc: win::RECT = Default::default();
                win::GetClientRect(hwnd, &mut rc);
                if let Some(i) = tile_hit(app, &rc, x, y) {
                    show_row_menu(app, hwnd, &RowKind::Shortcut(i));
                    return 0;
                }
                if let Some(i) = row_hit(app, &rc, x, y) {
                    let kind = main_rows(app)[i].0.clone();
                    show_row_menu(app, hwnd, &kind);
                }
                return 0;
            }
            win::WM_MOUSEWHEEL => {
                let mut rc: win::RECT = Default::default();
                win::GetClientRect(hwnd, &mut rc);
                if main_mode(app) == MainMode::Rows {
                    let delta = ((w >> 16) & 0xFFFF) as i16 as i32;
                    let step = if delta > 0 { -3 } else { 3 };
                    let before = clamped_scroll(app, &rc);
                    let max = (main_rows(app).len() as i32 - visible_rows(&rc)).max(0);
                    let after = (before + step).clamp(0, max);
                    if after != before {
                        app.scroll_rows = after;
                        app.hover_row = -1;
                        win::InvalidateRect(hwnd, std::ptr::null(), 0);
                    }
                }
                return 0;
            }
            win::WM_MOUSEMOVE => {
                if !app.tracking_leave {
                    // One TME_LEAVE request; without it the hover plate stays
                    // lit after the cursor leaves, which reads as a glitch.
                    let mut tme = win::TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<win::TRACKMOUSEEVENT>() as u32,
                        dwFlags: win::TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    app.tracking_leave = win::TrackMouseEvent(&mut tme) != 0;
                }
                let x = (l & 0xFFFF) as i16 as i32;
                let y = ((l >> 16) & 0xFFFF) as i16 as i32;
                let mut rc: win::RECT = Default::default();
                win::GetClientRect(hwnd, &mut rc);
                if app.drag_from >= 0 {
                    // While a tile is held, the whole window repaints so the
                    // lifted tile can follow the cursor. The 4px threshold
                    // (16 = 4*4) keeps a normal click from turning into a drag.
                    let (tl, tt) = tile_xy(&rc, item_slot(app, app.drag_from as usize));
                    let dx = x - (tl + app.drag_offset.0);
                    let dy = y - (tt + app.drag_offset.1);
                    app.drag_cur = (x, y);
                    if !app.drag_active && dx * dx + dy * dy >= 16 {
                        app.drag_active = true;
                    }
                    if app.drag_active {
                        win::InvalidateRect(hwnd, std::ptr::null(), 0);
                        return 0;
                    }
                }
                let t = tile_hit(app, &rc, x, y).map(|v| v as i32).unwrap_or(-1);
                let r = row_hit(app, &rc, x, y).map(|v| v as i32).unwrap_or(-1);
                let a = add_tile_rect(app, &rc).map_or(false, |b| inside(b, x, y));
                if t != app.hover_tile || r != app.hover_row || a != app.hover_add {
                    let mut boxes = hover_boxes(app, &rc, app.hover_tile, app.hover_row, app.hover_add);
                    boxes.extend(hover_boxes(app, &rc, t, r, a));
                    app.hover_tile = t;
                    app.hover_row = r;
                    app.hover_add = a;
                    if boxes.is_empty() {
                        win::InvalidateRect(hwnd, std::ptr::null(), 0);
                    } else {
                        invalidate_boxes(hwnd, &boxes);
                    }
                }
            }
            win::WM_MOUSELEAVE => {
                app.tracking_leave = false;
                if app.hover_tile >= 0 || app.hover_row >= 0 || app.hover_add {
                    let mut rc: win::RECT = Default::default();
                    win::GetClientRect(hwnd, &mut rc);
                    let boxes = hover_boxes(app, &rc, app.hover_tile, app.hover_row, app.hover_add);
                    app.hover_tile = -1;
                    app.hover_row = -1;
                    app.hover_add = false;
                    invalidate_boxes(hwnd, &boxes);
                }
                return 0;
            }
            _ => {}
        }
        return win::DefWindowProcW(hwnd, msg, w, l);
    }

    win::DefWindowProcW(hwnd, msg, w, l)
}

#[derive(Clone)]
enum RowKind { Shortcut(usize), File(String) }

/// One list of rows for both drawing and hit-testing, so a click can never
/// open a different item than the one under the cursor.
fn main_rows(app: &App) -> Vec<(RowKind, String, String)> {
    let q = app.query.trim().to_lowercase();
    let mut out = Vec::new();
    if !app.dx_result.is_empty() { return out; }
    if !app.search_hits.is_empty() {
        for p in &app.search_hits {
            let name = p.rsplit(|c| c == '\\' || c == '/').next().unwrap_or(p).to_string();
            let dir = p.rsplit_once(|c| c == '\\' || c == '/')
                .map(|x| x.0.to_string()).unwrap_or_default();
            out.push((RowKind::File(p.clone()), name, dir));
        }
        return out;
    }
    if !q.is_empty() && (app.search_busy || app.search_error.is_some()) { return out; }
    for (i, s) in app.shortcuts.iter().enumerate() {
        if q.is_empty()
            || s.name.to_lowercase().contains(&q)
            || s.path.to_lowercase().contains(&q)
        {
            out.push((RowKind::Shortcut(i), s.display_name(), s.path.clone()));
        }
    }
    out
}

fn working_dir(path: &str) -> String {
    if path.starts_with("shell:") { return String::new(); }
    std::path::Path::new(path).parent()
        .map(|p| p.to_string_lossy().into_owned())
        .filter(|d| std::path::Path::new(d).is_dir())
        .unwrap_or_default()
}

fn launch_shortcut(s: &Shortcut) {
    if let Some(url) = profile::normalized_url(&s.path) {
        open_in_browser(&url, &s.args);
        return;
    }
    let verb = win::wide("open");
    let p = win::wide(&s.path);
    let args = win::wide(&s.args);
    let wd = working_dir(&s.path);
    let wdw = win::wide(&wd);
    unsafe {
        win::ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), p.as_ptr(), args.as_ptr(),
            if wd.is_empty() { std::ptr::null() } else { wdw.as_ptr() }, 5);
    }
}

/// The browser the user chose for http, as an exe path.
///
/// `ShellExecute("open", url)` would also work, but on Windows 10+ it can raise
/// the "which app should open this?" picker — a chooser in front of someone who
/// double-clicked a tile they use every day. Resolving the association by hand
/// (UserChoice ProgId → that ProgId's open command) and launching the exe
/// directly always goes straight to the default browser.
///
/// Only a browser is accepted. A mail client or a music app can legitimately
/// register itself for http, and hijacking a launcher click into a non-browser
/// is worse than a clean failure, so a path outside a browser's install folder
/// is rejected here and in `open_in_browser`.
fn default_browser_exe() -> Option<String> {
    let progid = win::reg_read_string(win::HKEY_CURRENT_USER_KEY,
        "Software\\Microsoft\\Windows\\Shell\\Associations\\UrlAssociations\\http\\UserChoice",
        "ProgId")?;
    let cmd = win::reg_read_progid_command(&progid)?;
    let exe = command_exe_path(&cmd)?;
    resolve_browser_exe(&exe).filter(|p| looks_like_browser(p))
}

/// Turn whatever the registration names into a file that exists. A UWP-registered
/// browser points at its `WindowsApps` execution alias, which is not a real
/// image, but the store package folder beside it holds the true exe — which is
/// also the path the browser gives itself when it relaunches after updating, so
/// launching it is what the browser would have done anyway.
fn resolve_browser_exe(named: &str) -> Option<String> {
    if std::path::Path::new(named).exists() { return Some(named.to_string()); }
    let low = named.to_ascii_lowercase().replace('/', "\\");
    let i = low.find("\\windowsapps\\")?;
    let tail = &named[i + "\\windowsapps\\".len()..];
    let file = tail.split('\\').next().unwrap_or("");
    if !file.to_ascii_lowercase().ends_with(".exe") { return None; }
    let base = named.get(..i)?; // …\AppData\Local\Microsoft
    let packages = format!("{}\\Packages", base);
    let stem = file[..file.len() - 4].to_ascii_lowercase();
    let mut found: Option<String> = None;
    if let Ok(rd) = std::fs::read_dir(&packages) {
        for entry in rd.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.to_ascii_lowercase().contains(&stem) { continue; }
            let cand = format!("{}\\LocalCache\\Local\\Microsoft\\WindowsApps\\{}",
                entry.path().display(), file);
            if std::path::Path::new(&cand).exists() {
                let key = cand.to_ascii_lowercase();
                if found.as_ref().map(|f| key < f.to_ascii_lowercase()).unwrap_or(true) {
                    found = Some(cand);
                }
            }
        }
    }
    found
}

/// The exe inside a registered open command: a quoted path, or the bare path up
/// to the first space when the value is unquoted (`%1` variants included).
fn command_exe_path(cmd: &str) -> Option<String> {
    let s = cmd.trim();
    if let Some(rest) = s.strip_prefix('"') {
        let end = rest.find('"')?;
        return Some(rest[..end].to_string());
    }
    if s.contains("%1") {
        // `C:\dir\app.exe %1` unquoted: the path itself has no space then,
        // otherwise the registration would already be broken.
        let head = s.split('%').next().unwrap_or("").trim();
        if head.to_ascii_lowercase().ends_with(".exe") { return Some(head.to_string()); }
    }
    s.split_whitespace().next().map(|x| x.to_string())
}

/// Path shape test for a browser install, not a name test. Edge's ProgId
/// command can name the exe through the `WindowsApps` alias, which is not a
/// real file, and `C:\Program Files\Google\Chrome\Application\chrome.exe`
/// beats any guess from the ProgId string.
fn looks_like_browser(exe: &str) -> bool {
    // Install roots of the common engines, all browsers included.
    const ROOTS: [&str; 6] = ["\\program files (x86)\\microsoft\\edge\\",
        "\\program files\\microsoft\\edge\\",
        "\\program files\\google\\chrome\\",
        "\\program files (x86)\\google\\chrome\\",
        "\\program files\\mozilla firefox\\",
        "\\program files (x86)\\mozilla firefox\\"];
    let low = exe.to_ascii_lowercase().replace('/', "\\");
    if ROOTS.iter().any(|r| low.contains(r)) { return true; }
    // UWP-registered browsers: …\AppData\Local\Microsoft\WindowsApps\*
    low.contains("\\windowsapps\\")
}

/// Open a URL in the default browser, falling back to the shell when the
/// association cannot be resolved to a browser exe.
fn open_in_browser(url: &str, extra_args: &str) {
    if let Some(exe) = default_browser_exe() {
        // The URL goes inside the quoted command line below, so a quote of its
        // own would end the argument early and hand the rest to the browser as
        // separate switches. Percent-encoding is the URL-correct equivalent.
        let url = url.replace('"', "%22");
        let mut line = format!("\"{}\"", exe);
        if !extra_args.trim().is_empty() {
            line.push(' ');
            line.push_str(extra_args.trim());
        }
        line.push_str(&format!(" \"{}\"", url));
        let mut cmd: Vec<u16> = line.encode_utf16().chain(std::iter::once(0)).collect();
        let ok = unsafe {
            win::CreateProcessW(std::ptr::null(), cmd.as_mut_ptr(), std::ptr::null_mut(),
                std::ptr::null_mut(), 0, 0, std::ptr::null_mut(), std::ptr::null(),
                std::ptr::null_mut(), std::ptr::null_mut())
        };
        if ok != 0 { return; }
    }
    let verb = win::wide("open");
    let u = win::wide(url);
    unsafe {
        win::ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), u.as_ptr(),
            std::ptr::null(), std::ptr::null(), 5);
    }
}

fn open_path_location(path: &str) {
    let verb = win::wide("open");
    let exe = win::wide("explorer.exe");
    let arg = if std::path::Path::new(path).is_dir() {
        format!("\"{}\"", path)
    } else {
        format!("/select,\"{}\"", path)
    };
    let a = win::wide(&arg);
    unsafe {
        win::ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), exe.as_ptr(), a.as_ptr(),
            std::ptr::null(), 5);
    }
}

fn open_row(app: &mut App, kind: Option<&RowKind>) {
    match kind {
        Some(RowKind::Shortcut(i)) => {
            let s = app.shortcuts.get(*i).cloned();
            if let Some(s) = s { launch_shortcut(&s); app.hide_main(); }
        }
        Some(RowKind::File(p)) => { open_with_shell(p); app.hide_main(); }
        None => {}
    }
}

const ROW_OPEN: i32 = 201;
const ROW_EDIT: i32 = 202;
const ROW_COPY: i32 = 203;
const ROW_LOCATION: i32 = 204;
const ROW_DELETE: i32 = 205;

fn show_row_menu(app: &mut App, hwnd: win::HWND, kind: &RowKind) {
    unsafe {
        let menu = win::CreatePopupMenu();
        if menu.is_null() { return; }
        let entries: &[(i32, &str)] = match kind {
            RowKind::Shortcut(_) => &[
                (ROW_OPEN, "打开"), (ROW_EDIT, "编辑"), (ROW_COPY, "复制路径"),
                (ROW_LOCATION, "打开所在位置"), (ROW_DELETE, "删除"),
            ],
            RowKind::File(_) => &[
                (ROW_OPEN, "打开"), (ROW_LOCATION, "打开所在位置"), (ROW_COPY, "复制完整路径"),
            ],
        };
        for (id, text) in entries {
            let t = win::wide(text);
            win::AppendMenuW(menu, win::MF_STRING, *id as usize, t.as_ptr());
        }
        let mut pt: win::POINT = Default::default();
        win::GetCursorPos(&mut pt);
        let cmd = win::TrackPopupMenu(menu,
            win::TPM_RETURNCMD | win::TPM_RIGHTBUTTON | win::TPM_NONOTIFY,
            pt.x, pt.y, 0, hwnd, std::ptr::null());
        win::DestroyMenu(menu);

        let path = match kind {
            RowKind::Shortcut(i) => app.shortcuts.get(*i).map(|s| s.path.clone()).unwrap_or_default(),
            RowKind::File(p) => p.clone(),
        };
        match cmd {
            ROW_OPEN => open_row(app, Some(kind)),
            ROW_EDIT => if let RowKind::Shortcut(i) = kind { app.open_shortcut_dialog(Some(*i)); },
            ROW_COPY => { let _ = capture::copy_text(&path); app.toast("路径已复制", true, 1800); }
            ROW_LOCATION => open_path_location(&path),
            ROW_DELETE => if let RowKind::Shortcut(i) = kind {
                if *i < app.shortcuts.len() && dialogs::confirm(hwnd, "确定删除这个快捷方式？", "TT酱") {
                    app.shortcuts.remove(*i);
                    let _ = profile::save_all(&app.shortcuts, &app.settings);
                    app.refresh_main();
                }
            },
            _ => {}
        }
    }
}

// ---- main window: 3.3 layout --------------------------------------------
//
// The 3.3 window is a light panel with a dashed search box (brand mark on the
// left), a wrap grid of 110x108 shortcut tiles, and a 46px result list that
// replaces the grid while searching. An earlier Rust pass drew plain text
// rows, which read like Explorer's "details" view; these constants and the
// painters below keep the original geometry so the two builds look the same.

const MAIN_MARGIN: i32 = 14;
const INNER_PAD_X: i32 = 22;
const INNER_PAD_TOP: i32 = 18;
const INNER_PAD_BOTTOM: i32 = 16;
const SEARCH_H: i32 = 46;
const TILE_GAP: i32 = 2;
const ROW_H: i32 = 46;

/// The grid is a fixed 8x4 board of 32 cells; the panel is sized around it.
/// The "+ 添加" tile lives in one of those cells, so it moves to whatever cell
/// is free. There is no separate cap on the number of shortcuts: the board is
/// the cap, and a full board simply leaves no cell for the + to sit in.
const GRID_COLS: i32 = 8;
const GRID_ROWS: i32 = 4;
const GRID_CELLS: usize = (GRID_COLS * GRID_ROWS) as usize;

/// Frame of the launcher: caption, menu, minimise box — deliberately without
/// WS_THICKFRAME. A dragged edge would only shear the fixed grid, so the size
/// follows the screen and nothing else.
/// `WS_CLIPCHILDREN` is not cosmetic: the panel paints into one off-screen DIB
/// and blits it in a single call, so without this bit the blit would paint over
/// the search field's rectangle every frame.
const PANEL_STYLE: u32 = win::WS_CAPTION | win::WS_SYSMENU | win::WS_MINIMIZEBOX
    | win::WS_CLIPCHILDREN;

/// Client size of the panel: a fixed share of the primary work area, so the
/// launcher takes up the same fraction of the screen on a 1080p laptop and on
/// a 4K monitor. Clamped so an unusual resolution cannot produce a panel that
/// is unusable at either end.
///
/// The shares are 40% x 52% rather than 52% x 64% because the cells came down
/// by a quarter: the chrome around the grid (search field, footer, margins) is
/// fixed, so shrinking the window by 25% would have squeezed the cells by 34%
/// vertically. With these numbers a cell lands at 75% of its old width and
/// height on a 1080p screen.
fn panel_client_size() -> (i32, i32) {
    let wa = win::primary_work_area();
    let w = (wa.w * 40 / 100).clamp(700, 1280);
    let h = (wa.h * 52 / 100).clamp(460, 800);
    (w, h)
}

/// How far below the optical centre the panel opens, in percent of the work
/// area height. Dead centre reads as slightly high on a large screen, so the
/// launcher drops a tenth of the screen and lets the eye land on the grid.
const PANEL_DROP_PCT: i32 = 10;

#[derive(Clone, Copy, PartialEq)]
enum MainMode { Tiles, Rows, Dx }

fn main_mode(app: &App) -> MainMode {
    if !app.dx_result.is_empty() { return MainMode::Dx; }
    if !app.query.trim().is_empty() { return MainMode::Rows; }
    if !app.search_hits.is_empty() { return MainMode::Rows; }
    MainMode::Tiles
}

fn inner_left() -> i32 { MAIN_MARGIN + INNER_PAD_X }
fn inner_right(rc: &win::RECT) -> i32 { rc.right - MAIN_MARGIN - INNER_PAD_X }
fn content_top() -> i32 { MAIN_MARGIN + INNER_PAD_TOP + SEARCH_H + 18 }

/// One cell: whatever is left between the search box and the footer, split
/// 8x4. Deriving the size instead of hard-coding 110x108 is what lets the
/// panel grow with the screen while the cell count stays fixed.
fn tile_size(rc: &win::RECT) -> (i32, i32) {
    let w = (inner_right(rc) - inner_left() - (GRID_COLS - 1) * TILE_GAP) / GRID_COLS;
    let h = (grid_bottom(rc) - content_top() - (GRID_ROWS - 1) * TILE_GAP) / GRID_ROWS;
    (w.max(56), h.max(64))
}

/// Side of the icon plate. Tracks the cell width (~44%, as 48px in a 110px
/// tile always was) so a bigger screen brings bigger icons, not more air.
fn icon_size(rc: &win::RECT) -> i32 {
    let (tw, _) = tile_size(rc);
    (tw * 44 / 100).clamp(32, 72)
}

/// Height reserved under the icon for the caption band.
const CAPTION_BAND: i32 = 26;

/// Top-left of the icon plate inside a cell, with icon + caption centred as a
/// block so the label never drifts away from the icon on a tall cell.
fn icon_origin(rc: &win::RECT, l: i32, t: i32) -> (i32, i32) {
    let (tw, th) = tile_size(rc);
    let icon = icon_size(rc);
    let block = icon + 8 + CAPTION_BAND;
    (l + (tw - icon) / 2, t + ((th - block) / 2).max(4))
}

/// Hard cap on grid slots so a corrupt `pos` in shortcuts.json cannot make the
/// launcher allocate or iterate over an absurd grid.
const MAX_SLOTS: usize = 400;

/// Grid slot -> shortcut index.
///
/// Shortcuts stay in the slot they were dropped in (`pos`); everything without
/// one — a new entry, or a pos that no longer fits after a delete — fills the
/// lowest free slot in array order. That keeps pre-4.1 configs (no pos at all)
/// laid out exactly as before, while letting a user leave gaps in the grid.
fn slot_map_of(shortcuts: &[profile::Shortcut]) -> Vec<Option<usize>> {
    let n = shortcuts.len();
    if n == 0 { return Vec::new(); }
    let mut len = n;
    for s in shortcuts {
        if s.pos >= 0 {
            let p = s.pos as usize;
            if p < MAX_SLOTS { len = len.max(p + 1); }
        }
    }
    let mut map: Vec<Option<usize>> = vec![None; len];
    for (i, s) in shortcuts.iter().enumerate() {
        if s.pos < 0 { continue; }
        let p = s.pos as usize;
        if p < len && map[p].is_none() { map[p] = Some(i); }
    }
    for i in 0..n {
        if map.iter().any(|v| *v == Some(i)) { continue; }
        match map.iter().position(|v| v.is_none()) {
            Some(k) => map[k] = Some(i),
            None => map.push(Some(i)),
        }
    }
    map
}

fn slot_map(app: &App) -> Vec<Option<usize>> { slot_map_of(&app.shortcuts) }

/// Which grid slot a shortcut currently occupies.
fn item_slot(app: &App, item: usize) -> usize {
    slot_map(app).iter().position(|v| *v == Some(item)).unwrap_or(item)
}

/// Pin every shortcut to the slot it is showing right now. Called before the
/// first drop so that moving one tile cannot drag the auto-placed ones along
/// with it.
fn materialize_slots(shortcuts: &mut [profile::Shortcut]) {
    let map = slot_map_of(shortcuts);
    for (slot, item) in map.iter().enumerate() {
        if let Some(i) = *item { shortcuts[i].pos = slot as i32; }
    }
}

/// Keep every stored slot inside the fixed board. A pos written when the grid
/// had more cells would otherwise have nowhere to be drawn and the shortcut
/// would silently vanish from the panel; falling back to auto-placement puts it
/// in the first free cell instead.
fn normalize_slots(shortcuts: &mut [profile::Shortcut]) {
    for s in shortcuts.iter_mut() {
        if s.pos >= GRID_CELLS as i32 { s.pos = -1; }
    }
}

/// Drop shortcut `item` onto grid slot `slot`: an occupied slot swaps, a free
/// one is simply taken, which is how the user opens a gap.
fn place_at(shortcuts: &mut Vec<profile::Shortcut>, item: usize, slot: usize) {
    if item >= shortcuts.len() || slot >= MAX_SLOTS { return; }
    materialize_slots(shortcuts);
    let slots = slot_map_of(shortcuts);
    let occupant = slots.get(slot).copied().flatten();
    if occupant == Some(item) { return; }
    let origin = slots.iter().position(|v| *v == Some(item)).unwrap_or(slot);
    if let Some(occ) = occupant {
        // The tile that was there slides back to where the dragged one came
        // from, so a swap never drops anything on the floor.
        shortcuts[occ].pos = origin as i32;
    }
    shortcuts[item].pos = slot as i32;
}

/// Screen-independent tile rectangle for grid slot `slot`. The column count is
/// the fixed 8, never derived from the width — that is the point of the board.
fn tile_xy(rc: &win::RECT, idx: usize) -> (i32, i32) {
    let (tw, th) = tile_size(rc);
    let c = (idx as i32) % GRID_COLS;
    let r = (idx as i32) / GRID_COLS;
    (
        inner_left() + c * (tw + TILE_GAP),
        content_top() + r * (th + TILE_GAP),
    )
}

fn row_top(idx: usize) -> i32 { content_top() + (idx as i32) * ROW_H }

/// Bottom of the content area: the grid board ends here.
fn content_bottom(rc: &win::RECT) -> i32 { grid_bottom(rc) }

fn visible_rows(rc: &win::RECT) -> i32 {
    ((content_bottom(rc) - content_top()) / ROW_H).max(1)
}

/// Clamps the scroll offset after the result set shrinks.
fn clamped_scroll(app: &App, rc: &win::RECT) -> i32 {
    let max = (main_rows(app).len() as i32 - visible_rows(rc)).max(0);
    app.scroll_rows.clamp(0, max)
}

fn tile_hit(app: &App, rc: &win::RECT, x: i32, y: i32) -> Option<usize> {
    if main_mode(app) != MainMode::Tiles { return None; }
    let (tw, th) = tile_size(rc);
    for (slot, item) in slot_map(app).iter().enumerate() {
        let Some(i) = *item else { continue };
        let (tl, tt) = tile_xy(rc, slot);
        if x >= tl && x < tl + tw && y >= tt && y < tt + th {
            return Some(i);
        }
    }
    None
}

/// Cell the dragged tile would land on. Unlike a reorder target this is not
/// capped at the item count: dropping on an empty cell is exactly how a gap is
/// created. Coordinates outside the grid clamp to the nearest cell — and to
/// the last of the 32 cells, never beyond it.
fn tile_drop_slot(rc: &win::RECT, x: i32, y: i32) -> usize {
    let (tw, th) = tile_size(rc);
    let col = ((x - inner_left()) / (tw + TILE_GAP)).clamp(0, GRID_COLS - 1);
    let row = ((y - content_top()) / (th + TILE_GAP)).clamp(0, GRID_ROWS - 1);
    (row * GRID_COLS + col) as usize
}

fn row_hit(app: &App, rc: &win::RECT, x: i32, y: i32) -> Option<usize> {
    if main_mode(app) != MainMode::Rows { return None; }
    let rows = main_rows(app);
    if y < content_top() || y >= content_bottom(rc) { return None; }
    let idx = clamped_scroll(app, rc) + (y - content_top()) / ROW_H;
    if idx < 0 || (idx as usize) >= rows.len() { return None; }
    let _ = x;
    Some(idx as usize)
}

/// Footer hit boxes, shared by the painter and the mouse handler so a click
/// can never land on a different control than the one drawn.
fn footer_baseline(rc: &win::RECT) -> i32 { rc.bottom - MAIN_MARGIN - INNER_PAD_BOTTOM }

/// Top of the footer strip. The board stops 10px above it, which is what keeps
/// the gear button from ever sitting on the last row of cells.
fn footer_top(rc: &win::RECT) -> i32 { footer_baseline(rc) - 34 }

fn grid_bottom(rc: &win::RECT) -> i32 { footer_top(rc) - 10 }

/// The search box, same geometry as paint_main. The text itself belongs to the
/// EDIT child (see `searchbox`), but the box around it is painted here, and a
/// click on that padding has to read as a click into the field.
fn search_rect(rc: &win::RECT) -> (i32, i32, i32, i32) {
    let top = MAIN_MARGIN + INNER_PAD_TOP;
    (inner_left(), top, inner_right(rc), top + SEARCH_H)
}

/// Cell rectangle for slot `slot`; used for hover plates, hit tests and the
/// drag drop outline.
fn tile_rect(rc: &win::RECT, idx: usize) -> (i32, i32, i32, i32) {
    let (l, t) = tile_xy(rc, idx);
    let (tw, th) = tile_size(rc);
    (l, t, l + tw, t + th)
}

/// Cell the "+ 添加" tile occupies: the first free cell, so it follows the
/// icons instead of living in a corner. `None` once all 32 cells are taken —
/// there is then no room for it, and no room to add anything either.
fn add_slot_of(shortcuts: &[profile::Shortcut]) -> Option<usize> {
    let map = slot_map_of(shortcuts);
    let n = map.len().min(GRID_CELLS);
    if let Some(p) = map.iter().take(n).position(|v| v.is_none()) { return Some(p); }
    if map.len() < GRID_CELLS { return Some(map.len()); }
    None
}

fn add_slot(app: &App) -> Option<usize> { add_slot_of(&app.shortcuts) }

fn add_tile_rect_of(shortcuts: &[profile::Shortcut], rc: &win::RECT) -> Option<(i32, i32, i32, i32)> {
    add_slot_of(shortcuts).map(|slot| tile_rect(rc, slot))
}

fn add_tile_rect(app: &App, rc: &win::RECT) -> Option<(i32, i32, i32, i32)> {
    add_tile_rect_of(&app.shortcuts, rc)
}

fn row_rect(idx: usize) -> (i32, i32) {
    let t = row_top(idx);
    (t, t + ROW_H)
}

/// Cursor for a client point; `None` leaves the default arrow in place.
///
/// The field wins over the item test: a click that starts in the search box
/// must never read as a shortcut click, so the shapes can never disagree with
/// what the click handler will do.
fn cursor_target(rc: &win::RECT, x: i32, y: i32, over_item: bool) -> Option<usize> {
    if inside(search_rect(rc), x, y) {
        Some(win::IDC_IBEAM)
    } else if over_item {
        Some(win::IDC_HAND)
    } else {
        None
    }
}

fn gear_button_rect(rc: &win::RECT) -> (i32, i32, i32, i32) {
    let right = inner_right(rc);
    let bottom = footer_baseline(rc);
    (right - 30, bottom - 14 - 30, right, bottom - 14)
}

fn inside(r: (i32, i32, i32, i32), x: i32, y: i32) -> bool {
    x >= r.0 && x < r.2 && y >= r.1 && y < r.3
}

/// Boxes a given hover state occupies, so a state change repaints only those.
/// `-1` (nothing hovered) contributes nothing.
fn hover_boxes(app: &App, rc: &win::RECT, tile: i32, row: i32, add: bool) -> Vec<(i32, i32, i32, i32)> {
    let mut boxes = Vec::with_capacity(3);
    if tile >= 0 && (tile as usize) < app.shortcuts.len() {
        boxes.push(tile_rect(rc, item_slot(app, tile as usize)));
    }
    if row >= 0 {
        let (top, bottom) = row_rect(row as usize);
        boxes.push((inner_left() - 2, top, inner_right(rc) + 2, bottom));
    }
    if add {
        if let Some(r) = add_tile_rect(app, rc) { boxes.push(r); }
    }
    boxes
}

/// Repaints only the given boxes, grown by 2px for the rounded corners.
///
/// The hover state used to invalidate the whole client area, which repainted
/// every tile icon for a 110px plate change. Crossing the grid now touches at
/// most the two boxes whose state actually changed.
fn invalidate_boxes(hwnd: win::HWND, boxes: &[(i32, i32, i32, i32)]) {
    for b in boxes {
        let rc = win::RECT { left: b.0 - 2, top: b.1 - 2, right: b.2 + 2, bottom: b.3 + 2 };
        unsafe { win::InvalidateRect(hwnd, &rc, 0); }
    }
}

/// Paints the main panel into an off-screen DIB and pushes it with one BitBlt.
///
/// Drawing straight into the window DC made the icons blink while the cursor
/// swept the grid: every tile issues a RoundRect plus an AlphaBlend, and the
/// driver hands those to the screen one call at a time, so the frame is
/// briefly half-painted. A single blit makes the update atomic.
unsafe fn paint_main_buffered(dc: win::HDC, app: &App, rc: &win::RECT) -> bool {
    let w = rc.right.max(1);
    let h = rc.bottom.max(1);
    let mem = win::CreateCompatibleDC(dc);
    if mem.is_null() { return false; }
    let bi = win::BITMAPINFOHEADER {
        biSize: std::mem::size_of::<win::BITMAPINFOHEADER>() as u32,
        biWidth: w,
        biHeight: -h, // top-down: same row order as the painting below
        biPlanes: 1,
        biBitCount: 32,
        biCompression: 0,
        biSizeImage: 0,
        biXPelsPerMeter: 0,
        biYPelsPerMeter: 0,
        biClrUsed: 0,
        biClrImportant: 0,
    };
    let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
    let hbm = win::CreateDIBSection(mem, &bi, 0, &mut bits, std::ptr::null_mut(), 0);
    if hbm.is_null() || bits.is_null() {
        win::DeleteDC(mem);
        return false;
    }
    let old = win::SelectObject(mem, hbm);
    paint_main(mem, app);
    win::GdiFlush();
    win::BitBlt(dc, 0, 0, w, h, mem, 0, 0, win::SRCCOPY);
    win::SelectObject(mem, old);
    win::DeleteObject(hbm);
    win::DeleteDC(mem);
    true
}

unsafe fn fill_box(dc: win::HDC, l: i32, t: i32, r: i32, b: i32, color: u32) {
    if r <= l || b <= t { return; }
    let br = win::CreateSolidBrush(color);
    let rc = win::RECT { left: l, top: t, right: r, bottom: b };
    win::FillRect(dc, &rc, br);
    win::DeleteObject(br);
}

unsafe fn round_box(dc: win::HDC, l: i32, t: i32, r: i32, b: i32, rad: i32,
                    fill: u32, border: Option<u32>, dash: bool) {
    let br = win::CreateSolidBrush(fill);
    let pen = match border {
        Some(c) => win::CreatePen(if dash { 1 } else { 0 }, 1, c),
        None => win::GetStockObject(8), // NULL_PEN
    };
    let old_b = win::SelectObject(dc, br);
    let old_p = win::SelectObject(dc, pen);
    win::RoundRect(dc, l, t, r, b, rad, rad);
    win::SelectObject(dc, old_p);
    win::SelectObject(dc, old_b);
    win::DeleteObject(br);
    if border.is_some() { win::DeleteObject(pen); }
}

/// Shortcut tile: 48px icon on a rounded hover plate, name below, both
/// centred. Missing icons fall back to the first character of the name, which
/// is what 3.3 does instead of showing an empty slot.
unsafe fn paint_tile(dc: win::HDC, app: &App, rc: &win::RECT, l: i32, t: i32, idx: usize) {
    let (tw, th) = tile_size(rc);
    let hovered = app.hover_tile == idx as i32;
    if hovered {
        round_box(dc, l, t, l + tw, t + th, 4, win::rgb(0xE3, 0xEE, 0xFB), None, false);
    }
    let icon = icon_size(rc);
    let (icon_x, icon_y) = icon_origin(rc, l, t);
    round_box(dc, icon_x, icon_y, icon_x + icon, icon_y + icon, 8,
        win::rgb(0xEA, 0xF2, 0xFD), None, false);

    let s = &app.shortcuts[idx];
    let mut drew = false;
    // An icon the user picked wins over the target's own; web links have no
    // target icon to extract, so this is also the only way one gets a logo.
    let custom = s.icon.trim();
    if !custom.is_empty() {
        if let Some(img) = image::icon_from_image_file(custom, icon) {
            img.blend_onto(dc, icon_x, icon_y);
            drew = true;
        }
    }
    if !drew {
        if let Some(img) = image::icon_for_path(&s.path, icon) {
            img.blend_onto(dc, icon_x, icon_y);
            drew = true;
        }
    }
    if !drew {
        // A link falls back to its domain's first letter rather than the name's,
        // so www.baidu.com and www.bilibili.com do not both become a W.
        let ch = if profile::target_of(&s.path) == profile::Target::Web {
            profile::link_initial(&s.path, &s.display_name())
        } else {
            s.display_name().chars().next().map(|c| c.to_string()).unwrap_or_else(|| "?".into())
        };
        let f = win::font((icon * 42 / 100).clamp(14, 30), false);
        let old_f = win::SelectObject(dc, f);
        win::SetBkMode(dc, win::TRANSPARENT);
        win::SetTextColor(dc, win::rgb(0x0F, 0x6C, 0xBD));
        let mut rc = win::RECT { left: icon_x, top: icon_y, right: icon_x + icon, bottom: icon_y + icon };
        let w = win::wide(&ch);
        win::DrawTextW(dc, w.as_ptr(), 1, &mut rc, win::DT_CENTER | win::DT_VCENTER | win::DT_SINGLELINE);
        win::SelectObject(dc, old_f);
    }

    let name = win::font(12, false);
    let old_f = win::SelectObject(dc, name);
    win::SetBkMode(dc, win::TRANSPARENT);
    win::SetTextColor(dc, win::rgb(0x1F, 0x1F, 0x1F));
    let mut nrc = win::RECT {
        left: l + 4, top: icon_y + icon + 8,
        right: l + tw - 4, bottom: icon_y + icon + 8 + CAPTION_BAND,
    };
    let wt = win::wide(&s.display_name());
    win::DrawTextW(dc, wt.as_ptr(), (wt.len() - 1) as i32, &mut nrc,
        win::DT_CENTER | win::DT_SINGLELINE | win::DT_VCENTER | win::DT_END_ELLIPSIS);
    win::SelectObject(dc, old_f);
}

/// The "+ 添加" tile. It is a cell like any other — same plate, same caption
/// band — so the grid reads as one board instead of a board plus a button.
unsafe fn paint_add_tile(dc: win::HDC, rc: &win::RECT, app: &App, r: (i32, i32, i32, i32)) {
    let (l, t, rb, b) = r;
    if app.hover_add {
        round_box(dc, l, t, rb, b, 4, win::rgb(0xE3, 0xEE, 0xFB), None, false);
    }
    let icon = icon_size(rc);
    let (ix, iy) = icon_origin(rc, l, t);
    // Empty plate with a hairline: an empty slot with an invitation in it,
    // rather than a filled icon that would read as an installed app.
    let fill = if app.hover_add { win::rgb(0xF4, 0xF9, 0xFF) } else { win::rgb(0xFF, 0xFF, 0xFF) };
    let edge = if app.hover_add { win::rgb(0x6F, 0xA8, 0xDC) } else { win::rgb(0xC9, 0xC9, 0xC9) };
    round_box(dc, ix, iy, ix + icon, iy + icon, 8, fill, Some(edge), true);

    let color = if app.hover_add { win::rgb(0x0F, 0x6C, 0xBD) } else { win::rgb(0x6B, 0x6B, 0x6B) };
    let f = win::font((icon * 55 / 100).clamp(18, 40), false);
    let old_f = win::SelectObject(dc, f);
    win::SetBkMode(dc, win::TRANSPARENT);
    win::SetTextColor(dc, color);
    let mut prc = win::RECT { left: ix, top: iy, right: ix + icon, bottom: iy + icon };
    let plus = win::wide("+");
    win::DrawTextW(dc, plus.as_ptr(), 1, &mut prc, win::DT_CENTER | win::DT_VCENTER | win::DT_SINGLELINE);
    win::SelectObject(dc, old_f);

    let (tw, _) = tile_size(rc);
    let lf = win::font(12, false);
    let old_lf = win::SelectObject(dc, lf);
    win::SetBkMode(dc, win::TRANSPARENT);
    win::SetTextColor(dc, if app.hover_add { win::rgb(0x0F, 0x6C, 0xBD) } else { win::rgb(0x9A, 0x9A, 0x9A) });
    let mut lrc = win::RECT {
        left: l + 4, top: iy + icon + 8,
        right: l + tw - 4, bottom: iy + icon + 8 + CAPTION_BAND,
    };
    let lbl = win::wide("添加");
    win::DrawTextW(dc, lbl.as_ptr(), 2, &mut lrc,
        win::DT_CENTER | win::DT_SINGLELINE | win::DT_VCENTER);
    win::SelectObject(dc, old_lf);
}

unsafe fn paint_main(dc: win::HDC, app: &App) {
    let mut rc: win::RECT = Default::default();
    win::GetClientRect(app.main, &mut rc);
    let mode = main_mode(app);

    // Window background + the light panel with a 1px border, matching the
    // Border/CornerRadius 8 wrapper in MainWindow.xaml.
    fill_box(dc, 0, 0, rc.right, rc.bottom, win::rgb(0xFF, 0xFF, 0xFF));
    round_box(dc, MAIN_MARGIN, MAIN_MARGIN, rc.right - MAIN_MARGIN, rc.bottom - MAIN_MARGIN,
        8, win::rgb(0xFA, 0xFA, 0xFA), Some(win::rgb(0xE3, 0xE3, 0xE3)), false);

    let left = inner_left();
    let right = inner_right(&rc);
    let st = MAIN_MARGIN + INNER_PAD_TOP; // search box top

    // ---- search box: white fill, dashed border, brand mark, divider -------
    round_box(dc, left, st, right, st + SEARCH_H, 6,
        win::rgb(0xFF, 0xFF, 0xFF), Some(win::rgb(0xC9, 0xC9, 0xC9)), true);
    let logo_x = left + 8;
    let logo_y = st + (SEARCH_H - 30) / 2;
    match &app.logo {
        Some(img) => img.blend_onto(dc, logo_x, logo_y),
        None => {
            let f = win::make_font(16, true);
            let old_f = win::SelectObject(dc, f);
            win::SetBkMode(dc, win::TRANSPARENT);
            win::SetTextColor(dc, win::rgb(0x0F, 0x6C, 0xBD));
            let w = win::wide("T");
            win::TextOutW(dc, logo_x + 9, logo_y + 6, w.as_ptr(), 1);
            win::SelectObject(dc, old_f);
            win::DeleteObject(f);
        }
    }
    fill_box(dc, left + 44, st + 13, left + 45, st + SEARCH_H - 13, win::rgb(0xE8, 0xE8, 0xE8));

    // The query itself is not painted here: the field is an EDIT control
    // clipped out of this blit (WS_CLIPCHILDREN), so it draws its own text,
    // caret, selection and placeholder.

    // ---- footer ----------------------------------------------------------
    let bottom = footer_baseline(&rc);
    let gear_r = gear_button_rect(&rc);
    let (gear_l, gear_t) = (gear_r.0, gear_r.1);
    let gear = gear_r.2 - gear_r.0;
    round_box(dc, gear_l, gear_t, gear_l + gear, gear_t + gear, 6,
        win::rgb(0xF2, 0xF5, 0xF9), Some(win::rgb(0xE3, 0xE3, 0xE3)), false);
    {
        let f = win::make_font(15, false);
        let old_f = win::SelectObject(dc, f);
        win::SetBkMode(dc, win::TRANSPARENT);
        win::SetTextColor(dc, win::rgb(0x6B, 0x6B, 0x6B));
        let mut grc = win::RECT { left: gear_l, top: gear_t, right: gear_l + gear, bottom: gear_t + gear };
        let g = win::wide("\u{2699}");
        win::DrawTextW(dc, g.as_ptr(), 1, &mut grc, win::DT_CENTER | win::DT_VCENTER | win::DT_SINGLELINE);
        win::SelectObject(dc, old_f);
        win::DeleteObject(f);
    }
    // The add control is no longer a footer button: it lives in a grid cell
    // (see paint_add_tile), so the footer holds only the gear and the status
    // line.

    let status_font = win::make_font(11, false);
    let old_s = win::SelectObject(dc, status_font);
    win::SetBkMode(dc, win::TRANSPARENT);
    win::SetTextColor(dc, win::rgb(0x9A, 0x9A, 0x9A));
    let status = if !app.search_hits.is_empty() {
        format!("文件搜索：{} 条结果", app.search_hits.len())
    } else if let Some(e) = &app.search_error {
        e.clone()
    } else {
        "Ctrl+1 截图   ·   Esc 隐藏".to_string()
    };
    let mut src = win::RECT {
        left: left + 2, top: bottom - 20, right: gear_l - 16, bottom: bottom,
    };
    let sw = win::wide(&status);
    win::DrawTextW(dc, sw.as_ptr(), (sw.len() - 1) as i32, &mut src,
        win::DT_SINGLELINE | win::DT_VCENTER | win::DT_END_ELLIPSIS | win::DT_NOCLIP);
    win::SelectObject(dc, old_s);
    win::DeleteObject(status_font);

    // ---- content ---------------------------------------------------------
    let content_floor = grid_bottom(&rc);
    match mode {
        MainMode::Dx => {
            let f = win::make_font(28, true);
            let old_f = win::SelectObject(dc, f);
            win::SetBkMode(dc, win::TRANSPARENT);
            win::SetTextColor(dc, win::rgb(0x0F, 0x6C, 0xBD));
            let mut drc = win::RECT { left: left, top: content_top() + 40, right: right, bottom: content_top() + 90 };
            let d = win::wide(&app.dx_result);
            win::DrawTextW(dc, d.as_ptr(), (d.len() - 1) as i32, &mut drc,
                win::DT_CENTER | win::DT_SINGLELINE | win::DT_VCENTER | win::DT_NOCLIP);
            win::SelectObject(dc, old_f);
            win::DeleteObject(f);
            let f2 = win::make_font(12, false);
            let old_f2 = win::SelectObject(dc, f2);
            win::SetTextColor(dc, win::rgb(0x9A, 0x9A, 0x9A));
            let mut trc = win::RECT { left: left, top: content_top() + 96, right: right, bottom: content_top() + 120 };
            let tip = win::wide("点击金额或按 Enter 复制到剪贴板");
            win::DrawTextW(dc, tip.as_ptr(), (tip.len() - 1) as i32, &mut trc,
                win::DT_CENTER | win::DT_SINGLELINE);
            win::SelectObject(dc, old_f2);
            win::DeleteObject(f2);
        }
        MainMode::Rows => {
            let rows = main_rows(app);
            if rows.is_empty() {
                let f = win::make_font(12, false);
                let old_f = win::SelectObject(dc, f);
                win::SetBkMode(dc, win::TRANSPARENT);
                win::SetTextColor(dc, win::rgb(0x9A, 0x9A, 0x9A));
                let msg = if let Some(e) = &app.search_error {
                    e.clone()
                } else if app.search_busy {
                    "正在搜索…".to_string()
                } else {
                    "没有找到匹配的文件".to_string()
                };
                let w = win::wide(&msg);
                win::TextOutW(dc, left + 4, content_top() + 12, w.as_ptr(), (w.len() - 1) as i32);
                win::SelectObject(dc, old_f);
                win::DeleteObject(f);
            }
            let name_f = win::make_font(13, false);
            let sub_f = win::make_font(11, false);
            let scroll = clamped_scroll(app, &rc) as usize;
            let mut y = content_top();
            for (i, (_, title, sub)) in rows.iter().enumerate().skip(scroll) {
                if y + ROW_H > content_floor { break; }
                if app.hover_row == i as i32 {
                    round_box(dc, left, y, right, y + ROW_H, 4, win::rgb(0xE3, 0xEE, 0xFB), None, false);
                }
                let old_f = win::SelectObject(dc, name_f);
                win::SetBkMode(dc, win::TRANSPARENT);
                win::SetTextColor(dc, win::rgb(0x1F, 0x1F, 0x1F));
                let mut nrc = win::RECT { left: left + 10, top: y + 5, right: right - 10, bottom: y + 22 };
                let wt = win::wide(title);
                win::DrawTextW(dc, wt.as_ptr(), (wt.len() - 1) as i32, &mut nrc,
                    win::DT_SINGLELINE | win::DT_VCENTER | win::DT_END_ELLIPSIS | win::DT_NOCLIP);
                win::SelectObject(dc, sub_f);
                win::SetTextColor(dc, win::rgb(0x6B, 0x6B, 0x6B));
                let mut sbrc = win::RECT { left: left + 10, top: y + 24, right: right - 10, bottom: y + 40 };
                let ws = win::wide(sub);
                win::DrawTextW(dc, ws.as_ptr(), (ws.len() - 1) as i32, &mut sbrc,
                    win::DT_SINGLELINE | win::DT_VCENTER | win::DT_END_ELLIPSIS | win::DT_NOCLIP);
                win::SelectObject(dc, old_f);
                y += ROW_H;
            }
            win::DeleteObject(name_f);
            win::DeleteObject(sub_f);
            let visible = visible_rows(&rc) as usize;
            if rows.len() > visible && visible > 0 {
                let track_top = content_top();
                let track_h = (content_floor - track_top).max(1);
                let h = ((track_h as usize * visible / rows.len()).max(24)) as i32;
                let top = track_top
                    + (track_h - h) * scroll as i32 / (rows.len() - visible).max(1) as i32;
                round_box(dc, right + 2, top, right + 6, top + h, 2,
                    win::rgb(0xC2, 0xC2, 0xC2), None, false);
            }
        }
        MainMode::Tiles => {
            let (tw, th) = tile_size(&rc);
            // Cell guides, only while a tile is in the air: empty cells are drop
            // targets, and without them the drop outline landed on blank space
            // with no hint that the space was a cell. Idle keeps the board clean.
            if app.drag_active {
                let guide = win::rgb(0xEC, 0xEC, 0xEC);
                for slot in 0..GRID_CELLS {
                    let (gl, gt) = tile_xy(&rc, slot);
                    if gt + th > content_floor { break; }
                    round_box(dc, gl, gt, gl + tw, gt + th, 4, win::rgb(0xFA, 0xFA, 0xFA),
                        Some(guide), false);
                }
            }
            if app.shortcuts.is_empty() {
                let f = win::make_font(12, false);
                let old_f = win::SelectObject(dc, f);
                win::SetBkMode(dc, win::TRANSPARENT);
                win::SetTextColor(dc, win::rgb(0x9A, 0x9A, 0x9A));
                let w = win::wide("暂无快捷方式，点击格子里的 + 添加");
                win::TextOutW(dc, left + 4, content_top() + 12, w.as_ptr(), (w.len() - 1) as i32);
                win::SelectObject(dc, old_f);
                win::DeleteObject(f);
            }
            // A fixed 32-cell board: slots past the board simply are not drawn
            // (they stay in shortcuts.json and remain searchable).
            let map = slot_map(app);
            for slot in 0..GRID_CELLS.min(map.len()) {
                let Some(i) = map[slot] else { continue };
                let (tl, tt) = tile_xy(&rc, slot);
                if tt + th > content_floor { break; }
                // The carried tile is lifted out of the grid: leave its slot
                // empty so the user can see where it came from.
                if app.drag_active && app.drag_from == i as i32 { continue; }
                paint_tile(dc, app, &rc, tl, tt, i);
            }
            if let Some(r) = add_tile_rect(app, &rc) {
                paint_add_tile(dc, &rc, app, r);
            }
            if app.drag_active && app.drag_from >= 0 && (app.drag_from as usize) < app.shortcuts.len() {
                // Drop target first, then the lifted tile drawn on top so it
                // reads as "above" the grid rather than in it.
                let to = tile_drop_slot(&rc, app.drag_cur.0, app.drag_cur.1);
                // Outline only: a filled plate would hide the tile that
                // already sits in that slot.
                let (dl, dt) = tile_xy(&rc, to);
                let c = win::rgb(0x6F, 0xA8, 0xDC);
                fill_box(dc, dl, dt, dl + tw, dt + 2, c);
                fill_box(dc, dl, dt + th - 2, dl + tw, dt + th, c);
                fill_box(dc, dl, dt, dl + 2, dt + th, c);
                fill_box(dc, dl + tw - 2, dt, dl + tw, dt + th, c);

                let (dx, dy) = (app.drag_cur.0 - app.drag_offset.0,
                                app.drag_cur.1 - app.drag_offset.1);
                paint_tile(dc, app, &rc, dx, dy, app.drag_from as usize);
            }
        }
    }
}

// ---- self checks ---------------------------------------------------------

/// Render the real main window into an off-screen DIB and save it as PNG.
///
/// The build machine has no interactive desktop, so the only way to confirm
/// the 3.3 layout (brand mark, dashed search box, tile grid) is to paint it
/// into memory with the very same `paint_main` the live window uses.
/// Drive a synthetic drag through the real overlay window, then grab the
/// screen so the selection frame can be inspected exactly as the user sees it.
/// This is the only verification of the highlight box available to a build
/// machine without an interactive desktop.
pub fn capture_overlay_snapshot(app: &mut App, out: &str) -> i32 {
    const PM_REMOVE: u32 = 1;
    unsafe {
        app.grab_and_show();
        let ov = match app.overlay.as_ref() {
            Some(o) => o.hwnd,
            None => { win::log("overlay 未创建，无法验证高亮框"); return 1; }
        };
        let lp = |x: i32, y: i32| -> win::LPARAM {
            (((y as isize) & 0xFFFF) << 16) | ((x as isize) & 0xFFFF)
        };
        // The overlay throttles repaints to one per 16 ms, so pump the queue
        // between steps instead of dumping the whole drag at once.
        win::PostMessageW(ov, win::WM_LBUTTONDOWN, 1, lp(220, 160));
        for i in 1..=6 {
            win::PostMessageW(ov, win::WM_MOUSEMOVE, 1, lp(220 + i * 90, 160 + i * 62));
            let mut msg: win::MSG = Default::default();
            while win::PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) > 0 {
                win::TranslateMessage(&msg);
                win::DispatchMessageW(&msg);
            }
            win::Sleep(30);
        }
        win::UpdateWindow(ov);
        win::Sleep(150);
        // Release to hand the selection to the action bar, then grab again so
        // the bar's position next to the cursor is part of the snapshot.
        win::PostMessageW(ov, win::WM_LBUTTONUP, 0, lp(760, 532));
        let mut msg: win::MSG = Default::default();
        while win::PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) > 0 {
            win::TranslateMessage(&msg);
            win::DispatchMessageW(&msg);
        }
        win::Sleep(200);
        let shot = match capture::grab_virtual_screen() {
            Ok(s) => s,
            Err(e) => { win::log(&format!("overlay 快照抓屏失败: {}", e)); return 1; }
        };
        match capture::save_png_as(&shot, out) {
            Ok(_) => 0,
            Err(e) => { win::log(&format!("overlay 快照保存失败: {}", e)); 1 }
        }
    }
}

/// Record where Windows actually placed the windows, next to the snapshot.
/// A PNG shows the dialog's contents but not which monitor it landed on, and
/// that is exactly what broke once a second display was attached.
fn dump_layout(png: &str, main: win::HWND, dlg: win::HWND) {
    unsafe {
        let wa = win::primary_work_area();
        let vs = win::virtual_screen();
        let rect = |h: win::HWND| -> (i32, i32, i32, i32) {
            let mut rc: win::RECT = Default::default();
            if h.is_null() || win::GetWindowRect(h, &mut rc) == 0 { return (0, 0, 0, 0); }
            (rc.left, rc.top, rc.right - rc.left, rc.bottom - rc.top)
        };
        let (ml, mt, mw, mh) = rect(main);
        let (dl, dt, dw, dh) = rect(dlg);
        let inside = dl >= wa.x && dt >= wa.y
            && dl + dw <= wa.x + wa.w && dt + dh <= wa.y + wa.h;
        let body = format!(
            "{{\n  \"primaryWorkArea\": [{}, {}, {}, {}],\n  \"virtualScreen\": [{}, {}, {}, {}],\n  \
             \"main\": [{}, {}, {}, {}],\n  \"dialog\": [{}, {}, {}, {}],\n  \
             \"dialogInsidePrimary\": {}\n}}\n",
            wa.x, wa.y, wa.w, wa.h, vs.x, vs.y, vs.w, vs.h,
            ml, mt, mw, mh, dl, dt, dw, dh, inside);
        let path = match png.rsplit_once('.') {
            Some((stem, _)) => format!("{}.layout.json", stem),
            None => format!("{}.layout.json", png),
        };
        let _ = std::fs::write(path, body);
    }
}

/// Render a window (client + non-client + child controls) to a PNG through
/// PrintWindow. Used by --about-shot so the About layout can be reviewed
/// without right-clicking the tray icon on a real desktop.
fn capture_window_png(hwnd: win::HWND, out: &str) -> i32 {
    unsafe {
        let mut rc: win::RECT = Default::default();
        if win::GetWindowRect(hwnd, &mut rc) == 0 { return 1; }
        let (w, h) = (rc.right - rc.left, rc.bottom - rc.top);
        if w <= 0 || h <= 0 { return 1; }
        let screen = win::GetDC(std::ptr::null_mut());
        if screen.is_null() { return 1; }
        let mem = win::CreateCompatibleDC(screen);
        let bi = win::BITMAPINFOHEADER {
            biSize: std::mem::size_of::<win::BITMAPINFOHEADER>() as u32,
            biWidth: w, biHeight: -h, biPlanes: 1, biBitCount: 32,
            biCompression: 0, biSizeImage: 0, biXPelsPerMeter: 0,
            biYPelsPerMeter: 0, biClrUsed: 0, biClrImportant: 0,
        };
        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let hbm = win::CreateDIBSection(mem, &bi, 0, &mut bits, std::ptr::null_mut(), 0);
        if hbm.is_null() || bits.is_null() {
            win::DeleteDC(mem);
            win::ReleaseDC(std::ptr::null_mut(), screen);
            return 1;
        }
        let old = win::SelectObject(mem, hbm);
        // PW_RENDERFULLCONTENT (0x2): without it the non-client frame and
        // themed buttons come back blank when the window is not foreground.
        win::PrintWindow(hwnd, mem, 0x0000_0002);
        win::GdiFlush();

        let mut img = Image::new(w, h);
        std::ptr::copy_nonoverlapping(bits as *const u8, img.data.as_mut_ptr(), img.data.len());
        img.force_opaque();

        win::SelectObject(mem, old);
        win::DeleteObject(hbm);
        win::DeleteDC(mem);
        win::ReleaseDC(std::ptr::null_mut(), screen);

        match capture::save_png_as(&img, out) {
            Ok(_) => {
                win::log(&format!("window snapshot -> {} ({}x{})", out, w, h));
                0
            }
            Err(e) => {
                win::log(&format!("window snapshot 失败: {}", e));
                1
            }
        }
    }
}

pub fn render_main_snapshot(app: &App, out: &str) -> i32 {
    unsafe {
        let mut rc: win::RECT = Default::default();
        win::GetClientRect(app.main, &mut rc);
        let (dw, dh) = panel_client_size();
        let w = if rc.right > 0 { rc.right } else { dw };
        let h = if rc.bottom > 0 { rc.bottom } else { dh };

        let screen = win::GetDC(std::ptr::null_mut());
        if screen.is_null() { return 1; }
        let mem = win::CreateCompatibleDC(screen);
        let bi = win::BITMAPINFOHEADER {
            biSize: std::mem::size_of::<win::BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: 0,
            biSizeImage: 0,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        };
        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let hbm = win::CreateDIBSection(mem, &bi, 0, &mut bits, std::ptr::null_mut(), 0);
        if hbm.is_null() || bits.is_null() {
            win::DeleteDC(mem);
            win::ReleaseDC(std::ptr::null_mut(), screen);
            return 1;
        }
        let old = win::SelectObject(mem, hbm);
        paint_main(mem, app);
        win::GdiFlush();

        // Second pass through the double-buffered path the live WM_PAINT uses.
        // The two must be pixel identical, otherwise the flicker fix would
        // paint the screen differently from the snapshot we review here.
        let mut diff = -1i32;
        let mem2 = win::CreateCompatibleDC(screen);
        if !mem2.is_null() {
            let mut bits2: *mut std::ffi::c_void = std::ptr::null_mut();
            let hbm2 = win::CreateDIBSection(mem2, &bi, 0, &mut bits2, std::ptr::null_mut(), 0);
            if !hbm2.is_null() && !bits2.is_null() {
                let old2 = win::SelectObject(mem2, hbm2);
                if paint_main_buffered(mem2, app, &rc) {
                    win::GdiFlush();
                    let n = (w * h * 4) as usize;
                    let a = std::slice::from_raw_parts(bits as *const u8, n);
                    let b = std::slice::from_raw_parts(bits2 as *const u8, n);
                    diff = a.iter().zip(b).filter(|(u, v)| u != v).count() as i32;
                }
                win::SelectObject(mem2, old2);
                win::DeleteObject(hbm2);
            }
            win::DeleteDC(mem2);
        }
        win::log(&format!("ui snapshot 双缓冲一致性: {} 不同字节", diff));

        let mut img = Image::new(w, h);
        std::ptr::copy_nonoverlapping(bits as *const u8, img.data.as_mut_ptr(), img.data.len());
        img.force_opaque();

        win::SelectObject(mem, old);
        win::DeleteObject(hbm);
        win::DeleteDC(mem);
        win::ReleaseDC(std::ptr::null_mut(), screen);

        match capture::save_png_as(&img, out) {
            Ok(_) => {
                win::log(&format!("ui snapshot -> {} ({}x{})", out, w, h));
                0
            }
            Err(e) => {
                win::log(&format!("ui snapshot 失败: {}", e));
                1
            }
        }
    }
}

fn rd16(b: &[u8], o: usize) -> Option<u16> {
    (o + 2 <= b.len()).then(|| u16::from_le_bytes([b[o], b[o + 1]]))
}

fn rd32(b: &[u8], o: usize) -> Option<u32> {
    (o + 4 <= b.len()).then(|| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]))
}

/// Does this executable carry an RT_GROUP_ICON in its PE resource directory?
///
/// Explorer, shortcuts and Alt+Tab read the file icon from there; a HICON made
/// at runtime is invisible to them. `build.rs` emits the `.rsrc` object, and
/// this probe proves it survived the link — a silently skipped build script
/// would otherwise only show up as a blank icon on another machine.
fn exe_has_icon_resource() -> bool {
    let Ok(b) = std::env::current_exe().and_then(std::fs::read) else { return false };
    let Some(pe) = rd32(&b, 0x3c).map(|v| v as usize) else { return false };
    let Some(magic) = rd16(&b, pe + 24) else { return false };
    let dd = pe + 24 + if magic == 0x20b { 112 } else { 96 };
    let Some(rva) = rd32(&b, dd + 16) else { return false };
    if rva == 0 { return false; }
    let (Some(nsec), Some(opt)) = (rd16(&b, pe + 6), rd16(&b, pe + 20)) else { return false };
    let so = pe + 24 + opt as usize;
    let mut base = None;
    for i in 0..nsec as usize {
        let s = so + i * 40;
        let (Some(vs), Some(va), Some(rs), Some(rp)) =
            (rd32(&b, s + 8), rd32(&b, s + 12), rd32(&b, s + 16), rd32(&b, s + 20)) else { return false };
        if rva >= va && rva < va + vs.max(rs) {
            base = Some((rva - va + rp) as usize);
            break;
        }
    }
    let Some(base) = base else { return false };
    let (Some(named), Some(ids)) = (rd16(&b, base + 12), rd16(&b, base + 14)) else { return false };
    (0..(named + ids) as usize).any(|i| rd32(&b, base + 16 + i * 8) == Some(14))
}

/// Headless verification of everything that cannot be clicked in a locked /
/// RDP session: config round-trip, import compatibility, JSON escaping,
/// Chinese uppercase amounts and the built-in search component inventory.
fn run_selftest(args: &[String]) -> i32 {
    let out_path = args.get(1).cloned().unwrap_or_else(|| "selftest.json".into());
    let mut checks: Vec<(String, bool, String)> = Vec::new();

    let items = vec![
        profile::Shortcut {
            name: "计算器".into(), path: "C:\\Program Files\\Tools\\calc.exe".into(),
            args: "--mode \"fast\"".into(), pos: 0,
            icon: String::new(),
        },
        profile::Shortcut {
            name: "带引号\"和反斜杠\\".into(), path: "mspaint.exe".into(),
            args: String::new(), pos: 1,
            icon: "C:\\Icons\\mspaint.png".into(),
        },
    ];
    let st = profile::Settings::default();
    let json = profile::build_config(&items, &st);
    let parsed = profile::parse_config(&json);
    let roundtrip = parsed.as_ref().map(|(v, s)| {
        v.len() == 2 && v[0].path == items[0].path && v[0].args == items[0].args
            && v[1].name == items[1].name && s.search_sort_key == 0
            // Grid slots must survive a write/read cycle, and a source file
            // without the key must come back as "auto" (-1).
            && v[0].pos == 0 && v[1].pos == 1
    }).unwrap_or(false);
    checks.push(("configRoundTrip".into(), roundtrip, format!("{} bytes", json.len())));

    let bare = "[{\"name\":\"纯数组\",\"path\":\"notepad.exe\"}]";
    let bare_ok = profile::parse_config(bare)
        .map(|(v, _)| v.len() == 1 && v[0].name == "纯数组" && v[0].pos == -1)
        .unwrap_or(false);
    checks.push(("bareArrayImport".into(), bare_ok, String::new()));

    let escaped = profile::quote("a\"b\\c\n中文");
    let escape_ok = escaped.contains("\\\"") && escaped.contains("\\\\") && escaped.contains("中文");
    checks.push(("jsonEscape".into(), escape_ok, escaped));

    let dx_cases = [
        ("dx 12345", "壹万贰仟叁佰肆拾伍元整"),
        ("dx 10001", "壹万零壹元整"),
        ("dx 20.5", "贰拾元伍角"),
    ];
    for (input, expected) in dx_cases {
        let got = to_chinese_amount(input).unwrap_or_default();
        checks.push((format!("dx:{}", input), got == expected,
            if got == expected { expected.into() } else { format!("{} != {}", got, expected) }));
    }
    let non_dx = to_chinese_amount("hello").is_none();
    checks.push(("dxNonMatch".into(), non_dx, String::new()));

    // Web links: classification must be pure and cheap to assert here, because
    // a link that was mistaken for a file path would open the shell picker.
    let link_cases: [(&str, bool); 8] = [
        ("https://www.baidu.com", true), ("http://a.cn/x", true),
        ("www.qq.com", true), ("\"https://x.com\"", true),
        ("file:///C:/a.html", false), ("C:\\a.exe", false),
        ("shell:Downloads", false), ("https://", false),
    ];
    let links_ok = link_cases.iter()
        .all(|(p, want)| (profile::target_of(p) == profile::Target::Web) == *want);
    let domain_ok = profile::domain_of("https://tieba.baidu.com/f") == "baidu.com"
        && profile::domain_of("www.weather.com.cn") == "weather.com.cn"
        && profile::link_initial("www.baidu.com", "www.baidu.com") == "B";
    checks.push(("webLinkClassify".into(), links_ok && domain_ok,
        format!("{:?} 条 + 域名解析", link_cases.len())));

    // The browser lookup is read-only, so it can report on the real machine.
    let browser = default_browser_exe();
    checks.push(("browserAssociation".into(), browser.is_some(),
        browser.unwrap_or_else(|| "未找到默认浏览器，链接将交给系统打开拓".into())));

    let comps = everything::components_present();
    checks.push(("everythingComponents".into(), true,
        if comps { "present".into() } else { "not deployed (search shows a clear hint)".into() }));

    let icon_ok = exe_has_icon_resource();
    checks.push(("exeIconResource".into(), icon_ok,
        if icon_ok { "RT_GROUP_ICON embedded".into() }
        else { "missing: Explorer would show the generic file icon".into() }));

    // Same call the shell uses to draw a file icon: proves the resource is not
    // just present but readable as an icon at the size Explorer asks for.
    let file_icon = std::env::current_exe().ok().map(|p| {
        let w = win::wide(&p.to_string_lossy());
        let mut h: win::HICON = std::ptr::null_mut();
        let mut id: u32 = 0;
        let n = unsafe { win::PrivateExtractIconsW(w.as_ptr(), 0, 32, 32, &mut h, &mut id, 1, 0) };
        if !h.is_null() { unsafe { win::DestroyIcon(h); } }
        n > 0
    }).unwrap_or(false);
    checks.push(("exeFileIconExtractable".into(), file_icon,
        if file_icon { "32x32 frame extracted".into() } else { "no frame could be extracted".into() }));

    let ok = checks.iter().all(|c| c.1);
    let body = checks.iter().map(|(n, pass, detail)| {
        format!("\"{}\":{{\"pass\":{},\"detail\":\"{}\"}}", escj(n), pass, escj(detail))
    }).collect::<Vec<_>>().join(",");
    write_json(&out_path, &format!("{{\"ok\":{},{}}}", ok, body));
    if ok { 0 } else { 1 }
}

fn run_ocr_check(args: &[String]) -> i32 {
    let out_path = args.get(1).cloned().unwrap_or_else(|| "ocr-check.json".into());
    let img = if let Some(p) = args.get(2) {
        match Image::load(p) {
            Ok(i) => i,
            Err(e) => { write_json(&out_path, &format!("{{\"ok\":false,\"error\":\"{}\"}}", escj(&e))); return 1; }
        }
    } else {
        match capture::grab_virtual_screen() {
            Ok(i) => i,
            Err(e) => { write_json(&out_path, &format!("{{\"ok\":false,\"error\":\"{}\"}}", escj(&e))); return 1; }
        }
    };
    let dir = match paddle::find_runtime_dir() {
        Ok(d) => d,
        Err(e) => { write_json(&out_path, &format!("{{\"ok\":false,\"error\":\"{}\"}}", escj(&e))); return 1; }
    };
    let engine = match PaddleOcr::new(&dir) {
        Ok(e) => e,
        Err(e) => { write_json(&out_path, &format!("{{\"ok\":false,\"error\":\"{}\"}}", escj(&e))); return 1; }
    };
    engine.warmup();
    // C# RunCheck calls RecognizeInto twice and reports the second run, so an
    // ORT session that already allocated full-size buffers is the baseline.
    // Do the same here or the comparison measures a cold first inference.
    let _ = engine.recognize(&img);
    let start = std::time::Instant::now();
    match engine.recognize(&img) {
        Ok(res) => {
            let tsv = layout::to_tsv(&res);
            let plain = layout::to_plain_text(&res);
            // Geometry dump: table layout bugs are almost always about box x/w,
            // so keep the raw boxes next to the rendered table.
            let boxes = res.words.iter().map(|w| {
                format!("{{\"t\":\"{}\",\"x\":{},\"y\":{},\"w\":{},\"h\":{}}}",
                    escj(&w.text), w.x as i32, w.y as i32, w.w as i32, w.h as i32)
            }).collect::<Vec<_>>().join(",");
            let json = format!(
                "{{\"ok\":true,\"engine\":\"{}\",\"language\":\"{}\",\"ocrMs\":{},\"ocrWords\":{},\"text\":\"{}\",\"table\":\"{}\",\"ocrTiming\":\"{}\",\"model\":\"pp-ocr-v6-small-det+tiny-rec\",\"boxes\":[{}]}}",
                escj(&res.engine), escj(&res.language), start.elapsed().as_millis(),
                res.words.len(), escj(&plain), escj(&tsv), escj(&engine.last_timing()), boxes);
            write_json(&out_path, &json);
            0
        }
        Err(e) => {
            write_json(&out_path, &format!("{{\"ok\":false,\"error\":\"{}\"}}", escj(&e)));
            1
        }
    }
}

fn run_capture_smoke(args: &[String]) -> i32 {
    let out_path = args.get(1).cloned().unwrap_or_else(|| "capture-smoke.json".into());
    unsafe {
        win::set_dpi_awareness();
        let proc: win::WNDPROC = Some(wnd_proc);
        let cls = register(overlay::CLASS_HOST, proc);
        let title = win::wide("TT酱自检");
        let host = win::CreateWindowExW(0, cls.as_ptr(), title.as_ptr(), win::WS_POPUP,
            0, 0, 1, 1, std::ptr::null_mut(), std::ptr::null_mut(),
            win::GetModuleHandleW(std::ptr::null()), std::ptr::null_mut());
        let vs = win::virtual_screen();
        let img = match capture::grab_virtual_screen() {
            Ok(i) => i,
            Err(e) => { write_json(&out_path, &format!("{{\"ok\":false,\"error\":\"{}\"}}", escj(&e))); return 1; }
        };
        let cropped = img.crop(Rect::new(0, 0, img.w.min(200), img.h.min(120)));
        let overlay_ok = Overlay::new(img.clone(), vs, &register(overlay::CLASS_OVERLAY, proc)).is_ok();
        let bar_ok = ActionBar::new(&register(overlay::CLASS_BAR, proc)).is_ok();
        let toast_ok = Toast::new(&register(overlay::CLASS_TOAST, proc)).is_ok();
        let png_ok = !png_bytes_smoke(&cropped).is_empty();
        // A display DC may exist but be unreadable (locked / RDP / headless).
        // Report it instead of silently claiming a successful screenshot.
        let variance = img.sampled_variance();
        let screen_readable = variance > 0.01;
        let json = format!(
            "{{\"ok\":{},\"virtualScreen\":\"{}x{} at ({},{})\",\"grab\":\"{}x{}\",\"crop\":\"{}x{}\",\"overlay\":{},\"actionBar\":{},\"toast\":{},\"png\":{},\"screenReadable\":{},\"pixelVariance\":{:.4},\"renderer\":\"opaque-full-redraw (no WS_EX_LAYERED)\"}}",
            overlay_ok && bar_ok && toast_ok && png_ok,
            vs.w, vs.h, vs.x, vs.y, img.w, img.h, cropped.w, cropped.h,
            overlay_ok, bar_ok, toast_ok, png_ok, screen_readable, variance);
        write_json(&out_path, &json);
        if !host.is_null() { win::DestroyWindow(host); }
        if overlay_ok && bar_ok && toast_ok && png_ok { 0 } else { 1 }
    }
}

fn png_bytes_smoke(img: &Image) -> Vec<u8> {
    // Reuse the encoder by saving to the temp dir and reading it back.
    let dir = std::env::temp_dir();
    let dir_s = dir.to_string_lossy().to_string();
    match capture::save_png(img, &dir_s) {
        Ok(name) => std::fs::read(dir.join(name)).unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

fn write_json(path: &str, body: &str) {
    let _ = std::fs::write(path, body.as_bytes());
}

fn escj(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\r' => out.push_str("\\r"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> win::RECT {
        // The real panel size, not a made-up one: the board has to fit whatever
        // the screen hands us.
        let (w, h) = panel_client_size();
        win::RECT { left: 0, top: 0, right: w, bottom: h }
    }

    fn fake(w: i32, h: i32) -> win::RECT {
        win::RECT { left: 0, top: 0, right: w, bottom: h }
    }

    #[test]
    fn centered_windows_land_inside_the_primary_work_area() {
        let wa = win::primary_work_area();
        assert!(wa.w > 0 && wa.h > 0);
        for (w, h) in [(800, 600), (520, 330), (420, 206)] {
            let (x, y) = win::center_on_primary(w, h);
            // Fully on the primary monitor, never on the seam between displays.
            assert!(x >= wa.x, "{}x{} left outside", w, h);
            assert!(y >= wa.y, "{}x{} top outside", w, h);
            if w <= wa.w && h <= wa.h {
                assert!(x + w <= wa.x + wa.w, "{}x{} overflow right", w, h);
                assert!(y + h <= wa.y + wa.h, "{}x{} overflow bottom", w, h);
                // Equal slack on both sides, within one pixel of rounding.
                let lr = (x - wa.x) - (wa.x + wa.w - x - w);
                let tb = (y - wa.y) - (wa.y + wa.h - y - h);
                assert!(lr.abs() <= 1 && tb.abs() <= 1, "{}x{} not centred", w, h);
            }
        }
    }

    #[test]
    fn the_panel_opens_a_tenth_of_the_screen_below_centre() {
        let wa = win::primary_work_area();
        let (w, h) = panel_client_size();
        let (x, y) = win::center_on_primary_below(w, h, PANEL_DROP_PCT);
        let (cx, cy) = win::center_on_primary(w, h);
        // Same column, dropped by 10% of the work area height.
        assert_eq!(x, cx);
        let drop = y - cy;
        let want = wa.h * PANEL_DROP_PCT / 100;
        assert!(y >= cy, "panel should open at or below centre");
        assert!(drop <= want && drop >= want - 1, "dropped {}px, wanted {}px", drop, want);
        // Dropping must never push the panel off the primary screen.
        assert!(y >= wa.y);
        assert!(y + h <= wa.y + wa.h, "dropped panel hangs off the bottom");
    }

    #[test]
    fn primary_work_area_is_a_subset_of_the_virtual_screen() {
        let wa = win::primary_work_area();
        let vs = win::virtual_screen();
        assert!(wa.x >= vs.x && wa.y >= vs.y);
        assert!(wa.x + wa.w <= vs.x + vs.w && wa.y + wa.h <= vs.y + vs.h);
        // The taskbar is excluded, so the primary work area is never taller
        // than the primary screen itself.
        assert!(wa.h <= unsafe { win::GetSystemMetrics(win::SM_CYSCREEN) });
    }

    #[test]
    fn search_field_spans_the_inner_width_and_sits_above_the_grid() {
        let rc = client();
        let s = search_rect(&rc);
        assert_eq!(s.0, inner_left());
        assert_eq!(s.2, inner_right(&rc));
        assert_eq!(s.3 - s.1, SEARCH_H);
        assert!(s.3 < content_top(), "the tile grid must start below the field");
    }

    #[test]
    fn cursor_is_an_i_beam_inside_the_search_field() {
        let rc = client();
        let s = search_rect(&rc);
        let (cx, cy) = ((s.0 + s.2) / 2, (s.1 + s.3) / 2);
        assert_eq!(cursor_target(&rc, cx, cy, false), Some(win::IDC_IBEAM));
        // Even if the item test ever says "item", the field wins.
        assert_eq!(cursor_target(&rc, cx, cy, true), Some(win::IDC_IBEAM));
        let (x, y) = (s.0, s.1);
        assert_eq!(cursor_target(&rc, x, y, false), Some(win::IDC_IBEAM));
        assert_eq!(cursor_target(&rc, s.2 - 1, s.3 - 1, false), Some(win::IDC_IBEAM));
    }

    #[test]
    fn cursor_is_a_hand_on_items_and_an_arrow_on_empty_space() {
        let rc = client();
        let t = tile_rect(&rc, 0);
        let (cx, cy) = ((t.0 + t.2) / 2, (t.1 + t.3) / 2);
        assert_eq!(cursor_target(&rc, cx, cy, true), Some(win::IDC_HAND));
        // Just below the search field and left of any tile: empty space.
        assert_eq!(cursor_target(&rc, inner_left(), content_top() - 2, false), None);
        assert_eq!(cursor_target(&rc, 2, 2, false), None);
    }

    #[test]
    fn exe_carries_the_icon_resource_explorer_reads() {
        // Guards build.rs: if the .rsrc object stops being linked, the taskbar
        // and tray still look right while the file icon goes blank.
        assert!(exe_has_icon_resource(), "PE 里没有 RT_GROUP_ICON，build.rs 的资源对象没链进来");
    }

    #[test]
    fn hover_boxes_cover_exactly_the_tile_that_changed() {
        let rc = client();
        let (tw, th) = tile_size(&rc);
        for i in [0usize, 3, 11] {
            let t = tile_rect(&rc, i);
            assert_eq!(t.2 - t.0, tw);
            assert_eq!(t.3 - t.1, th);
            // Neighbouring tiles must not overlap, or a hover would smear.
            let n = tile_rect(&rc, i + 1);
            assert!(t.2 <= n.0 || t.1 >= n.3 || t.3 <= n.1);
        }
    }

    #[test]
    fn drop_slot_maps_the_cell_under_the_pointer() {
        let rc = client();
        let (l0, t0) = tile_xy(&rc, 0);
        let (l1, _) = tile_xy(&rc, 1);
        let cols = GRID_COLS as usize;
        assert_eq!(tile_drop_slot(&rc, l0 + 4, t0 + 4), 0);
        assert_eq!(tile_drop_slot(&rc, l1 + 4, t0 + 4), 1);
        // Past the bottom clamps to the last row of the board, never past it.
        let last_cell = (GRID_COLS * GRID_ROWS - 1) as usize;
        assert_eq!(tile_drop_slot(&rc, l0 + 4, t0 + 2000), last_cell + 1 - cols);
        // Dropping past the right edge stays inside the current row.
        assert!(tile_drop_slot(&rc, inner_right(&rc) + 400, t0 + 4) < cols);
    }

    fn dummy(name: &str, pos: i32) -> profile::Shortcut {
        profile::Shortcut { name: name.into(), pos, ..Default::default() }
    }

    #[test]
    fn the_board_is_always_8_by_4_and_nothing_hangs_off_the_panel() {
        // The panel size follows the screen; the cell count does not. Every
        // resolution in range must land all 32 cells inside the board.
        let (pw, ph) = panel_client_size();
        for (w, h) in [(pw, ph), (700, 460), (768, 541), (1280, 800)] {
            let rc = fake(w, h);
            let (tw, th) = tile_size(&rc);
            assert!(tw > 40 && th > 40, "{}x{} cells collapsed to {}x{}", w, h, tw, th);
            let (last_l, last_t) = tile_xy(&rc, GRID_CELLS - 1);
            assert!(last_l + tw <= inner_right(&rc), "{}x{} right column clipped", w, h);
            assert!(last_t + th <= grid_bottom(&rc), "{}x{} bottom row clipped", w, h);
            // Footer owns the strip the gear lives in; the board stops above it.
            assert!(grid_bottom(&rc) < footer_top(&rc));
            assert!(footer_top(&rc) < footer_baseline(&rc));
            assert!(footer_baseline(&rc) < rc.bottom);
            assert!(search_rect(&rc).3 < content_top(), "{}x{} grid overlaps the field", w, h);
            // Icon plus caption must fit in a cell, or the label is clipped.
            let icon = icon_size(&rc);
            let (_, icon_t) = icon_origin(&rc, 0, content_top());
            assert!(icon + 8 + CAPTION_BAND <= th, "{}x{} icon block taller than a cell", w, h);
            assert!(icon_t >= content_top());
        }
    }

    #[test]
    fn a_slot_outside_the_board_falls_back_to_auto_placement() {
        // Written against a wider grid: the tile must not disappear, it just
        // lands in the first free cell.
        let mut v = vec![dummy("a", 0), dummy("far", 90)];
        normalize_slots(&mut v);
        assert_eq!(v[1].pos, -1);
        assert_eq!(slot_map_of(&v), vec![Some(0), Some(1)]);
        // A slot inside the board is left exactly where the user put it.
        let mut w = vec![dummy("a", 5), dummy("b", GRID_CELLS as i32 - 1)];
        normalize_slots(&mut w);
        assert_eq!(w[0].pos, 5);
        assert_eq!(w[1].pos, GRID_CELLS as i32 - 1);
    }

    #[test]
    fn the_add_tile_takes_the_first_free_cell() {
        // Empty board: the + is the very first cell.
        assert_eq!(add_slot_of(&[]), Some(0));
        // Auto-placed shortcuts: + follows them.
        let v = vec![dummy("a", -1), dummy("b", -1), dummy("c", -1)];
        assert_eq!(add_slot_of(&v), Some(3));
        // A deliberate gap is where a new icon would go, so the + takes it.
        let gapped = vec![dummy("a", 0), dummy("b", 2)];
        assert_eq!(add_slot_of(&gapped), Some(1));
        // 31 shortcuts: the + gets the last of the 32 cells.
        let mut full = Vec::new();
        for i in 0..GRID_CELLS - 1 { full.push(dummy("x", i as i32)); }
        assert_eq!(add_slot_of(&full), Some(GRID_CELLS - 1));
        // 32: the board is full, there is nowhere left to put it.
        full.push(dummy("y", GRID_CELLS as i32 - 1));
        assert_eq!(add_slot_of(&full), None);
    }

    #[test]
    fn the_add_tile_never_sits_on_top_of_a_shortcut() {
        let rc = client();
        let v = vec![dummy("a", 0), dummy("b", 2), dummy("c", 7)];
        let add = add_tile_rect_of(&v, &rc).expect("board has room");
        for (slot, _) in slot_map_of(&v).iter().enumerate() {
            if slot >= GRID_CELLS { continue; }
            let t = tile_rect(&rc, slot);
            let clash = add.0 < t.2 && t.0 < add.2 && add.1 < t.3 && t.1 < add.3;
            // The + sits in an empty cell, so it can only "touch" its own cell.
            assert!(!clash || tile_xy(&rc, slot) == (add.0, add.1),
                "add tile overlaps the shortcut in slot {}", slot);
        }
    }

    #[test]
    fn slots_free_layout_keeps_gaps_and_swaps_on_collision() {
        // A legacy config carries no pos at all: it must fill 0,1,2 in order.
        let mut v = vec![dummy("a", -1), dummy("b", -1), dummy("c", -1)];
        assert_eq!(slot_map_of(&v), vec![Some(0), Some(1), Some(2)]);

        // Drop "a" on the empty 4th cell: it stays there and slot 0 goes empty,
        // while b and c keep their places instead of sliding left.
        place_at(&mut v, 0, 3);
        assert_eq!(slot_map_of(&v), vec![None, Some(1), Some(2), Some(0)]);

        // Fill the gap again.
        place_at(&mut v, 2, 0);
        assert_eq!(slot_map_of(&v), vec![Some(2), Some(1), None, Some(0)]);

        // Dropping onto an occupied cell swaps the two, losing nothing.
        place_at(&mut v, 2, 1);
        assert_eq!(slot_map_of(&v), vec![Some(1), Some(2), None, Some(0)]);

        // Dropping a tile back on its own cell is a no-op.
        let before = v.clone();
        place_at(&mut v, 2, 1);
        assert_eq!(slot_map_of(&v), slot_map_of(&before));
    }
}

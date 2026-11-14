//! The search field: a real single-line EDIT child control.
//!
//! It used to be painted into the panel's own DIB, with characters fed in by
//! `WM_CHAR`. That cost a full board repaint per keystroke, and it also lacked
//! everything a text field is expected to do: caret, selection, Ctrl+A/C/X,
//! double-click word select, IME composition placement and paste. The control
//! owns all of that now; the panel only positions it and reads `EN_CHANGE`.
//!
//! What the control does not own is the panel's own key map (Esc, Enter,
//! Ctrl+Q/,,F). Those arrive here, in the field's window procedure, so they are
//! forwarded to `App::on_search_key` instead of being left to the EDIT default,
//! which would only beep.

use crate::app::with_app;
use crate::win;

/// Control id, so `WM_COMMAND` notifications can be told apart.
pub const ID: usize = 101;

/// Placeholder shown while the field is empty.
pub const HINT: &str = "搜索文件，或输入 dx 12345 转中文大写";

const TEXT_SIZE: i32 = 15;
/// Inset that lines the text up with the old painted field (56px from the
/// panel's inner left edge, field starts at 48).
const MARGIN_LEFT: i32 = 8;
const MARGIN_RIGHT: i32 = 4;

const HINT_COLOR: u32 = 0x009A_9A9A; // rgb(0x9A,0x9A,0x9A), little-endian BGR

pub struct Field {
    pub hwnd: win::HWND,
    /// True when `EM_SETCUEBANNER` was refused, so the placeholder has to be
    /// drawn by hand. Every Windows since Vista accepts the message; the flag
    /// exists so an older or shimmed control still shows the hint.
    pub own_hint: bool,
}

/// Create the field inside `parent`, using a client-space rectangle.
pub fn create(parent: win::HWND, inst: win::HINSTANCE, rc: (i32, i32, i32, i32)) -> Field {
    unsafe {
        let cls = win::wide("EDIT");
        let empty = win::wide("");
        let style = win::WS_CHILD | win::WS_VISIBLE | win::ES_LEFT | win::ES_AUTOHSCROLL;
        let hwnd = win::CreateWindowExW(0, cls.as_ptr(), empty.as_ptr(), style,
            rc.0, rc.1, rc.2, rc.3, parent, ID as win::HMENU, inst, std::ptr::null_mut());
        if hwnd.is_null() {
            win::log("搜索框 EDIT 创建失败");
            return Field { hwnd, own_hint: false };
        }

        let font = win::font(TEXT_SIZE, false);
        win::SendMessageW(hwnd, win::WM_SETFONT, font as usize, 1);

        // Left margin first, so text and caret start at the same x that the
        // painted field used; the right margin only decides when the text
        // starts scrolling.
        let margins = (MARGIN_LEFT as isize) | ((MARGIN_RIGHT as isize) << 16);
        win::SendMessageW(hwnd, win::EM_SETMARGINS,
            win::EC_LEFTMARGIN | win::EC_RIGHTMARGIN, margins);

        // wParam 1 keeps the cue visible while the field has focus, which is
        // what the painted version did.
        let hint = win::wide(HINT);
        let cue = win::SendMessageW(hwnd, win::EM_SETCUEBANNER, 1, hint.as_ptr() as isize);
        let own_hint = cue == 0;

        let old = win::SetWindowLongPtrW(hwnd, win::GWLP_WNDPROC, edit_proc as *const () as isize);
        win::SetWindowLongPtrW(hwnd, win::GWLP_USERDATA, old);
        win::log(if own_hint { "搜索框提示语: 自绘兜底" } else { "搜索框提示语: 系统 cue banner" });
        Field { hwnd, own_hint }
    }
}

pub fn text(h: win::HWND) -> String {
    win::window_text(h)
}

pub fn set_text(h: win::HWND, s: &str) {
    if h.is_null() { return; }
    unsafe {
        let w = win::wide(s);
        win::SetWindowTextW(h, w.as_ptr());
    }
}

/// Ctrl+F: select the whole query so the next keystroke replaces it.
pub fn select_all(h: win::HWND) {
    if h.is_null() { return; }
    unsafe {
        win::SendMessageW(h, win::EM_SETSEL, 0, -1);
    }
}

/// Put the caret after the last character. `SetWindowText` resets it to the
/// start, which would jump away from where the user was typing.
pub fn caret_to_end(h: win::HWND) {
    if h.is_null() { return; }
    unsafe {
        let n = win::GetWindowTextLengthW(h).max(0) as isize;
        win::SendMessageW(h, win::EM_SETSEL, n as usize, n);
    }
}

unsafe fn chain(old: isize, h: win::HWND, msg: u32, w: win::WPARAM, l: win::LPARAM) -> win::LRESULT {
    let proc: win::WNDPROC = std::mem::transmute(old);
    win::CallWindowProcW(proc, h, msg, w, l)
}

unsafe extern "system" fn edit_proc(h: win::HWND, msg: u32, w: win::WPARAM, l: win::LPARAM) -> win::LRESULT {
    let old = win::GetWindowLongPtrW(h, win::GWLP_USERDATA);
    // Before the class procedure, not after: Esc and Enter have to be answered
    // before the control turns them into a beep. WM_SYSKEYDOWN is included so
    // Alt+F4 still closes the panel instead of being swallowed by the child.
    if msg == win::WM_KEYDOWN || msg == win::WM_SYSKEYDOWN {
        if with_app(|app| app.on_search_key(w as u32)) == Some(true) { return 0; }
    }
    // The wheel goes to the window with focus, and the list it is meant to
    // scroll belongs to the panel.
    if msg == win::WM_MOUSEWHEEL {
        let parent = win::GetParent(h);
        if !parent.is_null() { win::SendMessageW(parent, msg, w, l); }
        return 0;
    }
    if msg == win::WM_PAINT {
        let r = chain(old, h, msg, w, l);
        paint_hint(h);
        return r;
    }
    chain(old, h, msg, w, l)
}

/// Drawn only when the system cue banner is unavailable and the field is empty.
unsafe fn paint_hint(h: win::HWND) {
    let own = with_app(|app| app.search.own_hint).unwrap_or(false);
    if !own { return; }
    if win::GetWindowTextLengthW(h) > 0 { return; }
    let dc = win::GetDC(h);
    if dc.is_null() { return; }
    let mut rc: win::RECT = Default::default();
    if win::GetClientRect(h, &mut rc) != 0 {
        rc.left += MARGIN_LEFT;
        rc.right -= MARGIN_RIGHT;
        let font = win::font(TEXT_SIZE, false);
        let old_font = win::SelectObject(dc, font);
        win::SetBkMode(dc, win::TRANSPARENT);
        win::SetTextColor(dc, HINT_COLOR);
        let s = win::wide(HINT);
        win::DrawTextW(dc, s.as_ptr(), (s.len() - 1) as i32, &mut rc,
            win::DT_VCENTER | win::DT_SINGLELINE | win::DT_NOCLIP | win::DT_END_ELLIPSIS);
        win::SelectObject(dc, old_font);
    }
    win::ReleaseDC(h, dc);
}

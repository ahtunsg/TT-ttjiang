// Selection overlay, four-button action bar and toast.
//
// THE FIX for "the highlight box is invisible on another machine":
//
//   The C# build used WS_EX_LAYERED + SetLayeredWindowAttributes(alpha=148)
//   and only invalidated a small dirty rect while dragging. Whether DWM
//   recomposites a layered window after a partial InvalidateRect depends on
//   the display driver / RDP session / composition path, so on some machines
//   the first (dim-only) frame showed up and no selection frame ever did.
//   The blue border was also multiplied by 148/255, making it very faint.
//
//   This version uses a completely OPAQUE full-screen popup. The dimming is
//   baked into the pixels (same visual result as alpha blending the dark
//   layer over the desktop), the border is drawn at full opacity with a dark
//   outline, and every mouse move triggers a FULL window redraw via
//   InvalidateRect(NULL) + UpdateWindow. No layered window, no LWA_ALPHA, no
//   partial invalidation, no DWM recomposition dependency.
//
//   Grab alpha is also forced to 0xFF (see capture.rs), so the selection
//   "clear" area can never be a transparent source over the dim layer.

use std::ffi::c_void;

use crate::image::Image;
use crate::types::Rect;
use crate::win;

pub const CLASS_OVERLAY: &str = "TTJiangOverlay4";
pub const CLASS_BAR: &str = "TTJiangActionBar4";
pub const CLASS_TOAST: &str = "TTJiangToast4";
pub const CLASS_HOST: &str = "TTJiangHost4";
pub const CLASS_MAIN: &str = "TTJiangMain4";

pub const ACTION_IMAGE: i32 = 0;
pub const ACTION_SAVE: i32 = 1;
pub const ACTION_TABLE: i32 = 2;
pub const ACTION_TEXT: i32 = 3;
pub const ACTION_CANCEL: i32 = -1;

const DIM_R: u8 = 10;
const DIM_G: u8 = 15;
const DIM_B: u8 = 26;
const OVERLAY_ALPHA: u32 = 148;
const INV_ALPHA: u32 = 255 - OVERLAY_ALPHA;
const ACCENT: (u8, u8, u8) = (59, 130, 246); // R,G,B
const OUTLINE: (u8, u8, u8) = (9, 14, 24);

fn blend(c: u8, screen: u8) -> u8 {
    ((c as u32 * OVERLAY_ALPHA + screen as u32 * INV_ALPHA + 127) / 255) as u8
}

pub struct Overlay {
    pub hwnd: win::HWND,
    pub mem_dc: win::HDC,
    pub hbm: win::HBITMAP,
    pub old_bm: win::HGDIOBJ,
    pub bits: *mut u8,
    pub w: i32,
    pub h: i32,
    pub vs: Rect,
    pub screen: Image,
    pub dim_bg: Vec<u8>,
    pub frame: Vec<u8>,
    pub dragging: bool,
    pub start: (i32, i32),
    pub rect: Rect,
    pub has_rect: bool,
    pub last_draw: u64,
}

impl Overlay {
    pub fn new(screen: Image, vs: Rect, class: &[u16]) -> Result<Overlay, String> {
        let w = screen.w;
        let h = screen.h;
        unsafe {
            let mem_dc = win::CreateCompatibleDC(std::ptr::null_mut());
            if mem_dc.is_null() {
                return Err("CreateCompatibleDC 失败".into());
            }
            let mut bmi: win::BITMAPINFOHEADER = std::mem::zeroed();
            bmi.biSize = std::mem::size_of::<win::BITMAPINFOHEADER>() as u32;
            bmi.biWidth = w;
            bmi.biHeight = -h;
            bmi.biPlanes = 1;
            bmi.biBitCount = 32;
            bmi.biCompression = 0;
            let mut bits: *mut c_void = std::ptr::null_mut();
            let hbm = win::CreateDIBSection(mem_dc, &bmi, 0, &mut bits, std::ptr::null_mut(), 0);
            if hbm.is_null() || bits.is_null() {
                win::DeleteDC(mem_dc);
                return Err(format!("CreateDIBSection 失败 err={}", win::GetLastError()));
            }
            let old_bm = win::SelectObject(mem_dc, hbm);
            let title = win::wide("TT酱截图");
            let hwnd = win::CreateWindowExW(
                win::WS_EX_TOPMOST, class.as_ptr(), title.as_ptr(), win::WS_POPUP,
                vs.x, vs.y, w, h,
                std::ptr::null_mut(), std::ptr::null_mut(),
                win::GetModuleHandleW(std::ptr::null()), std::ptr::null_mut(),
            );
            if hwnd.is_null() {
                win::SelectObject(mem_dc, old_bm);
                win::DeleteObject(hbm);
                win::DeleteDC(mem_dc);
                return Err(format!("创建遮罩窗口失败 err={}", win::GetLastError()));
            }
            let len = (w as usize) * (h as usize) * 4;
            let mut dim_bg = vec![0u8; len];
            for i in 0..(w as usize) * (h as usize) {
                let p = i * 4;
                dim_bg[p] = blend(DIM_B, screen.data[p]);         // B
                dim_bg[p + 1] = blend(DIM_G, screen.data[p + 1]); // G
                dim_bg[p + 2] = blend(DIM_R, screen.data[p + 2]); // R
                dim_bg[p + 3] = 0xFF;                             // opaque
            }
            let mut ov = Overlay {
                hwnd, mem_dc, hbm, old_bm, bits: bits as *mut u8, w, h, vs,
                screen, dim_bg: dim_bg.clone(), frame: dim_bg,
                dragging: false, start: (0, 0), rect: Rect::new(0, 0, 0, 0),
                has_rect: false, last_draw: 0,
            };
            ov.redraw();
            win::log(&format!("overlay ok hwnd={:?} {}x{} at ({},{}) opaque full-redraw", hwnd, w, h, vs.x, vs.y));
            Ok(ov)
        }
    }

    pub fn show(&self) {
        unsafe {
            win::ShowWindow(self.hwnd, win::SW_SHOW);
            // Ctrl+1 fires while another app owns the foreground, so a plain
            // SetForegroundWindow is refused and the overlay ends up painted
            // over the screen but unable to hear Esc.
            win::take_foreground(self.hwnd);
            win::SetCursor(win::LoadCursorW(std::ptr::null_mut(), win::IDC_CROSS as *const u16));
            // A brand-new topmost popup can have its first WM_PAINT deferred by
            // DWM on some drivers. Force a synchronous full paint so the first
            // frame is never the blank desktop-coloured window.
            win::InvalidateRect(self.hwnd, std::ptr::null(), 0);
            win::UpdateWindow(self.hwnd);
        }
    }

    /// Full-frame rebuild. Called on every mouse move (16 ms throttle in app).
    pub fn redraw(&mut self) {
        self.frame.copy_from_slice(&self.dim_bg);
        if self.has_rect && self.rect.w > 1 && self.rect.h > 1 {
            let r = self.rect.clamp(self.w, self.h);
            if !r.is_empty() {
                for y in r.y..r.bottom() {
                    let s = ((y * self.w + r.x) * 4) as usize;
                    let d = s;
                    let n = (r.w * 4) as usize;
                    self.frame[d..d + n].copy_from_slice(&self.screen.data[s..s + n]);
                }
                draw_border(&mut self.frame, self.w, self.h, r);
            }
        }
        unsafe {
            let len = (self.w as usize) * (self.h as usize) * 4;
            std::ptr::copy_nonoverlapping(self.frame.as_ptr(), self.bits, len);
            // Full invalidation + synchronous paint. This is the key change:
            // no partial dirty rect, no layered-surface recomposition.
            win::InvalidateRect(self.hwnd, std::ptr::null(), 0);
            win::UpdateWindow(self.hwnd);
        }
    }

    pub fn paint(&self, dc: win::HDC) {
        unsafe {
            if win::BitBlt(dc, 0, 0, self.w, self.h, self.mem_dc, 0, 0, win::SRCCOPY) == 0 {
                win::log(&format!("overlay BitBlt 失败 err={}", win::GetLastError()));
            }
            let font = win::font(14, false);
            let old = win::SelectObject(dc, font);
            win::SetBkMode(dc, win::TRANSPARENT);
            win::SetTextColor(dc, win::rgb(235, 240, 250));
            let hint = win::wide("拖动鼠标框选区域    ·    Esc 取消");
            win::TextOutW(dc, 20, 18, hint.as_ptr(), (hint.len() - 1) as i32);
            if self.has_rect && self.rect.w > 1 && self.rect.h > 1 {
                let label = format!("{} × {}", self.rect.w, self.rect.h);
                let lw = 110;
                let lx = self.rect.x.max(0);
                let ly = (self.rect.y - 26).max(2);
                let rc = win::RECT { left: lx, top: ly, right: lx + lw, bottom: ly + 21 };
                let brush = win::CreateSolidBrush(win::rgb(17, 24, 39));
                win::FillRect(dc, &rc, brush);
                win::DeleteObject(brush);
                win::SetTextColor(dc, win::rgb(249, 250, 251));
                let t = win::wide(&label);
                let mut trc = rc;
                win::DrawTextW(dc, t.as_ptr(), (t.len() - 1) as i32, &mut trc,
                    win::DT_CENTER | win::DT_VCENTER | win::DT_SINGLELINE);
            }
            win::SelectObject(dc, old);
        }
    }

    pub fn screen_rect(&self) -> Rect {
        Rect::new(self.rect.x + self.vs.x, self.rect.y + self.vs.y, self.rect.w, self.rect.h)
    }

    pub fn close(&mut self) {
        unsafe {
            if !self.hwnd.is_null() { win::DestroyWindow(self.hwnd); self.hwnd = std::ptr::null_mut(); }
            if !self.mem_dc.is_null() {
                win::SelectObject(self.mem_dc, self.old_bm);
                win::DeleteObject(self.hbm);
                win::DeleteDC(self.mem_dc);
                self.mem_dc = std::ptr::null_mut();
            }
        }
    }
}

fn put(buf: &mut [u8], w: i32, h: i32, x: i32, y: i32, color: (u8, u8, u8)) {
    if x < 0 || y < 0 || x >= w || y >= h { return; }
    let p = ((y * w + x) * 4) as usize;
    if p + 3 >= buf.len() { return; }
    buf[p] = color.2;
    buf[p + 1] = color.1;
    buf[p + 2] = color.0;
    buf[p + 3] = 0xFF;
}

fn hline(buf: &mut [u8], w: i32, h: i32, x0: i32, x1: i32, y: i32, color: (u8, u8, u8)) {
    for x in x0.min(x1)..=x0.max(x1) { put(buf, w, h, x, y, color); }
}
fn vline(buf: &mut [u8], w: i32, h: i32, x: i32, y0: i32, y1: i32, color: (u8, u8, u8)) {
    for y in y0.min(y1)..=y0.max(y1) { put(buf, w, h, x, y, color); }
}
fn fill_rect(buf: &mut [u8], w: i32, h: i32, r: Rect, color: (u8, u8, u8)) {
    for y in r.y..r.bottom() {
        for x in r.x..r.right() { put(buf, w, h, x, y, color); }
    }
}

/// Fully opaque border: 1px dark outline + 2px accent + corner grips.
fn draw_border(buf: &mut [u8], w: i32, h: i32, r: Rect) {
    let l = r.x;
    let t = r.y;
    let rr = r.right() - 1;
    let bb = r.bottom() - 1;
    // Dark outer outline (1 px) so the blue survives on light wallpapers.
    hline(buf, w, h, l - 1, rr + 1, t - 1, OUTLINE);
    hline(buf, w, h, l - 1, rr + 1, bb + 1, OUTLINE);
    vline(buf, w, h, l - 1, t - 1, bb + 1, OUTLINE);
    vline(buf, w, h, rr + 1, t - 1, bb + 1, OUTLINE);
    // 2 px accent, fully opaque.
    for k in 0..2 {
        hline(buf, w, h, l + k, rr - k, t + k, ACCENT);
        hline(buf, w, h, l + k, rr - k, bb - k, ACCENT);
        vline(buf, w, h, l + k, t + k, bb - k, ACCENT);
        vline(buf, w, h, rr - k, t + k, bb - k, ACCENT);
    }
    // Corner grips.
    for &(cx, cy) in &[(l, t), (rr, t), (l, bb), (rr, bb)] {
        fill_rect(buf, w, h, Rect::new(cx - 3, cy - 3, 7, 7), ACCENT);
        fill_rect(buf, w, h, Rect::new(cx - 1, cy - 1, 3, 3), (255, 255, 255));
    }
}

// ---- action bar ----------------------------------------------------------
pub struct ActionBar {
    pub hwnd: win::HWND,
    pub w: i32,
    pub h: i32,
    pub hover: i32,
}

impl ActionBar {
    pub fn new(class: &[u16]) -> Result<ActionBar, String> {
        let w = 216;
        let h = 52;
        unsafe {
            let title = win::wide("TT酱动作条");
            let hwnd = win::CreateWindowExW(
                win::WS_EX_TOPMOST | win::WS_EX_TOOLWINDOW,
                class.as_ptr(), title.as_ptr(), win::WS_POPUP,
                0, 0, w, h,
                std::ptr::null_mut(), std::ptr::null_mut(),
                win::GetModuleHandleW(std::ptr::null()), std::ptr::null_mut(),
            );
            if hwnd.is_null() { return Err("创建动作条失败".into()); }
            Ok(ActionBar { hwnd, w, h, hover: -1 })
        }
    }

    /// Anchor the bar to the mouse cursor, not to the selection corner.
    /// The 3.3 build used selection.Bottom/Right, so dragging up-left left the
    /// bar stranded in the opposite corner from the cursor. Follow the cursor:
    /// centred below it, flipping above when there is no room, and clamped so
    /// it never leaves the work area.
    pub fn place_near(&self, _sel: Rect) {
        unsafe {
            let wa = win::work_area();
            let mut pt = win::POINT { x: 0, y: 0 };
            win::GetCursorPos(&mut pt);
            let cx = if pt.x != 0 || pt.y != 0 { pt.x } else { _sel.right() };
            let cy = if pt.x != 0 || pt.y != 0 { pt.y } else { _sel.bottom() };

            let gap = 12;
            let mut x = cx - self.w / 2;
            let mut y = cy + gap;
            if y + self.h > wa.bottom() - 8 {
                y = cy - self.h - gap;
            }
            if y < wa.y + 8 { y = wa.y + 8; }
            let min_x = wa.x + 4;
            let max_x = (wa.right() - self.w - 4).max(min_x);
            if x < min_x { x = min_x; }
            if x > max_x { x = max_x; }

            // SWP_NOMOVE must NOT be set: that was why the bar stayed at (0,0)
            // instead of following the cursor.
            win::SetWindowPos(
                self.hwnd, -1isize as win::HWND, x, y, 0, 0,
                win::SWP_NOSIZE | win::SWP_NOACTIVATE | win::SWP_SHOWWINDOW,
            );
            win::ShowWindow(self.hwnd, win::SW_SHOW);
            // Same story as the overlay: the bar carries Esc / Enter / 1-4, so
            // it has to own the keyboard rather than only look focused.
            win::take_foreground(self.hwnd);
        }
    }

    pub fn hit(&self, x: i32, _y: i32) -> i32 {
        let bw = 46;
        let gap = 4;
        let pad = 7;
        for i in 0..4 {
            let l = pad + i * (bw + gap);
            if x >= l && x < l + bw { return i; }
        }
        -1
    }

    pub fn paint(&self, dc: win::HDC) {
        unsafe {
            let bg = win::CreateSolidBrush(win::rgb(17, 24, 39));
            let frame = win::CreatePen(0, 1, win::rgb(70, 75, 85));
            let old_b = win::SelectObject(dc, bg);
            let old_p = win::SelectObject(dc, frame);
            win::RoundRect(dc, 0, 0, self.w, self.h, 8, 8);
            let accent = win::CreateSolidBrush(win::rgb(59, 130, 246));
            let pen_w = win::CreatePen(0, 2, win::rgb(249, 250, 251));
            let mut old_pen: win::HGDIOBJ = std::ptr::null_mut();
            for i in 0..4 {
                let l = 7 + i * 50;
                let r = l + 46;
                if i == self.hover {
                    let old = win::SelectObject(dc, accent);
                    win::RoundRect(dc, l, 7, r, 45, 6, 6);
                    win::SelectObject(dc, old);
                }
                let oldp = win::SelectObject(dc, pen_w);
                if old_pen.is_null() { old_pen = oldp; }
                draw_icon(dc, i, l + 8, 12);
            }
            if !old_pen.is_null() { win::SelectObject(dc, old_pen); }
            let small = win::font(9, false);
            let old_f = win::SelectObject(dc, small);
            win::SetBkMode(dc, win::TRANSPARENT);
            win::SetTextColor(dc, win::rgb(156, 163, 175));
            for i in 0..4 {
                let t = win::wide(&(i + 1).to_string());
                win::TextOutW(dc, 7 + i * 50 + 34, 34, t.as_ptr(), 1);
            }
            win::SelectObject(dc, old_f);
            win::SelectObject(dc, old_p);
            win::SelectObject(dc, old_b);
            win::DeleteObject(bg);
            win::DeleteObject(frame);
            win::DeleteObject(accent);
            win::DeleteObject(pen_w);
        }
    }

    pub fn close(&mut self) {
        unsafe {
            if !self.hwnd.is_null() { win::DestroyWindow(self.hwnd); self.hwnd = std::ptr::null_mut(); }
        }
    }
}

fn line(dc: win::HDC, x0: i32, y0: i32, x1: i32, y1: i32) {
    unsafe {
        win::MoveToEx(dc, x0, y0, std::ptr::null_mut());
        win::LineTo(dc, x1, y1);
    }
}

fn box_(dc: win::HDC, l: i32, t: i32, r: i32, b: i32) {
    line(dc, l, t, r, t); line(dc, r, t, r, b);
    line(dc, r, b, l, b); line(dc, l, b, l, t);
}

fn draw_icon(dc: win::HDC, idx: i32, x: i32, y: i32) {
    match idx {
        0 => { // image: two frames + a hill
            box_(dc, x, y, x + 26, y + 20);
            box_(dc, x + 3, y + 3, x + 23, y + 17);
            line(dc, x + 6, y + 15, x + 12, y + 9);
            line(dc, x + 12, y + 9, x + 17, y + 14);
            line(dc, x + 17, y + 14, x + 21, y + 11);
        }
        1 => { // save: down arrow onto a tray
            line(dc, x + 13, y + 1, x + 13, y + 15);
            line(dc, x + 8, y + 10, x + 13, y + 15);
            line(dc, x + 18, y + 10, x + 13, y + 15);
            line(dc, x + 3, y + 19, x + 23, y + 19);
            line(dc, x + 3, y + 19, x + 3, y + 15);
            line(dc, x + 23, y + 19, x + 23, y + 15);
        }
        2 => { // table: 2x2 grid
            box_(dc, x + 1, y + 2, x + 25, y + 19);
            line(dc, x + 13, y + 2, x + 13, y + 19);
            line(dc, x + 1, y + 10, x + 25, y + 10);
        }
        _ => { // text: three lines
            for k in 0..3 { line(dc, x + 2, y + 3 + k * 7, x + 24 - k * 4, y + 3 + k * 7); }
        }
    }
}

// ---- toast ---------------------------------------------------------------
pub struct Toast {
    pub hwnd: win::HWND,
    pub text: String,
    pub ok: bool,
    pub timer_id: usize,
}

impl Toast {
    pub fn new(class: &[u16]) -> Result<Toast, String> {
        unsafe {
            let title = win::wide("TT酱提示");
            let hwnd = win::CreateWindowExW(
                win::WS_EX_TOPMOST | win::WS_EX_TOOLWINDOW | win::WS_EX_NOACTIVATE,
                class.as_ptr(), title.as_ptr(), win::WS_POPUP,
                0, 0, 470, 46,
                std::ptr::null_mut(), std::ptr::null_mut(),
                win::GetModuleHandleW(std::ptr::null()), std::ptr::null_mut(),
            );
            if hwnd.is_null() { return Err("创建提示条失败".into()); }
            Ok(Toast { hwnd, text: String::new(), ok: true, timer_id: 1 })
        }
    }

    pub fn show(&mut self, text: &str, ok: bool, ms: u32) {
        unsafe {
            self.text = text.into();
            self.ok = ok;
            let wa = win::work_area();
            let x = wa.right() - 470 - 24;
            let y = wa.bottom() - 46 - 28;
            win::SetWindowPos(
                self.hwnd, -1isize as win::HWND, x, y, 0, 0,
                win::SWP_NOSIZE | win::SWP_NOACTIVATE | win::SWP_SHOWWINDOW,
            );
            win::ShowWindow(self.hwnd, win::SW_SHOWNOACTIVATE);
            win::InvalidateRect(self.hwnd, std::ptr::null(), 0);
            win::UpdateWindow(self.hwnd);
            win::SetTimer(self.hwnd, self.timer_id, ms.max(300), std::ptr::null_mut());
        }
    }

    pub fn hide(&mut self) {
        unsafe {
            win::KillTimer(self.hwnd, self.timer_id);
            win::ShowWindow(self.hwnd, win::SW_HIDE);
        }
    }

    pub fn paint(&self, dc: win::HDC) {
        unsafe {
            let bg = win::CreateSolidBrush(win::rgb(17, 24, 39));
            let rc = win::RECT { left: 0, top: 0, right: 470, bottom: 46 };
            win::FillRect(dc, &rc, bg);
            win::DeleteObject(bg);
            let color = if self.ok { win::rgb(52, 211, 153) } else { win::rgb(248, 113, 113) };
            let bar = win::CreateSolidBrush(color);
            let strip = win::RECT { left: 0, top: 0, right: 3, bottom: 46 };
            win::FillRect(dc, &strip, bar);
            win::DeleteObject(bar);
            let font = win::font(13, false);
            let old = win::SelectObject(dc, font);
            win::SetBkMode(dc, win::TRANSPARENT);
            win::SetTextColor(dc, win::rgb(249, 250, 251));
            let t = win::wide(&self.text);
            let mut trc = win::RECT { left: 16, top: 0, right: 458, bottom: 46 };
            win::DrawTextW(dc, t.as_ptr(), (t.len() - 1) as i32, &mut trc,
                win::DT_VCENTER | win::DT_SINGLELINE | win::DT_NOCLIP);
            win::SelectObject(dc, old);
        }
    }

    pub fn close(&mut self) {
        unsafe {
            win::KillTimer(self.hwnd, self.timer_id);
            if !self.hwnd.is_null() { win::DestroyWindow(self.hwnd); self.hwnd = std::ptr::null_mut(); }
        }
    }
}

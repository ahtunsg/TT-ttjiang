// Card-style Settings / About windows.
//
// The launcher paints itself: flat white, one light card with a 1px border,
// a tight type scale and rounded buttons. The two info windows were the only
// places still built from themed Win32 BUTTON/STATIC children, so they came
// out with grey plates, sunken borders and 2009-era spacing next to a panel
// that looks nothing like that. They are painted here instead, with the same
// palette, radii and hover behaviour the launcher already uses.
//
// Only these two windows are custom-drawn; the shortcut editor keeps real
// EDIT controls, because a text field has to behave like one.

use crate::win;

#[derive(Clone, Copy, PartialEq)]
pub enum Tone { Ink, Dim, Faint, Accent }

struct Line {
    text: String,
    x: i32,
    y: i32,
    size: i32,
    bold: bool,
    tone: Tone,
}

struct Button {
    id: i32,
    label: String,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    primary: bool,
}

pub struct Card {
    lines: Vec<Line>,
    rules: Vec<i32>,
    buttons: Vec<Button>,
    hover: i32,
    pressed: i32,
    tracking: bool,
}

fn ink() -> u32 { win::rgb(0x1F, 0x1F, 0x1F) }
fn dim() -> u32 { win::rgb(0x6B, 0x6B, 0x6B) }
fn faint() -> u32 { win::rgb(0x9A, 0x9A, 0x9A) }
fn accent() -> u32 { win::rgb(0x0F, 0x6C, 0xBD) }
fn hover_fill() -> u32 { win::rgb(0xE3, 0xEE, 0xFB) }
fn press_fill() -> u32 { win::rgb(0xD3, 0xE5, 0xF8) }
fn hairline() -> u32 { win::rgb(0xE3, 0xE3, 0xE3) }

impl Card {
    pub fn new() -> Card {
        Card {
            lines: Vec::new(),
            rules: Vec::new(),
            buttons: Vec::new(),
            hover: -1,
            pressed: -1,
            tracking: false,
        }
    }

    pub fn text(&mut self, text: &str, x: i32, y: i32, size: i32, bold: bool, tone: Tone) {
        self.lines.push(Line { text: text.to_string(), x, y, size, bold, tone });
    }

    /// 1px separator spanning the card's inner width.
    pub fn rule(&mut self, y: i32) { self.rules.push(y); }

    pub fn button(&mut self, id: i32, label: &str, x: i32, y: i32, w: i32, h: i32, primary: bool) {
        self.buttons.push(Button { id, label: label.to_string(), x, y, w, h, primary });
    }

    unsafe fn hit(&self, x: i32, y: i32) -> i32 {
        for b in &self.buttons {
            if x >= b.x && x < b.x + b.w && y >= b.y && y < b.y + b.h {
                return b.id;
            }
        }
        -1
    }
}

/// Hand the card to the window. Freed in WM_DESTROY.
pub fn attach(card: Card, hwnd: win::HWND) {
    let raw = Box::into_raw(Box::new(card));
    unsafe { win::SetWindowLongPtrW(hwnd, win::GWLP_USERDATA, raw as isize); }
}

unsafe fn card_of(hwnd: win::HWND) -> *mut Card {
    win::GetWindowLongPtrW(hwnd, win::GWLP_USERDATA) as *mut Card
}

fn tone_color(t: Tone) -> u32 {
    match t {
        Tone::Ink => ink(),
        Tone::Dim => dim(),
        Tone::Faint => faint(),
        Tone::Accent => accent(),
    }
}

unsafe fn text_at(dc: win::HDC, s: &str, x: i32, y: i32, w: i32, size: i32, bold: bool, color: u32) {
    draw_text(dc, s, x, y, x + w, y + size + 10, size, bold, color, 0);
}

/// Button captions are centred inside the whole button box. DT_VCENTER centres
/// the line in whatever rect it is handed, so this must be the button rect and
/// not a text-height strip anchored at the button's top edge — that earlier
/// version left every caption sitting high in its button.
unsafe fn text_center(dc: win::HDC, s: &str, l: i32, t: i32, w: i32, h: i32,
                      size: i32, bold: bool, color: u32) {
    draw_text(dc, s, l, t, l + w, t + h, size, bold, color, win::DT_CENTER);
}

unsafe fn draw_text(dc: win::HDC, s: &str, l: i32, t: i32, r: i32, b: i32, size: i32,
                    bold: bool, color: u32, align: u32) {
    let f = win::make_font(size, bold);
    let old = win::SelectObject(dc, f);
    win::SetBkMode(dc, win::TRANSPARENT);
    win::SetTextColor(dc, color);
    let mut rc = win::RECT { left: l, top: t, right: r, bottom: b };
    let wt = win::wide(s);
    win::DrawTextW(dc, wt.as_ptr(), (wt.len() - 1) as i32, &mut rc,
        align | win::DT_VCENTER | win::DT_SINGLELINE | win::DT_NOPREFIX);
    win::SelectObject(dc, old);
    win::DeleteObject(f);
}

unsafe fn fill_box(dc: win::HDC, l: i32, t: i32, r: i32, b: i32, color: u32) {
    if r <= l || b <= t { return; }
    let br = win::CreateSolidBrush(color);
    let rc = win::RECT { left: l, top: t, right: r, bottom: b };
    win::FillRect(dc, &rc, br);
    win::DeleteObject(br);
}

unsafe fn round_box(dc: win::HDC, l: i32, t: i32, r: i32, b: i32, rad: i32,
                    fill: u32, border: Option<u32>) {
    let br = win::CreateSolidBrush(fill);
    let pen = match border {
        Some(c) => win::CreatePen(0, 1, c),
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

/// Box a caption is drawn into: always the full button rect, never a tighter
/// strip. Kept separate so a layout test can pin the invariant.
fn caption_box(b: &Button) -> (i32, i32, i32, i32) {
    (b.x, b.y, b.w, b.h)
}

unsafe fn paint_button(dc: win::HDC, b: &Button, hovered: bool, pressed: bool) {
    let (l, t) = (b.x, b.y);
    let (r, bt) = (b.x + b.w, b.y + b.h);
    let (cl, ct, cw, ch) = caption_box(b);
    if b.primary {
        // One filled accent button per window: the action the user came for.
        let fill = if pressed { win::rgb(0x0B, 0x5A, 0x9E) }
                   else if hovered { win::rgb(0x1A, 0x7B, 0xD0) }
                   else { accent() };
        round_box(dc, l, t, r, bt, 6, fill, None);
        text_center(dc, &b.label, cl, ct, cw, ch, 13, false, win::rgb(0xFF, 0xFF, 0xFF));
    } else {
        let fill = if pressed { press_fill() } else if hovered { hover_fill() }
                   else { win::rgb(0xFF, 0xFF, 0xFF) };
        let border = if hovered { win::rgb(0x6F, 0xA8, 0xDC) } else { win::rgb(0xC9, 0xC9, 0xC9) };
        round_box(dc, l, t, r, bt, 6, fill, Some(border));
        text_center(dc, &b.label, cl, ct, cw, ch, 13, false, ink());
    }
}

unsafe fn paint(hwnd: win::HWND, c: &Card) {
    let mut rc: win::RECT = Default::default();
    win::GetClientRect(hwnd, &mut rc);
    let (w, h) = (rc.right.max(1), rc.bottom.max(1));

    let mut ps: win::PAINTSTRUCT = Default::default();
    let dc = win::BeginPaint(hwnd, &mut ps);
    if dc.is_null() { return; }

    // Same off-screen pass as the launcher: hover changes repaint small areas
    // and a half-drawn frame would show the old and new button at once.
    let mem = win::CreateCompatibleDC(dc);
    let target = if mem.is_null() { dc } else { mem };
    let mut hbm: win::HBITMAP = std::ptr::null_mut();
    if !mem.is_null() {
        let bi = win::BITMAPINFOHEADER {
            biSize: std::mem::size_of::<win::BITMAPINFOHEADER>() as u32,
            biWidth: w, biHeight: -h, biPlanes: 1, biBitCount: 32,
            biCompression: 0, biSizeImage: 0, biXPelsPerMeter: 0,
            biYPelsPerMeter: 0, biClrUsed: 0, biClrImportant: 0,
        };
        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        hbm = win::CreateDIBSection(mem, &bi, 0, &mut bits, std::ptr::null_mut(), 0);
        if hbm.is_null() { win::DeleteDC(mem); }
    }
    let old = if !hbm.is_null() {
        win::SelectObject(target, hbm)
    } else {
        std::ptr::null_mut()
    };

    // Window background, then the single light card with its hairline border —
    // the identical construction to the launcher body.
    fill_box(target, 0, 0, w, h, win::rgb(0xFF, 0xFF, 0xFF));
    round_box(target, 12, 12, w - 12, h - 12, 8, win::rgb(0xFA, 0xFA, 0xFA), Some(hairline()));

    for l in &c.lines {
        text_at(target, &l.text, l.x, l.y, w - l.x - 32, l.size, l.bold, tone_color(l.tone));
    }
    for y in &c.rules {
        fill_box(target, 32, *y, w - 32, *y + 1, hairline());
    }
    for b in &c.buttons {
        paint_button(target, b, c.hover == b.id, c.pressed == b.id);
    }

    win::GdiFlush();
    if !hbm.is_null() {
        win::BitBlt(dc, 0, 0, w, h, mem, 0, 0, win::SRCCOPY);
        win::SelectObject(mem, old);
        win::DeleteObject(hbm);
        win::DeleteDC(mem);
    }
    win::EndPaint(hwnd, &ps);
}

/// Message hook for the two card windows. Returns false for messages the
/// default dialog proc should still see.
pub unsafe fn handle(hwnd: win::HWND, msg: u32, w: win::WPARAM, l: win::LPARAM) -> bool {
    let p = card_of(hwnd);
    if p.is_null() { return false; }
    let c = &mut *p;

    match msg {
        win::WM_ERASEBKGND => true,
        win::WM_PAINT => { paint(hwnd, c); true }
        win::WM_MOUSEMOVE => {
            let (x, y) = mouse_xy(l);
            let hit = c.hit(x, y);
            if hit != c.hover {
                c.hover = hit;
                win::InvalidateRect(hwnd, std::ptr::null(), 0);
            }
            if !c.tracking {
                let mut tme = win::TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<win::TRACKMOUSEEVENT>() as u32,
                    dwFlags: win::TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                win::TrackMouseEvent(&mut tme);
                c.tracking = true;
            }
            true
        }
        win::WM_MOUSELEAVE => {
            c.tracking = false;
            if c.hover != -1 {
                c.hover = -1;
                win::InvalidateRect(hwnd, std::ptr::null(), 0);
            }
            true
        }
        win::WM_LBUTTONDOWN => {
            let (x, y) = mouse_xy(l);
            let hit = c.hit(x, y);
            if hit >= 0 {
                c.pressed = hit;
                win::SetCapture(hwnd);
                win::InvalidateRect(hwnd, std::ptr::null(), 0);
            }
            true
        }
        win::WM_LBUTTONUP => {
            let (x, y) = mouse_xy(l);
            let hit = c.hit(x, y);
            let pressed = c.pressed;
            c.pressed = -1;
            win::ReleaseCapture();
            win::InvalidateRect(hwnd, std::ptr::null(), 0);
            if pressed >= 0 && pressed == hit {
                // Same path a native button would take: the dialog proc turns
                // this into WM_APP_DIALOG_CMD for the host.
                win::SendMessageW(hwnd, win::WM_COMMAND, pressed as usize, 0);
            }
            true
        }
        win::WM_CAPTURECHANGED => {
            if c.pressed != -1 {
                c.pressed = -1;
                win::InvalidateRect(hwnd, std::ptr::null(), 0);
            }
            true
        }
        win::WM_SETCURSOR => {
            if (l & 0xFFFF) as i32 != win::HTCLIENT { return false; }
            let mut pt: win::POINT = Default::default();
            win::GetCursorPos(&mut pt);
            win::ScreenToClient(hwnd, &mut pt);
            let cur = c.hit(pt.x, pt.y);
            let id = if cur >= 0 { win::IDC_HAND } else { win::IDC_ARROW };
            let h = win::LoadCursorW(std::ptr::null_mut(), id as *const u16);
            if !h.is_null() { win::SetCursor(h); }
            true
        }
        win::WM_KEYDOWN => {
            match w as u32 {
                win::VK_ESCAPE => { win::PostMessageW(hwnd, win::WM_CLOSE, 0, 0); }
                win::VK_RETURN => {
                    // Enter means "dismiss", never the accent action: in
                    // Settings the accent button starts the index service and
                    // an accidental Enter should not kick that off. Close is
                    // always the last button in both windows.
                    if let Some(b) = c.buttons.last() {
                        win::SendMessageW(hwnd, win::WM_COMMAND, b.id as usize, 0);
                    }
                }
                _ => return false,
            }
            true
        }
        win::WM_DESTROY => {
            drop(Box::from_raw(p));
            win::SetWindowLongPtrW(hwnd, win::GWLP_USERDATA, 0);
            true
        }
        _ => false,
    }
}

fn mouse_xy(l: win::LPARAM) -> (i32, i32) {
    let x = (l & 0xFFFF) as i16 as i32;
    let y = ((l >> 16) & 0xFFFF) as i16 as i32;
    (x, y)
}

/// Centre of a button in client coordinates. Used by the click self-check,
/// which drives the window with real mouse messages instead of guessing at
/// pixel positions from the outside.
pub unsafe fn button_center(hwnd: win::HWND, id: i32) -> Option<(i32, i32)> {
    let p = card_of(hwnd);
    if p.is_null() { return None; }
    (*p).buttons.iter().find(|b| b.id == id)
        .map(|b| (b.x + b.w / 2, b.y + b.h / 2))
}

// Layout introspection for the tests below. Card windows have no native
// children, so nothing else can catch a button that slid off the card or a
// label that ended up under a button.
#[cfg(test)]
impl Card {
    pub fn button_rects(&self) -> Vec<(i32, i32, i32, i32, i32)> {
        self.buttons.iter().map(|b| (b.id, b.x, b.y, b.w, b.h)).collect()
    }

    /// Approximate ink box of each label: a CJK glyph is about one em wide,
    /// Latin about half. Exact metrics would need a DC, and this only has to
    /// be good enough to notice a label running into a button.
    pub fn line_rects(&self) -> Vec<(i32, i32, i32, i32)> {
        self.lines.iter().map(|l| {
            let mut w = 0.0f32;
            for ch in l.text.chars() {
                w += if (ch as u32) > 0x2000 { l.size as f32 } else { l.size as f32 * 0.56 };
            }
            (l.x, l.y, (w as i32).max(8), l.size + 10)
        }).collect()
    }

    pub fn rule_ys(&self) -> Vec<i32> { self.rules.clone() }

    pub fn hit_test(&self, x: i32, y: i32) -> i32 { unsafe { self.hit(x, y) } }

    pub fn primary_count(&self) -> usize {
        self.buttons.iter().filter(|b| b.primary).count()
    }

    /// The boxes captions are actually drawn into, one per button.
    pub fn caption_boxes(&self) -> Vec<(i32, i32, i32, i32)> {
        self.buttons.iter().map(caption_box).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialogs::{build_card, card_client_size, DialogKind};

    fn cards() -> Vec<(DialogKind, Card, i32, i32)> {
        [DialogKind::Settings, DialogKind::About].iter().map(|k| {
            let (w, h) = card_client_size(*k).unwrap();
            (k.clone(), build_card(*k, "索引服务：内置实例未运行，可在下方启动"), w, h)
        }).collect()
    }

    #[test]
    fn buttons_sit_inside_the_card_and_do_not_overlap() {
        for (kind, card, w, h) in cards() {
            let rects = card.button_rects();
            assert!(!rects.is_empty());
            for (id, x, y, bw, bh) in &rects {
                // 12px card inset on every side, or the border cuts the button.
                assert!(*x >= 12 && *y >= 12, "{:?} button {} outside card", kind, id);
                assert!(x + bw <= w - 12 && y + bh <= h - 12,
                    "{:?} button {} runs past the card", kind, id);
            }
            for i in 0..rects.len() {
                for j in (i + 1)..rects.len() {
                    let (_, ax, ay, aw, ah) = rects[i];
                    let (_, bx, by, bw, bh) = rects[j];
                    let overlap = ax < bx + bw && bx < ax + aw && ay < by + bh && by < ay + ah;
                    assert!(!overlap, "{:?} buttons {} and {} overlap", kind, rects[i].0, rects[j].0);
                }
            }
        }
    }

    #[test]
    fn labels_clear_the_buttons_and_the_rules_stay_inside() {
        for (kind, card, w, h) in cards() {
            let buttons = card.button_rects();
            for (lx, ly, lw, lh) in card.line_rects() {
                for (id, bx, by, bw, bh) in &buttons {
                    let (bx, by, bw, bh) = (*bx, *by, *bw, *bh);
                    let clash = lx < bx + bw && bx < lx + lw && ly < by + bh && by < ly + lh;
                    assert!(!clash, "{:?} label at ({},{}) collides with button {}", kind, lx, ly, id);
                }
            }
            let rules = card.rule_ys();
            assert!(rules.windows(2).all(|p| p[0] < p[1]), "{:?} rules out of order", kind);
            for y in rules {
                assert!(y > 12 && y < h - 12, "{:?} rule at {} outside card", kind, y);
            }
            // One accent action per window; everything else is a quiet button.
            assert!(card.primary_count() <= 1, "{:?} has too many primary buttons", kind);
            assert!(w > 0 && h > 0);
        }
    }

    #[test]
    fn hit_test_is_half_open_so_neighbouring_buttons_never_both_fire() {
        let mut c = Card::new();
        c.button(1, "A", 32, 40, 100, 34, false);
        c.button(2, "B", 132, 40, 100, 34, false);
        assert_eq!(c.hit_test(32, 40), 1);
        assert_eq!(c.hit_test(131, 73), 1);
        assert_eq!(c.hit_test(132, 40), 2);
        assert_eq!(c.hit_test(231, 73), 2);
        // Right and bottom edges belong to the neighbour / empty space.
        assert_eq!(c.hit_test(232, 40), -1);
        assert_eq!(c.hit_test(32, 74), -1);
        assert_eq!(c.hit_test(0, 0), -1);
    }

    /// Paint a caption exactly the way paint_button does and report the rows
    /// its ink covers. GDI draws into a DIB without needing a window, so this
    /// runs headless under cargo test.
    fn caption_ink(label: &str, w: i32, h: i32) -> (i32, i32) {
        unsafe {
            let mem = win::CreateCompatibleDC(std::ptr::null_mut());
            assert!(!mem.is_null(), "no memory DC");
            let bi = win::BITMAPINFOHEADER {
                biSize: std::mem::size_of::<win::BITMAPINFOHEADER>() as u32,
                biWidth: w, biHeight: -h, biPlanes: 1, biBitCount: 32,
                biCompression: 0, biSizeImage: 0, biXPelsPerMeter: 0,
                biYPelsPerMeter: 0, biClrUsed: 0, biClrImportant: 0,
            };
            let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
            let bm = win::CreateDIBSection(mem, &bi, 0, &mut bits, std::ptr::null_mut(), 0);
            assert!(!bm.is_null() && !bits.is_null(), "no DIB section");
            let old = win::SelectObject(mem, bm);
            // Black plate, white glyphs: every lit pixel is caption ink.
            fill_box(mem, 0, 0, w, h, 0);
            text_center(mem, label, 0, 0, w, h, 13, false, 0x00FF_FFFF);
            win::GdiFlush();
            let px = std::slice::from_raw_parts(bits as *const u8, (w * h * 4) as usize);
            let (mut top, mut bottom) = (h, -1);
            for y in 0..h {
                for x in 0..w {
                    let i = ((y * w + x) * 4) as usize;
                    if px[i] > 8 || px[i + 1] > 8 || px[i + 2] > 8 {
                        top = top.min(y);
                        bottom = bottom.max(y);
                    }
                }
            }
            win::SelectObject(mem, old);
            win::DeleteObject(bm);
            win::DeleteDC(mem);
            (top, bottom)
        }
    }

    #[test]
    fn button_captions_are_vertically_centred_not_pinned_to_the_top() {
        // DT_VCENTER centres inside the rect it is given. Centring in a
        // text-height strip anchored at the button's top edge pushed every
        // caption about 6px high, which is what looked wrong on screen.
        for (label, w, h) in [("启动内置索引服务", 180, 34), ("关闭", 104, 34), ("导出…", 104, 34)] {
            let (top, bottom) = caption_ink(label, w, h);
            assert!(bottom >= top, "no ink drawn for {}", label);
            let centre = (top + bottom) / 2;
            assert!((centre - h / 2).abs() <= 2,
                "{} ink rows {}..{} in a {}px box: centre {} vs {}", label, top, bottom, h, centre, h / 2);
        }
    }

    #[test]
    fn caption_boxes_are_the_full_button_rect() {
        for (kind, card, _, _) in cards() {
            let boxes = card.caption_boxes();
            let rects = card.button_rects();
            assert_eq!(boxes.len(), rects.len());
            for (cap, (id, x, y, w, h)) in boxes.iter().zip(rects.iter()) {
                assert_eq!(*cap, (*x, *y, *w, *h), "{:?} button {} caption box shrank", kind, id);
                assert!(*h > 13 + 10, "{:?} button {} too short for its caption", kind, id);
            }
        }
    }
}

// Screen grab, clipboard and PNG output.
//
// Two fixes versus the C# build that mattered on the second machine:
//  * BitBlt into a 32bpp DIB leaves the alpha byte undefined. We force every
//    alpha byte to 0xFF immediately after the copy, so the "cleared" selection
//    area can never composite as invisible.
//  * Grab and overlay geometry are both strictly physical pixels.

use crate::image::Image;
use crate::types::Rect;
use crate::win;

pub fn grab_virtual_screen() -> Result<Image, String> {
    let vs = win::virtual_screen();
    match Image::grab_screen(vs) {
        Ok(img) => Ok(img),
        Err(e) => {
            win::log(&format!("抓屏失败: {}", e));
            Err(e)
        }
    }
}

/// CF_UNICODETEXT, with the same 8 x 60 ms retry loop as the C# build.
pub fn copy_text(text: &str) -> Result<(), String> {
    unsafe {
        let mut units: Vec<u16> = text.encode_utf16().collect();
        units.push(0);
        let bytes = units.len() * 2;
        for attempt in 0..8 {
            if win::OpenClipboard(std::ptr::null_mut()) != 0 {
                win::EmptyClipboard();
                let h = win::GlobalAlloc(win::GMEM_MOVEABLE, bytes);
                if !h.is_null() {
                    let p = win::GlobalLock(h) as *mut u16;
                    if !p.is_null() {
                        std::ptr::copy_nonoverlapping(units.as_ptr(), p, units.len());
                        win::GlobalUnlock(h);
                        if win::SetClipboardData(win::CF_UNICODETEXT, h).is_null() {
                            win::GlobalFree(h);
                            win::CloseClipboard();
                            return Err(format!("SetClipboardData 文本失败 err={}", win::GetLastError()));
                        }
                        win::CloseClipboard();
                        return Ok(());
                    }
                    win::GlobalFree(h);
                }
                win::CloseClipboard();
            }
            if attempt < 7 { win::Sleep(60); }
        }
        Err("写入剪贴板失败：剪贴板被其他程序占用".into())
    }
}

/// CF_DIB: 24bpp bottom-up DIB, no BITMAPFILEHEADER (matches C# Capture.CopyImage).
pub fn copy_image(img: &Image) -> Result<(), String> {
    unsafe {
        if img.w <= 0 || img.h <= 0 { return Err("图片为空".into()); }
        let stride = ((img.w * 3 + 3) / 4) * 4;
        let total = 40usize + (stride as usize) * (img.h as usize);
        for attempt in 0..8 {
            if win::OpenClipboard(std::ptr::null_mut()) != 0 {
                win::EmptyClipboard();
                let h = win::GlobalAlloc(win::GMEM_MOVEABLE | win::GMEM_ZEROINIT, total);
                if !h.is_null() {
                    let base = win::GlobalLock(h) as *mut u8;
                    if !base.is_null() {
                        let hdr = base as *mut win::BITMAPINFOHEADER;
                        (*hdr).biSize = 40;
                        (*hdr).biWidth = img.w;
                        (*hdr).biHeight = img.h; // positive = bottom-up
                        (*hdr).biPlanes = 1;
                        (*hdr).biBitCount = 24;
                        (*hdr).biCompression = 0;
                        (*hdr).biSizeImage = (stride * img.h) as u32;
                        (*hdr).biXPelsPerMeter = 0;
                        (*hdr).biYPelsPerMeter = 0;
                        (*hdr).biClrUsed = 0;
                        (*hdr).biClrImportant = 0;
                        for y in 0..img.h {
                            let sy = img.h - 1 - y;
                            let row = base.add(40 + (y as usize) * (stride as usize));
                            for x in 0..img.w {
                                let (b, g, r) = img.pixel(x, sy);
                                let d = row.add((x * 3) as usize);
                                *d = b; *d.add(1) = g; *d.add(2) = r;
                            }
                        }
                        win::GlobalUnlock(h);
                        if win::SetClipboardData(win::CF_DIB, h).is_null() {
                            win::GlobalFree(h);
                            win::CloseClipboard();
                            return Err(format!("SetClipboardData 图片失败 err={}", win::GetLastError()));
                        }
                        win::CloseClipboard();
                        return Ok(());
                    }
                    win::GlobalFree(h);
                }
                win::CloseClipboard();
            }
            if attempt < 7 { win::Sleep(60); }
        }
        Err("写入剪贴板失败：剪贴板被其他程序占用".into())
    }
}

#[repr(C)]
struct SystemTime {
    year: u16, month: u16, day_of_week: u16, day: u16,
    hour: u16, minute: u16, second: u16, milliseconds: u16,
}
#[link(name = "kernel32")]
extern "system" { fn GetLocalTime(t: *mut SystemTime); }

fn stamp() -> String {
    unsafe {
        let mut t: SystemTime = std::mem::zeroed();
        GetLocalTime(&mut t);
        format!("{:04}{:02}{:02}_{:02}{:02}{:02}_{:03}",
            t.year, t.month, t.day, t.hour, t.minute, t.second, t.milliseconds)
    }
}

/// Default file name offered by the save-as dialog. Same pattern `save_png`
/// writes, so a picked name is indistinguishable from a quick save.
pub fn stamp_name() -> String {
    format!("TTJiang_{}.png", stamp())
}

/// PNG via stored (uncompressed) DEFLATE blocks: no zlib dependency, and the
/// result is a fully valid PNG that every viewer and editor accepts.
pub fn save_png(img: &Image, dir: &str) -> Result<String, String> {    if let Err(e) = std::fs::create_dir_all(dir) {
        return Err(format!("创建保存目录失败：{}", e));
    }
    let name = format!("TTJiang_{}.png", stamp());
    let path = format!("{}\\{}", dir, name);
    let bytes = png_bytes(img);
    std::fs::write(&path, &bytes).map_err(|e| format!("保存 PNG 失败：{}", e))?;
    Ok(name)
}

/// Write a PNG to an explicit path. Used by `--ui-smoke`, which renders the
/// real main window off-screen so the layout can be reviewed without an
/// interactive desktop.
pub fn save_png_as(img: &Image, path: &str) -> Result<(), String> {
    let bytes = png_bytes(img);
    std::fs::write(path, &bytes).map_err(|e| format!("保存 PNG 失败：{}", e))
}

fn png_bytes(img: &Image) -> Vec<u8> {
    let w = img.w.max(0) as usize;
    let h = img.h.max(0) as usize;
    let mut raw = Vec::with_capacity(h * (1 + w * 3));
    for y in 0..h {
        raw.push(0u8); // filter type: None
        for x in 0..w {
            let (b, g, r) = img.pixel(x as i32, y as i32);
            raw.push(r); raw.push(g); raw.push(b);
        }
    }
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&(w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit truecolour RGB
    chunk(&mut out, b"IHDR", &ihdr);
    let z = zlib_stored(&raw);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

fn zlib_stored(raw: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78u8, 0x01u8];
    if raw.is_empty() {
        out.extend_from_slice(&[0x01, 0x00, 0x00, 0xFF, 0xFF]);
    } else {
        let mut i = 0usize;
        while i < raw.len() {
            let n = (raw.len() - i).min(65535);
            let last = i + n >= raw.len();
            out.push(if last { 1 } else { 0 });
            out.extend_from_slice(&(n as u16).to_le_bytes());
            out.extend_from_slice(&(!(n as u16)).to_le_bytes());
            out.extend_from_slice(&raw[i..i + n]);
            i += n;
        }
    }
    out.extend_from_slice(&adler32(raw).to_be_bytes());
    out
}

fn adler32(data: &[u8]) -> u32 {
    let mut a: u32 = 1;
    let mut b: u32 = 0;
    for &c in data {
        a = (a + c as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut body = Vec::with_capacity(4 + data.len());
    body.extend_from_slice(kind);
    body.extend_from_slice(data);
    out.extend_from_slice(&crc32(&body).to_be_bytes());
}

/// Crop in virtual-screen coordinates, clamped to the grabbed image.
pub fn crop(screen: &Image, screen_rect: Rect, vs: Rect) -> Image {
    let local = Rect::new(screen_rect.x - vs.x, screen_rect.y - vs.y, screen_rect.w, screen_rect.h);
    screen.crop(local.clamp(screen.w, screen.h))
}

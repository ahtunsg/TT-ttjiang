// BGRA image buffer plus a GDI+-backed resampler.
//
// The C# pipeline used GDI+ Graphics.DrawImage with HighQualityBilinear,
// SourceCopy and HighQuality pixel offset. Rust calls the very same flat GDI+
// entry points so the float tensors fed to det/rec are bit-for-bit identical;
// a hand-written bilinear resampler would introduce 1-2 grey levels of drift
// and is not acceptable while keeping OCR accuracy unchanged.

use std::ffi::c_void;
use std::sync::OnceLock;

use crate::types::Rect;
use crate::win;

#[derive(Clone)]
pub struct Image {
    pub w: i32,
    pub h: i32,
    /// top-down BGRA, stride = w * 4, alpha always forced to 0xFF.
    pub data: Vec<u8>,
}

impl Image {
    pub fn new(w: i32, h: i32) -> Self {
        let w = w.max(0);
        let h = h.max(0);
        Image { w, h, data: vec![0u8; (w as usize) * (h as usize) * 4] }
    }

    pub fn from_bgra(w: i32, h: i32, mut data: Vec<u8>) -> Self {
        let w = w.max(0);
        let h = h.max(0);
        let need = (w as usize) * (h as usize) * 4;
        data.resize(need, 0);
        data.truncate(need);
        // BitBlt into a 32bpp DIB does not guarantee the 4th byte. Force it,
        // otherwise the "clear" selection area composites as invisible.
        for a in data.chunks_exact_mut(4) {
            a[3] = 0xFF;
        }
        Image { w, h, data }
    }

    pub fn pixel(&self, x: i32, y: i32) -> (u8, u8, u8) {
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            return (0, 0, 0);
        }
        let p = ((y * self.w + x) * 4) as usize;
        (self.data[p], self.data[p + 1], self.data[p + 2])
    }

    /// Lanczos-3 resample whose kernel widens with the downscale factor.
    ///
    /// `scaled_bicubic` reads four source pixels per destination pixel, which
    /// is right for OCR crops (1:1 or a 2-3x upscale) but wrong for an icon:
    /// shrinking 256px to 37px would sample 4 of every ~7x7 block and throw the
    /// rest away, so curves come out ragged. Here every source pixel inside the
    /// 3*scale support contributes, which is what makes a scaled icon look
    /// smooth. Alpha is premultiplied for the resample, then restored.
    pub fn scaled_lanczos(&self, nw: i32, nh: i32) -> Option<Image> {
        if nw <= 0 || nh <= 0 || self.w <= 0 || self.h <= 0 {
            return None;
        }
        if nw == self.w && nh == self.h {
            return Some(self.clone());
        }
        let hx = lanczos_taps(self.w, nw);
        let hy = lanczos_taps(self.h, nh);

        // Straight alpha would bleed the colour of fully transparent pixels
        // into the icon rim, so weight the colour by coverage first.
        let mut src = vec![0f32; self.data.len()];
        for (i, p) in self.data.chunks_exact(4).enumerate() {
            let a = p[3] as f32 * (1.0 / 255.0);
            src[i * 4] = p[0] as f32 * a;
            src[i * 4 + 1] = p[1] as f32 * a;
            src[i * 4 + 2] = p[2] as f32 * a;
            src[i * 4 + 3] = p[3] as f32;
        }

        let mut tmp = vec![0f32; (nw as usize) * (self.h as usize) * 4];
        for y in 0..self.h as usize {
            let srow = y * self.w as usize;
            let drow = y * nw as usize;
            for (x, taps) in hx.iter().enumerate() {
                let mut acc = [0f32; 4];
                for (sx, wt) in taps {
                    let p = (srow + *sx) * 4;
                    acc[0] += src[p] * wt;
                    acc[1] += src[p + 1] * wt;
                    acc[2] += src[p + 2] * wt;
                    acc[3] += src[p + 3] * wt;
                }
                let d = (drow + x) * 4;
                tmp[d..d + 4].copy_from_slice(&acc);
            }
        }

        let mut out = vec![0u8; (nw as usize) * (nh as usize) * 4];
        for (y, taps) in hy.iter().enumerate() {
            let drow = y * nw as usize;
            for x in 0..nw as usize {
                let mut acc = [0f32; 4];
                for (sy, wt) in taps {
                    let p = (sy * nw as usize + x) * 4;
                    acc[0] += tmp[p] * wt;
                    acc[1] += tmp[p + 1] * wt;
                    acc[2] += tmp[p + 2] * wt;
                    acc[3] += tmp[p + 3] * wt;
                }
                let d = (drow + x) * 4;
                let a = acc[3].clamp(0.0, 255.0);
                // Un-premultiply: GDI+ and AlphaBlend both want straight
                // alpha, and a near-zero alpha has no colour to recover.
                let k = if a > 0.5 { 255.0 / a } else { 0.0 };
                out[d] = clamp8(acc[0] * k);
                out[d + 1] = clamp8(acc[1] * k);
                out[d + 2] = clamp8(acc[2] * k);
                out[d + 3] = clamp8(a);
            }
        }
        Some(Image { w: nw, h: nh, data: out })
    }

    /// Catmull-Rom bicubic resample, separable two-pass, edge-clamped.
    ///
    /// GDI+ is not used on purpose: its bilinear filter smears a 1px table
    /// rule into a 2-3px grey band, which is exactly the bar height DB needs
    /// to fuse two cells into one component. Catmull-Rom keeps edges tighter
    /// (mild overshoot, no plateau), so an upscaled table screenshot keeps
    /// its rules thin and its digits legible.
    pub fn scaled_bicubic(&self, nw: i32, nh: i32) -> Option<Image> {
        if nw <= 0 || nh <= 0 || self.w <= 0 || self.h <= 0 {
            return None;
        }
        if nw == self.w && nh == self.h {
            return Some(self.clone());
        }
        // Horizontal pass: (w,h) -> (nw,h). Weights are shared by every row.
        let (hx, wx) = cubic_taps(self.w, nw);
        let mut tmp = vec![0u8; (nw as usize) * (self.h as usize) * 4];
        for y in 0..self.h {
            let srow = (y * self.w) as usize * 4;
            let drow = (y * nw) as usize * 4;
            for x in 0..nw as usize {
                let mut acc = [0f32; 3];
                for t in 0..4 {
                    let sx = hx[x * 4 + t];
                    let w = wx[x * 4 + t];
                    let p = srow + sx * 4;
                    acc[0] += self.data[p] as f32 * w;
                    acc[1] += self.data[p + 1] as f32 * w;
                    acc[2] += self.data[p + 2] as f32 * w;
                }
                let d = drow + x * 4;
                tmp[d] = clamp8(acc[0]);
                tmp[d + 1] = clamp8(acc[1]);
                tmp[d + 2] = clamp8(acc[2]);
                tmp[d + 3] = 0xFF;
            }
        }
        // Vertical pass: (nw,h) -> (nw,nh).
        let (hy, wy) = cubic_taps(self.h, nh);
        let mut out = vec![0u8; (nw as usize) * (nh as usize) * 4];
        for y in 0..nh as usize {
            let drow = y * nw as usize * 4;
            for x in 0..nw as usize {
                let mut acc = [0f32; 3];
                for t in 0..4 {
                    let sy = hy[y * 4 + t];
                    let w = wy[y * 4 + t];
                    let p = (sy * nw as usize + x) * 4;
                    acc[0] += tmp[p] as f32 * w;
                    acc[1] += tmp[p + 1] as f32 * w;
                    acc[2] += tmp[p + 2] as f32 * w;
                }
                let d = drow + x * 4;
                out[d] = clamp8(acc[0]);
                out[d + 1] = clamp8(acc[1]);
                out[d + 2] = clamp8(acc[2]);
                out[d + 3] = 0xFF;
            }
        }
        Some(Image { w: nw, h: nh, data: out })
    }

    /// One separable [1,2,1]/4 pass (σ ≈ 0.5). Catmull-Rom upscaling leaves a
    /// faint sharpening halo around each stroke; DB thresholds that halo into
    /// ragged edges and drops thin components (measured 31 boxes vs 36 with a
    /// 0.4px blur on the same 1280x320 frame). This is only applied to the
    /// detector copy after an upscale, never to the recognition crops.
    pub fn blur_light(&self) -> Image {
        let w = self.w;
        let h = self.h;
        if w < 3 || h < 3 {
            return self.clone();
        }
        let mut tmp = vec![0u8; self.data.len()];
        // Horizontal pass.
        for y in 0..h as usize {
            let row = y * w as usize * 4;
            for x in 0..w as usize {
                let l = if x > 0 { row + (x - 1) * 4 } else { row };
                let c = row + x * 4;
                let r = if x + 1 < w as usize { row + (x + 1) * 4 } else { c };
                for ch in 0..3 {
                    tmp[c + ch] = ((self.data[l + ch] as u16
                        + 2 * self.data[c + ch] as u16
                        + self.data[r + ch] as u16)
                        >> 2) as u8;
                }
                tmp[c + 3] = 0xFF;
            }
        }
        // Vertical pass.
        let mut out = vec![0u8; self.data.len()];
        for y in 0..h as usize {
            for x in 0..w as usize {
                let c = (y * w as usize + x) * 4;
                let u = if y > 0 { c - w as usize * 4 } else { c };
                let d = if y + 1 < h as usize { c + w as usize * 4 } else { c };
                for ch in 0..3 {
                    out[c + ch] = ((tmp[u + ch] as u16 + 2 * tmp[c + ch] as u16 + tmp[d + ch] as u16)
                        >> 2) as u8;
                }
                out[c + 3] = 0xFF;
            }
        }
        Image { w, h, data: out }
    }

    /// Clamped crop; out-of-bounds pixels become opaque black.
    pub fn crop(&self, r: Rect) -> Image {
        let rw = r.w.max(0);
        let rh = r.h.max(0);
        let mut out = Image::new(rw, rh);
        for y in 0..rh {
            let sy = r.y + y;
            if sy < 0 || sy >= self.h {
                continue;
            }
            for x in 0..rw {
                let sx = r.x + x;
                if sx < 0 || sx >= self.w {
                    continue;
                }
                let s = ((sy * self.w + sx) * 4) as usize;
                let d = ((y * rw + x) * 4) as usize;
                out.data[d..d + 4].copy_from_slice(&self.data[s..s + 4]);
            }
        }
        for a in out.data.chunks_exact_mut(4) {
            a[3] = 0xFF;
        }
        out
    }

    pub fn force_opaque(&mut self) {
        for a in self.data.chunks_exact_mut(4) {
            a[3] = 0xFF;
        }
    }

    /// Load a PNG/JPG/BMP file through GDI+ for `--ocr-check <image>`.
    /// Kept in this module because it shares the same GDI+ handle and lock path.
    pub fn load(path: &str) -> Result<Image, String> {
        let g = Gdi::get().ok_or_else(|| "GDI+ 不可用".to_string())?;
        let load = g.load_file.ok_or_else(|| "GdipLoadImageFromFile 不可用".to_string())?;
        let get_w = g.get_width.ok_or_else(|| "GdipGetImageWidth 不可用".to_string())?;
        let get_h = g.get_height.ok_or_else(|| "GdipGetImageHeight 不可用".to_string())?;
        let p: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            let mut img: *mut c_void = std::ptr::null_mut();
            if load(p.as_ptr(), &mut img) != 0 || img.is_null() {
                return Err(format!("无法打开图片 {}", path));
            }
            let mut w: u32 = 0;
            let mut h: u32 = 0;
            get_w(img, &mut w);
            get_h(img, &mut h);
            let w = w as i32;
            let h = h as i32;
            if w <= 0 || h <= 0 {
                (g.dispose_image)(img);
                return Err("图片尺寸无效".into());
            }
            let rc = GpRectI { x: 0, y: 0, w, h };
            let mut bd = BitmapData::zero();
            let status = (g.lock_bits)(img, &rc, IMAGE_LOCK_READ, PIXEL_FORMAT_32BPP_ARGB, &mut bd);
            if status != 0 || bd.scan0.is_null() {
                (g.dispose_image)(img);
                return Err(format!("GdipBitmapLockBits 失败 status={}", status));
            }
            let len = (w as usize) * (h as usize) * 4;
            let mut data = vec![0u8; len];
            let stride = bd.stride as isize;
            for y in 0..h {
                let src = bd.scan0.offset(stride * y as isize);
                let dst = data.as_mut_ptr().add((y * w * 4) as usize);
                std::ptr::copy_nonoverlapping(src, dst, (w * 4) as usize);
            }
            (g.unlock_bits)(img, &mut bd);
            (g.dispose_image)(img);
            Ok(Image::from_bgra(w, h, data))
        }
    }

    /// Fraction of sampled pixels that differ from the top-left pixel.
    /// `--capture-smoke` uses it to distinguish a real desktop from a session
    /// where the display DC exists but cannot be read (locked/RDP/headless).
    pub fn sampled_variance(&self) -> f32 {
        let total = (self.w.max(0) as usize) * (self.h.max(0) as usize);
        if total == 0 || self.data.len() < 4 {
            return 0.0;
        }
        let base = &self.data[0..3];
        let step = (total / 4096).max(1);
        let mut hits = 0usize;
        let mut seen = 0usize;
        for i in (0..total).step_by(step) {
            let p = i * 4;
            if p + 2 >= self.data.len() { break; }
            seen += 1;
            if self.data[p] != base[0]
                || self.data[p + 1] != base[1]
                || self.data[p + 2] != base[2]
            {
                hits += 1;
            }
        }
        if seen == 0 { 0.0 } else { hits as f32 / seen as f32 }
    }

    /// Grab the whole virtual screen.
    ///
    /// Primary path is exactly the C# 3.3 pipeline (GDI+ 32bppArgb bitmap ->
    /// Graphics.GetHdc -> BitBlt from GetDC(NULL)). Some machines refuse GDI
    /// reads against a GDI+ DC (RDP / mirror drivers / non-interactive
    /// desktops) while a plain memory DC still works, so a pure-GDI DIB path
    /// is tried on failure. A total failure stays non-fatal, matching C# which
    /// ignores BitBlt's return value.
    pub fn grab_screen(vs: Rect) -> Result<Image, String> {
        let (img, ok) = Self::grab_screen_gdiplus(vs)?;
        if ok {
            return Ok(img);
        }
        win::log("GDI+ 路径 BitBlt 失败，尝试纯 GDI DIB 回退");
        match Self::grab_screen_gdi(vs) {
            Ok(fallback) => Ok(fallback),
            Err(e) => {
                win::log(&format!("GDI 回退失败: {}；沿用主路径画面", e));
                Ok(img)
            }
        }
    }

    /// C# Capture.GrabVirtualScreen, 1:1. Returns (image, bitblt_succeeded).
    fn grab_screen_gdiplus(vs: Rect) -> Result<(Image, bool), String> {
        let g = Gdi::get().ok_or_else(|| "GDI+ 不可用".to_string())?;
        let get_dc = g.get_dc.ok_or_else(|| "GdipGetDC 不可用".to_string())?;
        let release_dc = g.release_dc.ok_or_else(|| "GdipReleaseDC 不可用".to_string())?;
        unsafe {
            let screen_dc = win::GetDC(std::ptr::null_mut());
            if screen_dc.is_null() { return Err("GetDC(NULL) 失败".into()); }
            let mut bmp: *mut c_void = std::ptr::null_mut();
            if (g.create_bmp)(vs.w, vs.h, 0, PIXEL_FORMAT_32BPP_ARGB,
                std::ptr::null_mut(), &mut bmp) != 0 || bmp.is_null() {
                win::ReleaseDC(std::ptr::null_mut(), screen_dc);
                return Err("GdipCreateBitmapFromScan0 失败".into());
            }
            let mut gr: *mut c_void = std::ptr::null_mut();
            if (g.get_graphics)(bmp, &mut gr) != 0 {
                (g.dispose_image)(bmp);
                win::ReleaseDC(std::ptr::null_mut(), screen_dc);
                return Err("GdipGetImageGraphicsContext 失败".into());
            }
            let mut hdc: *mut c_void = std::ptr::null_mut();
            if get_dc(gr, &mut hdc) != 0 || hdc.is_null() {
                (g.delete_graphics)(gr);
                (g.dispose_image)(bmp);
                win::ReleaseDC(std::ptr::null_mut(), screen_dc);
                return Err("GdipGetDC 失败".into());
            }
            let mut ok = win::BitBlt(hdc, 0, 0, vs.w, vs.h, screen_dc, vs.x, vs.y, win::SRCCOPY);
            let mut err = win::GetLastError();
            if ok == 0 {
                // CAPTUREBLT includes layered/DWM-composited windows; some
                // drivers only accept this variant. Same destination, same
                // pixels, so it cannot change OCR output.
                ok = win::BitBlt(
                    hdc, 0, 0, vs.w, vs.h, screen_dc, vs.x, vs.y,
                    win::SRCCOPY | win::CAPTUREBLT,
                );
                err = win::GetLastError();
            }
            release_dc(gr, hdc);
            (g.delete_graphics)(gr);
            win::ReleaseDC(std::ptr::null_mut(), screen_dc);
            if ok == 0 {
                win::log(&format!(
                    "BitBlt 失败 err={} size={}x{} origin=({},{})；显示 DC 可能不可读",
                    err, vs.w, vs.h, vs.x, vs.y
                ));
            }
            let rc = GpRectI { x: 0, y: 0, w: vs.w, h: vs.h };
            let mut bd = BitmapData::zero();
            let status = (g.lock_bits)(bmp, &rc, IMAGE_LOCK_READ, PIXEL_FORMAT_32BPP_ARGB, &mut bd);
            if status != 0 || bd.scan0.is_null() {
                (g.dispose_image)(bmp);
                return Err(format!("GdipBitmapLockBits 失败 status={}", status));
            }
            let mut data = vec![0u8; (vs.w as usize) * (vs.h as usize) * 4];
            let stride = bd.stride as isize;
            let row_bytes = (vs.w * 4) as usize;
            for y in 0..vs.h {
                let src = bd.scan0.offset(stride * y as isize);
                let dst = data.as_mut_ptr().add((y as usize) * row_bytes);
                std::ptr::copy_nonoverlapping(src, dst, row_bytes);
            }
            (g.unlock_bits)(bmp, &mut bd);
            (g.dispose_image)(bmp);
            Ok((Image::from_bgra(vs.w, vs.h, data), ok != 0))
        }
    }

    /// Pure-GDI fallback: memory DC + 32bpp top-down DIB section.
    fn grab_screen_gdi(vs: Rect) -> Result<Image, String> {
        unsafe {
            let screen_dc = win::GetDC(std::ptr::null_mut());
            if screen_dc.is_null() { return Err("GetDC(NULL) 失败".into()); }
            let mem = win::CreateCompatibleDC(screen_dc);
            if mem.is_null() {
                win::ReleaseDC(std::ptr::null_mut(), screen_dc);
                return Err("CreateCompatibleDC 失败".into());
            }
            let bi = win::BITMAPINFOHEADER {
                biSize: std::mem::size_of::<win::BITMAPINFOHEADER>() as u32,
                biWidth: vs.w,
                biHeight: -vs.h, // top-down, matches our BGRA row order
                biPlanes: 1,
                biBitCount: 32,
                biCompression: 0, // BI_RGB
                biSizeImage: 0,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            };
            let mut bits: *mut c_void = std::ptr::null_mut();
            let hbm = win::CreateDIBSection(mem, &bi, 0, &mut bits, std::ptr::null_mut(), 0);
            if hbm.is_null() || bits.is_null() {
                win::DeleteDC(mem);
                win::ReleaseDC(std::ptr::null_mut(), screen_dc);
                return Err("CreateDIBSection 失败".into());
            }
            let old = win::SelectObject(mem, hbm);
            let ok = win::BitBlt(mem, 0, 0, vs.w, vs.h, screen_dc, vs.x, vs.y, win::SRCCOPY);
            let err = win::GetLastError();
            let mut data = vec![0u8; (vs.w as usize) * (vs.h as usize) * 4];
            if ok != 0 {
                std::ptr::copy_nonoverlapping(bits as *const u8, data.as_mut_ptr(), data.len());
            }
            win::SelectObject(mem, old);
            win::DeleteObject(hbm);
            win::DeleteDC(mem);
            win::ReleaseDC(std::ptr::null_mut(), screen_dc);
            if ok == 0 { return Err(format!("BitBlt 失败 err={}", err)); }
            Ok(Image::from_bgra(vs.w, vs.h, data))
        }
    }

    /// Same as PaddleOcr.ToChw: GDI+ HighQualityBilinear -> CHW f32 with
    /// per-channel (v - mean) / std, channel order R,G,B.
    pub fn to_chw(&self, sw: i32, sh: i32, mean: &[f32; 3], std: &[f32; 3]) -> Vec<f32> {
        let plane = (sw.max(0) as usize) * (sh.max(0) as usize);
        let mut out = vec![0f32; plane * 3];
        let g = match Gdi::get() {
            Some(g) => g,
            None => return out,
        };
        unsafe {
            // Source bitmap wraps our own buffer; GDI+ does not own the memory.
            let mut src: *mut c_void = std::ptr::null_mut();
            let mut dst: *mut c_void = std::ptr::null_mut();
            let ok = (g.create_bmp)(
                self.w, self.h, self.w * 4, PIXEL_FORMAT_32BPP_ARGB,
                self.data.as_ptr() as *mut u8, &mut src,
            ) == 0 && (g.create_bmp)(
                sw, sh, 0, PIXEL_FORMAT_32BPP_ARGB, std::ptr::null_mut(), &mut dst,
            ) == 0;
            if !ok {
                if !src.is_null() { (g.dispose_image)(src); }
                if !dst.is_null() { (g.dispose_image)(dst); }
                return out;
            }
            let mut gr: *mut c_void = std::ptr::null_mut();
            if (g.get_graphics)(dst, &mut gr) != 0 {
                (g.dispose_image)(src);
                (g.dispose_image)(dst);
                return out;
            }
            (g.set_interp)(gr, INTERP_HIGH_QUALITY_BILINEAR);
            (g.set_pixel_offset)(gr, PIXEL_OFFSET_HIGH_QUALITY);
            (g.set_compositing)(gr, COMPOSITING_SOURCE_COPY);
            (g.draw_rect_rect)(
                gr, src,
                0.0, 0.0, sw as f32, sh as f32,
                0.0, 0.0, self.w as f32, self.h as f32,
                UNIT_PIXEL,
                std::ptr::null(), std::ptr::null(), std::ptr::null(),
            );
            (g.delete_graphics)(gr);

            let rc = GpRectI { x: 0, y: 0, w: sw, h: sh };
            let mut bd = BitmapData::zero();
            if (g.lock_bits)(dst, &rc, IMAGE_LOCK_READ, PIXEL_FORMAT_32BPP_ARGB, &mut bd) == 0 {
                let stride = bd.stride as isize;
                let base = bd.scan0;
                // Bounds-checked Vec indexing costs ~40 ms on a 1280x1280
                // det tensor. Raw pointers keep this loop at C#/unsafe speed
                // while producing exactly the same float values.
                let dst_ptr = out.as_mut_ptr();
                for y in 0..sh {
                    let row = base.offset(stride * y as isize);
                    let row_base = (y as usize) * (sw as usize);
                    for x in 0..sw {
                        let p = (x as usize) * 4;
                        let b = *row.add(p) as f32 / 255.0;
                        let gg = *row.add(p + 1) as f32 / 255.0;
                        let r = *row.add(p + 2) as f32 / 255.0;
                        let i = row_base + x as usize;
                        // Keep C#'s exact `(v - mean) / std` expression; a
                        // reciprocal multiply is 1 ULP off and can move a
                        // borderline detection score.
                        *dst_ptr.add(i) = (r - mean[0]) / std[0];
                        *dst_ptr.add(plane + i) = (gg - mean[1]) / std[1];
                        *dst_ptr.add(2 * plane + i) = (b - mean[2]) / std[2];
                    }
                }
                (g.unlock_bits)(dst, &mut bd);
            }
            (g.dispose_image)(src);
            (g.dispose_image)(dst);
        }
        out
    }

    /// Resample an arbitrary GDI+ source bitmap into `w x h` BGRA, keeping the
    /// alpha channel. `to_chw` deliberately forces opaque pixels because OCR
    /// only sees the screen; UI assets must keep alpha so the logo and the
    /// shortcut icons do not paste a black box onto the panel.
    fn resample(source: *mut c_void, w: i32, h: i32) -> Option<Image> {
        if w <= 0 || h <= 0 { return None; }
        let g = Gdi::get()?;
        let get_w = g.get_width?;
        let get_h = g.get_height?;
        unsafe {
            let mut sw: u32 = 0;
            let mut sh: u32 = 0;
            get_w(source, &mut sw);
            get_h(source, &mut sh);
            if sw == 0 || sh == 0 { return None; }
            let mut dst: *mut c_void = std::ptr::null_mut();
            if (g.create_bmp)(w, h, 0, PIXEL_FORMAT_32BPP_ARGB,
                std::ptr::null_mut(), &mut dst) != 0 || dst.is_null() {
                return None;
            }
            let mut gr: *mut c_void = std::ptr::null_mut();
            if (g.get_graphics)(dst, &mut gr) != 0 {
                (g.dispose_image)(dst);
                return None;
            }
            (g.set_interp)(gr, INTERP_HIGH_QUALITY_BILINEAR);
            (g.set_pixel_offset)(gr, PIXEL_OFFSET_HIGH_QUALITY);
            // SourceCopy keeps the source alpha; SourceOver would composite
            // the transparent margin onto premultiplied black.
            (g.set_compositing)(gr, COMPOSITING_SOURCE_COPY);
            // Source rect must carry the real pixel size: passing 0 there
            // draws nothing at all, which is why the logo came out blank.
            (g.draw_rect_rect)(
                gr, source,
                0.0, 0.0, w as f32, h as f32,
                0.0, 0.0, sw as f32, sh as f32,
                UNIT_PIXEL, std::ptr::null(), std::ptr::null(), std::ptr::null(),
            );
            (g.delete_graphics)(gr);

            let rc = GpRectI { x: 0, y: 0, w, h };
            let mut bd = BitmapData::zero();
            let mut out = None;
            if (g.lock_bits)(dst, &rc, IMAGE_LOCK_READ, PIXEL_FORMAT_32BPP_ARGB, &mut bd) == 0 {
                let mut data = vec![0u8; (w as usize) * (h as usize) * 4];
                let stride = bd.stride as isize;
                for y in 0..h {
                    let src = bd.scan0.offset(stride * y as isize);
                    let d = data.as_mut_ptr().add((y as usize) * (w as usize) * 4);
                    std::ptr::copy_nonoverlapping(src, d, (w * 4) as usize);
                }
                (g.unlock_bits)(dst, &mut bd);
                out = Some(Image { w, h, data });
            }
            (g.dispose_image)(dst);
            out
        }
    }

    /// Load a PNG/ICO-style file through GDI+ and scale it to `w x h`.
    pub fn load_scaled(path: &str, w: i32, h: i32) -> Option<Image> {
        let g = Gdi::get()?;
        let load = g.load_file?;
        let p: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            let mut img: *mut c_void = std::ptr::null_mut();
            if load(p.as_ptr(), &mut img) != 0 || img.is_null() {
                return None;
            }
            let out = Self::resample(img, w, h);
            (g.dispose_image)(img);
            out
        }
    }

    /// Copy a GDI+ bitmap out at its native size, keeping alpha.
    unsafe fn read_bitmap(g: &Gdi, img: *mut c_void) -> Option<Image> {
        let get_w = g.get_width?;
        let get_h = g.get_height?;
        let mut w: u32 = 0;
        let mut h: u32 = 0;
        get_w(img, &mut w);
        get_h(img, &mut h);
        let (w, h) = (w as i32, h as i32);
        if w <= 0 || h <= 0 {
            return None;
        }
        let rc = GpRectI { x: 0, y: 0, w, h };
        let mut bd = BitmapData::zero();
        if (g.lock_bits)(img, &rc, IMAGE_LOCK_READ, PIXEL_FORMAT_32BPP_ARGB, &mut bd) != 0
            || bd.scan0.is_null()
        {
            return None;
        }
        let mut data = vec![0u8; (w as usize) * (h as usize) * 4];
        let stride = bd.stride as isize;
        for y in 0..h {
            let src = bd.scan0.offset(stride * y as isize);
            let d = data.as_mut_ptr().add((y as usize) * (w as usize) * 4);
            std::ptr::copy_nonoverlapping(src, d, (w * 4) as usize);
        }
        (g.unlock_bits)(img, &mut bd);
        Some(Image { w, h, data })
    }

    /// Convert a shell `HICON` into a scaled BGRA image.
    ///
    /// The icon is read at the size the resource actually carries and only then
    /// shrunk by our own filter. Letting `PrivateExtractIcons` do the scaling
    /// (asking it for the draw size directly) stretches the nearest stored
    /// frame with a cheap filter, which is what made the tiles look jagged.
    pub fn from_hicon(ico: win::HICON, w: i32, h: i32) -> Option<Image> {
        if ico.is_null() { return None; }
        let g = Gdi::get()?;
        let make = g.from_hicon?;
        unsafe {
            let mut img: *mut c_void = std::ptr::null_mut();
            if make(ico, &mut img) != 0 || img.is_null() {
                return None;
            }
            let native = Self::read_bitmap(g, img);
            let native = match native {
                Some(n) => n,
                // A bitmap GDI+ refuses to lock still has to come out at the
                // right size, so keep the old GDI+ path as a fallback.
                None => {
                    let out = Self::resample(img, w, h);
                    (g.dispose_image)(img);
                    return out;
                }
            };
            (g.dispose_image)(img);
            native.scaled_lanczos(w, h)
        }
    }

    /// AlphaBlend this image onto `dc` at (x, y).
    ///
    /// GDI's AC_SRC_ALPHA contract wants premultiplied colour, while GDI+
    /// hands back straight alpha, so the multiply happens while copying.
    pub fn blend_onto(&self, dc: win::HDC, x: i32, y: i32) {
        if self.w <= 0 || self.h <= 0 || dc.is_null() { return; }
        unsafe {
            let mem = win::CreateCompatibleDC(dc);
            if mem.is_null() { return; }
            let bi = win::BITMAPINFOHEADER {
                biSize: std::mem::size_of::<win::BITMAPINFOHEADER>() as u32,
                biWidth: self.w,
                biHeight: -self.h, // top-down, same row order as our buffer
                biPlanes: 1,
                biBitCount: 32,
                biCompression: 0,
                biSizeImage: 0,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            };
            let mut bits: *mut c_void = std::ptr::null_mut();
            let hbm = win::CreateDIBSection(mem, &bi, 0, &mut bits, std::ptr::null_mut(), 0);
            if hbm.is_null() || bits.is_null() {
                win::DeleteDC(mem);
                return;
            }
            let dst = std::slice::from_raw_parts_mut(bits as *mut u8, self.data.len());
            for (s, d) in self.data.chunks_exact(4).zip(dst.chunks_exact_mut(4)) {
                let a = s[3] as u32;
                d[0] = ((s[0] as u32 * a + 127) / 255) as u8;
                d[1] = ((s[1] as u32 * a + 127) / 255) as u8;
                d[2] = ((s[2] as u32 * a + 127) / 255) as u8;
                d[3] = s[3];
            }
            let old = win::SelectObject(mem, hbm);
            let bf = win::BLENDFUNCTION {
                BlendOp: 0,          // AC_SRC_OVER
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: 1,      // AC_SRC_ALPHA
            };
            win::AlphaBlend(dc, x, y, self.w, self.h, mem, 0, 0, self.w, self.h, bf);
            win::SelectObject(mem, old);
            win::DeleteObject(hbm);
            win::DeleteDC(mem);
        }
    }
}

// ---- shortcut icons ------------------------------------------------------

use std::collections::HashMap;
use std::sync::Mutex;

static ICON_CACHE: OnceLock<Mutex<HashMap<String, Option<Image>>>> = OnceLock::new();

/// Turn a config `path` into something the shell can read.
///
/// Mirrors IconLoader.Resolve: an existing file/dir wins; a bare command name
/// (notepad.exe, calc.exe) is searched once on PATH, because the shortcut list
/// stores commands rather than absolute paths.
fn resolve_path(raw: &str) -> Option<String> {
    let p = raw.trim().trim_matches('"');
    if p.is_empty() {
        return None;
    }
    if std::path::Path::new(p).exists() {
        return Some(p.to_string());
    }
    if !p.contains('\\') && !p.contains('/') {
        let path_var = std::env::var("PATH").unwrap_or_default();
        for dir in path_var.split(';') {
            let dir = dir.trim().trim_matches('"');
            if dir.is_empty() {
                continue;
            }
            for cand in [format!("{}\\{}", dir, p), format!("{}\\{}.exe", dir, p)] {
                if std::path::Path::new(&cand).exists() {
                    return Some(cand);
                }
            }
        }
    }
    None
}

/// Shortcut icon at the requested size, cached per config path.
///
/// A `None` result is cached too: a missing target must not trigger a PATH
/// sweep on every repaint.
/// Per-destination-pixel 4-tap source indices and Catmull-Rom weights.
/// Packed as 4 entries per destination pixel to keep the inner loop flat.
fn cubic_taps(src: i32, dst: i32) -> (Vec<usize>, Vec<f32>) {
    let mut idx = vec![0usize; dst as usize * 4];
    let mut wts = vec![0f32; dst as usize * 4];
    let scale = src as f32 / dst as f32;
    let last = (src - 1).max(0) as usize;
    for i in 0..dst as usize {
        let center = (i as f32 + 0.5) * scale - 0.5;
        let base = center.floor();
        let f = center - base;
        // Catmull-Rom (a = -0.5) over taps base-1 .. base+2.
        let w0 = -0.5 * f * f * f + f * f - 0.5 * f;
        let w1 = 1.5 * f * f * f - 2.5 * f * f + 1.0;
        let w2 = -1.5 * f * f * f + 2.0 * f * f + 0.5 * f;
        let w3 = 0.5 * f * f * f - 0.5 * f * f;
        let b = base as i64;
        let taps = [b - 1, b, b + 1, b + 2];
        let ws = [w0, w1, w2, w3];
        for t in 0..4 {
            let s = taps[t].clamp(0, last as i64) as usize;
            idx[i * 4 + t] = s;
            wts[i * 4 + t] = ws[t];
        }
    }
    (idx, wts)
}

/// Per-destination-pixel source indices and Lanczos-3 weights, with the
/// support growing by the downscale factor so nothing is skipped.
fn lanczos_taps(src: i32, dst: i32) -> Vec<Vec<(usize, f32)>> {
    let scale = (src as f32 / dst as f32).max(1.0);
    let support = 3.0 * scale;
    let last = (src - 1).max(0);
    let mut out = Vec::with_capacity(dst as usize);
    for i in 0..dst {
        let center = (i as f32 + 0.5) * scale - 0.5;
        let lo = (center - support).floor() as i32;
        let hi = (center + support).ceil() as i32;
        let mut taps: Vec<(usize, f32)> = Vec::with_capacity((hi - lo + 1) as usize);
        let mut sum = 0f32;
        for j in lo..=hi {
            let w = lanczos3((j as f32 - center) / scale);
            if w == 0.0 {
                continue;
            }
            sum += w;
            taps.push((j.clamp(0, last) as usize, w));
        }
        // Weights must sum to 1 or a flat icon plate would shift in tone.
        if sum.abs() > 1e-6 {
            for t in taps.iter_mut() {
                t.1 /= sum;
            }
        }
        out.push(taps);
    }
    out
}

fn lanczos3(x: f32) -> f32 {
    let x = x.abs();
    if x < 1e-6 {
        return 1.0;
    }
    if x >= 3.0 {
        return 0.0;
    }
    let px = std::f32::consts::PI * x;
    3.0 * px.sin() * (px / 3.0).sin() / (px * px)
}

#[inline]
fn clamp8(v: f32) -> u8 {
    if v <= 0.0 {
        0
    } else if v >= 255.0 {
        255
    } else {
        (v + 0.5) as u8
    }
}

pub fn icon_for_path(raw: &str, size: i32) -> Option<Image> {
    // The size is part of the key: the icon box follows the panel, which follows
    // the monitor DPI, and a bitmap scaled for the old size would come back
    // stale after the window moves to a display with a different scale.
    cached_icon(format!("target\u{1}{}\u{1}{}", size, raw),
        || load_icon_uncached(raw, size))
}

/// Load through the shared cache, or decode and remember.
///
/// Every tile paints on each repaint, and the panel repaints in full on each
/// hover change, so an uncached icon means decoding the same file dozens of
/// times a second.
fn cached_icon(key: String, load: impl FnOnce() -> Option<Image>) -> Option<Image> {
    let cache = ICON_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(m) = cache.lock() {
        if let Some(hit) = m.get(&key) {
            return hit.clone();
        }
    }
    let img = load();
    if let Ok(mut m) = cache.lock() {
        m.insert(key, img.clone());
    }
    img
}

/// mtime and length of a file, folded into one key component.
///
/// The user replaces a picture file to change a tile, so the cache key has to
/// notice. A stat costs microseconds against a decode.
fn file_stamp(path: &str) -> u128 {
    let Ok(m) = std::fs::metadata(path) else { return 0 };
    let nanos = m.modified().ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    nanos ^ ((m.len() as u128) << 64)
}

/// Takes the first group icon the file offers, whether its name is an integer
/// id or a string. The name pointer is only valid inside the callback, so the
/// frame list is read here rather than back in the caller.
unsafe extern "system" fn take_first_group(
    m: win::HMODULE, _kind: *const u16, name: *const u16, lparam: isize,
) -> i32 {
    let out = &mut *(lparam as *mut Option<Vec<i32>>);
    if out.is_some() {
        return 0; // found one, stop enumerating
    }
    if let Some(sizes) = group_frames(m, name as usize) {
        *out = Some(sizes);
    }
    1
}

/// Read one group icon's frame list; `None` when the id has no readable data.
unsafe fn group_frames(hmod: win::HMODULE, id: usize) -> Option<Vec<i32>> {
    let rsrc = win::FindResourceW(hmod, id as *const u16, win::RT_GROUP_ICON as *const u16);
    if rsrc.is_null() {
        return None;
    }
    let data = win::LoadResource(hmod, rsrc);
    if data.is_null() {
        return None;
    }
    let p = win::LockResource(data) as *const u8;
    if p.is_null() {
        return None;
    }
    let count = u16::from_le_bytes([*p.add(4), *p.add(5)]) as usize;
    let mut sizes = Vec::with_capacity(count);
    for i in 0..count {
        // GRPICONDIRENTRY: w, h, colors, reserved, planes, bitcount, bytes, id
        // — 14 bytes each, a width of 0 means 256.
        let e = p.add(6 + i * 14);
        sizes.push(if *e == 0 { 256 } else { *e as i32 });
    }
    sizes.sort_unstable();
    sizes.dedup();
    Some(sizes)
}

/// Frame sizes actually present in the file's first `RT_GROUP_ICON`.
///
/// An empty result means "not a PE file" (a .lnk, a folder, a document) — the
/// caller then falls back to the shell. The group has to be enumerated rather
/// than probed at ids 1..n: calc.exe and cmd.exe store their icon under an id
/// far outside that range, which is why probing silently returned nothing and
/// the loader fell back to a stretched frame.
struct IconFrames {
    opened: bool,
    open_err: u32,
    groups: usize,
    sizes: Vec<i32>,
}

unsafe fn native_icon_sizes(path: &str) -> IconFrames {
    let w = win::wide(path);
    let mut hmod = win::LoadLibraryExW(w.as_ptr(), std::ptr::null_mut(),
        win::LOAD_LIBRARY_AS_DATAFILE);
    if hmod.is_null() {
        hmod = win::LoadLibraryExW(w.as_ptr(), std::ptr::null_mut(),
            win::LOAD_LIBRARY_AS_DATAFILE | 0x20); // AS_IMAGE_RESOURCE
    }
    if hmod.is_null() {
        return IconFrames { opened: false, open_err: win::GetLastError(), groups: 0, sizes: Vec::new() };
    }
    let mut found: Option<Vec<i32>> = None;
    win::EnumResourceNamesW(hmod, win::RT_GROUP_ICON as *const u16,
        Some(take_first_group), &mut found as *mut Option<Vec<i32>> as isize);
    win::FreeLibrary(hmod);
    IconFrames { opened: true, open_err: 0, groups: usize::from(found.is_some()), sizes: found.unwrap_or_default() }
}

/// Size to ask for: the smallest frame that is at least the draw size, so the
/// only scaling left is our own shrink. Falls back to the largest frame when
/// every frame is smaller than the tile.
fn source_size_for(path: &str, size: i32) -> Option<i32> {
    // Debug override for `--icon-dump`: force a frame size to compare the
    // shrink quality of, say, the 48px art against the 256px art.
    if let Ok(v) = std::env::var("TTJIANG_ICON_SRC") {
        if let Ok(px) = v.parse::<i32>() { return Some(px); }
    }
    let sizes = unsafe { native_icon_sizes(path) }.sizes;
    if sizes.is_empty() {
        return None;
    }
    Some(
        sizes.iter().copied().filter(|px| *px >= size).min()
            .unwrap_or_else(|| *sizes.last().unwrap()),
    )
}

/// Requested source size when pulling an icon out of a file.
///
/// Not the draw size: Windows scales a stored frame up to whatever `cx/cy` asks
/// for, with a filter that leaves stair-stepped edges, so asking for 37px is
/// what made the tiles look unsmooth. 256 is where Win10+ icons usually top
/// out; our own Lanczos pass then does the shrink.
const ICON_SOURCE_PX: [i32; 3] = [48, 256, 32];

/// Diagnostic for `--icon-dump`: which frames the file carries and which one
/// the loader would use for a tile of `size`.
pub fn icon_source_report(raw: &str, size: i32) -> String {
    match resolve_path(raw) {
        Some(p) => {
            let f = unsafe { native_icon_sizes(&p) };
            format!("opened={} err={} groups={} frames={:?} chosen={:?} path={}",
                f.opened, f.open_err, f.groups, f.sizes, source_size_for(&p, size), p)
        }
        None => format!("unresolved path={}", raw),
    }
}

/// Use a picture file as a tile icon, bypassing the target's own icon.
///
/// This is how a web link gets a real site logo: the launcher cannot read one
/// out of a browser's cache (see `profile::md5_hex`), so the user points the
/// tile's `icon` at a `.png`/`.ico` they saved themselves.
pub fn icon_from_image_file(path: &str, size: i32) -> Option<Image> {
    cached_icon(format!("file\u{1}{}\u{1}{}\u{1}{}", size, file_stamp(path), path),
        || load_custom_icon(path, size))
}

/// Decode a user-picked picture.
///
/// An `.ico` holds several frames and GDI+ returns whichever one it prefers,
/// usually the smallest, so those go through the same frame picker the exe
/// icons use. Bitmaps and photos go through GDI+, which reads them at their
/// stored size before our own downscale.
fn load_custom_icon(path: &str, size: i32) -> Option<Image> {
    if path.to_ascii_lowercase().ends_with(".ico") {
        if let Some(img) = extract_icon_frames(path, size) {
            return Some(img);
        }
    }
    load_file_icon(path, size)
}

/// Load an image file (`.png`/`.jpg`/`.ico`) as a tile icon. GDI+'s
/// `GdipCreateBitmapFromFile` takes a wide path, so non-ASCII names are fine.
fn load_file_icon(path: &str, size: i32) -> Option<Image> {
    let g = Gdi::get()?;
    let load = g.load_file?;
    let w = win::wide(path);
    unsafe {
        let mut bmp: *mut c_void = std::ptr::null_mut();
        if (load)(w.as_ptr(), &mut bmp) != 0 || bmp.is_null() {
            return None;
        }
        let out = Image::read_bitmap(g, bmp)
            .and_then(|native| native.scaled_lanczos(size, size));
        (g.dispose_image)(bmp);
        out
    }
}

fn load_icon_uncached(raw: &str, size: i32) -> Option<Image> {
    // A link has no icon on disk to extract, and pulling one from a browser
    // cache is off the table. Returning None here is what lets the tile draw
    // the domain's initial letter instead of an unrelated system icon.
    if crate::profile::target_of(raw) == crate::profile::Target::Web {
        return None;
    }
    let path = match resolve_path(raw) {
        Some(p) => p,
        None => return None,
    };
    if let Some(img) = extract_icon_frames(&path, size) {
        return Some(img);
    }
    let w = win::wide(&path);
    unsafe {
        // Fall back to the shell's large icon (folders, documents, lnk).
        let mut sfi = win::SHFILEINFO::default();
        let attr = if std::path::Path::new(&path).is_dir() { 0x10 } else { 0x80 };
        let ok = win::SHGetFileInfoW(
            w.as_ptr(), attr, &mut sfi,
            std::mem::size_of::<win::SHFILEINFO>() as u32, 0x100,
        );
        let hicon = if ok != 0 { sfi.hIcon } else { std::ptr::null_mut() };
        if hicon.is_null() {
            return None;
        }
        let img = Image::from_hicon(hicon, size, size);
        win::DestroyIcon(hicon);
        img
    }
}

/// Pull a frame of at least `size` out of a file's icon resources, giving up
/// when the file has none (a .lnk, a document, a folder).
///
/// Shared by the exe loader and the user-picked `.ico` path: an .ico holds
/// several frames and GDI+ hands back whichever one it likes, so asking for the
/// frame we need is the only way that path stays sharp.
fn extract_icon_frames(path: &str, size: i32) -> Option<Image> {
    let w = win::wide(path);
    // Frames the file really has beat any guess: asking for a size it does not
    // carry returns a stretched copy, which is exactly how cmd.exe ended up as
    // a blurry black box. The guesses stay as a fallback for .ico files, whose
    // resources this reader cannot see.
    let mut wanted: Vec<i32> = source_size_for(path, size).into_iter().collect();
    for px in ICON_SOURCE_PX {
        if !wanted.contains(&px) { wanted.push(px); }
    }
    unsafe {
        for src_px in wanted {
            let mut hicon: win::HICON = std::ptr::null_mut();
            let mut id: u32 = 0;
            let got = win::PrivateExtractIconsW(
                w.as_ptr(), 0, src_px, src_px,
                &mut hicon, &mut id, 1, 0,
            );
            if got == 0 || hicon.is_null() {
                continue;
            }
            let img = Image::from_hicon(hicon, size, size);
            win::DestroyIcon(hicon);
            if img.is_some() {
                return img;
            }
        }
    }
    None
}

// ---- GDI+ flat API, loaded dynamically so no import library is needed ----

const PIXEL_FORMAT_32BPP_ARGB: i32 = 0x0026_200A;
const INTERP_HIGH_QUALITY_BILINEAR: i32 = 6;
const PIXEL_OFFSET_HIGH_QUALITY: i32 = 2;
const COMPOSITING_SOURCE_COPY: i32 = 1;
const UNIT_PIXEL: i32 = 2;
const IMAGE_LOCK_READ: u32 = 1;

#[repr(C)]
struct GdiplusStartupInput {
    version: u32,
    debug_callback: usize,
    suppress_background: i32,
    suppress_external: i32,
}
impl GdiplusStartupInput {
    fn default_() -> Self {
        GdiplusStartupInput { version: 1, debug_callback: 0, suppress_background: 0, suppress_external: 0 }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct GpRectI { x: i32, y: i32, w: i32, h: i32 }

#[repr(C)]
struct BitmapData {
    width: u32,
    height: u32,
    stride: i32,
    pixel_format: i32,
    scan0: *mut u8,
    reserved: usize,
}
impl BitmapData {
    fn zero() -> Self {
        BitmapData { width: 0, height: 0, stride: 0, pixel_format: 0, scan0: std::ptr::null_mut(), reserved: 0 }
    }
}

type StartupFn = unsafe extern "system" fn(*mut usize, *const GdiplusStartupInput, *mut c_void) -> i32;
type CreateBmpFn = unsafe extern "system" fn(i32, i32, i32, i32, *mut u8, *mut *mut c_void) -> i32;
type GetGraphicsFn = unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> i32;
type SetModeFn = unsafe extern "system" fn(*mut c_void, i32) -> i32;
type DrawRectRectFn = unsafe extern "system" fn(
    *mut c_void, *mut c_void, f32, f32, f32, f32, f32, f32, f32, f32,
    i32, *const c_void, *const c_void, *const c_void,
) -> i32;
type LockBitsFn = unsafe extern "system" fn(*mut c_void, *const GpRectI, u32, i32, *mut BitmapData) -> i32;
type UnlockBitsFn = unsafe extern "system" fn(*mut c_void, *mut BitmapData) -> i32;
type DisposeFn = unsafe extern "system" fn(*mut c_void) -> i32;
type LoadFileFn = unsafe extern "system" fn(*const u16, *mut *mut c_void) -> i32;
type GetDimFn = unsafe extern "system" fn(*mut c_void, *mut u32) -> i32;
type GetDCFn = unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> i32;
type ReleaseDCFn = unsafe extern "system" fn(*mut c_void, *mut c_void) -> i32;
type FromHIconFn = unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> i32;

struct Gdi {
    // Optional: a missing file-loader must never break the OCR resampler.
    load_file: Option<LoadFileFn>,
    from_hicon: Option<FromHIconFn>,
    get_width: Option<GetDimFn>,
    get_height: Option<GetDimFn>,
    get_dc: Option<GetDCFn>,
    release_dc: Option<ReleaseDCFn>,
    create_bmp: CreateBmpFn,
    get_graphics: GetGraphicsFn,
    set_interp: SetModeFn,
    set_pixel_offset: SetModeFn,
    set_compositing: SetModeFn,
    draw_rect_rect: DrawRectRectFn,
    lock_bits: LockBitsFn,
    unlock_bits: UnlockBitsFn,
    dispose_image: DisposeFn,
    delete_graphics: DisposeFn,
}

static GDI: OnceLock<Option<Gdi>> = OnceLock::new();

impl Gdi {
    fn get() -> Option<&'static Gdi> {
        GDI.get_or_init(|| unsafe { load_gdi() }).as_ref()
    }
}

#[link(name = "kernel32")]
extern "system" {
    fn LoadLibraryW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const i8) -> *mut c_void;
}

unsafe fn sym<T: Copy>(m: *mut c_void, name: &[u8]) -> Option<T> {
    let p = GetProcAddress(m, name.as_ptr() as *const i8);
    if p.is_null() {
        None
    } else {
        Some(std::mem::transmute_copy(&p))
    }
}

unsafe fn load_gdi() -> Option<Gdi> {
    let name: Vec<u16> = "gdiplus.dll\0".encode_utf16().collect();
    let m = LoadLibraryW(name.as_ptr());
    if m.is_null() {
        return None;
    }
    let startup: StartupFn = sym(m, b"GdiplusStartup\0")?;
    let mut token: usize = 0;
    // Start GDI+ exactly once. The C# host keeps it alive for the whole
    // process; a per-call startup leaks tokens and eventually fails.
    if startup(&mut token, &GdiplusStartupInput::default_(), std::ptr::null_mut()) != 0 {
        return None;
    }
    Some(Gdi {
        load_file: sym(m, b"GdipLoadImageFromFile\0"),
        from_hicon: sym(m, b"GdipCreateBitmapFromHICON\0"),
        get_width: sym(m, b"GdipGetImageWidth\0"),
        get_height: sym(m, b"GdipGetImageHeight\0"),
        get_dc: sym(m, b"GdipGetDC\0"),
        release_dc: sym(m, b"GdipReleaseDC\0"),
        create_bmp: sym(m, b"GdipCreateBitmapFromScan0\0")?,
        get_graphics: sym(m, b"GdipGetImageGraphicsContext\0")?,
        set_interp: sym(m, b"GdipSetInterpolationMode\0")?,
        set_pixel_offset: sym(m, b"GdipSetPixelOffsetMode\0")?,
        set_compositing: sym(m, b"GdipSetCompositingMode\0")?,
        draw_rect_rect: sym(m, b"GdipDrawImageRectRect\0")?,
        lock_bits: sym(m, b"GdipBitmapLockBits\0")?,
        unlock_bits: sym(m, b"GdipBitmapUnlockBits\0")?,
        dispose_image: sym(m, b"GdipDisposeImage\0")?,
        delete_graphics: sym(m, b"GdipDeleteGraphics\0")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: i32, h: i32, v: u8) -> Image {
        let mut im = Image::new(w, h);
        for p in im.data.chunks_exact_mut(4) {
            p[0] = v;
            p[1] = v;
            p[2] = v;
            p[3] = 0xFF;
        }
        im
    }

    #[test]
    fn bicubic_keeps_flat_field_flat() {
        // Catmull-Rom weights must sum to 1; otherwise a flat area drifts
        // toward black or white after a scale, which would quietly darken
        // every upscaled table before the detector sees it.
        let src = solid(17, 11, 200);
        let out = src.scaled_bicubic(43, 29).expect("scale");
        assert_eq!((out.w, out.h), (43, 29));
        for p in out.data.chunks_exact(4) {
            assert_eq!(p[0], 200);
            assert!(p[3] == 0xFF);
        }
    }

    #[test]
    fn bicubic_keeps_impulse_centred() {
        // A half-pixel shift would move every detected box and clip glyphs.
        let mut src = solid(9, 9, 0);
        let put = |im: &mut Image, x: i32, y: i32| {
            let p = ((y * im.w + x) * 4) as usize;
            im.data[p] = 255;
            im.data[p + 1] = 255;
            im.data[p + 2] = 255;
        };
        put(&mut src, 4, 4);
        let out = src.scaled_bicubic(18, 18).expect("scale");
        let mut best = (0i32, 0i32, 0u8);
        for y in 0..out.h {
            for x in 0..out.w {
                let (b, _, _) = out.pixel(x, y);
                if b > best.2 {
                    best = (x, y, b);
                }
            }
        }
        assert!((best.0 - 8).abs() <= 1, "x drifted to {}", best.0);
        assert!((best.1 - 8).abs() <= 1, "y drifted to {}", best.1);
    }

    #[test]
    fn blur_light_preserves_size_and_edges() {
        let src = solid(6, 4, 120);
        let out = src.blur_light();
        assert_eq!((out.w, out.h), (6, 4));
        assert!(out.data.chunks_exact(4).all(|p| p[0] == 120 && p[3] == 0xFF));
    }

    #[test]
    fn lanczos_keeps_flat_field_flat() {
        // Icons sit on a flat plate: a tone shift after the shrink would tint
        // every tile, so the weights must stay normalised at any ratio.
        let src = solid(64, 64, 180);
        let out = src.scaled_lanczos(37, 37).expect("scale");
        assert_eq!((out.w, out.h), (37, 37));
        for p in out.data.chunks_exact(4) {
            assert!((p[0] as i32 - 180).abs() <= 1, "flat field drifted to {}", p[0]);
            assert_eq!(p[3], 0xFF);
        }
    }

    #[test]
    fn lanczos_does_not_bleed_colour_out_of_transparent_pixels() {
        // A 256px icon is mostly transparent margin. Straight-alpha weighting
        // would drag whatever colour sits in that margin into the rim and
        // leave a grey halo around every icon.
        let mut src = Image::new(64, 64);
        for (i, p) in src.data.chunks_exact_mut(4).enumerate() {
            let inside = (16..48).contains(&(i % 64)) && (16..48).contains(&(i / 64));
            if inside {
                p[0] = 200;
                p[1] = 60;
                p[2] = 30;
                p[3] = 255;
            } else {
                p[3] = 0; // transparent, but with white colour bytes
                p[0] = 255;
                p[1] = 255;
                p[2] = 255;
            }
        }
        let out = src.scaled_lanczos(16, 16).expect("scale");
        let (r, _, _) = out.pixel(0, 0);
        assert_eq!(out.data[3], 0, "corner must stay fully transparent");
        assert!(r < 40, "corner colour bled to {}", r);
        let mid = ((8 * 16) + 8) * 4;
        assert!(out.data[mid + 3] > 240, "centre lost its alpha");
        assert!(out.data[mid] > 150, "centre colour washed out");
    }
}

// PP-OCR (PaddleOCR) offline pipeline, ported from
// github-upload/src/Capture/PaddleOcr.cs (3.3 / pp-ocr-v6 tiny).
//
// 4.2 起识别模型换成官方 PP-OCRv6 small（det 9.4 MB、rec 20.2 MB、
// 字典 18708 字），det/rec 的前后处理参数仍沿用 3.3 这一套：在 17 张
// 回归图上实测，换成官方 min/736 + thresh 0.2 / box 0.45 之后表格与 UI
// 都没有变好，耗时反而涨 2~4 倍，所以维持原参数。
//
// 目标：推理与后处理输出与 C# 完全一致。
// 所有阈值/常量、每条规则逐条照搬；ToChw 改用 crate::image::Image::to_chw
// （内部同样是 GDI+ HighQualityBilinear，与 C# ToChw 完全一致）。
//
// 精度要点：
//   * C# Math.Round(double) 默认 MidpointRounding.ToEven（银行家舍入），
//     Rust f64::round 是 half-away-from-zero，故统一用 rnd() = round_ties_even。
//   * C# char 是 UTF-16 code unit，故字典与 RecognisedChar.Value 用 u16 精确复刻；
//     最后用 String::from_utf16_lossy 还原 text。
//   * C# OcrWord.X/Y/W/H 是 double，而约定 types.rs 是 f32 —— 写入时做 as f32。
//   * result.text 调用 crate::layout::to_plain_text（签名固定，直接引用，不自行实现排版）。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use crate::image::Image;
use crate::ort;
use crate::types::{OcrResult, OcrWord, Rect};

// ---- PaddleOcr.cs 常量 ------------------------------------------------------
// DetMaxSide stays at the 3.3 value. Raising it to 1920 was measured on a
// 1900px table: det went 98ms -> 191ms and one order number came out as
// "SO-2025-01 20" instead of "SO-2025-0120", so the extra resolution bought
// nothing. Table accuracy comes from the rule stripping below instead.
const DET_MAX_SIDE: i32 = 1280;
// Small bordered tables are upscaled for the detector only. Below ~1280px the
// tiny det model starts losing whole narrow columns (see the comment in
// recognize). The target is capped just under DET_MAX_SIDE on purpose: run
// detector then resamples 1270 -> 1280 (ratio 1.008, no visible filtering),
// whereas overshooting to 1500 would make it resample back down 1500 -> 1280,
// which blurs the digits and measured 15 boxes instead of 35.
const DET_UPSCALE_BELOW: i32 = 1270;
const DET_UPSCALE_TARGET: i32 = 1270;
const DET_UPSCALE_MAX: f64 = 3.0;
const DET_THRESHOLD: f32 = 0.30;                      // PaddleOcr.cs: DetThreshold = 0.30f
const BOX_THRESHOLD: f32 = 0.50;                      // PaddleOcr.cs: BoxThreshold = 0.50f
// 4.2 small 字典 18708 行，CTC 布局为 blank + 字典 + space，共 18710 类。
const EXPECTED_CLASSES: usize = 18710;
const REC_HEIGHT: i32 = 48;                           // PaddleOcr.cs: RecHeight
const REC_MAX_WIDTH: i32 = 640;                       // PaddleOcr.cs: RecMaxWidth
const REC_OVERLAP: i32 = 96;                          // PaddleOcr.cs: RecOverlap
const REC_MIN_SEGMENT: i32 = 140;                     // PaddleOcr.cs: RecMinSegment
const DET_MEAN: [f32; 3] = [0.485, 0.456, 0.406];    // PaddleOcr.cs: DetMean
const DET_STD: [f32; 3] = [0.229, 0.224, 0.225];     // PaddleOcr.cs: DetStd
const REC_MEAN: [f32; 3] = [0.5, 0.5, 0.5];          // PaddleOcr.cs: RecMean
const REC_STD: [f32; 3] = [0.5, 0.5, 0.5];           // PaddleOcr.cs: RecStd
const INK_DELTA: i32 = 44;                            // PaddleOcr.cs: InkDelta

// ---- 数据结构 ----------------------------------------------------------------

// PaddleOcr.cs: struct Box { Rectangle Rect; float Score; }
#[derive(Clone, Copy)]
struct DetBox {
    rect: Rect,
    #[allow(dead_code)]
    score: f32,
}

// PaddleOcr.cs: struct RecognisedChar { char Value; double X; double W; bool Space; }
// C# char = UTF-16 code unit，故用 u16 精确复刻。
#[derive(Clone, Copy)]
struct RecognisedChar {
    value: u16,
    x: f64,
    // C# RunJob 会写 W，但 BuildWords 第一轮会把它整体覆盖掉（等价于这里的 cw），
    // 故该字段在移植里同样不被读取。保留以对齐结构体定义。
    #[allow(dead_code)]
    w: f64,
    space: bool,
}

// PaddleOcr.cs: struct RecJob
struct RecJob {
    data: Vec<f32>, // CHW 张量
    width: i32,     // 48 高时的自然宽度
    from: i32,      // 张量左边缘的自然 x
    core0: i32,     // 计入字符的自然区间
    core1: i32,
    scale: f64,     // 自然单位 -> 源像素
    rect: Rect,     // 整行的源矩形
}

pub struct PaddleOcr {
    det: ort::Session, // PaddleOcr.cs: _det = new Ort.Session(det, 4)
    rec: ort::Session, // PaddleOcr.cs: _rec = new Ort.Session(rec, 2)
    dict: Vec<Vec<u16>>, // LoadDictionary: blank + keys.txt 全部行 + 空格类（N+2 类）
    last_timing: Mutex<String>, // PaddleOcr.cs: LastTiming
}

// C# Math.Round(double) = MidpointRounding.ToEven；Rust 用 round_ties_even 对齐。
#[inline]
fn rnd(v: f64) -> f64 {
    v.round_ties_even()
}

// ---- Runtime 目录查找（复刻 Runtime.cs）--------------------------------------

// Runtime.cs: Required = { onnxruntime.dll, det.onnx, rec.onnx, keys.txt }
const REQUIRED: [&str; 4] = ["onnxruntime.dll", "det.onnx", "rec.onnx", "keys.txt"];

// Runtime.cs: 缺目录时的明确错误文案
const MISSING_DIR_ERROR: &str = "缺少 ocr 文件夹：det.onnx、rec.onnx、keys.txt 和 onnxruntime.dll 需要与 TT酱.exe 放在同一目录下的 ocr 子目录里。请解压完整压缩包后再运行。";

fn join_path(dir: &str, name: &str) -> String {
    std::path::Path::new(dir).join(name).to_string_lossy().into_owned()
}

// Runtime.cs: HasModels(path) —— 目录存在且 4 个必需文件齐全
fn has_models(dir: &str) -> bool {
    if dir.is_empty() {
        return false;
    }
    let p = std::path::Path::new(dir);
    if !p.is_dir() {
        return false;
    }
    for name in REQUIRED {
        if !p.join(name).is_file() {
            return false;
        }
    }
    true
}

// Runtime.cs: ExeDir() —— 入口程序所在目录（fallback 到当前工作目录）
fn exe_dir() -> String {
    if let Ok(p) = std::env::current_exe() {
        if let Some(d) = p.parent() {
            return d.to_string_lossy().into_owned();
        }
    }
    std::env::current_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
        .to_string_lossy()
        .into_owned()
}

// Runtime.cs: LegacyRoot() —— %LOCALAPPDATA%\TT酱\runtime
fn legacy_root() -> Option<String> {
    let local = std::env::var("LOCALAPPDATA").ok().filter(|s| !s.is_empty())?;
    Some(join_path(&join_path(&local, "TT酱"), "runtime"))
}

/// 复刻 Runtime.EnsureFiles 的查找顺序：
/// 环境变量 TTJIANG_RUNTIME（含 4 必需文件才采纳）→ <exe>\ocr → <exe>\runtime
/// → %LOCALAPPDATA%\TT酱\runtime（及其子目录，对应 FindLegacy）。
pub fn find_runtime_dir() -> Result<String, String> {
    // Runtime.EnsureFiles: overrideDir = Environment.GetEnvironmentVariable("TTJIANG_RUNTIME")
    if let Ok(d) = std::env::var("TTJIANG_RUNTIME") {
        if !d.is_empty() && has_models(&d) {
            return Ok(d);
        }
    }
    let exe = exe_dir();
    // Runtime.EnsureFiles: Path.Combine(ExeDir(), "ocr")
    let shipped = join_path(&exe, "ocr");
    if has_models(&shipped) {
        return Ok(shipped);
    }
    // Runtime.EnsureFiles: Path.Combine(ExeDir(), "runtime")
    let dev = join_path(&exe, "runtime");
    if has_models(&dev) {
        return Ok(dev);
    }
    // Runtime.EnsureFiles / FindLegacy: %LOCALAPPDATA%\TT酱\runtime 及其子目录
    if let Some(root) = legacy_root() {
        if has_models(&root) {
            return Ok(root);
        }
        if let Ok(rd) = std::fs::read_dir(&root) {
            let mut subs: Vec<String> = rd
                .filter_map(|e| e.ok())
                .filter(|e| e.path().is_dir())
                .map(|e| e.path().to_string_lossy().into_owned())
                .collect();
            subs.sort(); // C# GetDirectories 顺序未定义，这里固定为确定顺序
            for sub in subs {
                if has_models(&sub) {
                    return Ok(sub);
                }
            }
        }
    }
    Err(MISSING_DIR_ERROR.to_string())
}

// ---- 字典（LoadDictionary：3.3 修过的关键 bug，N+2 类）------------------------

/// 复刻 PaddleOcrEngine.LoadDictionary：
/// chars[0] = ""（blank），chars[1..=N] = keys.txt 全部行，chars[N+1] = " "（空格类）。
/// 返回 N+2 个条目；CTC 布局为 blank + 字典 + space。不能退回 lines+1。
fn load_dictionary(path: &str) -> Result<Vec<Vec<u16>>, String> {
    let raw = std::fs::read(path).map_err(|e| format!("无法读取字典：keys.txt（{}）", e))?;
    // File.ReadAllLines(path, Encoding.UTF8) 会剥离 UTF-8 BOM。
    let bytes: &[u8] = if raw.starts_with(&[0xEF, 0xBB, 0xBF]) {
        &raw[3..]
    } else {
        &raw[..]
    };
    let cow = String::from_utf8_lossy(bytes);
    let text: &str = cow.as_ref();
    // File.ReadAllLines：按 \r\n / \r / \n 分行，末尾换行不产生额外空行。
    let b = text.as_bytes();
    let mut lines: Vec<&str> = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < b.len() {
        if b[i] == b'\n' || b[i] == b'\r' {
            lines.push(&text[start..i]);
            if b[i] == b'\r' && i + 1 < b.len() && b[i + 1] == b'\n' {
                i += 1;
            }
            i += 1;
            start = i;
        } else {
            i += 1;
        }
    }
    if start < b.len() {
        lines.push(&text[start..]);
    }
    let mut dict: Vec<Vec<u16>> = Vec::with_capacity(lines.len() + 2);
    dict.push(Vec::new()); // chars[0] = ""（blank，索引 0）
    for line in &lines {
        dict.push(line.encode_utf16().collect()); // chars[1..=N]
    }
    dict.push(vec![0x20u16]); // chars[N+1] = " "（空格类，索引 N+1）
    Ok(dict)
}

// ---- PaddleOcr 对外接口 ------------------------------------------------------

impl PaddleOcr {
    /// 对应 PaddleOcrEngine.EnsureReady：目录校验 → Ort.EnsureLoaded → 字典 →
    /// det(4 线程) / rec(2 线程) 两个 Session。
    pub fn new(dir: &str) -> Result<PaddleOcr, String> {
        if !has_models(dir) {
            return Err(MISSING_DIR_ERROR.to_string());
        }
        ort::ensure_loaded(dir)?;
        let keys = join_path(dir, "keys.txt");
        let dict = load_dictionary(&keys)?;
        let det_path = join_path(dir, "det.onnx");
        let rec_path = join_path(dir, "rec.onnx");
        let det = ort::Session::new(&det_path, 4)?; // PaddleOcr.cs: new Ort.Session(det, 4)
        let rec = ort::Session::new(&rec_path, 2)?; // PaddleOcr.cs: new Ort.Session(rec, 2)
        let engine = PaddleOcr {
            det,
            rec,
            dict,
            last_timing: Mutex::new(String::new()),
        };
        engine.verify_model_dict()?;
        Ok(engine)
    }

    /// 4.2：small 的 rec 输出 18710 类（blank + 18708 字典 + space）。
    /// 只覆盖 exe 却留着 4.1 的 ocr 目录，或把新旧模型与字典混着放，
    /// CTC 都会按错的下标取字，所以加载时跑一次最小推理直接核对。
    fn verify_model_dict(&self) -> Result<(), String> {
        let probe = vec![0f32; 3 * (REC_HEIGHT as usize) * 16];
        let (_, shape) = self.rec.run(probe, &[1, 3, REC_HEIGHT as i64, 16])
            .map_err(|e| format!("OCR 识别模型无法初始化：{}", e))?;
        let classes = shape.last().copied().unwrap_or(0) as usize;
        if classes != self.dict.len() || self.dict.len() != EXPECTED_CLASSES {
            return Err(format!(
                "OCR 模型与字典不是 4.2 的那一套（模型输出 {} 类，keys.txt 对应 {} 类）。\n\
                 请解压 4.2 完整包，用里面的 det.onnx、rec.onnx、keys.txt 三个文件\n\
                 整体替换 ocr 目录，不要只覆盖 TT酱.exe。",
                classes, self.dict.len()
            ));
        }
        Ok(())
    }

    /// 对应 PaddleOcrEngine.Warmup：32x32 det / 48x16 rec 各跑一次。
    /// （C# 里还调 Runtime.CleanLegacy 删除旧 payload，属破坏性维护、非推理路径，略过。）
    pub fn warmup(&self) {
        let det_in = vec![0f32; 3 * 32 * 32];
        let rec_in = vec![0f32; 3 * (REC_HEIGHT as usize) * 16];
        let _ = self.det.run(det_in, &[1, 3, 32, 32]);
        let _ = self.rec.run(rec_in, &[1, 3, REC_HEIGHT as i64, 16]);
    }

    /// 对应 PaddleOcrEngine.LastTiming："det=..ms rec=..ms lines=.."
    pub fn last_timing(&self) -> String {
        self.last_timing
            .lock()
            .map(|s| s.clone())
            .unwrap_or_default()
    }

    /// 对应 PaddleOcrEngine.Recognize(Bitmap)。
    pub fn recognize(&self, img: &Image) -> Result<OcrResult, String> {
        self.recognize_impl(img, true)
    }

    /// `allow_up` is false on the recursive call so a tiny crop cannot be
    /// upscaled twice (282px -> 846px -> 1269px), which doubled det+rec work
    /// for no extra accuracy.
    fn recognize_impl(&self, img: &Image, allow_up: bool) -> Result<OcrResult, String> {
        // PaddleOcr.cs: if (image == null) throw new OcrException("选区图像为空")
        if img.w <= 0 || img.h <= 0 {
            return Err("选区图像为空".to_string());
        }
        let watch = std::time::Instant::now(); // Stopwatch.StartNew()
        let (gray, gray_background) = to_gray(img); // ToGray(image, out grayBackground)
        // Table grid lines are the main cause of "two columns merged into
        // one": DB joins a cell's glyphs with the rule it touches, so the
        // component spans two cells and the row collapses. Erase thin rules
        // for the detector only; recognition still crops the original pixels.
        let det_probe = strip_table_rules(img, &gray, gray_background);
        let strip_ms = watch.elapsed().as_millis() as i64;

        // A bordered table captured in a small window keeps ~15px glyphs while
        // its columns are ~50px wide. At that size the v6 tiny detector drops
        // whole narrow columns (measured: 731x190 six-column table lost the
        // 数量 column entirely, 29/36 cells), and even detected cells read as
        // empty because the recognition crop is only 18px tall.
        //
        // So for a small bordered table the whole pass runs on an upscaled
        // copy: scale -> erase rules -> one light smoothing pass -> recurse.
        // The recursion cannot trigger again (the copy is >= DET_UPSCALE_BELOW)
        // and leaves every word in the upscaled coordinate space, so the last
        // step maps the rects back. Order matters on all three steps: rules
        // must go before smoothing (blurred rule edges survive the eraser and
        // fuse rows), and smoothing is needed at all because Catmull-Rom
        // leaves a sharpening halo that DB thresholds into ragged edges
        // (measured 31 vs 36 cells on the same frame).
        let long = img.w.max(img.h) as i32;
        if allow_up && det_probe.is_some() && long < DET_UPSCALE_BELOW {
            let k = (DET_UPSCALE_TARGET as f64 / long as f64).clamp(1.0, DET_UPSCALE_MAX);
            if k > 1.02 {
                // Round to the detector's 32px grid so to_chw stays a 1:1 copy;
                // any leftover resample is GDI+ bilinear without area
                // averaging, which on thin strokes costs more than it gains.
                let nw = (((img.w as f64) * k / 32.0).round() as i32 * 32).clamp(32, DET_MAX_SIDE);
                let nh = (((img.h as f64) * k / 32.0).round() as i32 * 32).clamp(32, DET_MAX_SIDE);
                if let Some(big) = img.scaled_bicubic(nw, nh) {
                    let (bg_gray, bg_bg) = to_gray(&big);
                    let cleaned = strip_table_rules(&big, &bg_gray, bg_bg).unwrap_or(big);
                    let cleaned = cleaned.blur_light();
                    // Prep cost only: the recursive call reports its own
                    // det/rec/rules, so subtracting avoids double counting.
                    let prep_ms = watch.elapsed().as_millis() as i64 - strip_ms;
                    let mut result = self.recognize_impl(&cleaned, false)?;
                    // Map the words back onto the caller's pixel grid.
                    let sx = img.w as f64 / cleaned.w.max(1) as f64;
                    let sy = img.h as f64 / cleaned.h.max(1) as f64;
                    for w in &mut result.words {
                        w.x = (w.x as f64 * sx) as f32;
                        w.y = (w.y as f64 * sy) as f32;
                        w.w = ((w.w as f64 * sx) as f32).max(1.0);
                        w.h = ((w.h as f64 * sy) as f32).max(1.0);
                    }
                    result.text = crate::layout::to_plain_text(&result);
                    if let Ok(mut t) = self.last_timing.lock() {
                        let inner = t.clone();
                        *t = format!("{} up={}ms", inner, prep_ms);
                    }
                    return Ok(result);
                }
            }
        }

        let det_start = std::time::Instant::now();
        let (prob, dw, dh) = self.run_detector(det_probe.as_ref().unwrap_or(img))?;
        let det_ms = det_start.elapsed().as_millis() as i64;
        let mut boxes = extract_boxes(&prob, dw, dh, img.w, img.h); // ExtractBoxes
        snap_to_ink(img, &gray, gray_background, &mut boxes); // SnapToInk
        sort_reading_order(&mut boxes); // SortReadingOrder

        let mut result = OcrResult {
            words: Vec::new(),
            text: String::new(),
            language: "zh-Hans-CN".to_string(), // PaddleOcr.cs: result.Language
            engine: "pp-ocr-v6".to_string(),    // PaddleOcr.cs: result.Engine
            max_image_dimension: 0,             // PaddleOcr.cs: result.MaxImageDimension = 0
        };
        self.recognise_lines(img, &gray, gray_background, &boxes, &mut result);
        result.text = crate::layout::to_plain_text(&result); // Layout.ToPlainText(result)

        let total_ms = watch.elapsed().as_millis() as i64;
        let rec_ms = total_ms - det_ms - strip_ms;
        let timing = if strip_ms > 0 {
            format!("det={}ms rec={}ms rules={}ms lines={}", det_ms, rec_ms, strip_ms, boxes.len())
        } else {
            format!("det={}ms rec={}ms lines={}", det_ms, rec_ms, boxes.len())
        };
        if let Ok(mut t) = self.last_timing.lock() {
            *t = timing;
        }
        Ok(result)
    }

    // ---- detection -----------------------------------------------------------

    /// PaddleOcrEngine.RunDetector（ToChw 改用 Image::to_chw）。
    fn run_detector(&self, img: &Image) -> Result<(Vec<f32>, i32, i32), String> {
        // Same rule as PaddleOcr.cs: shrink only what exceeds DET_MAX_SIDE.
        // Upscaling small crops was tried and reverted — GDI+ bilinear blurs
        // 1px rules into glyph-height bars, and DB then merges whole rows.
        let long = img.w.max(img.h) as f64;
        let scale = if long > DET_MAX_SIDE as f64 {
            DET_MAX_SIDE as f64 / long
        } else {
            1.0
        };
        // sw = Math.Max(32, (int)Math.Round(W * scale / 32.0) * 32)
        let sw = 32i32.max((rnd(img.w as f64 * scale / 32.0) as i32) * 32);
        let sh = 32i32.max((rnd(img.h as f64 * scale / 32.0) as i32) * 32);
        let t0 = std::time::Instant::now();
        let data = img.to_chw(sw, sh, &DET_MEAN, &DET_STD); // PaddleOcr.ToChw
        let t1 = std::time::Instant::now();
        let (prob, shape) = self.det.run(data, &[1, 3, sh as i64, sw as i64])?;
        if std::env::var("TTJIANG_TIMING").is_ok() {
            crate::win::log(&format!(
                "det to_chw={}ms ort={}ms size={}x{}",
                t1.duration_since(t0).as_millis(), t1.elapsed().as_millis(), sw, sh
            ));
        }
        // outH = shape.Length >= 4 ? (int)shape[2] : sh;  outW = ...shape[3]...
        let out_h = if shape.len() >= 4 { shape[2] as i32 } else { sh };
        let out_w = if shape.len() >= 4 { shape[3] as i32 } else { sw };
        Ok((prob, out_w, out_h))
    }
}

/// PaddleOcrEngine.ExtractBoxes：二值化 / 8 连通域 / w,h<3 丢弃 /
/// score<0.50 丢弃 / padX=max(5,h*0.6)、padY=max(3,h*0.45) 外扩。
fn extract_boxes(prob: &[f32], pw: i32, ph: i32, image_w: i32, image_h: i32) -> Vec<DetBox> {
    let mut boxes: Vec<DetBox> = Vec::new();
    if pw <= 0 || ph <= 0 {
        return boxes;
    }
    let n = (pw as usize) * (ph as usize);
    if prob.len() < n {
        return boxes;
    }
    let mut mask = vec![0u8; n];
    for i in 0..n {
        mask[i] = if prob[i] > DET_THRESHOLD { 1 } else { 0 };
    }
    let mut stack: Vec<i32> = vec![0; n];
    let fx = image_w as f64 / pw as f64;
    let fy = image_h as f64 / ph as f64;

    for start in 0..n {
        if mask[start] != 1 {
            continue;
        }
        let mut top: usize = 0;
        stack[top] = start as i32;
        top += 1;
        mask[start] = 2;
        let mut min_x = pw;
        let mut max_x = -1;
        let mut min_y = ph;
        let mut max_y = -1;
        let mut sum: f64 = 0.0;
        let mut count: i32 = 0;
        while top > 0 {
            top -= 1;
            let index = stack[top] as usize;
            let x = (index % pw as usize) as i32;
            let y = (index / pw as usize) as i32;
            if x < min_x { min_x = x; }
            if x > max_x { max_x = x; }
            if y < min_y { min_y = y; }
            if y > max_y { max_y = y; }
            sum += prob[index] as f64; // C# sum 是 double
            count += 1;
            for dy in -1..=1 {
                let ny = y + dy;
                if ny < 0 || ny >= ph {
                    continue;
                }
                for dx in -1..=1 {
                    let nx = x + dx;
                    if nx < 0 || nx >= pw {
                        continue;
                    }
                    let nidx = (ny as usize) * (pw as usize) + (nx as usize);
                    if mask[nidx] == 1 {
                        mask[nidx] = 2;
                        stack[top] = nidx as i32;
                        top += 1;
                    }
                }
            }
        }
        let w = max_x - min_x + 1;
        let h = max_y - min_y + 1;
        if w < 3 || h < 3 {
            continue;
        }
        let score = (sum / count as f64) as f32; // float score = (float)(sum / count)
        if score < BOX_THRESHOLD {
            continue;
        }
        // Pads are applied in the caller's pixel space, so they must be sized
        // by the smaller of the two glyph heights. With min():
        //   fx >= 1 (4K capture shrunk to 1280): pad = det height, byte-for-byte
        //           the 3.3 behaviour, so existing screenshots keep their cuts.
        //   fx < 1  (small-image upscale): pad = original height, instead of
        //           1/fx times too large, which made rows overlap and fuse
        //           (36 boxes detected, 18 incoherent words).
        let h_orig = ((max_y - min_y + 1) as f64 * fy).max(1.0);
        let pad_h = (h as f64).min(h_orig);
        let pad_x = 5i32.max(rnd(pad_h * 0.6) as i32);
        let pad_y = 3i32.max(rnd(pad_h * 0.45) as i32);
        let x1 = 0i32.max(((min_x as f64 * fx) as i32) - pad_x);
        let y1 = 0i32.max(((min_y as f64 * fy) as i32) - pad_y);
        let x2 = image_w.min((((max_x + 1) as f64 * fx) as i32) + pad_x);
        let y2 = image_h.min((((max_y + 1) as f64 * fy) as i32) + pad_y);
        // Rectangle.FromLTRB(x1, y1, Math.Max(x1+1,x2), Math.Max(y1+1,y2))
        let r = Rect::new(x1, y1, (x1 + 1).max(x2) - x1, (y1 + 1).max(y2) - y1);
        boxes.push(DetBox { rect: r, score });
    }
    boxes
}

/// Erase long thin rules (table grid lines) so DB does not merge a cell with
/// its border and, through it, with the next cell. Only lines that are long
/// and thin qualify: a CJK horizontal stroke is at most one glyph wide, and a
/// rule is at most a few pixels thick, so neither bold text nor underlines of
/// ordinary prose are touched. Returns `None` when nothing was removed, in
/// which case the caller keeps the original image.
fn strip_table_rules(img: &Image, gray: &[u8], bg: i32) -> Option<Image> {
    let w = img.w;
    let h = img.h;
    if w < 48 || h < 24 {
        return None;
    }
    let n = (w as usize) * (h as usize);
    let mut mask = vec![false; n];
    let mut ink = 0usize;
    for i in 0..n {
        // 16, not 46: a default Excel/window grid line is #D9D9D9, only 38
        // levels below white. Text is far darker, so this threshold admits
        // faint rules while ignoring alternating row tint (~5 levels).
        if (gray[i] as i32 - bg).abs() > 16 {
            mask[i] = true;
            ink += 1;
        }
    }
    if ink * 100 < n {
        return None;
    }
    let mut hlen = vec![0u16; n];
    let mut vlen = vec![0u16; n];
    let wu = w as usize;
    let hu = h as usize;
    for y in 0..hu {
        let row = y * wu;
        for x in 0..wu {
            let i = row + x;
            if mask[i] {
                hlen[i] = if x > 0 { hlen[i - 1].saturating_add(1) } else { 1 };
            }
        }
    }
    for y in 0..hu {
        let row = y * wu;
        let mut run: u16 = 0;
        for x in (0..wu).rev() {
            let i = row + x;
            if mask[i] {
                if run == 0 { run = hlen[i]; }
                hlen[i] = run;
            } else {
                run = 0;
            }
        }
    }
    for x in 0..wu {
        for y in 0..hu {
            let i = y * wu + x;
            if mask[i] {
                vlen[i] = if y > 0 { vlen[i - wu].saturating_add(1) } else { 1 };
            }
        }
    }
    for x in 0..wu {
        let mut run: u16 = 0;
        for y in (0..hu).rev() {
            let i = y * wu + x;
            if mask[i] {
                if run == 0 { run = vlen[i]; }
                vlen[i] = run;
            } else {
                run = 0;
            }
        }
    }

    let min_h = 24i32.max(w / 10) as u16;
    let min_v = 26i32.max(h / 10) as u16;
    // Thickness is measured locally: a rule that touches a glyph is still
    // 1-2px wide at any given row, while the whole connected run may be as
    // long as the text. Testing the total run length would refuse to erase
    // exactly the rules that cause the merge.
    let mut out = img.clone();
    let mut removed = 0usize;
    for y in 0..hu {
        let y0 = y.saturating_sub(2);
        let y1 = (y + 2).min(hu - 1);
        for x in 0..wu {
            let i = y * wu + x;
            if !mask[i] {
                continue;
            }
            // Length first: thickness scans only run for genuine candidates.
            let hline = hlen[i] >= min_h && {
                let mut t = 0u16;
                for yy in y0..=y1 {
                    if mask[yy * wu + x] {
                        t += 1;
                    }
                }
                t <= 4
            };
            let vline = !hline && vlen[i] >= min_v && {
                let x0 = x.saturating_sub(2);
                let x1 = (x + 2).min(wu - 1);
                let mut t = 0u16;
                for xx in x0..=x1 {
                    if mask[y * wu + xx] {
                        t += 1;
                    }
                }
                t <= 4
            };
            if hline || vline {
                let p = i * 4;
                out.data[p] = bg as u8;
                out.data[p + 1] = bg as u8;
                out.data[p + 2] = bg as u8;
                removed += 1;
            }
        }
    }
    if removed < 32 {
        return None;
    }
    Some(out)
}

/// PaddleOcrEngine.ToGray：亮度 (B*29+G*150+R*77)>>8 与 256 直方图众数全局背景。
fn to_gray(img: &Image) -> (Vec<u8>, i32) {
    let w = img.w;
    let h = img.h;
    let mut gray = vec![0u8; (w as usize) * (h as usize)];
    let mut hist = [0i32; 256];
    for y in 0..h {
        for x in 0..w {
            let (b, g, r) = img.pixel(x, y); // (B, G, R)
            let lum = (b as i32 * 29 + g as i32 * 150 + r as i32 * 77) >> 8;
            gray[(y * w + x) as usize] = lum as u8;
            hist[lum as usize] += 1;
        }
    }
    let mut mode: i32 = 255;
    let mut best: i32 = -1;
    for i in 0..256 {
        if hist[i] > best {
            best = hist[i];
            mode = i as i32;
        }
    }
    (gray, mode)
}

/// PaddleOcrEngine.LocalBackground：框外 3px 环中位数。/// 注意下标：C# 正确写法是 hist[gray[y*width+x]]++（取灰度值作下标），
/// 千万不要写成 hist[y*width+x]++。
fn local_background(gray: &[u8], width: i32, height: i32, rect: Rect, fallback: i32) -> i32 {
    if rect.right() <= 0 || rect.bottom() <= 0 || rect.x >= width || rect.y >= height {
        return fallback;
    }
    let mut hist = [0i32; 256];
    let mut count: i32 = 0;
    let x1 = 0.max((width - 1).min(rect.x - 3));
    let x2 = x1.max((width - 1).min(rect.right() + 2));
    let y1 = 0.max((height - 1).min(rect.y - 3));
    let y2 = y1.max((height - 1).min(rect.bottom() + 2));
    for x in x1..=x2 {
        hist[gray[(y1 * width + x) as usize] as usize] += 1;
        count += 1;
        if y2 != y1 {
            hist[gray[(y2 * width + x) as usize] as usize] += 1;
            count += 1;
        }
    }
    for y in y1..=y2 {
        hist[gray[(y * width + x1) as usize] as usize] += 1;
        count += 1;
        if x2 != x1 {
            hist[gray[(y * width + x2) as usize] as usize] += 1;
            count += 1;
        }
    }
    if count == 0 {
        return fallback;
    }
    let half = count / 2;
    let mut acc = 0i32;
    for i in 0..256 {
        acc += hist[i];
        if acc >= half {
            return i as i32;
        }
    }
    fallback
}

/// PaddleOcrEngine.ColumnHasInk：|gray-bg| > InkDelta。
fn column_has_ink(gray: &[u8], width: i32, x: i32, y1: i32, y2: i32, background: i32) -> bool {
    for y in y1..y2 {
        let delta = gray[(y * width + x) as usize] as i32 - background;
        if delta > INK_DELTA || delta < -INK_DELTA {
            return true;
        }
    }
    false
}

/// PaddleOcrEngine.RowHasInk：|gray-bg| > InkDelta。
fn row_has_ink(gray: &[u8], width: i32, y: i32, x1: i32, x2: i32, background: i32) -> bool {
    let offset = y * width;
    for x in x1..x2 {
        let delta = gray[(offset + x) as usize] as i32 - background;
        if delta > INK_DELTA || delta < -INK_DELTA {
            return true;
        }
    }
    false
}

/// PaddleOcrEngine.SnapToInk：
/// maxGrow=max(6,min(h,40))，四边外扩到无墨为止，各边再留 ±2 余量。
fn snap_to_ink(img: &Image, gray: &[u8], background: i32, boxes: &mut Vec<DetBox>) {
    if boxes.is_empty() {
        return;
    }
    let w = img.w;
    let hgt = img.h;
    for i in 0..boxes.len() {
        // r.Intersect(new Rectangle(0, 0, w, hgt))：先夹到图像范围
        let r = boxes[i].rect.intersect(Rect::new(0, 0, w, hgt));
        if r.w < 2 || r.h < 2 {
            boxes[i].rect = Rect::new(0, 0, 0, 0); // Rectangle.Empty
            continue;
        }
        let h = 1.max(r.h);
        let local_bg = local_background(gray, w, hgt, r, background);
        let mut y1 = r.y + 2;
        let mut y2 = r.bottom() - 2;
        if y2 - y1 < 4 {
            y1 = r.y;
            y2 = r.bottom();
        }
        let max_grow = 6.max(h.min(40));
        let mut left = r.x;
        let mut right = r.right();
        let mut steps = 0;
        while left > 0
            && steps < max_grow
            && column_has_ink(gray, w, left - 1, y1, y2, local_bg)
        {
            left -= 1;
            steps += 1;
        }
        if left < r.x {
            left = 0.max(left - 2);
        }
        steps = 0;
        while right < w
            && steps < max_grow
            && column_has_ink(gray, w, right, y1, y2, local_bg)
        {
            right += 1;
            steps += 1;
        }
        if right > r.right() {
            right = w.min(right + 2);
        }
        let mut top = r.y;
        let mut bottom = r.bottom();
        steps = 0;
        while top > 0
            && steps < max_grow
            && row_has_ink(gray, w, top - 1, left, right, local_bg)
        {
            top -= 1;
            steps += 1;
        }
        if top < r.y {
            top = 0.max(top - 2);
        }
        steps = 0;
        while bottom < hgt
            && steps < max_grow
            && row_has_ink(gray, w, bottom, left, right, local_bg)
        {
            bottom += 1;
            steps += 1;
        }
        if bottom > r.bottom() {
            bottom = hgt.min(bottom + 2);
        }
        boxes[i].rect = Rect::new(
            left,
            top,
            (left + 1).max(right) - left,
            (top + 1).max(bottom) - top,
        );
    }
}

/// PaddleOcrEngine.SortReadingOrder：
/// band=round(Y/(0.6h))、overlap>0.5*min(ha,hb) 视为同一行。
fn sort_reading_order(boxes: &mut Vec<DetBox>) {
    // C# List.Sort tolerates a comparator that is not a total order (the
    // same-line test depends on both operands). Every Rust sort panics on
    // that, so compute a per-box key first, then group into lines with the
    // same overlap rule. Result is deterministic and matches the C# intent:
    // lines ordered by band/Y, boxes inside a line ordered by X.
    let mut keyed: Vec<(i32, i32, i32, usize)> = boxes.iter().enumerate().map(|(i, b)| {
        let h = 1.max(b.rect.h);
        let band = rnd(b.rect.y as f64 / (h as f64 * 0.6)) as i32;
        (band, b.rect.y, b.rect.x, i)
    }).collect();
    keyed.sort_by(|a, b| a.0.cmp(&b.0)
        .then(a.1.cmp(&b.1))
        .then(a.2.cmp(&b.2))
        .then(a.3.cmp(&b.3)));

    let mut lines: Vec<Vec<usize>> = Vec::new();
    let mut cur: Vec<usize> = Vec::new();
    let mut cur_band = 0i32;
    let mut cur_min_y = 0i32;
    let mut cur_max_bottom = 0i32;
    for &(band, _y, _x, idx) in &keyed {
        let b = boxes[idx];
        let h = 1.max(b.rect.h);
        let overlap = cur_max_bottom.min(b.rect.bottom()) - cur_min_y.max(b.rect.y);
        let same_line = !cur.is_empty()
            && band == cur_band
            && (overlap as f64) > (h as f64) * 0.5;
        if same_line {
            cur.push(idx);
            cur_min_y = cur_min_y.min(b.rect.y);
            cur_max_bottom = cur_max_bottom.max(b.rect.bottom());
        } else {
            if !cur.is_empty() { lines.push(std::mem::take(&mut cur)); }
            cur = vec![idx];
            cur_band = band;
            cur_min_y = b.rect.y;
            cur_max_bottom = b.rect.bottom();
        }
    }
    if !cur.is_empty() { lines.push(cur); }

    let src = boxes.clone();
    let mut out: Vec<DetBox> = Vec::with_capacity(src.len());
    for mut line in lines {
        line.sort_by(|&i, &j| src[i].rect.x.cmp(&src[j].rect.x).then(i.cmp(&j)));
        for i in line { out.push(src[i]); }
    }
    *boxes = out;
}

// ---- recognition -------------------------------------------------------------

// PaddleOcr.cs: _dumpIndex（DumpTensor 的静态自增序号）
static DUMP_INDEX: AtomicUsize = AtomicUsize::new(0);

/// PaddleOcrEngine.DumpTensor：TTSPRINT_DUMP=<dir> 时写出识别张量（纯诊断，
/// 不影响推理结果）。文件 rec{index}_{width}x{height}.f32，小端 f32。
fn dump_tensor(data: &[f32], width: i32, height: i32) {
    let dir = match std::env::var("TTSPRINT_DUMP") {
        Ok(d) if !d.is_empty() => d,
        _ => return,
    };
    let idx = DUMP_INDEX.fetch_add(1, Ordering::Relaxed);
    let path = std::path::Path::new(&dir).join(format!("rec{}_{}x{}.f32", idx, width, height));
    let _ = std::fs::create_dir_all(&dir);
    let mut bytes = Vec::with_capacity(data.len() * 4);
    for &f in data {
        bytes.extend_from_slice(&f.to_le_bytes()); // Buffer.BlockCopy 等价
    }
    let _ = std::fs::write(path, bytes);
}

impl PaddleOcr {
    /// PaddleOcrEngine.RecogniseLines：为每个框收集段、跑 CTC、按行 BuildWords。
    fn recognise_lines(
        &self,
        img: &Image,
        gray: &[u8],
        background: i32,
        boxes: &[DetBox],
        result: &mut OcrResult,
    ) {
        let mut jobs: Vec<RecJob> = Vec::new();
        let mut first: Vec<usize> = Vec::with_capacity(boxes.len() + 1);
        for b in boxes {
            first.push(jobs.len());
            let rect = b.rect;
            if rect.w < 6 || rect.h < 6 {
                continue;
            }
            self.collect_segments(img, rect, &mut jobs);
        }
        first.push(jobs.len());

        let decoded = self.run_jobs(&jobs);
        for i in 0..boxes.len() {
            let mut chars: Vec<RecognisedChar> = Vec::new();
            for j in first[i]..first[i + 1] {
                // C# if (part == null) continue; 失败时的空 Vec 等价。
                for &c in &decoded[j] {
                    chars.push(c);
                }
            }
            let rect = boxes[i].rect;
            // PaddleOcr.cs: BuildWords(gray, image.Width,
            //   LocalBackground(gray, image.Width, image.Height, rect, background), rect, ...)
            let bg = local_background(gray, img.w, img.h, rect, background);
            let words = build_words(gray, img.w, bg, rect, &chars);
            result.words.extend(words);
        }
    }

    /// PaddleOcrEngine.CollectSegments：段宽 640 / 重叠 96 / 尾段 <140 合并。
    fn collect_segments(&self, img: &Image, rect: Rect, jobs: &mut Vec<RecJob>) {
        let scale_to_source = rect.h as f64 / REC_HEIGHT as f64;
        let mut natural = rnd(rect.w as f64 / scale_to_source) as i32;
        if natural < 16 {
            natural = 16;
        }
        let mut start = 0;
        while start < natural {
            let mut core1 = natural.min(start + REC_MAX_WIDTH);
            let remain = natural - core1;
            if remain > 0 && remain < REC_MIN_SEGMENT {
                core1 = natural;
            }
            let from = 0.max(start - REC_OVERLAP / 2); // 96/2 = 48
            let to = natural.min(core1 + REC_OVERLAP / 2);
            if to - from >= 16 {
                jobs.push(self.make_job(img, rect, scale_to_source, from, to, start, core1));
            }
            if core1 >= natural {
                break;
            }
            start = core1;
        }
    }

    /// PaddleOcrEngine.MakeJob：裁剪(1:1) + 缩放到 RecHeight 高 + Image::to_chw(RecMean/Std)。
    fn make_job(
        &self,
        img: &Image,
        rect: Rect,
        scale_to_source: f64,
        from: i32,
        to: i32,
        core0: i32,
        core1: i32,
    ) -> RecJob {
        let target_w = to - from;
        let mut src_x = rect.x + rnd(from as f64 * scale_to_source) as i32;
        let mut src_w = rnd(target_w as f64 * scale_to_source) as i32;
        // srcX = Math.Max(rect.X, Math.Min(rect.Right - 1, srcX))
        src_x = rect.x.max((rect.right() - 1).min(src_x));
        // srcW = Math.Max(1, Math.Min(rect.Right - srcX, srcW))
        src_w = 1.max((rect.right() - src_x).min(src_w));
        // 第一步：把源 (srcX, rect.Y, srcW, rect.Height) 区域 1:1 裁出（C# 用
        // HighQualityBicubic 但目标/源尺寸相同，即无缩放拷贝），越界像素为不透明黑。
        let crop = img.crop(Rect::new(src_x, rect.y, src_w, rect.h));
        // 第二步：ToChw(crop, targetW, RecHeight, RecMean, RecStd) —— GDI+ HighQualityBilinear。
        let data = crop.to_chw(target_w, REC_HEIGHT, &REC_MEAN, &REC_STD);
        RecJob {
            data,
            width: target_w,
            from,
            core0,
            core1,
            scale: scale_to_source,
            rect,
        }
    }

    /// PaddleOcrEngine.RunJobs：与 C# 一样用 min(4, CPU核数) 工作池。
    /// ORT 的 Session::Run 允许同一 session 并发调用；每个 job 只写回自己的
    /// 下标，因此输出顺序、张量、解码结果与串行版逐字符一致。
    fn run_jobs(&self, jobs: &[RecJob]) -> Vec<Vec<RecognisedChar>> {
        let n = jobs.len();
        if n == 0 { return Vec::new(); }
        let cpu = std::thread::available_parallelism().map(|v| v.get()).unwrap_or(1);
        let workers = cpu.min(4).min(n);
        if workers <= 1 {
            return jobs.iter().map(|j| self.run_job(j)).collect();
        }

        // Session 里都是裸指针，不能自动推导 Sync；只在作用域线程内共享只读引用。
        struct Shared(*const PaddleOcr);
        unsafe impl Send for Shared {}
        unsafe impl Sync for Shared {}

        let shared = Shared(self as *const PaddleOcr);
        let cursor = AtomicUsize::new(0);
        let slots: Mutex<Vec<Option<Vec<RecognisedChar>>>> =
            Mutex::new((0..n).map(|_| None).collect());

        std::thread::scope(|scope| {
            for _ in 0..workers {
                let shared = &shared;
                let cursor = &cursor;
                let slots = &slots;
                let jobs = jobs;
                scope.spawn(move || loop {
                    let i = cursor.fetch_add(1, Ordering::Relaxed);
                    if i >= n { break; }
                    let eng = unsafe { &*shared.0 };
                    let out = eng.run_job(&jobs[i]);
                    if let Ok(mut guard) = slots.lock() {
                        guard[i] = Some(out);
                    }
                });
            }
        });

        match slots.into_inner() {
            Ok(v) => v.into_iter().map(|x| x.unwrap_or_default()).collect(),
            // Mutex poisoned (a worker panicked): degrade to serial, never
            // let a diagnostic failure change OCR content silently.
            Err(e) => e.into_inner().into_iter().map(|x| x.unwrap_or_default()).collect(),
        }
    }

    /// PaddleOcrEngine.RunJob：CTC 贪心解码。
    /// spaceClass=dict.len()-1；丢弃与前帧同类 / blank /
    /// core 边缘 sliver (naturalX<Core0+2 或 >=Core1)；空格类置 pendingSpace。
    fn run_job(&self, job: &RecJob) -> Vec<RecognisedChar> {
        dump_tensor(&job.data, job.width, REC_HEIGHT);
        // C# RunJob 抛异常时由 RunJobs 捕获 → decoded[index] = null；这里返回空 Vec。
        let (output, shape) = match self
            .rec
            .run(job.data.clone(), &[1, 3, REC_HEIGHT as i64, job.width as i64])
        {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        };
        let mut chars: Vec<RecognisedChar> = Vec::new();
        let steps = if shape.len() >= 3 { shape[1] as usize } else { 0 };
        let classes = if shape.len() >= 3 { shape[2] as usize } else { 0 };
        if steps == 0 || classes == 0 || output.len() < steps * classes {
            return chars;
        }
        let step_width = job.width as f64 / steps as f64;
        let space_class = self.dict.len() - 1; // blank + 字典 + space，最后一类是空格
        let mut pending_space = false;
        let mut previous: i32 = -1;
        for t in 0..steps {
            let offset = t * classes;
            let mut best = 0usize;
            let mut best_score = output[offset];
            for c in 1..classes {
                let v = output[offset + c];
                if v > best_score {
                    best_score = v;
                    best = c;
                }
            }
            // 丢弃 blank(0) 与连续重复类；注意 previous 仍要更新。
            if best == 0 || best as i32 == previous {
                previous = best as i32;
                continue;
            }
            previous = best as i32;

            let natural_x = job.from as f64 + (t as f64 + 0.5) * step_width;
            // 丢掉 core 边缘的 sliver（相邻段可能重复解码同一字形）
            if natural_x < (job.core0 + 2) as f64 || natural_x >= job.core1 as f64 {
                continue;
            }
            if best == space_class {
                pending_space = true;
                continue;
            }
            let glyph: &[u16] = if best < self.dict.len() {
                &self.dict[best]
            } else {
                &[]
            };
            if glyph.is_empty() {
                continue;
            }
            chars.push(RecognisedChar {
                value: glyph[0], // C# item.Value = glyph[0]（取第一个 UTF-16 code unit）
                w: step_width * job.scale,
                x: job.rect.x as f64 + natural_x * job.scale,
                space: pending_space,
            });
            pending_space = false;
        }
        chars
    }
}

/// PaddleOcrEngine.BuildWords：
/// 字宽 = 相邻 X 间距 clamp[2,0.72h]，fallback 0.6h；gapLimit=0.5h 切词；
/// 插空格条件 c.Space || wideBefore || (spaceBefore && CJK→非CJK)；
/// SuppressSpace / IsDecimalPoint 抑制；词框 y=rect.Y, h=rect.Height, w>=0.35h。
fn build_words(
    gray: &[u8],
    gray_width: i32,
    background: i32,
    rect: Rect,
    chars: &[RecognisedChar],
) -> Vec<OcrWord> {
    let mut words: Vec<OcrWord> = Vec::new();
    if chars.is_empty() {
        return words;
    }
    // 由相邻 X 间距推导字宽（CTC timestep 不是字宽），并 cap 到合理字宽。
    let cap = rect.h as f64 * 0.72;
    let fallback = rect.h as f64 * 0.6;
    let mut cw: Vec<f64> = vec![0.0; chars.len()];
    for i in 0..chars.len() {
        let mut pitch = f64::MAX;
        if i > 0 {
            pitch = chars[i].x - chars[i - 1].x;
        }
        if i + 1 < chars.len() {
            pitch = pitch.min(chars[i + 1].x - chars[i].x);
        }
        if pitch == f64::MAX || pitch <= 0.0 {
            pitch = fallback;
        }
        cw[i] = 2.0f64.max(pitch.min(cap));
    }

    let (space_before, wide_before) = measure_spaces(gray, gray_width, background, rect, chars);

    let gap_limit = rect.h as f64 * 0.5;
    let mut index = 0usize;
    while index < chars.len() {
        if chars[index].value == 0x20 {
            index += 1;
            continue;
        }
        let mut text: Vec<u16> = Vec::new(); // StringBuilder(UTF-16 code units)
        let first_left = chars[index].x - cw[index] * 0.5;
        let mut last_right = first_left + cw[index];
        while index < chars.len() {
            let c = chars[index];
            if c.value == 0x20 {
                break;
            }
            let left = c.x - cw[index] * 0.5;
            if !text.is_empty() && left - last_right > gap_limit {
                break;
            }
            if !text.is_empty() {
                let previous_char = text[text.len() - 1];
                let space = c.space
                    || wide_before[index] == 1
                    || (space_before[index] == 1
                        && is_cjk_char(previous_char)
                        && !is_cjk_char(c.value));
                if space
                    && previous_char != 0x20
                    && !suppress_space(previous_char, c.value)
                    && !is_decimal_point(&text, c.value)
                {
                    text.push(0x20);
                }
            }
            text.push(c.value);
            last_right = c.x + cw[index] * 0.5;
            index += 1;
        }
        if text.is_empty() {
            continue;
        }
        words.push(OcrWord {
            text: String::from_utf16_lossy(&text),
            // C# OcrWord 是 double，约定 types.rs 是 f32 → 这里 as f32。
            x: (rect.x as f64).max(first_left) as f32,
            w: ((rect.h as f64 * 0.35).max(last_right - first_left)) as f32,
            y: rect.y as f32,
            h: rect.h as f32,
        });
    }
    words
}

/// PaddleOcrEngine.MeasureSpaces：InkRuns→gaps→SpaceThreshold→MarkSpaces；
/// 若 marked*3 > chars.Count 则阈值×1.8 重算（wideLimit 保持原值）。
fn measure_spaces(
    gray: &[u8],
    gray_width: i32,
    background: i32,
    rect: Rect,
    chars: &[RecognisedChar],
) -> (Vec<i32>, Vec<i32>) {
    let mut space_before = vec![0i32; chars.len()];
    let mut wide_before = vec![0i32; chars.len()];
    let (run_start, run_end) = ink_runs(gray, gray_width, background, rect);
    if run_start.len() < 2 {
        return (space_before, wide_before);
    }
    let mut run_of = vec![0i32; chars.len()];
    let mut gaps: Vec<i32> = Vec::with_capacity(chars.len());
    for i in 0..chars.len() {
        run_of[i] = nearest_run(&run_start, &run_end, rnd(chars[i].x) as i32);
        if i == 0 {
            gaps.push(0);
            continue;
        }
        let prev = run_of[i - 1];
        let cur = run_of[i];
        gaps.push(if prev == cur || prev < 0 || cur < 0 {
            0
        } else {
            0.max(run_start[cur as usize] - run_end[prev as usize] - 1)
        });
    }
    let mut threshold = space_threshold(&gaps, rect.h);
    // wideLimit = (int)Math.Ceiling(Math.Max(threshold*2.0, rect.Height*0.45))
    let wide_limit = ((threshold as f64 * 2.0).max(rect.h as f64 * 0.45)).ceil() as i32;
    let mut marked = mark_spaces(
        &gaps,
        chars,
        threshold,
        wide_limit,
        &mut space_before,
        &mut wide_before,
    );
    // 密集等宽/严重抗锯齿行：1/3 以上触发说明阈值过低，抬高重算。
    if !chars.is_empty() && marked * 3 > chars.len() as i32 {
        threshold = (threshold as f64 * 1.8).ceil() as i32;
        space_before.fill(0); // Array.Clear
        wide_before.fill(0);
        marked = mark_spaces(
            &gaps,
            chars,
            threshold,
            wide_limit,
            &mut space_before,
            &mut wide_before,
        );
    }
    let _ = (marked, threshold); // C# 仅用于 SpaceTrace 诊断
    (space_before, wide_before)
}

/// PaddleOcrEngine.MarkSpaces：
/// wideLimit=max(2*threshold,0.45h)；CJK-CJK 时 limit=ceil(2.2*threshold)。
fn mark_spaces(
    gaps: &[i32],
    chars: &[RecognisedChar],
    threshold: i32,
    wide_limit: i32,
    space_before: &mut [i32],
    wide_before: &mut [i32],
) -> i32 {
    let mut marked = 0i32;
    for i in 1..gaps.len() {
        if gaps[i] >= wide_limit {
            wide_before[i] = 1;
        }
        let mut limit = threshold;
        if is_cjk_char(chars[i - 1].value) && is_cjk_char(chars[i].value) {
            limit = (threshold as f64 * 2.2).ceil() as i32;
        }
        if gaps[i] >= limit {
            space_before[i] = 1;
            marked += 1;
        }
    }
    marked
}

/// PaddleOcrEngine.InkRuns：逐列检测墨迹，归并成 [start,end] 区间。
fn ink_runs(gray: &[u8], gray_width: i32, background: i32, rect: Rect) -> (Vec<i32>, Vec<i32>) {
    let mut starts: Vec<i32> = Vec::with_capacity(32);
    let mut ends: Vec<i32> = Vec::with_capacity(32);
    let mut y1 = rect.y + 1;
    let mut y2 = rect.bottom() - 1;
    if y2 - y1 < 3 {
        y1 = rect.y;
        y2 = rect.bottom();
    }
    let x1 = 0.max(rect.x);
    let x2 = gray_width.min(rect.right());
    let mut open = false;
    for x in x1..x2 {
        if column_has_ink(gray, gray_width, x, y1, y2, background) {
            if !open {
                starts.push(x);
                open = true;
            }
        } else if open {
            ends.push(x - 1);
            open = false;
        }
    }
    if open {
        ends.push(x2 - 1);
    }
    while ends.len() < starts.len() {
        ends.push(x2 - 1);
    }
    (starts, ends)
}

/// PaddleOcrEngine.NearestRun：二分找 x 落入的 run，未命中取最近的一个。
fn nearest_run(starts: &[i32], ends: &[i32], x: i32) -> i32 {
    let mut lo: i32 = 0;
    let mut hi: i32 = starts.len() as i32 - 1;
    while lo <= hi {
        let mid = (lo + hi) / 2;
        if x < starts[mid as usize] {
            hi = mid - 1;
        } else if x > ends[mid as usize] {
            lo = mid + 1;
        } else {
            return mid;
        }
    }
    let right = lo.min(starts.len() as i32 - 1);
    let left = right - 1;
    if left < 0 {
        return right;
    }
    if right == left {
        return left;
    }
    if (x - ends[left as usize]) <= (starts[right as usize] - x) {
        left
    } else {
        right
    }
}

/// PaddleOcrEngine.SpaceThreshold：ceil(max(2.5, 0.18*h))。
fn space_threshold(_gaps: &[i32], height: i32) -> i32 {
    (2.5f64.max(height as f64 * 0.18)).ceil() as i32
}

// PaddleOcrEngine.SuppressSpace 的收尾标点（从 C# 源码逐字复制；注意含全角「）」U+FF09）
const CLOSING: &str = ".,;:!?%)]}>，。！？；：、）》】」』";
// PaddleOcrEngine.SuppressSpace 的起始括号
const OPENING: &str = "([{<（《【「『";

fn contains_utf16(s: &str, u: u16) -> bool {
    s.encode_utf16().any(|c| c == u)
}

/// PaddleOcrEngine.SuppressSpace：收尾标点 / 起始括号 /
/// ':'|';' 后跟 '\\'|'/'（盘符路径）时不插空格。
fn suppress_space(previous: u16, next: u16) -> bool {
    if contains_utf16(CLOSING, next) {
        return true;
    }
    if contains_utf16(OPENING, previous) {
        return true;
    }
    if (previous == b':' as u16 || previous == b';' as u16)
        && (next == b'\\' as u16 || next == b'/' as u16)
    {
        return true;
    }
    false
}

/// PaddleOcrEngine.IsCjkChar（char 用 u16，与 C# UTF-16 code unit 一致）。
fn is_cjk_char(c: u16) -> bool {
    (c >= 0x2E80 && c <= 0x9FFF)
        || (c >= 0x3000 && c <= 0x303F)
        || (c >= 0xFF00 && c <= 0xFFEF)
}

/// 复刻 C# char.IsDigit（UnicodeCategory.DecimalDigitNumber）。
/// keys.txt 中属于 Nd 的仅 ASCII 0-9 与全角 ０-９；字典里的上标 ²³¹、
/// 下标 ₀-₉、圈号 ①-⑨ 等为 No 类别，C# char.IsDigit 返回 false，故不纳入。
fn is_digit(u: u16) -> bool {
    (u >= 0x30 && u <= 0x39) || (u >= 0xFF10 && u <= 0xFF19)
}

/// PaddleOcrEngine.IsDecimalPoint："25. 50" 视为一个小数。
fn is_decimal_point(text: &[u16], next: u16) -> bool {
    let n = text.len();
    if n < 2 || text[n - 1] != b'.' as u16 {
        return false;
    }
    is_digit(text[n - 2]) && is_digit(next)
}

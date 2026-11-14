// layout.rs
// TT酱 3.3 OCR 排版后处理 —— 逐行移植自 github-upload/src/Capture/Layout.cs。
// 零依赖：仅使用 std。所有文本以 UTF-16 码元（Vec<u16>）处理，严格对齐 C# 的
// char / string 索引与 Length 语义；几何量统一先提升为 f64 再运算，以匹配 C#
// Layout.cs 里的 double 算术（types.rs 的字段是 f32）。

use crate::types::OcrWord;
use std::cmp::Ordering;

// AsciiPunct = ".,;!?%)]}/\\|"（故意不含冒号）；OpenPunct = "([{<"
const ASCII_PUNCT: &[u16] = &[
    0x2E, // .
    0x2C, // ,
    0x3B, // ;
    0x21, // !
    0x3F, // ?
    0x25, // %
    0x29, // )
    0x5D, // ]
    0x7D, // }
    0x2F, // /
    0x5C, // 反斜杠
    0x7C, // |
];
const OPEN_PUNCT: &[u16] = &[
    0x28, // (
    0x5B, // [
    0x7B, // {
    0x3C, // <
];

// ---- UTF-16 输入/输出 ----
// 输入 &str 是合法 UTF-8，encode_utf16 得到合法 UTF-16；本文件对文本只做等长的
// BMP 码元替换，从不拆分或插入单个代理项，因此结果一定仍是合法 UTF-16，
// from_utf16_lossy 不会插入 U+FFFD（且全程不使用 unwrap）。
fn to_units(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}
fn from_units(u: &[u16]) -> String {
    String::from_utf16_lossy(u)
}

// ---- C# char 静态方法等价（入参/索引均为 UTF-16 码元）----
fn is_cjk(c: u16) -> bool {
    (0x2E80..=0x9FFF).contains(&c)
        || (0x3000..=0x303F).contains(&c)
        || (0xFF00..=0xFFEF).contains(&c)
}

// C# char.IsWhiteSpace 的 BMP 全集。
fn is_white_space(c: u16) -> bool {
    matches!(c,
        0x0009..=0x000D | 0x0020 | 0x0085 | 0x00A0 | 0x1680
        | 0x2000..=0x200A | 0x2028 | 0x2029 | 0x202F | 0x205F | 0x3000)
}

// C# char.IsDigit = UnicodeCategory.DecimalDigitNumber。UTF-16 增补平面的数字由
// 代理对表示，单个码元类别是 Surrogate 而非 Nd，故 C# 逐 char 判定时它们恒为
// false；这里只列 BMP 内的 Nd 区段（每段都是连续的 0..9）。
fn is_digit(c: u16) -> bool {
    const ND: &[(u16, u16)] = &[
        (0x0030, 0x0039), (0x0660, 0x0669), (0x06F0, 0x06F9), (0x07C0, 0x07C9),
        (0x0966, 0x096F), (0x09E6, 0x09EF), (0x0A66, 0x0A6F), (0x0AE6, 0x0AEF),
        (0x0B66, 0x0B6F), (0x0BE6, 0x0BEF), (0x0C66, 0x0C6F), (0x0CE6, 0x0CEF),
        (0x0D66, 0x0D6F), (0x0DE6, 0x0DEF), (0x0E50, 0x0E59), (0x0ED0, 0x0ED9),
        (0x0F20, 0x0F29), (0x1040, 0x1049), (0x1090, 0x1099), (0x17E0, 0x17E9),
        (0x1810, 0x1819), (0x1946, 0x194F), (0x19D0, 0x19D9), (0x1A80, 0x1A89),
        (0x1A90, 0x1A99), (0x1B50, 0x1B59), (0x1BB0, 0x1BB9), (0x1C40, 0x1C49),
        (0x1C50, 0x1C59), (0xA620, 0xA629), (0xA8D0, 0xA8D9), (0xA900, 0xA909),
        (0xA9D0, 0xA9D9), (0xA9F0, 0xA9F9), (0xAA50, 0xAA59), (0xABF0, 0xABF9),
        (0xFF10, 0xFF19),
    ];
    ND.iter().any(|&(lo, hi)| c >= lo && c <= hi)
}

// ---- 几何辅助：字段是 f32，C# 是 double，故先提升再算 Right/Cy ----
fn cy_f64(w: &OcrWord) -> f64 {
    w.y as f64 + w.h as f64 / 2.0
}
fn right_f64(w: &OcrWord) -> f64 {
    w.x as f64 + w.w as f64
}

// Median(values, fallback)：偶数取中间两值平均。
fn median(mut values: Vec<f64>, fallback: f64) -> f64 {
    if values.is_empty() {
        return fallback;
    }
    values.sort_by(|a, b| a.total_cmp(b));
    let n = values.len();
    if n & 1 == 1 {
        values[n / 2]
    } else {
        (values[n / 2 - 1] + values[n / 2]) / 2.0
    }
}

// Normalize(text, tableMode)：tableMode 在 C# 中未使用，保留参数以逐行对应。
fn normalize(text: &[u16], _table_mode: bool) -> Vec<u16> {
    text.iter()
        .map(|&c| match c {
            0xFF0E => 0x2E, // U+FF0E -> '.'
            0xFF05 => 0x25, // U+FF05 -> '%'
            0xFF0D => 0x2D, // U+FF0D -> '-'
            0x3000 => 0x20, // U+3000 -> ' '
            _ => c,
        })
        .collect()
}

fn first_normalized(c: u16) -> u16 {
    match c {
        0xFF0E => 0x2E,
        0xFF05 => 0x25,
        0xFF0D => 0x2D,
        _ => c,
    }
}

// CharWidth(word.W / 非空白字符数)，按 UTF-16 码元计数。
fn char_width(w: &OcrWord) -> f64 {
    let mut n: usize = 0;
    for c in w.text.encode_utf16() {
        if !is_white_space(c) {
            n += 1;
        }
    }
    if n == 0 {
        return 0.0;
    }
    w.w as f64 / n as f64
}

// NeedsSeparator(prev, cur, medianW)
fn needs_separator(prev: &OcrWord, cur: &OcrWord, median_w: f64) -> bool {
    let pu = to_units(&prev.text);
    let cu = to_units(&cur.text);
    if pu.is_empty() || cu.is_empty() {
        return false;
    }
    let a = first_normalized(pu[pu.len() - 1]);
    let b = first_normalized(cu[0]);
    if is_white_space(a) || is_white_space(b) {
        return false;
    }
    if ASCII_PUNCT.contains(&a) || ASCII_PUNCT.contains(&b) || OPEN_PUNCT.contains(&a) {
        return false;
    }
    let cjk_a = is_cjk(a);
    let cjk_b = is_cjk(b);
    if cjk_a && cjk_b {
        // 两个汉字并排一般是同一短语；仅当列间距大于 0.9 倍行高（表格列）才插分隔。
        let h = (prev.h as f64).max(cur.h as f64);
        return h > 0.0 && (cur.x as f64 - right_f64(prev)) > h * 0.9;
    }
    let mut glyph = char_width(prev).min(char_width(cur));
    let cap = (prev.h as f64).max(cur.h as f64) * 0.9;
    if glyph <= 0.0 || glyph > cap {
        glyph = cap;
    }
    if glyph <= 0.0 {
        glyph = median_w * 0.5;
    }
    (cur.x as f64 - right_f64(prev)) > 2.0_f64.max(glyph * 0.35)
}

// Flatten(text)：删除所有空白码元。Layout.cs 的 internal helper，主流程不调用，
// 依约保留移植。
#[allow(dead_code)]
fn flatten(text: &[u16]) -> Vec<u16> {
    text.iter().copied().filter(|&c| !is_white_space(c)).collect()
}

// has_double_space 对应 C# text.IndexOf("  ", Ordinal) >= 0。
fn has_double_space(text: &[u16]) -> bool {
    text.windows(2).any(|w| w[0] == 0x20 && w[1] == 0x20)
}

// CollapseSpaces(text)：仅当存在两个连续空格时才进入折叠；空格或 tab 折叠成单个空格。
fn collapse_spaces(text: &[u16]) -> Vec<u16> {
    if !has_double_space(text) {
        return text.to_vec();
    }
    let mut sb = Vec::with_capacity(text.len());
    let mut space = false;
    for &c in text {
        if c == 0x20 || c == 0x09 {
            if !space {
                sb.push(0x20);
            }
            space = true;
        } else {
            sb.push(c);
            space = false;
        }
    }
    sb
}

// DropCjkSpaces(text)：删除夹在两个 CJK 码元之间的空格。
fn drop_cjk_spaces(text: &[u16]) -> Vec<u16> {
    if !text.contains(&0x20) {
        return text.to_vec();
    }
    let mut sb = Vec::with_capacity(text.len());
    for i in 0..text.len() {
        if text[i] == 0x20
            && i > 0
            && i + 1 < text.len()
            && is_cjk(text[i - 1])
            && is_cjk(text[i + 1])
        {
            continue;
        }
        sb.push(text[i]);
    }
    sb
}

// trim_whitespace 对应无参 string.Trim()；trim_tab_space 对应 Trim('\t',' ')。
fn trim_whitespace(text: &[u16]) -> Vec<u16> {
    let mut start = 0;
    let mut end = text.len();
    while start < end && is_white_space(text[start]) {
        start += 1;
    }
    while end > start && is_white_space(text[end - 1]) {
        end -= 1;
    }
    text[start..end].to_vec()
}
fn trim_tab_space(text: &[u16]) -> Vec<u16> {
    let is_trim = |c: u16| c == 0x09 || c == 0x20;
    let mut start = 0;
    let mut end = text.len();
    while start < end && is_trim(text[start]) {
        start += 1;
    }
    while end > start && is_trim(text[end - 1]) {
        end -= 1;
    }
    text[start..end].to_vec()
}

// JoinWords(words, medianW, tableMode)
// 3.3 修复：每个 word 先单独 Normalize + DropCjkSpaces 再决定列分隔，几何分隔符
// 是最后插入的，不会被 DropCjkSpaces 当作字间松散空格删掉；分隔符判断仍基于
// 原始相邻 word（words[i-1] / words[i]）。
fn join_words(words: &[OcrWord], median_w: f64, table_mode: bool) -> String {
    let mut sb: Vec<u16> = Vec::new();
    let mut has_content = false;
    for i in 0..words.len() {
        let text = drop_cjk_spaces(&normalize(&to_units(&words[i].text), table_mode));
        if text.is_empty() {
            continue;
        }
        // has_content 为真时必有 i >= 1，短路保证 words[i - 1] 不越界。
        if has_content && needs_separator(&words[i - 1], &words[i], median_w) {
            sb.push(0x20);
        }
        sb.extend_from_slice(&text);
        has_content = true;
    }
    from_units(&trim_whitespace(&collapse_spaces(&sb)))
}

struct Cell {
    x1: f64,
    x2: f64,
    text: String,
}

// GroupRows：按 Cy 排序（同 Cy 按 X）；tol=max(7, 0.62*medianH) 分行；行内按 X 排序。
fn group_rows(source: &[OcrWord]) -> (Vec<Vec<OcrWord>>, f64, f64) {
    let mut heights: Vec<f64> = Vec::new();
    let mut widths: Vec<f64> = Vec::new();
    for w in source {
        if w.h > 0.0 {
            heights.push(w.h as f64);
        }
        if w.w > 0.0 {
            widths.push(w.w as f64);
        }
    }
    let median_h = median(heights, 12.0);
    let median_w = median(widths, 12.0);
    let mut rows: Vec<Vec<OcrWord>> = Vec::new();
    if source.is_empty() {
        return (rows, median_h, median_w);
    }
    let tol = 7.0_f64.max(median_h * 0.62);
    let mut ordered: Vec<OcrWord> = source.to_vec();
    ordered.sort_by(|a, b| {
        let c = cy_f64(a).total_cmp(&cy_f64(b));
        match c {
            Ordering::Equal => (a.x as f64).total_cmp(&(b.x as f64)),
            other => other,
        }
    });
    let mut current: Vec<OcrWord> = vec![ordered[0].clone()];
    for i in 1..ordered.len() {
        let word = ordered[i].clone();
        if cy_f64(&word) - cy_f64(&current[current.len() - 1]) > tol {
            current.sort_by(|a, b| (a.x as f64).total_cmp(&(b.x as f64)));
            rows.push(current);
            current = Vec::new();
        }
        current.push(word);
    }
    if !current.is_empty() {
        current.sort_by(|a, b| (a.x as f64).total_cmp(&(b.x as f64)));
        rows.push(current);
    }
    (rows, median_h, median_w)
}

fn make_cell(words: &[OcrWord], median_w: f64, table_mode: bool) -> Cell {
    let mut x1 = f64::MAX;
    let mut x2 = f64::MIN;
    for w in words {
        if (w.x as f64) < x1 {
            x1 = w.x as f64;
        }
        if right_f64(w) > x2 {
            x2 = right_f64(w);
        }
    }
    Cell {
        x1,
        x2,
        text: join_words(words, median_w, table_mode),
    }
}

fn split_cells(row: &[OcrWord], median_h: f64, median_w: f64, table_mode: bool) -> Vec<Cell> {
    // The column separator has to scale with the glyph height, not with the
    // average word width. A table full of long labels pushes median_w past
    // 100px, which used to make the threshold wider than the gap between
    // columns and collapse the whole row into a single cell.
    // 0.6 glyph heights sits above the space an OCR box leaves between two
    // words of one sentence (~0.3h) and below the smallest believable cell gap.
    let gap = 10.0_f64.max(median_h * 0.6).min(median_w.max(10.0) * 1.15);
    let mut cells: Vec<Cell> = Vec::new();
    if row.is_empty() {
        return cells;
    }
    let mut current: Vec<OcrWord> = vec![row[0].clone()];
    for i in 1..row.len() {
        let word = row[i].clone();
        if (word.x as f64) - right_f64(&current[current.len() - 1]) > gap {
            cells.push(make_cell(&current, median_w, table_mode));
            current = Vec::new();
        }
        current.push(word);
    }
    if !current.is_empty() {
        cells.push(make_cell(&current, median_w, table_mode));
    }
    cells
}

fn infer_columns(cell_rows: &[Vec<Cell>]) -> Vec<[f64; 2]> {
    let mut intervals: Vec<[f64; 2]> = Vec::new();
    for row in cell_rows {
        if row.len() < 2 {
            continue;
        }
        for cell in row {
            intervals.push([cell.x1, cell.x2]);
        }
    }
    intervals.sort_by(|a, b| {
        let c = a[0].total_cmp(&b[0]);
        match c {
            Ordering::Equal => a[1].total_cmp(&b[1]),
            other => other,
        }
    });
    let mut merged: Vec<[f64; 2]> = Vec::new();
    for item in intervals {
        if !merged.is_empty() && item[0] <= merged[merged.len() - 1][1] {
            let last = merged.len() - 1;
            if item[1] > merged[last][1] {
                merged[last][1] = item[1];
            }
        } else {
            merged.push(item);
        }
    }
    merged
}

fn overlap(a1: f64, a2: f64, b1: f64, b2: f64) -> f64 {
    0.0_f64.max(a2.min(b2) - a1.max(b1))
}

fn join_lines(lines: &[String]) -> String {
    lines.join("\r\n")
}

// ---- 行分割辅助（对应 C# String.Split / string.Join）----
// Split('\n')：按单个 LF 分割并保留空段（StringSplitOptions.None）。
fn split_lf(text: &[u16]) -> Vec<Vec<u16>> {
    let mut parts = Vec::new();
    let mut cur = Vec::new();
    for &c in text {
        if c == 0x0A {
            parts.push(std::mem::take(&mut cur));
        } else {
            cur.push(c);
        }
    }
    parts.push(cur);
    parts
}
fn join_lf(parts: &[Vec<u16>]) -> Vec<u16> {
    let mut out = Vec::new();
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            out.push(0x0A);
        }
        out.extend_from_slice(p);
    }
    out
}
// Split(new string[]{"\r\n"}, None)：按字面 CRLF 分割并保留空段。
fn split_crlf(text: &[u16]) -> Vec<Vec<u16>> {
    let mut parts = Vec::new();
    let mut cur = Vec::new();
    let mut i = 0;
    while i < text.len() {
        if text[i] == 0x0D && i + 1 < text.len() && text[i + 1] == 0x0A {
            parts.push(std::mem::take(&mut cur));
            i += 2;
        } else {
            cur.push(text[i]);
            i += 1;
        }
    }
    parts.push(cur);
    parts
}

// RepairDrivePath(line)：去掉前导空格/tab 后以 [A-Za-z]: 开头且第 3 字符是
// '/' 或 '\' 时，从第 3 字符起把连续的 / \ 折叠成单个 '\'；保护 URL/散文。
fn repair_drive_path(line: &[u16]) -> Vec<u16> {
    let len = line.len();
    let mut start = 0;
    while start < len && (line[start] == 0x20 || line[start] == 0x09) {
        start += 1;
    }
    if start + 2 >= len {
        return line.to_vec();
    }
    let drive = line[start];
    let letter = (0x41..=0x5A).contains(&drive) || (0x61..=0x7A).contains(&drive);
    if !letter || line[start + 1] != 0x3A {
        return line.to_vec();
    }
    let sep = line[start + 2];
    if sep != 0x2F && sep != 0x5C {
        return line.to_vec();
    }
    let mut sb = Vec::with_capacity(len);
    sb.extend_from_slice(&line[0..start + 2]);
    let mut last_sep = false;
    for i in start + 2..len {
        let c = line[i];
        if c == 0x2F || c == 0x5C {
            if !last_sep {
                sb.push(0x5C);
                last_sep = true;
            }
            continue;
        }
        last_sep = false;
        sb.push(c);
    }
    sb
}

fn repair_drive_paths(text: &[u16]) -> Vec<u16> {
    if !text.contains(&0x3A) {
        return text.to_vec();
    }
    let lines = split_lf(text);
    let mut changed = false;
    let mut out: Vec<Vec<u16>> = Vec::with_capacity(lines.len());
    for line in lines {
        let fixed = repair_drive_path(&line);
        if fixed != line {
            out.push(fixed);
            changed = true;
        } else {
            out.push(line);
        }
    }
    if changed {
        join_lf(&out)
    } else {
        text.to_vec()
    }
}

// FinalizeText(text)
fn finalize_text(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    let mut sb = to_units(text);
    let len = sb.len();
    let mut i = 1;
    while i + 1 < len {
        let c = sb[i];
        if !is_digit(sb[i - 1]) || !is_digit(sb[i + 1]) {
            i += 1;
            continue;
        }
        if c == 0x2C {
            // ',' 千分位例外：其后恰 3 位数字、且再后非数字则保留（while 已把
            // k 移到首个非数字，后半条件恒真，等价于 digits == 3）。
            let mut k = i + 1;
            let mut digits = 0usize;
            while k < len && is_digit(sb[k]) {
                k += 1;
                digits += 1;
            }
            let thousands = digits == 3 && (k >= len || !is_digit(sb[k]));
            if thousands {
                i += 1;
                continue;
            }
            sb[i] = 0x2E;
            i += 1;
            continue;
        }
        if c == 0xFF0E || c == 0xFF0C || c == 0x3001 || c == 0x00B7 {
            sb[i] = 0x2E; // '．' '，' '、' '·' -> '.'
        } else if c == 0x4E00 || c == 0x2014 || c == 0x2013 {
            sb[i] = 0x2D; // '一' '—' '–' -> '-'
        }
        i += 1;
    }
    from_units(&repair_drive_paths(&sb))
}

// ---- 对外接口 ----
pub fn to_plain_text(result: &crate::types::OcrResult) -> String {
    let (rows, _median_h, median_w) = group_rows(&result.words);
    let mut lines: Vec<String> = Vec::new();
    for row in &rows {
        let line = join_words(row, median_w, false);
        if !line.is_empty() {
            lines.push(line);
        }
    }
    finalize_text(&join_lines(&lines))
}

pub fn to_tsv(result: &crate::types::OcrResult) -> String {
    let (rows, median_h, median_w) = group_rows(&result.words);
    if rows.is_empty() {
        return String::new();
    }
    let mut cell_rows: Vec<Vec<Cell>> = Vec::new();
    for row in &rows {
        cell_rows.push(split_cells(row, median_h, median_w, true));
    }
    let columns = infer_columns(&cell_rows);
    if columns.len() < 2 {
        return to_plain_text(result);
    }
    let mut matrix: Vec<Vec<String>> = Vec::new();
    for row in &cell_rows {
        let mut cells: Vec<String> = vec![String::new(); columns.len()];
        for cell in row {
            let mut best = 0usize;
            let mut best_score = -1.0_f64;
            for i in 0..columns.len() {
                let score = overlap(cell.x1, cell.x2, columns[i][0], columns[i][1]);
                if score > best_score {
                    best_score = score;
                    best = i;
                }
            }
            if best_score <= 0.0 {
                best = 0;
                let mut min_dist = f64::MAX;
                for i in 0..columns.len() {
                    let dist = (columns[i][0] - cell.x1).abs();
                    if dist < min_dist {
                        min_dist = dist;
                        best = i;
                    }
                }
            }
            if !cells[best].is_empty() {
                let mut merged = to_units(&cells[best]);
                merged.push(0x20);
                merged.extend_from_slice(&to_units(&cell.text));
                cells[best] = from_units(&trim_whitespace(&merged));
            } else {
                cells[best] = cell.text.clone();
            }
        }
        matrix.push(cells);
    }
    let mut keep_cols: Vec<usize> = Vec::new();
    for i in 0..columns.len() {
        for row in &matrix {
            if !row[i].is_empty() {
                keep_cols.push(i);
                break;
            }
        }
    }
    let mut lines: Vec<String> = Vec::new();
    for row in &matrix {
        let parts: Vec<String> = keep_cols.iter().map(|&i| row[i].clone()).collect();
        let line = parts.join("\t");
        if !trim_tab_space(&to_units(&line)).is_empty() {
            lines.push(line);
        }
    }
    finalize_text(&join_lines(&lines))
}

pub fn column_count(tsv: &str) -> i32 {
    if tsv.is_empty() {
        return 0;
    }
    let mut max = 0i32;
    for line in split_crlf(&to_units(tsv)) {
        let tabs = line.iter().filter(|&&c| c == 0x09).count() as i32;
        let cols = tabs + 1; // string.Split('\t').Length
        if cols > max {
            max = cols;
        }
    }
    max
}

pub fn line_count(text: &str) -> i32 {
    if text.is_empty() {
        return 0;
    }
    split_crlf(&to_units(text)).len() as i32
}

// ---- 单元测试 ----
#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{OcrResult, OcrWord};

    fn word(text: &str, x: f32, y: f32, w: f32, h: f32) -> OcrWord {
        OcrWord {
            text: text.to_string(),
            x,
            y,
            w,
            h,
        }
    }

    #[test]
    fn thousands_separator_exception() {
        assert_eq!(finalize_text("1,234"), "1,234");
        assert_eq!(finalize_text("1,234.56"), "1,234.56");
        assert_eq!(finalize_text("1,5"), "1.5");
        assert_eq!(finalize_text("3,14"), "3.14");
        assert_eq!(finalize_text("12,3456"), "12.3456");
    }

    #[test]
    fn drive_path_repair() {
        assert_eq!(finalize_text("C://Users//Ryan"), "C:\\Users\\Ryan");
        assert_eq!(finalize_text("C:/Users\\Ryan"), "C:\\Users\\Ryan");
        assert_eq!(finalize_text("  C://x//y"), "  C:\\x\\y");
        // URL 与散文不受影响
        assert_eq!(
            finalize_text("http://example.com/a//b"),
            "http://example.com/a//b"
        );
        assert_eq!(finalize_text("说明:内容"), "说明:内容");
    }

    #[test]
    fn cjk_space_drop_and_normalize() {
        // U+3000 先被 Normalize 成普通空格，再被 DropCjkSpaces 在两个汉字间删除。
        let n = normalize(&to_units("中\u{3000}文"), false);
        assert_eq!(from_units(&drop_cjk_spaces(&n)), "中文");
        assert_eq!(from_units(&drop_cjk_spaces(&to_units("中 文"))), "中文");
        // 空格碰到 Latin 时保留
        assert_eq!(from_units(&drop_cjk_spaces(&to_units("A 文"))), "A 文");
        assert_eq!(from_units(&drop_cjk_spaces(&to_units("中 A"))), "中 A");
        assert_eq!(
            from_units(&normalize(&to_units("\u{FF0E}\u{FF05}\u{FF0D}\u{3000}"), false)),
            ".%- "
        );
    }

    #[test]
    fn crlf_line_count_and_plain_rows() {
        assert_eq!(line_count(""), 0);
        assert_eq!(line_count("一行"), 1);
        assert_eq!(line_count("a\nb"), 1); // 裸 \n 不切分
        assert_eq!(line_count("a\r\nb"), 2);
        assert_eq!(line_count("a\r\nb\r\nc"), 3);
        let r = OcrResult {
            words: vec![
                word("上", 0.0, 0.0, 20.0, 20.0),
                word("下", 0.0, 100.0, 20.0, 20.0),
            ],
            ..Default::default()
        };
        let plain = to_plain_text(&r);
        assert_eq!(plain, "上\r\n下"); // 行连接必须是 CRLF
        assert_eq!(line_count(&plain), 2);
    }

    #[test]
    fn tsv_columns() {
        assert_eq!(column_count(""), 0);
        assert_eq!(column_count("a\tb\tc"), 3);
        assert_eq!(column_count("a\tb\r\nc"), 2);
        let r = OcrResult {
            words: vec![
                word("A1", 0.0, 0.0, 40.0, 20.0),
                word("B1", 200.0, 0.0, 40.0, 20.0),
                word("A2", 0.0, 100.0, 40.0, 20.0),
                word("B2", 200.0, 100.0, 40.0, 20.0),
            ],
            ..Default::default()
        };
        let tsv = to_tsv(&r);
        assert_eq!(tsv, "A1\tB1\r\nA2\tB2");
        assert_eq!(column_count(&tsv), 2);
    }

    /// The column separator is measured in glyph heights. A table whose cells
    /// are long labels used to widen the threshold past the gap between
    /// columns (median word width * 1.15) and collapse every row into one
    /// cell; a 20px glyph height must split at a 14px gap regardless of how
    /// wide the words are.
    #[test]
    fn table_columns_split_by_glyph_height() {
        let row = vec![
            word("办公用品", 10.0, 0.0, 300.0, 20.0),
            word("12", 324.0, 0.0, 40.0, 20.0),
            word("1,160.17", 378.0, 0.0, 200.0, 20.0),
            word("3,480.50", 592.0, 0.0, 200.0, 20.0),
        ];
        let (rows, median_h, median_w) = group_rows(&row);
        assert_eq!(rows.len(), 1);
        let cells = split_cells(&rows[0], median_h, median_w, true);
        assert_eq!(cells.len(), 4);
        assert_eq!(cells[2].text, "1,160.17");
        assert_eq!(cells[3].text, "3,480.50");
    }

    /// Words of one sentence sit far closer than a glyph height, so they must
    /// stay in a single cell (and therefore in one column).
    #[test]
    fn prose_row_stays_one_cell() {
        let row = vec![
            word("The", 0.0, 0.0, 90.0, 40.0),
            word("quick", 105.0, 0.0, 140.0, 40.0),
            word("brown", 258.0, 0.0, 160.0, 40.0),
        ];
        let (rows, median_h, median_w) = group_rows(&row);
        let cells = split_cells(&rows[0], median_h, median_w, true);
        assert_eq!(cells.len(), 1);
        assert_eq!(cells[0].text, "The quick brown");
    }
}

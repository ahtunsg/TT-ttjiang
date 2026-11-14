// config.json beside the exe. Same keys, defaults and UTF-8-no-BOM contract
// as the C# build so an existing install keeps working after the swap.
//
// 4.3: show_toast defaults to false (C# / 4.2 used true). An explicit value
// in an existing config.json still wins, so upgrading an old install keeps
// whatever it had until that key is set to false or removed.

use std::collections::HashMap;

use crate::win;

#[derive(Clone, Debug)]
pub struct Config {
    pub hotkey: String,
    pub save_dir: String,
    pub ocr_language: String,
    pub image_format: String,
    pub ocr_engine: String,
    pub show_toast: bool,
    pub recent_language: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            hotkey: "Ctrl+1".into(),
            save_dir: String::new(),
            ocr_language: String::new(),
            image_format: "png".into(),
            ocr_engine: "auto".into(),
            // 默认不显示「文字已复制到剪贴板（N 行）」这类操作提示。
            show_toast: false,
            recent_language: String::new(),
        }
    }
}

impl Config {
    pub fn path() -> String {
        format!("{}\\config.json", win::exe_dir())
    }

    pub fn load() -> Self {
        let mut c = Config::default();
        let text = match std::fs::read_to_string(Config::path()) {
            Ok(t) => t,
            Err(_) => return c,
        };
        let map = parse_object(&text);
        if let Some(v) = map.get("hotkey").and_then(as_str) { c.hotkey = v; }
        if let Some(v) = map.get("save_dir").and_then(as_str) { c.save_dir = v; }
        if let Some(v) = map.get("ocr_language").and_then(as_str) { c.ocr_language = v; }
        if let Some(v) = map.get("image_format").and_then(as_str) { c.image_format = v; }
        if let Some(v) = map.get("ocr_engine").and_then(as_str) { c.ocr_engine = v; }
        if let Some(v) = map.get("recent_language").and_then(as_str) { c.recent_language = v; }
        if let Some(v) = map.get("show_toast").and_then(as_bool) { c.show_toast = v; }
        c
    }

    pub fn save(&self) {
        let s = format!(
            "{{\n  \"hotkey\": \"{}\",\n  \"save_dir\": \"{}\",\n  \"ocr_language\": \"{}\",\n  \"image_format\": \"{}\",\n  \"ocr_engine\": \"{}\",\n  \"show_toast\": {},\n  \"recent_language\": \"{}\"\n}}\n",
            esc(&self.hotkey), esc(&self.save_dir), esc(&self.ocr_language),
            esc(&self.image_format), esc(&self.ocr_engine),
            if self.show_toast { "true" } else { "false" },
            esc(&self.recent_language),
        );
        let path = Config::path();
        let tmp = format!("{}.tmp", path);
        if std::fs::write(&tmp, s.as_bytes()).is_ok() {
            let _ = std::fs::remove_file(&path);
            if std::fs::rename(&tmp, &path).is_err() {
                let _ = std::fs::write(&path, s.as_bytes());
            }
        }
    }

    pub fn effective_save_dir(&self) -> String {
        if !self.save_dir.is_empty() {
            return self.save_dir.clone();
        }
        let home = win::env("USERPROFILE");
        if home.is_empty() { ".\\Pictures\\TT酱".into() } else { format!("{}\\Pictures\\TT酱", home) }
    }

    /// HotkeyText for the tray menu.
    pub fn hotkey_text(&self) -> String {
        if self.hotkey.is_empty() { "Ctrl+1".into() } else { self.hotkey.clone() }
    }

    /// (modifiers, virtual key). Same parsing rules as ScreenshotService.ParseHotkey.
    pub fn parse_hotkey(&self) -> (u32, u32) {
        let mut mods = 0u32;
        let mut vk = win::VK_1;
        for part in self.hotkey.split('+') {
            let p = part.trim().to_ascii_lowercase();
            match p.as_str() {
                "ctrl" | "control" => mods |= win::MOD_CONTROL,
                "alt" => mods |= win::MOD_ALT,
                "shift" => mods |= win::MOD_SHIFT,
                "win" => mods |= win::MOD_WIN,
                "" => {}
                _ => vk = key_to_vk(part.trim()),
            }
        }
        if mods == 0 { mods = win::MOD_CONTROL; }
        (mods, vk)
    }
}

fn key_to_vk(k: &str) -> u32 {
    let u = k.to_ascii_uppercase();
    let chars: Vec<char> = u.chars().collect();
    if chars.len() == 1 && chars[0].is_ascii_alphanumeric() {
        return chars[0] as u32;
    }
    if u.starts_with('F') {
        if let Ok(n) = u[1..].parse::<u32>() {
            if (1..=24).contains(&n) { return win::VK_F1 + n - 1; }
        }
    }
    win::VK_1
}

fn esc(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

// ---- minimal JSON object reader (top-level string/bool/number/null) ------
#[derive(Clone, Debug)]
enum JVal { S(String), B(bool), N(f64), Null }

fn as_str(v: &JVal) -> Option<String> {
    if let JVal::S(s) = v { Some(s.clone()) } else { None }
}
fn as_bool(v: &JVal) -> Option<bool> {
    if let JVal::B(b) = v { Some(*b) } else { None }
}

struct Parser<'a> { b: &'a [u8], i: usize }
impl<'a> Parser<'a> {
    fn ws(&mut self) {
        while self.i < self.b.len() && self.b[self.i].is_ascii_whitespace() { self.i += 1; }
    }
    fn string(&mut self) -> Option<String> {
        if self.i >= self.b.len() || self.b[self.i] != b'"' { return None; }
        self.i += 1;
        let mut out = String::new();
        while self.i < self.b.len() {
            let c = self.b[self.i];
            self.i += 1;
            match c {
                b'"' => return Some(out),
                b'\\' => {
                    if self.i >= self.b.len() { return None; }
                    let e = self.b[self.i];
                    self.i += 1;
                    match e {
                        b'"' => out.push('"'), b'\\' => out.push('\\'), b'/' => out.push('/'),
                        b'n' => out.push('\n'), b'r' => out.push('\r'), b't' => out.push('\t'),
                        b'b' => out.push('\u{0008}'), b'f' => out.push('\u{000C}'),
                        b'u' => {
                            if self.i + 4 > self.b.len() { return None; }
                            let hex = std::str::from_utf8(&self.b[self.i..self.i + 4]).ok()?;
                            let cp = u32::from_str_radix(hex, 16).ok()?;
                            self.i += 4;
                            if let Some(ch) = char::from_u32(cp) { out.push(ch); }
                        }
                        _ => out.push(e as char),
                    }
                }
                _ => {
                    // UTF-8 continuation handled by collecting bytes then decoding.
                    let start = self.i - 1;
                    let mut end = start + 1;
                    while end < self.b.len() && (self.b[end] & 0xC0) == 0x80 { end += 1; }
                    if let Ok(s) = std::str::from_utf8(&self.b[start..end]) { out.push_str(s); }
                    self.i = end;
                }
            }
        }
        Some(out)
    }
    fn value(&mut self) -> Option<JVal> {
        self.ws();
        if self.i >= self.b.len() { return None; }
        match self.b[self.i] {
            b'"' => self.string().map(JVal::S),
            b't' if self.b[self.i..].starts_with(b"true") => { self.i += 4; Some(JVal::B(true)) }
            b'f' if self.b[self.i..].starts_with(b"false") => { self.i += 5; Some(JVal::B(false)) }
            b'n' if self.b[self.i..].starts_with(b"null") => { self.i += 4; Some(JVal::Null) }
            b'{' | b'[' => {
                // Skip nested containers (not used by config.json).
                let open = self.b[self.i];
                let close = if open == b'{' { b'}' } else { b']' };
                let mut depth = 0i32;
                while self.i < self.b.len() {
                    let c = self.b[self.i];
                    self.i += 1;
                    if c == open { depth += 1; }
                    if c == close { depth -= 1; if depth == 0 { break; } }
                    if c == b'"' { self.i -= 1; let _ = self.string(); }
                }
                Some(JVal::Null)
            }
            _ => {
                let start = self.i;
                while self.i < self.b.len() && !matches!(self.b[self.i], b',' | b'}' | b']' | b' ' | b'\n' | b'\r' | b'\t') {
                    self.i += 1;
                }
                let s = std::str::from_utf8(&self.b[start..self.i]).ok()?;
                Some(JVal::N(s.parse().unwrap_or(0.0)))
            }
        }
    }
}

fn parse_object(text: &str) -> HashMap<String, JVal> {
    let mut out = HashMap::new();
    let mut p = Parser { b: text.as_bytes(), i: 0 };
    p.ws();
    if p.i >= p.b.len() || p.b[p.i] != b'{' { return out; }
    p.i += 1;
    loop {
        p.ws();
        if p.i >= p.b.len() || p.b[p.i] == b'}' { break; }
        let key = match p.string() { Some(k) => k, None => break };
        p.ws();
        if p.i >= p.b.len() || p.b[p.i] != b':' { break; }
        p.i += 1;
        let val = match p.value() { Some(v) => v, None => break };
        out.insert(key, val);
        p.ws();
        if p.i < p.b.len() && p.b[p.i] == b',' { p.i += 1; } else { break; }
    }
    out
}

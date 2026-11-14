// shortcuts.json / import-export, ported from 3.3's ConfigStore.cs + ProfileIO.cs.
//
// Format is kept byte-compatible in structure: lowerCamelCase keys, 2-space
// indent, UTF-8 without BOM, "shortcuts" before "settings", trailing newline.
// A tiny recursive JSON reader replaces 3.3's JsonLite so imports of both
// {"shortcuts":[...]} and a bare array work, and settings survive an import.

use std::collections::BTreeMap;

use crate::everything::SearchOptions;
use crate::win;

#[derive(Clone)]
pub struct Shortcut {
    pub name: String,
    pub path: String,
    pub args: String,
    /// Launcher grid slot, or -1 to let the layout fill the first free one.
    /// Stored so a dragged tile stays where it was dropped, including when
    /// that leaves a gap in the grid.
    pub pos: i32,
    /// Optional picture file for the tile, overriding whatever the target
    /// carries. Mainly here for web links, whose site icon the launcher cannot
    /// read from a browser cache; a local target left empty keeps its own icon.
    pub icon: String,
}

impl Default for Shortcut {
    fn default() -> Self {
        // pos must default to -1, not 0: 0 is a real slot.
        Shortcut { name: String::new(), path: String::new(), args: String::new(),
                   pos: -1, icon: String::new() }
    }
}

impl Shortcut {
    pub fn display_name(&self) -> String {
        if !self.name.trim().is_empty() { return self.name.clone(); }
        let p = self.path.trim_end_matches('\\').trim_end_matches('/');
        let base = p.rsplit(|c| c == '\\' || c == '/').next().unwrap_or("");
        if base.is_empty() { "(未命名)".into() } else { base.into() }
    }
}

#[derive(Clone)]
pub struct Settings {
    pub everything_path: String,
    pub search_result_limit: i32,
    pub search_sort_key: i32,
    pub search_sort_ascending: i32,
    pub search_match_full_path: i32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            everything_path: String::new(),
            search_result_limit: 30,
            search_sort_key: 0,
            search_sort_ascending: 1,
            search_match_full_path: 0,
        }
    }
}

impl Settings {
    pub fn options(&self) -> SearchOptions {
        SearchOptions {
            sort_key: if (0..=2).contains(&self.search_sort_key) { self.search_sort_key } else { 0 },
            ascending: self.search_sort_ascending != 0,
            match_full_path: self.search_match_full_path != 0,
        }
    }
    pub fn normalized(mut self) -> Self {
        if !(0..=2).contains(&self.search_sort_key) { self.search_sort_key = 0; }
        if self.search_sort_ascending != 0 { self.search_sort_ascending = 1; }
        if self.search_match_full_path != 0 { self.search_match_full_path = 1; }
        self
    }
}

pub fn config_path() -> String {
    format!("{}\\shortcuts.json", win::exe_dir())
}

pub fn default_shortcuts() -> Vec<Shortcut> {
    vec![
        Shortcut { name: "计算器".into(), path: "calc.exe".into(), args: String::new(),
                   pos: 0, icon: String::new() },
        Shortcut { name: "画图".into(), path: "mspaint.exe".into(), args: String::new(),
                   pos: 1, icon: String::new() },
    ]
}

// ---- minimal JSON ---------------------------------------------------------

#[derive(Clone, Debug)]
enum J {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<J>),
    Obj(BTreeMap<String, J>),
}

struct Parser<'a> { b: &'a [u8], i: usize }

impl<'a> Parser<'a> {
    fn ws(&mut self) { while self.i < self.b.len() && self.b[self.i].is_ascii_whitespace() { self.i += 1; } }

    fn value(&mut self) -> Option<J> {
        self.ws();
        if self.i >= self.b.len() { return None; }
        match self.b[self.i] {
            b'{' => self.object(),
            b'[' => self.array(),
            b'"' => self.string().map(J::Str),
            b't' => { self.i += 4; Some(J::Bool(true)) }
            b'f' => { self.i += 5; Some(J::Bool(false)) }
            b'n' => { self.i += 4; Some(J::Null) }
            _ => self.number(),
        }
    }

    fn object(&mut self) -> Option<J> {
        self.i += 1;
        let mut map = BTreeMap::new();
        loop {
            self.ws();
            if self.i >= self.b.len() { return None; }
            if self.b[self.i] == b'}' { self.i += 1; return Some(J::Obj(map)); }
            if self.b[self.i] != b'"' { self.i += 1; continue; }
            let key = self.string()?;
            self.ws();
            if self.i >= self.b.len() || self.b[self.i] != b':' { return None; }
            self.i += 1;
            let v = self.value()?;
            map.insert(key, v);
            self.ws();
            if self.i < self.b.len() && self.b[self.i] == b',' { self.i += 1; }
        }
    }

    fn array(&mut self) -> Option<J> {
        self.i += 1;
        let mut out = Vec::new();
        loop {
            self.ws();
            if self.i >= self.b.len() { return None; }
            if self.b[self.i] == b']' { self.i += 1; return Some(J::Arr(out)); }
            out.push(self.value()?);
            self.ws();
            if self.i < self.b.len() && self.b[self.i] == b',' { self.i += 1; }
        }
    }

    fn string(&mut self) -> Option<String> {
        if self.i >= self.b.len() || self.b[self.i] != b'"' { return None; }
        self.i += 1;
        let mut out = String::new();
        while self.i < self.b.len() {
            let c = self.b[self.i];
            if c == b'"' { self.i += 1; return Some(out); }
            if c == b'\\' && self.i + 1 < self.b.len() {
                self.i += 1;
                match self.b[self.i] {
                    b'n' => out.push('\n'), b'r' => out.push('\r'), b't' => out.push('\t'),
                    b'b' => out.push('\u{8}'), b'f' => out.push('\u{c}'),
                    b'u' if self.i + 4 < self.b.len() => {
                        if let Ok(s) = std::str::from_utf8(&self.b[self.i + 1..self.i + 5]) {
                            if let Ok(cp) = u32::from_str_radix(s, 16) {
                                if let Some(ch) = char::from_u32(cp) { out.push(ch); }
                            }
                        }
                        self.i += 4;
                    }
                    x => out.push(x as char),
                }
                self.i += 1;
                continue;
            }
            let s = self.i;
            let mut e = self.i + 1;
            while e < self.b.len() && (self.b[e] & 0xC0) == 0x80 { e += 1; }
            if let Ok(v) = std::str::from_utf8(&self.b[s..e]) { out.push_str(v); }
            self.i = e;
        }
        None
    }

    fn number(&mut self) -> Option<J> {
        let s = self.i;
        while self.i < self.b.len()
            && matches!(self.b[self.i], b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E')
        { self.i += 1; }
        if self.i == s { self.i += 1; return Some(J::Null); }
        let t = std::str::from_utf8(&self.b[s..self.i]).ok()?;
        Some(J::Num(t.parse().unwrap_or(0.0)))
    }
}

fn parse(text: &str) -> Option<J> {
    let body = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut p = Parser { b: body.as_bytes(), i: 0 };
    p.value()
}

fn as_str(j: Option<&J>) -> String {
    match j {
        Some(J::Str(s)) => s.clone(),
        Some(J::Num(n)) => format!("{}", *n as i64),
        _ => String::new(),
    }
}

fn as_int(j: Option<&J>, fallback: i32) -> i32 {
    match j {
        Some(J::Num(n)) => *n as i32,
        Some(J::Str(s)) => s.parse().unwrap_or(fallback),
        _ => fallback,
    }
}

fn shortcut_from(j: &J) -> Option<Shortcut> {
    let J::Obj(m) = j else { return None; };
    let path = as_str(m.get("path")).trim().to_string();
    if path.is_empty() { return None; }
    Some(Shortcut {
        name: as_str(m.get("name")).trim().to_string(),
        path,
        args: as_str(m.get("args")).trim().to_string(),
        // "category" from 4.1 and earlier is simply not read any more: the
        // launcher never used it for grouping, sorting or searching, so old
        // files keep loading unchanged and the field disappears on next save.
        // Absent in pre-4.1 files and in hand-written imports: -1 = auto.
        pos: as_int(m.get("pos"), -1),
        icon: as_str(m.get("icon")).trim().to_string(),
    })
}

/// Pure parser, exposed for --selftest so config round-trips can be checked
/// without touching the real shortcuts.json.
pub fn parse_config(text: &str) -> Option<(Vec<Shortcut>, Settings)> {
    let root = parse(text)?;
    let mut items = Vec::new();
    let mut settings = Settings::default();
    // An empty list is only a "first run" when the file does not carry a
    // shortcuts array at all. A user who deletes every tile saves `[]`, and
    // that has to stay empty across restarts instead of growing the two
    // sample entries back.
    let mut saw_list = false;
    match &root {
        J::Obj(m) => {
            if let Some(J::Arr(a)) = m.get("shortcuts") {
                saw_list = true;
                for x in a { if let Some(s) = shortcut_from(x) { items.push(s); } }
            }
            if let Some(J::Obj(sm)) = m.get("settings") {
                settings.everything_path = as_str(sm.get("everythingPath"));
                settings.search_result_limit = as_int(sm.get("searchResultLimit"), 30);
                settings.search_sort_key = as_int(sm.get("searchSortKey"), 0);
                settings.search_sort_ascending = as_int(sm.get("searchSortAscending"), 1);
                settings.search_match_full_path = as_int(sm.get("searchMatchFullPath"), 0);
                settings = settings.normalized();
            }
        }
        J::Arr(a) => {
            saw_list = true;
            for x in a { if let Some(s) = shortcut_from(x) { items.push(s); } }
        }
        _ => {}
    }
    if !saw_list { items = default_shortcuts(); }
    Some((items, settings))
}

pub fn load_all() -> (Vec<Shortcut>, Settings) {
    let path = config_path();
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => {
            let items = default_shortcuts();
            let st = Settings::default();
            let _ = save_all(&items, &st);
            return (items, st);
        }
    };
    parse_config(&text).unwrap_or_else(|| (default_shortcuts(), Settings::default()))
}

// ---- writing --------------------------------------------------------------

pub fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
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
    out.push('"');
    out
}

fn shortcut_json(s: &Shortcut, indent: &str) -> String {
    format!(
        "{}{{\n{}  \"name\": {},\n{}  \"path\": {},\n{}  \"args\": {},\n{}  \"pos\": {},\n{}  \"icon\": {}\n{}}}",
        indent, indent, quote(&s.name), indent, quote(&s.path),
        indent, quote(&s.args), indent, s.pos,
        indent, quote(&s.icon), indent
    )
}

fn array_json(items: &[Shortcut], indent: &str) -> String {
    if items.is_empty() { return format!("{}[]", indent); }
    let mut out = format!("{}[\n", indent);
    for (i, s) in items.iter().enumerate() {
        out.push_str(&shortcut_json(s, &format!("{}  ", indent)));
        if i + 1 < items.len() { out.push(','); }
        out.push('\n');
    }
    out.push_str(indent);
    out.push(']');
    out
}

pub fn build_config(items: &[Shortcut], st: &Settings) -> String {
    let st = st.clone().normalized();
    format!(
        "{{\n  \"shortcuts\": {},\n  \"settings\": {{\n    \"everythingPath\": {},\n    \"searchResultLimit\": {},\n    \"searchSortKey\": {},\n    \"searchSortAscending\": {},\n    \"searchMatchFullPath\": {}\n  }}\n}}\n",
        array_json(items, "  "),
        quote(&st.everything_path), st.search_result_limit, st.search_sort_key,
        st.search_sort_ascending, st.search_match_full_path
    )
}

pub fn save_all(items: &[Shortcut], st: &Settings) -> Result<(), String> {
    let path = config_path();
    let tmp = format!("{}.tmp", path);
    let body = build_config(items, st);
    std::fs::write(&tmp, body.as_bytes()).map_err(|e| e.to_string())?;
    if std::fs::rename(&tmp, &path).is_ok() {
        return Ok(());
    }
    // The replace failed, normally because the old file is momentarily locked
    // (a virus scanner holding a handle) or marked read-only. Keep a copy
    // before touching it: deleting the live config and then failing to move the
    // new one in would leave the user with nothing to load on next start.
    let bak = format!("{}.bak", path);
    let had_old = std::fs::copy(&path, &bak).is_ok();
    let _ = std::fs::remove_file(&path);
    match std::fs::rename(&tmp, &path) {
        Ok(_) => {
            if had_old { let _ = std::fs::remove_file(&bak); }
            Ok(())
        }
        Err(e) => {
            if had_old { let _ = std::fs::rename(&bak, &path); }
            let _ = std::fs::remove_file(&tmp);
            Err(e.to_string())
        }
    }
}

#[link(name = "kernel32")]
extern "system" {
    fn GetLocalTime(t: *mut SYSTEMTIME);
}
#[repr(C)]
struct SYSTEMTIME {
    year: u16, month: u16, day_of_week: u16, day: u16,
    hour: u16, minute: u16, second: u16, milliseconds: u16,
}

fn date_stamp() -> String {
    unsafe {
        let mut t: SYSTEMTIME = std::mem::zeroed();
        GetLocalTime(&mut t);
        format!("{:04}{:02}{:02}", t.year, t.month, t.day)
    }
}

fn time_stamp() -> String {
    unsafe {
        let mut t: SYSTEMTIME = std::mem::zeroed();
        GetLocalTime(&mut t);
        format!("{:02}{:02}{:02}", t.hour, t.minute, t.second)
    }
}

fn now_text() -> String {
    unsafe {
        let mut t: SYSTEMTIME = std::mem::zeroed();
        GetLocalTime(&mut t);
        format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            t.year, t.month, t.day, t.hour, t.minute, t.second)
    }
}

pub fn default_export_name() -> String { format!("快捷方式-{}.json", date_stamp()) }

pub fn export_to(path: &str, items: &[Shortcut]) -> Result<(), String> {
    let body = format!(
        "{{\n  \"format\": \"hc-portal-shortcuts\",\n  \"version\": 1,\n  \"exportedAt\": {},\n  \"shortcuts\": {}\n}}\n",
        quote(&now_text()), array_json(items, "  ")
    );
    std::fs::write(path, body.as_bytes()).map_err(|e| e.to_string())
}

pub const MAX_IMPORT_COUNT: usize = 500;

pub fn import_from(path: &str) -> Result<Vec<Shortcut>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let root = parse(&text).ok_or_else(|| "这个文件不是有效的 JSON。".to_string())?;
    let arr: &Vec<J> = match &root {
        J::Obj(m) => match m.get("shortcuts") {
            Some(J::Arr(a)) => a,
            _ => return Err("这个文件里没有可用的快捷方式（每条都需要 path）。".into()),
        },
        J::Arr(a) => a,
        _ => return Err("这个文件里没有可用的快捷方式（每条都需要 path）。".into()),
    };
    let mut out = Vec::new();
    for x in arr { if let Some(s) = shortcut_from(x) { out.push(s); } }
    if out.is_empty() { return Err("这个文件里没有可用的快捷方式（每条都需要 path）。".into()); }
    if out.len() > MAX_IMPORT_COUNT { return Err("导入的快捷方式超过 500 条，已取消。".into()); }
    Ok(out)
}

pub fn backup() -> Option<String> {
    let src = config_path();
    let name = format!("{}\\shortcuts-{}-{}.bak", win::exe_dir(), date_stamp(), time_stamp());
    std::fs::copy(&src, &name).ok()?;
    Some(name)
}

// ---- web links -----------------------------------------------------------
//
// A shortcut whose path is an http(s) URL opens in the default browser. The
// path field carries the URL as typed, so shortcuts.json and the import/export
// format are unchanged: nothing new to parse, nothing old to migrate.
//
// Only http and https count as links. `file://`, a drive letter, a UNC share
// and a bare local path all stay on the shell path, because "open this local
// .html" is launching a file, not browsing the web — and the shell already
// knows which handler the user picked for it.

/// What a shortcut path means to the launcher.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Target {
    /// Program, document or shell verb: straight to ShellExecute.
    Local,
    /// http(s) URL: hand to the default browser.
    Web,
}

/// `https?://` (or a bare `www.` host) is a link; everything else is local.
/// Case-insensitive on the scheme, and only after trimming quotes/spaces —
/// the same trim `read()` applies, so the two callers cannot disagree.
pub fn target_of(path: &str) -> Target {
    if normalized_url(path).is_some() { Target::Web } else { Target::Local }
}

/// The URL to hand the browser: scheme normalised, `www.` kept, surrounding
/// quotes and spaces removed. `None` when the path is not a link.
pub fn normalized_url(path: &str) -> Option<String> {
    let s = path.trim().trim_matches('"').trim();
    if s.is_empty() || s.chars().any(|c| c == ' ' || c == '\t') {
        return None;
    }
    let lower = s.to_ascii_lowercase();
    for scheme in ["https://", "http://"] {
        if let Some(rest) = lower.strip_prefix(scheme) {
            // A scheme with nothing after it is a paste accident, not a link.
            return if rest.is_empty() { None } else { Some(s.to_string()) };
        }
    }
    // Bare `www.example.com`: accept it and add the scheme, since that is what
    // people paste. A bare `example.com` is left alone — too close to a file
    // or folder name to promote silently.
    if lower.starts_with("www.") {
        let host = host_of(&lower);
        if host.contains('.') && !host.contains('\\') && !host.contains('/') {
            return Some(format!("https://{}", s));
        }
    }
    None
}

/// Host of a URL, without port, path or credentials.
///
/// IPv6 needs the bracket check first: `https://[fe80::1]:8080/` has colons
/// inside the host, so stripping at the first `:` would cut it to `[fe80`.
fn host_of(url: &str) -> String {
    let mut rest = url;
    if let Some(i) = rest.find("://") {
        rest = &rest[i + 3..];
    }
    rest = rest.split(['/', '?', '#']).next().unwrap_or("");
    if rest.starts_with('[') {
        // bracketed literal: ends at ']', zone id and port are part of it
        let end = rest.find(']').unwrap_or(rest.len());
        return rest[..end + 1].to_string();
    }
    if let Some(at) = rest.rfind('@') {
        rest = &rest[at + 1..]; // strip user:pass@ (an @ in a path is gone already)
    }
    let host = rest.split(':').next().unwrap_or("");
    if host.contains('.') { host.to_string() } else { String::new() }
}

/// The registered domain a browser caches its site icons under: `www.baidu.com`
/// and `tieba.baidu.com` both give `baidu.com`, because a site ships its icon
/// for the registrable domain, not per subdomain.
///
/// Two-level public suffixes that actually carry user content in China are
/// handled explicitly (`com.cn`, `net.cn`, …); an unknown `a.b.c` falls back to
/// the last label, which is the best a dependency-free resolver can do.
pub fn registrable_domain(host: &str) -> String {
    const TWO_LEVEL: [&str; 11] = ["com.cn", "net.cn", "org.cn", "gov.cn", "edu.cn",
        "com.hk", "com.mo", "com.tw", "co.uk", "co.jp", "com.br"];
    let labels: Vec<&str> = host.split('.').filter(|l| !l.is_empty()).collect();
    if labels.len() < 2 { return String::new(); }
    if labels.len() >= 3 {
        let tail = format!("{}.{}", labels[labels.len() - 2], labels[labels.len() - 1]);
        if TWO_LEVEL.contains(&tail.as_str()) {
            let i = labels.len() - 3;
            return labels[i..].join(".");
        }
    }
    labels[labels.len() - 2..].join(".")
}

/// The domain a link points at, ready to show in a result row.
pub fn domain_of(path: &str) -> String {
    let url = normalized_url(path).unwrap_or_default();
    registrable_domain(&host_of(&url))
}

/// Letter shown on the tile when a link has no icon. An English-only name
/// would turn `www.baidu.com` and `www.bilibili.com` into the same glyph if
/// the first character of the *name* were used, so the domain decides.
pub fn link_initial(path: &str, name: &str) -> String {
    if name.chars().any(|c| c as u32 >= 0x2E80) {
        return name.chars().next().map(|c| c.to_string()).unwrap_or_else(|| "?".into());
    }
    let host = host_of(&normalized_url(path).unwrap_or_default());
    let host = host.strip_prefix("www.").unwrap_or(&host);
    host.chars().next()
        .map(|c| c.to_uppercase().to_string())
        .or_else(|| name.chars().next().map(|c| c.to_string()))
        .unwrap_or_else(|| "?".into())
}

/// Lowercase hex MD5.
///
/// Not used for tile icons. The idea was to read a browser's cached site icon
/// by its key, but a probe of real profile directories found no such layout:
/// Chromium keeps favicons only inside the `Favicons` SQLite database and
/// Firefox inside `favicons.sqlite`, and there is no per-domain PNG folder to
/// read. Parsing a browser's live database needs either a third-party engine or
/// unsafe byte-scanning of a file the browser may hold locked, and fetching the
/// icon needs the network. The launcher does neither, so a link shows the
/// domain's initial letter and can point at its own icon file instead.
///
/// Kept because it is small, dependency-free and pinned to RFC 1321 vectors,
/// and any future content-addressed cache needs exactly this.
#[allow(dead_code)]
fn md5_hex(data: &[u8]) -> String {
    // per-round left rotates and the K table (floor(2^32 * |sin(i+1)|))
    const SHIFT: [u32; 64] = [7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22,
        5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20,
        4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23,
        6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21];
    const K: [u32; 64] = [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613, 0xfd469501,
        0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193, 0xa679438e, 0x49b40821,
        0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d, 0x02441453, 0xd8a1e681, 0xe7d3fbc8,
        0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed, 0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a,
        0xfffa3942, 0x8771f681, 0x6d9d6122, 0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70,
        0x289b7ec6, 0xeaa127fa, 0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665,
        0xf4292244, 0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
        0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb, 0xeb86d391];

    let mut msg = data.to_vec();
    let bits = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 { msg.push(0); }
    msg.extend_from_slice(&bits.to_le_bytes());

    let mut a0: u32 = 0x67452301;
    let mut b0: u32 = 0xefcdab89;
    let mut c0: u32 = 0x98badcfe;
    let mut d0: u32 = 0x10325476;

    for block in msg.chunks_exact(64) {
        let mut m = [0u32; 16];
        for (i, w) in m.iter_mut().enumerate() {
            *w = u32::from_le_bytes([block[i * 4], block[i * 4 + 1], block[i * 4 + 2], block[i * 4 + 3]]);
        }
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (f, g) = match i {
                0..=15 => ((b & c) | ((!b) & d), i),
                16..=31 => ((d & b) | ((!d) & c), (5 * i + 1) % 16),
                32..=47 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | (!d)), (7 * i) % 16),
            };
            // RFC 1321 FF/GG/HH/II: the destination register takes
            // b + rotl(seed + a + K[i] + M[g], s), and the four registers then
            // rotate right (a<-d, d<-c, c<-b, b<-new). The other rotation
            // direction silently loses `a` after one step, so the direction here
            // is what the reference vectors below exist to pin down.
            let f = f
                .wrapping_add(a)
                .wrapping_add(K[i])
                .wrapping_add(m[g])
                .rotate_left(SHIFT[i])
                .wrapping_add(b);
            a = d;
            d = c;
            c = b;
            b = f;
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }
    // The digest is the four state words written as little-endian *bytes*, then
    // hex-encoded. Formatting each word with `{:08x}` byte-reverses every word
    // and yields a wrong-but-plausible hash.
    let mut out = String::with_capacity(32);
    for w in [a0, b0, c0, d0] {
        for byte in w.to_le_bytes() {
            out.push_str(&format!("{:02x}", byte));
        }
    }
    out
}

#[cfg(test)]
mod link_tests {
    use super::*;

    #[test]
    fn only_http_and_bare_www_count_as_links() {
        assert_eq!(target_of("https://www.baidu.com"), Target::Web);
        assert_eq!(target_of("http://example.com/a?b=1"), Target::Web);
        assert_eq!(target_of("  \"https://a.cn\" "), Target::Web);
        assert_eq!(target_of("www.baidu.com"), Target::Web);
        // local stays local: opening a file is the shell's job
        assert_eq!(target_of("file:///C:/docs/a.html"), Target::Local);
        assert_eq!(target_of("C:\\Tools\\app.exe"), Target::Local);
        assert_eq!(target_of("\\\\nas\\share\\x.txt"), Target::Local);
        assert_eq!(target_of("shell:Downloads"), Target::Local);
        assert_eq!(target_of("www"), Target::Local);
        assert_eq!(target_of("https://"), Target::Local);
        assert_eq!(target_of(""), Target::Local);
    }

    #[test]
    fn bare_www_gains_a_scheme_and_quotes_are_stripped() {
        assert_eq!(normalized_url("www.baidu.com").as_deref(),
            Some("https://www.baidu.com"));
        assert_eq!(normalized_url("\"HTTP://A.CN/x\"").as_deref(), Some("HTTP://A.CN/x"));
        assert_eq!(normalized_url("www.a b.txt"), None); // a name with a space is a file
    }

    #[test]
    fn icon_domain_is_the_registrable_one() {
        assert_eq!(registrable_domain("www.baidu.com"), "baidu.com");
        assert_eq!(registrable_domain("tieba.baidu.com"), "baidu.com");
        assert_eq!(registrable_domain("www.weather.com.cn"), "weather.com.cn");
        assert_eq!(registrable_domain("a.b.example.co.uk"), "example.co.uk");
        assert_eq!(registrable_domain("localhost"), "");
        assert_eq!(domain_of("https://tieba.baidu.com/f?k=1"), "baidu.com");
        assert_eq!(domain_of("C:\\x\\y.exe"), "");
    }

    #[test]
    fn host_parsing_survives_ports_and_credentials() {
        assert_eq!(host_of("https://a.example.com:8443/p"), "a.example.com");
        assert_eq!(host_of("https://user:pw@example.com/"), "example.com");
        assert_eq!(host_of("https://[::1]:3000/"), "[::1]");
        assert_eq!(host_of("not a url"), "");
        assert_eq!(host_of("https://[fe80::1%25eth0]:8080/x"), "[fe80::1%25eth0]");
    }

    #[test]
    fn link_initial_avoids_collisions_across_domains() {
        // the old first-letter-of-name rule turned every bare www. address into
        // a W; using the host instead separates them, and a CJK name still
        // reads as its own first character
        assert_eq!(link_initial("www.baidu.com", "www.baidu.com"), "B");
        assert_eq!(link_initial("www.bilibili.com", "www.bilibili.com"), "B");
        assert_eq!(link_initial("www.qq.com", "QQ 邮箱"), "Q");
        assert_eq!(link_initial("https://zhihu.com", "知乎"), "知");
    }

    /// Deleting every tile and restarting must not bring the two samples back:
    /// an empty `shortcuts` array is a saved state, not a missing file.
    #[test]
    fn an_empty_saved_list_stays_empty() {
        let (items, _) = parse_config("{\"shortcuts\": [], \"settings\": {}}").unwrap();
        assert!(items.is_empty());
        let (bare, _) = parse_config("[]").unwrap();
        assert!(bare.is_empty());
        // a file that carries no list at all is still a first run
        let (seeded, _) = parse_config("{\"settings\": {}}").unwrap();
        assert_eq!(seeded.len(), 2);
    }

    /// 4.1 and earlier stored a `category` per shortcut that nothing ever read:
    /// the grid is ordered by `pos`, and search only looks at name and path.
    /// Old files must keep loading, and the key must stop being written so it
    /// disappears on the next save instead of riding along through every
    /// export and import for another few versions.
    #[test]
    fn legacy_category_loads_but_is_never_written_again() {
        let legacy = "{\"shortcuts\": [{\"name\":\"计算器\",\"path\":\"calc.exe\",\
                      \"args\":\"--mode fast\",\"category\":\"工具\",\"pos\":0,\
                      \"icon\":\"\"}]}";
        let (items, _) = parse_config(legacy).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "计算器");
        assert_eq!(items[0].args, "--mode fast");
        assert_eq!(items[0].pos, 0);

        let out = build_config(&items, &Settings::default());
        assert!(!out.contains("category"), "json still writes the dead field: {out}");
        // Everything that does matter has to survive the lossy rewrite.
        let (again, _) = parse_config(&out).unwrap();
        assert_eq!(again.len(), 1);
        assert_eq!(again[0].path, "calc.exe");
        assert_eq!(again[0].args, "--mode fast");
        assert_eq!(again[0].pos, 0);
    }

    /// Reference vectors from RFC 1321 A.5, plus domain-length inputs that walk
    /// the block boundary. Cross-check against a trusted implementation before
    /// believing any change to the round function: the failure mode is a wrong
    /// hash that still looks like a hash.
    #[test]
    fn md5_matches_the_reference_vectors() {
        assert_eq!(md5_hex(b""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(md5_hex(b"a"), "0cc175b9c0f1b6a831c399e269772661");
        assert_eq!(md5_hex(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(md5_hex(b"message digest"), "f96b697d7cb7938d525a2f31aaf161d0");
        assert_eq!(md5_hex(b"abcdefghijklmnopqrstuvwxyz"),
            "c3fcd3d76192e4007dfb496cca67e13b");
        assert_eq!(md5_hex(b"baidu.com"), "60d2f4fe0275d790764f40abc6734499");
        assert_eq!(md5_hex(b"weather.com.cn"), "3b86cec414e562f1cc84d8426bb62759");
        // 55/56/64/65 bytes walk the length-field boundary of the 64-byte block
        assert_eq!(md5_hex(&[b'x'; 55]), "04364420e25c512fd958a70738aa8f72");
        assert_eq!(md5_hex(&[b'x'; 56]), "668a72d5ba17f08e62dabcafad6db14b");
        assert_eq!(md5_hex(&[b'x'; 64]), "c1bb4f81d892b2d57947682aeb252456");
        assert_eq!(md5_hex(&[b'x'; 65]), "1bc932052302d074bdec39795fe00cf6");
    }
}

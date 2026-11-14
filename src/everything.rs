// Built-in Everything search, ported at the behaviour level from 3.3's
// Services/EverythingHost.cs + EverythingSearch.cs.
//
// 3.3 ships Everything64.exe / es.exe / Everything3_x64.dll in an embedded
// zip and releases them into <exe>\everything\. The Rust build keeps the same
// layout but as plain files next to ocr\ (the user already accepted a
// multi-file package in 3.2). Querying goes through es.exe, which is exactly
// 3.3's documented fallback path; it returns full paths, one per line.
//
// No crates.io dependency: std::process + a handful of Win32 calls.

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::win;

const INSTANCE: &str = "TTJiang";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const MAX_RESULTS: usize = 200;

#[link(name = "kernel32")]
extern "system" {
    fn MultiByteToWideChar(cp: u32, flags: u32, src: *const i8, src_len: i32,
                           dst: *mut u16, dst_len: i32) -> i32;
    fn CreateFileW(name: *const u16, access: u32, share: u32, security: *mut c_void,
                   disposition: u32, flags: u32, template: *mut c_void) -> *mut c_void;
    fn CloseHandle(h: *mut c_void) -> i32;
}

#[derive(Clone, Copy)]
pub struct SearchOptions {
    /// 0 name, 1 type/extension, 2 modified time (matches AppSettings).
    pub sort_key: i32,
    pub ascending: bool,
    pub match_full_path: bool,
}

impl Default for SearchOptions {
    fn default() -> Self {
        SearchOptions { sort_key: 0, ascending: true, match_full_path: false }
    }
}

impl SearchOptions {
    fn es_sort(&self) -> &'static str {
        match (self.sort_key, self.ascending) {
            (1, true) => "extension-ascending",
            (1, false) => "extension-descending",
            (2, true) => "date-modified-ascending",
            (2, false) => "date-modified-descending",
            (_, true) => "name-ascending",
            (_, false) => "name-descending",
        }
    }
}

pub fn tools_dir() -> PathBuf {
    PathBuf::from(format!("{}\\everything", win::exe_dir()))
}

fn tool(name: &str) -> Option<PathBuf> {
    let p = tools_dir().join(name);
    if p.is_file() { Some(p) } else { None }
}

pub fn components_present() -> bool {
    tool("es.exe").is_some() && tool("Everything64.exe").is_some()
}

/// 3.3's EnsureStealthConfig: keep the private instance out of the taskbar
/// and tray. The ini lives next to the exe so no admin rights are needed.
fn ensure_stealth_config() -> PathBuf {
    let dir = tools_dir();
    let ini = dir.join("TTJiang.ini");
    let body = "run_in_background=1\r\nshow_in_taskbar=0\r\nshow_tray_icon=0\r\nminimize_to_tray=0\r\n";
    let _ = std::fs::write(&ini, body);
    ini
}

/// Named-pipe probe, same as EverythingHost.IsSearchReady().
pub fn is_client_running() -> bool {
    let name: Vec<u16> = format!("\\\\.\\pipe\\Everything IPC ({})\0", INSTANCE)
        .encode_utf16().collect();
    unsafe {
        let h = CreateFileW(name.as_ptr(), 0x8000_0000, 3, std::ptr::null_mut(), 3, 0, std::ptr::null_mut());
        // GENERIC_READ, OPEN_EXISTING. INVALID_HANDLE_VALUE == -1.
        if h as isize == -1 { return false; }
        CloseHandle(h);
        true
    }
}

/// Start the private Everything client. 3.3 uses
/// Everything64.exe -instance TTJiang -startup -config <ini> -db <db>.
/// A missing system index service still needs the one-time elevated install
/// from Settings; starting the client is enough once that exists.
pub fn start_client() -> Result<(), String> {
    if is_client_running() { return Ok(()); }
    let exe = tool("Everything64.exe").ok_or_else(|| "内置搜索组件缺失，请重新解压完整的 TT酱 压缩包".to_string())?;
    let dir = tools_dir();
    let ini = ensure_stealth_config();
    let db = dir.join("TTJiang.db");
    #[cfg(windows)]
    use std::os::windows::process::CommandExt;
    #[cfg(windows)]
    let mut cmd = {
        let mut c = Command::new(&exe);
        c.creation_flags(CREATE_NO_WINDOW);
        c
    };
    #[cfg(not(windows))]
    let mut cmd = Command::new(&exe);
    cmd.arg("-instance").arg(INSTANCE)
        .arg("-startup")
        .arg("-config").arg(&ini)
        .arg("-db").arg(&db);
    match cmd.spawn() {
        Ok(_) => Ok(()),
        Err(e) => Err(format!("内置搜索实例启动失败：{}", e)),
    }
}

/// Decode es.exe output. 3.3 tries UTF-8 first and falls back to GBK(936),
/// which is CP_ACP on a Chinese Windows install.
fn decode(bytes: &[u8]) -> String {
    if let Ok(s) = std::str::from_utf8(bytes) { return s.to_string(); }
    unsafe {
        let need = MultiByteToWideChar(0, 0, bytes.as_ptr() as *const i8, bytes.len() as i32,
            std::ptr::null_mut(), 0);
        if need <= 0 { return String::from_utf8_lossy(bytes).into_owned(); }
        let mut buf: Vec<u16> = vec![0u16; need as usize];
        MultiByteToWideChar(0, 0, bytes.as_ptr() as *const i8, bytes.len() as i32,
            buf.as_mut_ptr(), need);
        String::from_utf16_lossy(&buf)
    }
}

/// Split on whitespace but keep quoted phrases intact (3.3 SplitQuery).
fn split_query(q: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for ch in q.trim().chars() {
        match ch {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() { out.push(std::mem::take(&mut cur)); }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() { out.push(cur); }
    out
}

pub struct SearchOutcome {
    pub paths: Vec<String>,
    pub error: Option<String>,
}

/// Query the private instance through es.exe. Uses -viewport-count (NOT -n),
/// because ES 1.1.0.38 returns zero rows with -n + -viewport-offset.
pub fn query(text: &str, opts: SearchOptions) -> SearchOutcome {
    let words = split_query(text);
    if words.is_empty() {
        return SearchOutcome { paths: Vec::new(), error: None };
    }
    let es = match tool("es.exe") {
        Some(p) => p,
        None => return SearchOutcome {
            paths: Vec::new(),
            error: Some("内置搜索组件缺失，请重新解压完整的 TT酱 压缩包".into()),
        },
    };
    if !is_client_running() {
        if let Err(e) = start_client() {
            return SearchOutcome { paths: Vec::new(), error: Some(client_hint(&e)) };
        }
        // Give the private instance a moment to open its IPC pipe.
        let mut ready = false;
        for _ in 0..12 {
            if is_client_running() { ready = true; break; }
            std::thread::sleep(std::time::Duration::from_millis(120));
        }
        if !ready {
            return SearchOutcome {
                paths: Vec::new(),
                error: Some("内置搜索实例没有启动，请打开设置安装索引服务后重试。".into()),
            };
        }
    }

    #[cfg(windows)]
    use std::os::windows::process::CommandExt;
    #[cfg(windows)]
    let mut cmd = {
        let mut c = Command::new(&es);
        c.creation_flags(CREATE_NO_WINDOW);
        c
    };
    #[cfg(not(windows))]
    let mut cmd = Command::new(&es);
    cmd.arg("-instance").arg(INSTANCE)
        .arg("-viewport-count").arg(MAX_RESULTS.to_string())
        .arg("-viewport-offset").arg("0")
        .arg("-sort").arg(opts.es_sort());
    if opts.match_full_path { cmd.arg("-p"); }
    for w in &words { cmd.arg(w); }

    let output = match cmd.output() {
        Ok(o) => o,
        Err(e) => return SearchOutcome {
            paths: Vec::new(), error: Some(format!("搜索组件无法启动：{}", e)),
        },
    };
    let text_out = decode(&output.stdout);
    let mut paths = Vec::new();
    for line in text_out.lines() {
        let p = line.trim();
        if p.is_empty() { continue; }
        // es.exe with default options prints full paths, one per line.
        if Path::new(p).is_absolute() || p.starts_with("\\\\") {
            paths.push(p.to_string());
        }
    }
    if paths.is_empty() && !output.status.success() {
        let err = decode(&output.stderr);
        return SearchOutcome { paths, error: Some(map_error(&err)) };
    }
    SearchOutcome { paths, error: None }
}

fn client_hint(reason: &str) -> String {
    format!("内置搜索实例没有启动，请打开设置安装索引服务后重试。（{}）", reason)
}

fn map_error(raw: &str) -> String {
    let r = raw.trim();
    if r.contains("8") { return "搜索被拒绝：索引服务可能未安装或未就绪。".into(); }
    if r.is_empty() { return "搜索失败：内置搜索实例没有响应。".into(); }
    format!("搜索失败：{}", r.lines().next().unwrap_or("未知错误"))
}

/// One-time elevated service install, matching EverythingHost.InstallService.
/// Only called when the user clicks the button in Settings.
pub fn install_service() -> Result<(), String> {
    let exe = tool("Everything64.exe")
        .ok_or_else(|| "内置搜索组件缺失，请重新解压完整的 TT酱 压缩包".to_string())?;
    let args = format!("-instance {} -install-service", INSTANCE);
    let verb = win::wide("runas");
    let file = win::wide(&exe.to_string_lossy());
    let a = win::wide(&args);
    let r = unsafe {
        win::ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), file.as_ptr(),
            a.as_ptr(), std::ptr::null(), 0)
    };
    // ShellExecuteW returns >32 on success, an error code otherwise.
    if (r as isize) > 32 { Ok(()) } else { Err(format!("提权安装失败 code={}", r as isize)) }
}

/// Human-readable state shown by the Settings window.
pub fn service_state() -> String {
    if is_client_running() { return "索引服务：已就绪（内置实例正在运行）".into(); }
    if components_present() { return "索引服务：内置实例未运行，可在下方启动".into(); }
    "索引服务：组件缺失，请重新解压完整的 TT酱 压缩包".into()
}

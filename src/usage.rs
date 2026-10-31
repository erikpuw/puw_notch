//! Plan limits (5-hour and weekly) of the AI apps on this PC, read from what those apps already
//! keep locally, nothing is sent anywhere: Claude Code's status line (a one-line script in its
//! settings.json drops its JSON into %APPDATA%\PuwNotch), Codex's session logs, and Antigravity's
//! own language server on 127.0.0.1 (asked with the token on its command line, so only while the
//! app is open). A worker thread re-reads them every POLL; `Watch` turns the weekly numbers into
//! banner alerts.

use std::collections::HashMap;
use std::ffi::c_void;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use windows::core::{w, HSTRING, PCWSTR};
use windows::Data::Json::{JsonArray, JsonObject};
use windows::Wdk::System::Threading::{NtQueryInformationProcess, PROCESSINFOCLASS};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, UNICODE_STRING, WPARAM};
use windows::Win32::Networking::WinInet::*;
use windows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
use windows::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION};
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

use crate::i18n::tr;

/// Posted when a reading changes.
pub const WM_USAGE: u32 = WM_APP + 12;

/// How often the apps are read again.
const POLL: Duration = Duration::from_secs(60);
/// How much of a Codex session log's end is searched for the last rate-limit event.
const CODEX_TAIL: u64 = 512 * 1024;
/// Newest Codex logs tried before giving up.
const CODEX_FILES: usize = 6;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum App {
    Claude,
    Codex,
    Antigravity,
}

pub const APPS: [App; 3] = [App::Claude, App::Codex, App::Antigravity];

impl App {
    pub fn name(self) -> &'static str {
        match self {
            App::Claude => "Claude",
            App::Codex => "Codex",
            App::Antigravity => "Antigravity",
        }
    }

    /// What to do when there is nothing to show.
    pub fn open_hint(self) -> &'static str {
        match self {
            App::Claude => tr("Mở app Claude (hoặc Claude Code ở terminal) để tính", "Open the Claude app (or Claude Code in a terminal) to count"),
            App::Codex => tr("Mở Codex và dùng một lần để tính", "Open Codex and use it once to count"),
            App::Antigravity => tr("Mở Antigravity để tính", "Open Antigravity to count"),
        }
    }
}

/// One limit: how much of it is used and when it starts over.
#[derive(Clone, PartialEq, Debug)]
pub struct Bar {
    pub label: String,
    pub weekly: bool,
    /// 0..100.
    pub used: f32,
    /// Unix seconds.
    pub resets_at: i64,
}

/// What the three apps showed at the last read (None: nothing to read, the app is closed or has not counted yet).
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Snapshot {
    pub apps: [Option<Vec<Bar>>; 3],
    /// The plan of the account in use, when the app says ("free", "plus"...): each account has its own.
    pub plans: [Option<String>; 3],
}

impl Snapshot {
    pub fn plan(&self, app: App) -> Option<&str> {
        self.plans[app as usize].as_deref()
    }

    pub fn get(&self, app: App) -> Option<&[Bar]> {
        self.apps[app as usize].as_deref()
    }
}

pub fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// "3 d", "5 h", "20 min": how long until `at`.
pub fn in_words(at: i64, now: i64) -> String {
    let m = ((at - now).max(0) + 59) / 60;
    match m {
        0..=59 => format!("{} {}", m, tr("phút", "min")),
        60..=2879 => format!("{} {}", m / 60, tr("giờ", "h")),
        _ => format!("{} {}", m / 1440, tr("ngày", "d")),
    }
}

// ---- parsing -------------------------------------------------------------------------------

fn key(k: &str) -> HSTRING {
    HSTRING::from(k)
}

fn obj(o: &JsonObject, k: &str) -> Option<JsonObject> {
    o.GetNamedObject(&key(k)).ok()
}

fn num(o: &JsonObject, k: &str) -> Option<f64> {
    o.GetNamedNumber(&key(k)).ok()
}

/// Seconds for a time given as a number (Unix seconds) or as an ISO 8601 string ("2026-11-06T20:12:50Z").
fn when(o: &JsonObject, k: &str) -> Option<i64> {
    num(o, k).map(|n| n as i64).or_else(|| iso_to_epoch(&o.GetNamedString(&key(k)).ok()?.to_string()))
}

/// "2026-11-06T20:12:50Z" -> Unix seconds (UTC; fractions and offsets other than Z are not used by the apps).
fn iso_to_epoch(s: &str) -> Option<i64> {
    let n: Vec<i64> = s.split(|c: char| !c.is_ascii_digit()).filter(|p| !p.is_empty()).take(6).map(|p| p.parse().ok()).collect::<Option<_>>()?;
    let [y, m, d, hh, mm, ss] = <[i64; 6]>::try_from(n).ok()?;
    // Days since 1970-01-01 of a civil date (Howard Hinnant's algorithm).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some((era * 146_097 + doe - 719_468) * 86_400 + hh * 3600 + mm * 60 + ss)
}

/// The label of a limit window of `minutes` (the server sets it by plan: 5 hours and a week on
/// the paid plans, about 30 days on a free one) and whether it is the long one (a week or more),
/// the one the alerts watch.
fn window_label(minutes: f64) -> (String, bool) {
    let m = minutes.round() as i64;
    let long = m >= 7 * 24 * 60 - 60;
    let label = if (m - 300).abs() < 30 {
        tr("5 giờ", "5 h").to_string()
    } else if (m - 7 * 24 * 60).abs() < 60 {
        tr("Tuần", "Week").to_string()
    } else if (m - 30 * 24 * 60).abs() < 24 * 60 {
        tr("30 ngày", "30 days").to_string()
    } else if m >= 24 * 60 && m % (24 * 60) == 0 {
        format!("{} {}", m / (24 * 60), tr("ngày", "days"))
    } else if m >= 60 && m % 60 == 0 {
        format!("{} {}", m / 60, tr("giờ", "h"))
    } else {
        format!("{} {}", m, tr("phút", "min"))
    };
    (label, long)
}

/// Claude Code's status-line JSON: `rate_limits.five_hour` and `.seven_day` with `used_percentage` and `resets_at`.
pub fn parse_claude(text: &str) -> Vec<Bar> {
    let Some(limits) = JsonObject::Parse(&key(text)).ok().and_then(|r| obj(&r, "rate_limits")) else { return Vec::new() };
    [("five_hour", false), ("seven_day", true)]
        .into_iter()
        .filter_map(|(k, weekly)| {
            let w = obj(&limits, k)?;
            let label = if weekly { tr("Tuần", "Week") } else { tr("5 giờ", "5 h") };
            Some(Bar { label: label.to_string(), weekly, used: num(&w, "used_percentage")? as f32, resets_at: when(&w, "resets_at").unwrap_or(0) })
        })
        .collect()
}

/// One Codex session-log line: `payload.rate_limits.primary` / `.secondary`, each null or
/// `used_percent`, `window_minutes`, `resets_at`.
pub fn parse_codex_line(line: &str) -> Vec<Bar> {
    let Some(limits) = JsonObject::Parse(&key(line)).ok().and_then(|r| obj(&r, "payload")).and_then(|p| obj(&p, "rate_limits")) else { return Vec::new() };
    ["primary", "secondary"]
        .into_iter()
        .filter_map(|k| {
            let w = obj(&limits, k)?;
            let (label, weekly) = window_label(num(&w, "window_minutes")?);
            Some(Bar { label, weekly, used: num(&w, "used_percent")? as f32, resets_at: when(&w, "resets_at").unwrap_or(0) })
        })
        .collect()
}

/// Antigravity's `RetrieveUserQuotaSummary`: `response.groups[].buckets[]` with `remainingFraction`,
/// `window` ("5h", "weekly") and `resetTime`.
pub fn parse_antigravity(text: &str) -> Vec<Bar> {
    let mut out = Vec::new();
    let Some(groups) = JsonObject::Parse(&key(text)).ok().and_then(|r| obj(&r, "response")).and_then(|r| r.GetNamedArray(&key("groups")).ok()) else { return out };
    let each = |a: &JsonArray| (0..a.Size().unwrap_or(0)).filter_map(|i| a.GetObjectAt(i).ok()).collect::<Vec<_>>();
    for g in each(&groups) {
        let name = g.GetNamedString(&key("displayName")).map(|s| s.to_string()).unwrap_or_default();
        let lower = name.to_lowercase();
        let group = if lower.contains("argon") { "Argon".to_string() } else if lower.contains("claude") { "Claude · GPT".to_string() } else if lower.contains("gemini") { "Gemini".to_string() } else { name };
        let Ok(buckets) = g.GetNamedArray(&key("buckets")) else { continue };
        for b in each(&buckets) {
            let Some(remaining) = num(&b, "remainingFraction") else { continue };
            let weekly = b.GetNamedString(&key("window")).is_ok_and(|s| s.to_string() == "weekly");
            let win = if weekly { tr("Tuần", "Week") } else { tr("5 giờ", "5 h") };
            out.push(Bar { label: format!("{group} · {win}"), weekly, used: ((1.0 - remaining) * 100.0).clamp(0.0, 100.0) as f32, resets_at: when(&b, "resetTime").unwrap_or(0) });
        }
    }
    out
}

/// The Claude desktop app's own history of its plan usage (`plan-usage-history.json` in its data
/// folder: `{version, samples: [{t (ms), org, u: {fh: 5-hour %, sd: weekly %, ...}}]}`, a sample
/// every ~15 minutes while the app runs): the last sample, as (time in seconds, bars). It has no
/// reset times.
pub fn parse_claude_history(text: &str) -> Option<(i64, Vec<Bar>)> {
    let samples = JsonObject::Parse(&key(text)).ok()?.GetNamedArray(&key("samples")).ok()?;
    let last = samples.GetObjectAt(samples.Size().ok()?.checked_sub(1)?).ok()?;
    let used = obj(&last, "u")?;
    let bars: Vec<Bar> = [("fh", false), ("sd", true)]
        .into_iter()
        .filter_map(|(k, weekly)| {
            let label = if weekly { tr("Tuần", "Week") } else { tr("5 giờ", "5 h") };
            Some(Bar { label: label.to_string(), weekly, used: num(&used, k)? as f32, resets_at: 0 })
        })
        .collect();
    (!bars.is_empty()).then_some(((num(&last, "t")? / 1000.0) as i64, bars))
}

/// A window that already started over reads zero until the next use shows its new reset time.
fn rolled(mut bars: Vec<Bar>, now: i64) -> Vec<Bar> {
    for b in &mut bars {
        if b.resets_at > 0 && b.resets_at <= now {
            b.used = 0.0;
        }
    }
    bars
}

// ---- reading the apps ----------------------------------------------------------------------

fn some(bars: Vec<Bar>) -> Option<Vec<Bar>> {
    (!bars.is_empty()).then_some(bars)
}

/// The app's last sample is trusted for this long (it takes one every 15 minutes while it runs).
const CLAUDE_SAMPLE_MAX_AGE: i64 = 30 * 60;

/// Claude Code's status-line file (as (time written, bars)): it has the reset times.
fn read_claude_status_line() -> Option<(i64, Vec<Bar>)> {
    let path = PathBuf::from(std::env::var_os("APPDATA")?).join("PuwNotch").join("claude-limits.json");
    let written = std::fs::metadata(&path).ok()?.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64;
    Some((written, some(parse_claude(&std::fs::read_to_string(path).ok()?))?))
}

/// The desktop app's usage history, wherever its data folder is (its name carries the package id).
fn read_claude_app() -> Option<(i64, Vec<Bar>)> {
    let data = PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("Claude-Data");
    children(&data).into_iter().find_map(|d| parse_claude_history(&std::fs::read_to_string(d.join("plan-usage-history.json")).ok()?))
}

/// Whichever of the two readings is newer wins; the desktop app's, which has no reset times,
/// borrows those of the status line while they are still ahead.
fn newest_claude(status_line: Option<(i64, Vec<Bar>)>, app: Option<(i64, Vec<Bar>)>, now: i64) -> Option<Vec<Bar>> {
    let app = app.filter(|(at, _)| now - at <= CLAUDE_SAMPLE_MAX_AGE);
    match (status_line, app) {
        (Some((_, bars)), None) => Some(rolled(bars, now)),
        (Some((written, bars)), Some((at, _))) if at <= written => Some(rolled(bars, now)),
        (status_line, Some((_, mut bars))) => {
            for bar in &mut bars {
                bar.resets_at = status_line.iter().flat_map(|(_, b)| b).find(|o| o.label == bar.label && o.resets_at > now).map_or(0, |o| o.resets_at);
            }
            Some(bars)
        }
        (None, None) => None,
    }
}

fn read_claude() -> Option<Vec<Bar>> {
    newest_claude(read_claude_status_line(), read_claude_app(), now_secs())
}

/// Children of `dir` by name, newest (largest name) first: the logs sit in sessions/YYYY/MM/DD/rollout-<time>-....jsonl.
fn children(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir).map(|d| d.flatten().map(|e| e.path()).collect()).unwrap_or_default();
    v.sort_by(|a, b| b.file_name().cmp(&a.file_name()));
    v
}

/// The newest rate-limit event of the logs written to last: a log keeps growing after the day it
/// was started (a resumed session), and an account switch shows only in the newest event, so
/// the files are taken by when they were written and the events compared by their own time.
fn read_codex() -> Option<(Vec<Bar>, Option<String>)> {
    let root = PathBuf::from(std::env::var_os("USERPROFILE")?).join(".codex").join("sessions");
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = children(&root).into_iter().flat_map(|y| children(&y)).flat_map(|m| children(&m)).flat_map(|d| children(&d))
        .filter_map(|f| Some((std::fs::metadata(&f).ok()?.modified().ok()?, f)))
        .collect();
    files.sort_by(|a, b| b.0.cmp(&a.0));
    files.into_iter().take(CODEX_FILES).filter_map(|(_, f)| last_limits(&f)).max_by_key(|(at, _, _)| *at).map(|(_, bars, plan)| (bars, plan))
}

/// The `timestamp` of a Codex log line, Unix seconds.
fn codex_time(line: &str) -> Option<i64> {
    iso_to_epoch(&JsonObject::Parse(&key(line)).ok()?.GetNamedString(&key("timestamp")).ok()?.to_string())
}

/// The `plan_type` of a Codex rate-limit event ("free", "plus"...).
fn codex_plan(line: &str) -> Option<String> {
    let limits = JsonObject::Parse(&key(line)).ok().and_then(|r| obj(&r, "payload")).and_then(|p| obj(&p, "rate_limits"))?;
    limits.GetNamedString(&key("plan_type")).ok().map(|s| s.to_string()).filter(|s| !s.is_empty())
}

/// The newest rate-limit event in the end of a Codex log (events with null windows are skipped),
/// with its time and the account's plan.
fn last_limits(path: &Path) -> Option<(i64, Vec<Bar>, Option<String>)> {
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(CODEX_TAIL))).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    String::from_utf8_lossy(&bytes).lines().rev().filter(|l| l.contains("\"used_percent\"")).find_map(|l| Some((codex_time(l).unwrap_or(0), some(parse_codex_line(l))?, codex_plan(l))))
}

/// A process handle that closes itself.
struct Process(HANDLE);

impl Drop for Process {
    fn drop(&mut self) {
        unsafe { let _ = CloseHandle(self.0); }
    }
}

/// The running `exe` whose full path contains `dir_hint` (several apps ship a language_server.exe).
fn find_process(exe: &str, dir_hint: &str) -> Option<Process> {
    unsafe {
        let snapshot = Process(CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()?);
        let mut entry = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut more = Process32FirstW(snapshot.0, &mut entry).is_ok();
        while more {
            let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
            if String::from_utf16_lossy(&entry.szExeFile[..len]).eq_ignore_ascii_case(exe) {
                if let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, entry.th32ProcessID) {
                    let process = Process(h);
                    let mut buf = [0u16; 520];
                    let mut n = buf.len() as u32;
                    if QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, windows::core::PWSTR(buf.as_mut_ptr()), &mut n).is_ok()
                        && String::from_utf16_lossy(&buf[..n as usize]).to_lowercase().contains(dir_hint)
                    {
                        return Some(process);
                    }
                }
            }
            more = Process32NextW(snapshot.0, &mut entry).is_ok();
        }
        None
    }
}

/// The command line of a process of this user (ProcessCommandLineInformation).
fn command_line(process: &Process) -> Option<String> {
    unsafe {
        // 8-byte aligned room for the UNICODE_STRING header and the text that follows it.
        let mut buf = vec![0u64; 2048];
        let mut got = 0u32;
        NtQueryInformationProcess(process.0, PROCESSINFOCLASS(60), buf.as_mut_ptr() as *mut c_void, (buf.len() * 8) as u32, &mut got).ok().ok()?;
        let us = &*(buf.as_ptr() as *const UNICODE_STRING);
        Some(String::from_utf16_lossy(std::slice::from_raw_parts(us.Buffer.0, us.Length as usize / 2)))
    }
}

/// Port of the language server's plain-HTTP listener, from its log ("... listening on random port at N for HTTP").
fn antigravity_port() -> Option<u16> {
    let log = PathBuf::from(std::env::var_os("APPDATA")?).join("Antigravity").join("logs").join("language_server.log");
    let text = std::fs::read_to_string(log).ok()?;
    let line = text.lines().rev().find(|l| l.trim_end().ends_with("for HTTP"))?;
    line.split("port at ").nth(1)?.split_whitespace().next()?.parse().ok()
}

/// Where Antigravity's language server listens on this PC and the token it asks for (read from
/// its own command line, so it exists only while the app runs).
pub struct LocalServer {
    port: u16,
    token: String,
}

impl LocalServer {
    /// Call `method` of its service with a JSON `body`: the answer's body on success.
    pub fn call(&self, method: &str, body: &str) -> Option<String> {
        post_local(self.port, &format!("/exa.language_server_pb.LanguageServerService/{method}"), &self.token, body)
    }
}

pub fn antigravity_server() -> Option<LocalServer> {
    let process = find_process("language_server.exe", "antigravity")?;
    let command = command_line(&process)?;
    let token = command.split_whitespace().skip_while(|a| *a != "--csrf_token").nth(1)?.to_string();
    Some(LocalServer { port: antigravity_port()?, token })
}

fn read_antigravity() -> Option<Vec<Bar>> {
    some(parse_antigravity(&antigravity_server()?.call("RetrieveUserQuotaSummary", "{}")?))
}

/// POST a JSON `body` to a server on this PC, answer body on 200.
fn post_local(port: u16, path: &str, token: &str, body: &str) -> Option<String> {
    /// A WinINet handle, closed when it goes out of scope.
    struct Handle(*mut c_void);
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe { let _ = InternetCloseHandle(self.0); }
        }
    }
    unsafe {
        let session = InternetOpenW(w!("PUW-Notch/1.0 (Windows)"), INTERNET_OPEN_TYPE_DIRECT.0, PCWSTR::null(), PCWSTR::null(), 0);
        if session.is_null() {
            return None;
        }
        let session = Handle(session);
        let timeout = 5_000u32.to_ne_bytes();
        for option in [INTERNET_OPTION_CONNECT_TIMEOUT, INTERNET_OPTION_RECEIVE_TIMEOUT, INTERNET_OPTION_SEND_TIMEOUT] {
            let _ = InternetSetOptionW(Some(session.0), option, Some(timeout.as_ptr() as *const c_void), timeout.len() as u32);
        }
        let connection = InternetConnectW(session.0, w!("127.0.0.1"), port, PCWSTR::null(), PCWSTR::null(), INTERNET_SERVICE_HTTP, 0, None);
        if connection.is_null() {
            return None;
        }
        let connection = Handle(connection);
        let path: Vec<u16> = path.encode_utf16().chain([0]).collect();
        let request = HttpOpenRequestW(connection.0, w!("POST"), PCWSTR(path.as_ptr()), PCWSTR::null(), PCWSTR::null(), None, INTERNET_FLAG_RELOAD | INTERNET_FLAG_NO_UI | INTERNET_FLAG_NO_COOKIES, None);
        if request.is_null() {
            return None;
        }
        let request = Handle(request);
        let headers: Vec<u16> = format!("Content-Type: application/json\r\nConnect-Protocol-Version: 1\r\nX-Codeium-Csrf-Token: {token}\r\n").encode_utf16().collect();
        HttpSendRequestW(request.0, Some(&headers), Some(body.as_ptr() as *const c_void), body.len() as u32).ok()?;
        let (mut status, mut size) = (0u32, 4u32);
        HttpQueryInfoW(request.0, HTTP_QUERY_STATUS_CODE | HTTP_QUERY_FLAG_NUMBER, Some(&mut status as *mut u32 as *mut c_void), &mut size, None).ok()?;
        if status != 200 {
            return None;
        }
        let mut answer = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            let mut read = 0u32;
            InternetReadFile(request.0, chunk.as_mut_ptr() as *mut c_void, chunk.len() as u32, &mut read).ok()?;
            if read == 0 || answer.len() > 256 * 1024 {
                break;
            }
            answer.extend_from_slice(&chunk[..read as usize]);
        }
        String::from_utf8(answer).ok()
    }
}

fn read_all() -> Snapshot {
    let now = now_secs();
    let codex = read_codex();
    Snapshot { apps: [read_claude(), codex.as_ref().map(|(bars, _)| rolled(bars.clone(), now)), read_antigravity()], plans: [None, codex.and_then(|(_, plan)| plan), None] }
}

/// The worker that keeps a `Snapshot` fresh and tells the window when it changes.
pub struct Monitor {
    shared: Arc<Mutex<Snapshot>>,
}

impl Monitor {
    pub fn start(hwnd: HWND) -> Monitor {
        let shared = Arc::new(Mutex::new(Snapshot::default()));
        let (mine, target) = (shared.clone(), hwnd.0 as isize);
        std::thread::spawn(move || {
            unsafe { let _ = RoInitialize(RO_INIT_MULTITHREADED); }
            loop {
                let now = read_all();
                if let Ok(mut s) = mine.lock() {
                    if *s != now {
                        *s = now;
                        unsafe { let _ = PostMessageW(Some(HWND(target as _)), WM_USAGE, WPARAM(0), LPARAM(0)); }
                    }
                }
                std::thread::sleep(POLL);
            }
        });
        Monitor { shared }
    }

    pub fn snapshot(&self) -> Snapshot {
        self.shared.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

// ---- alerts --------------------------------------------------------------------------------

/// Weekly use at which the banner speaks, and how much of the week in one day is "a lot".
const WARN_AT: f32 = 80.0;
const HIGH_AT: f32 = 95.0;
const DAY_JUMP: f32 = 25.0;

const SAID_WARN: u8 = 1;
const SAID_HIGH: u8 = 2;
const SAID_DAY: u8 = 4;

struct Seen {
    resets_at: i64,
    day: i64,
    /// Weekly use when this day (or this week) was first seen.
    base: f32,
    said: u8,
}

pub struct Alert {
    pub app: App,
    pub bar: Bar,
    pub reason: Reason,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Reason {
    Warn,
    High,
    /// Used this much of the week since this morning.
    Day(f32),
}

impl Alert {
    /// (title, body) of the banner.
    pub fn text(&self, now: i64) -> (String, String) {
        let title = format!("{} · {}%", self.app.name(), self.bar.used.round());
        let resets = format!("{} {}", tr("Làm mới sau", "Resets in"), in_words(self.bar.resets_at, now));
        let body = match self.reason {
            Reason::Day(by) => format!("{} {}% {} ({})", tr("Hôm nay đã dùng", "Used today:"), by.round(), tr("hạn mức", "of the limit"), self.bar.label),
            Reason::Warn | Reason::High => format!("{} ({}) · {}", tr("Sắp hết hạn mức", "Limit almost used"), self.bar.label, resets),
        };
        (title, body)
    }
}

/// Remembers what was already announced, so each threshold speaks once a week and the daily jump once a day.
#[derive(Default)]
pub struct Watch {
    seen: HashMap<String, Seen>,
}

impl Watch {
    /// New alerts for this reading, most used first. `day` counts local days.
    pub fn check(&mut self, snap: &Snapshot, day: i64) -> Vec<Alert> {
        let mut out = Vec::new();
        for app in APPS {
            for bar in snap.get(app).unwrap_or(&[]).iter().filter(|b| b.weekly) {
                let seen = self.seen.entry(format!("{}:{}", app.name(), bar.label)).or_insert(Seen { resets_at: bar.resets_at, day, base: bar.used, said: 0 });
                // A new week: the thresholds speak again.
                if (seen.resets_at - bar.resets_at).abs() > 3600 {
                    *seen = Seen { resets_at: bar.resets_at, day, base: bar.used, said: 0 };
                }
                if seen.day != day {
                    (seen.day, seen.base, seen.said) = (day, bar.used, seen.said & !SAID_DAY);
                }
                let reason = if bar.used >= HIGH_AT && seen.said & SAID_HIGH == 0 {
                    seen.said |= SAID_HIGH | SAID_WARN;
                    Some(Reason::High)
                } else if bar.used >= WARN_AT && seen.said & SAID_WARN == 0 {
                    seen.said |= SAID_WARN;
                    Some(Reason::Warn)
                } else if bar.used - seen.base >= DAY_JUMP && seen.said & SAID_DAY == 0 {
                    seen.said |= SAID_DAY;
                    Some(Reason::Day(bar.used - seen.base))
                } else {
                    None
                };
                if let Some(reason) = reason {
                    out.push(Alert { app, bar: bar.clone(), reason });
                }
            }
        }
        out.sort_by(|a, b| b.bar.used.total_cmp(&a.bar.used));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn weekly(used: f32) -> Snapshot {
        let mut s = Snapshot::default();
        s.apps[App::Codex as usize] = Some(vec![Bar { label: "Tuần".into(), weekly: true, used, resets_at: 1_000_000 }]);
        s
    }

    #[test]
    fn iso_times() {
        assert_eq!(iso_to_epoch("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(iso_to_epoch("2026-11-06T20:12:50Z"), Some(1_793_995_970));
        assert_eq!(iso_to_epoch("x"), None);
    }

    #[test]
    fn apps_are_read_as_they_write_their_limits() {
        let codex = r#"{"timestamp":"t","type":"event_msg","payload":{"type":"token_count","info":null,"rate_limits":{"limit_id":"codex","primary":{"used_percent":99.0,"window_minutes":10080,"resets_at":1793911204},"secondary":null}}}"#;
        let bars = parse_codex_line(codex);
        assert_eq!(bars.len(), 1);
        assert!(bars[0].weekly && bars[0].used == 99.0 && bars[0].resets_at == 1_793_911_204);
        assert!(parse_codex_line(r#"{"payload":{"rate_limits":{"primary":null,"secondary":null}}}"#).is_empty());
        assert_eq!(codex_time(r#"{"timestamp":"2026-11-06T20:12:50.123Z","payload":{}}"#), Some(1_793_995_970));
        // A free account: one window of 30 days.
        let free = parse_codex_line(r#"{"payload":{"rate_limits":{"primary":{"used_percent":0.0,"window_minutes":43200,"resets_at":1796059769},"secondary":null,"plan_type":"free"}}}"#);
        assert_eq!((free[0].label.as_str(), free[0].weekly), ("30 ngày", true));
        assert_eq!(codex_plan(r#"{"payload":{"rate_limits":{"primary":null,"plan_type":"free"}}}"#).as_deref(), Some("free"));

        let claude = r#"{"model":{},"rate_limits":{"five_hour":{"used_percentage":19,"resets_at":1793000000},"seven_day":{"used_percentage":3.5,"resets_at":1793500000}}}"#;
        let bars = parse_claude(claude);
        assert_eq!((bars.len(), bars[0].weekly, bars[1].weekly, bars[1].used), (2, false, true, 3.5));
        assert!(parse_claude(r#"{"model":{}}"#).is_empty());

        let antigravity = r#"{"response":{"groups":[{"displayName":"Claude and GPT models","buckets":[{"bucketId":"3p-weekly","window":"weekly","remainingFraction":0.25,"resetTime":"2026-11-07T15:17:04Z"}]}]}}"#;
        let bars = parse_antigravity(antigravity);
        assert_eq!(bars.len(), 1);
        assert!(bars[0].weekly && bars[0].label.starts_with("Claude · GPT") && (bars[0].used - 75.0).abs() < 1e-3);
    }

    #[test]
    #[ignore = "reads the apps open on this PC and prints what each shows"]
    fn reads_this_pc() {
        let snap = read_all();
        for app in APPS {
            println!("{}: {:?}", app.name(), snap.get(app));
        }
    }

    #[test]
    fn the_claude_app_keeps_a_history_of_its_usage() {
        let (at, bars) = parse_claude_history(r#"{"version":2,"samples":[{"t":1000000,"org":"o","u":{"fh":5,"sd":1}},{"t":1793000000000,"org":"o","u":{"fh":36,"sd":8,"so":2}}]}"#).unwrap();
        assert_eq!(at, 1_793_000_000);
        assert_eq!((bars.len(), bars[0].used, bars[1].used, bars[1].weekly), (2, 36.0, 8.0, true), "the last sample; other keys are left out");
        assert!(parse_claude_history(r#"{"version":2,"samples":[]}"#).is_none());
    }

    #[test]
    fn the_newer_claude_reading_wins_and_borrows_reset_times() {
        let bar = |label: &str, used, resets_at| Bar { label: label.into(), weekly: label == "Tuần", used, resets_at };
        let hook = Some((100, vec![bar("5 giờ", 10.0, 5000), bar("Tuần", 4.0, 9000)]));
        let app = Some((200, vec![bar("5 giờ", 30.0, 0), bar("Tuần", 8.0, 0)]));
        let both = newest_claude(hook.clone(), app.clone(), 300).unwrap();
        assert_eq!((both[0].used, both[0].resets_at, both[1].used, both[1].resets_at), (30.0, 5000, 8.0, 9000));
        let late_hook = Some((250, hook.clone().unwrap().1));
        assert_eq!(newest_claude(late_hook, app.clone(), 300).unwrap()[0].used, 10.0, "the status line is newer");
        assert_eq!(newest_claude(None, app.clone(), 300).unwrap()[0].resets_at, 0);
        assert!(newest_claude(None, app, 200 + CLAUDE_SAMPLE_MAX_AGE + 1).is_none(), "an old sample is not shown");
        assert!(newest_claude(None, None, 300).is_none());
    }

    #[test]
    fn windows_are_named_by_their_length() {
        for (minutes, label, long) in [(300.0, "5 giờ", false), (10080.0, "Tuần", true), (43200.0, "30 ngày", true), (2880.0, "2 ngày", false), (120.0, "2 giờ", false), (45.0, "45 phút", false)] {
            assert_eq!(window_label(minutes), (label.to_string(), long), "{minutes} min");
        }
    }

    #[test]
    fn a_passed_reset_reads_zero() {
        let bar = |at| Bar { label: String::new(), weekly: true, used: 80.0, resets_at: at };
        assert_eq!(rolled(vec![bar(100)], 200)[0].used, 0.0);
        assert_eq!(rolled(vec![bar(300)], 200)[0].used, 80.0);
    }

    #[test]
    fn alerts_speak_once_per_threshold_and_per_day() {
        let mut watch = Watch::default();
        assert!(watch.check(&weekly(10.0), 1).is_empty(), "first sight is only the baseline");
        assert_eq!(watch.check(&weekly(40.0), 1)[0].reason, Reason::Day(30.0));
        assert!(watch.check(&weekly(45.0), 1).is_empty(), "the daily jump speaks once a day");
        assert_eq!(watch.check(&weekly(81.0), 1)[0].reason, Reason::Warn);
        assert_eq!(watch.check(&weekly(96.0), 1)[0].reason, Reason::High);
        assert!(watch.check(&weekly(99.0), 1).is_empty());
        // A new week starts over.
        let mut next = weekly(5.0);
        next.apps[App::Codex as usize].as_mut().unwrap()[0].resets_at = 2_000_000;
        assert!(watch.check(&next, 2).is_empty());
        assert_eq!(watch.check(&weekly(85.0), 2)[0].reason, Reason::Warn);
    }
}

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use windows::core::{BOOL, HSTRING, Interface};
use windows::Data::Json::JsonObject;
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, WAIT_OBJECT_0, WAIT_TIMEOUT, WPARAM};
use windows::Win32::Storage::FileSystem::{FindCloseChangeNotification, FindFirstChangeNotificationW, FindNextChangeNotification, FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_SIZE};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
use windows::Win32::UI::Accessibility::{CUIAutomation, IUIAutomation, IUIAutomation2, TreeScope_Descendants};
use windows::Win32::UI::Shell::{SHGetKnownFolderPath, FOLDERID_Downloads, KF_FLAG_DEFAULT};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{CreateEventW, GetExitCodeProcess, OpenProcess, SetEvent, WaitForMultipleObjects, PROCESS_QUERY_LIMITED_INFORMATION};
use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetClassNameW, GetWindowTextW, IsWindowVisible, PostMessageW, WM_APP};

pub const WM_ACTIVITY: u32 = WM_APP + 15;

use crate::i18n::tr;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivityKind {
    FileOp,
    Move,
    Delete,
    Download,
    Build,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LiveActivity {
    pub kind: ActivityKind,
    pub title: String,
    pub detail: String,
    pub progress: Option<f32>,
    pub result: Option<bool>,
    pub file: Option<PathBuf>,
    pub more: usize,
}

pub struct Watch {
    pub enabled: bool,
    reader: Option<Reader>,
}

const ACTIVE_POLL: Duration = Duration::from_millis(250);
const IDLE_CHECK: Duration = Duration::from_secs(10);
const CHANGE_QUIET: Duration = Duration::from_secs(1);
const DOWNLOAD_SCAN: Duration = Duration::from_secs(30);
const DIRS_REFRESH: Duration = Duration::from_secs(60);
const MAX_WATCHED_DIRS: usize = 63;
const CONFIRM_AFTER: Duration = Duration::from_millis(700);

static WAKE: OnceLock<isize> = OnceLock::new();

fn wake_event() -> isize {
    *WAKE.get_or_init(|| unsafe { CreateEventW(None, false, false, None) }.map_or(0, |h| h.0 as isize))
}

pub fn wake() {
    if let Some(&raw) = WAKE.get().filter(|&&raw| raw != 0) {
        unsafe { let _ = SetEvent(HANDLE(raw as _)); }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Woke {
    Signal,
    Change,
    Timeout,
}

fn wait_plan(active: bool, now: Instant, quiet_until: Instant) -> (bool, Duration) {
    if active {
        (false, ACTIVE_POLL)
    } else if now < quiet_until {
        (false, quiet_until - now)
    } else {
        (true, IDLE_CHECK)
    }
}

fn download_scan_due(last: Option<Instant>, now: Instant) -> bool {
    last.is_none_or(|t| now.saturating_duration_since(t) >= DOWNLOAD_SCAN)
}

fn wait_for(event: HANDLE, changes: &[HANDLE], timeout: Duration) -> Woke {
    let mut all = Vec::with_capacity(1 + changes.len());
    all.push(event);
    all.extend_from_slice(changes);
    let ms = timeout.as_millis().min(u32::MAX as u128 - 1) as u32;
    let result = unsafe { WaitForMultipleObjects(&all, false, ms) };
    if result == WAIT_TIMEOUT {
        return Woke::Timeout;
    }
    let index = result.0.wrapping_sub(WAIT_OBJECT_0.0) as usize;
    if index == 0 {
        Woke::Signal
    } else if index < all.len() {
        unsafe { let _ = FindNextChangeNotification(all[index]); }
        Woke::Change
    } else {
        std::thread::sleep(timeout);
        Woke::Timeout
    }
}

#[derive(Default)]
struct DownloadWatches {
    dirs: Vec<PathBuf>,
    handles: Vec<HANDLE>,
    checked_at: Option<Instant>,
}

impl DownloadWatches {
    fn refresh(&mut self, now: Instant) {
        if self.checked_at.is_some_and(|t| now.saturating_duration_since(t) < DIRS_REFRESH) {
            return;
        }
        self.checked_at = Some(now);
        let (dirs, _) = download_dirs();
        if dirs == self.dirs {
            return;
        }
        self.close();
        self.handles = dirs
            .iter()
            .take(MAX_WATCHED_DIRS)
            .filter_map(|dir| unsafe { FindFirstChangeNotificationW(&HSTRING::from(dir.as_os_str()), false, FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_SIZE) }.ok())
            .map(|h| HANDLE(h.0))
            .collect();
        self.dirs = dirs;
    }

    fn close(&mut self) {
        for handle in self.handles.drain(..) {
            unsafe { let _ = FindCloseChangeNotification(handle); }
        }
    }
}

struct Reader {
    stop: Arc<AtomicBool>,
    updates: Receiver<(Option<LiveActivity>, Option<LiveActivity>)>,
    current: Option<LiveActivity>,
    finished: Option<LiveActivity>,
    received_at: Instant,
}

impl Watch {
    pub fn new(enabled: bool) -> Self {
        Self { enabled, reader: None }
    }

    pub fn poll(&mut self, now: Instant, target: isize) -> Option<LiveActivity> {
        if !self.enabled {
            self.reader = None;
            return None;
        }
        self.reader.get_or_insert_with(|| Reader::new(target));
        self.latest(now)
    }

    pub fn latest(&mut self, now: Instant) -> Option<LiveActivity> {
        let reader = self.reader.as_mut().filter(|_| self.enabled)?;
        for (current, finished) in reader.updates.try_iter() {
            reader.current = current;
            if finished.is_some() {
                reader.finished = finished;
            }
            reader.received_at = now;
        }
        if now.saturating_duration_since(reader.received_at) > Duration::from_secs(5) {
            reader.current = None;
        }
        reader.current.clone()
    }

    pub fn take_finished(&mut self) -> Option<LiveActivity> {
        self.reader.as_mut().and_then(|r| r.finished.take())
    }
}

impl Reader {
    fn new(target: isize) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let (updates, receiver) = channel();
        let (flag, event) = (stop.clone(), wake_event());
        crate::system::spawn("live-activity", move || {
            let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
            let event = HANDLE(event as _);
            let mut monitor = Monitor::new(true);
            let mut watches = DownloadWatches::default();
            let (mut active, mut woke, mut quiet_until) = (false, Woke::Signal, Instant::now());
            while !flag.load(Ordering::Relaxed) {
                let now = Instant::now();
                let current = monitor.poll(now, active || woke != Woke::Timeout);
                let finished = monitor.take_finished();
                let show = current.is_some() || finished.is_some();
                active = current.is_some() || monitor.waiting();
                if woke == Woke::Change {
                    quiet_until = now + CHANGE_QUIET;
                }
                if updates.send((current, finished)).is_err() {
                    break;
                }
                if show {
                    unsafe { let _ = PostMessageW(Some(HWND(target as _)), WM_ACTIVITY, WPARAM(0), LPARAM(0)); }
                }
                let now = Instant::now();
                watches.refresh(now);
                let (listen, timeout) = wait_plan(active, now, quiet_until);
                woke = wait_for(event, if listen { &watches.handles } else { &[] }, timeout);
            }
            watches.close();
            drop(monitor);
            if com.is_ok() {
                unsafe { CoUninitialize(); }
            }
        });
        Self { stop, updates: receiver, current: None, finished: None, received_at: Instant::now() }
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        wake();
    }
}

#[derive(Default)]
pub struct Monitor {
    pub enabled: bool,
    download_file: Option<PathBuf>,
    download_size: u64,
    download_at: Option<Instant>,
    download_changed_at: Option<Instant>,
    download_speed: f64,
    download_dirs: Vec<PathBuf>,
    download_histories: Vec<PathBuf>,
    download_dirs_at: Option<Instant>,
    download_scanned_at: Option<Instant>,
    download_name: Option<String>,
    download_lookup_at: Option<Instant>,
    download_done: Option<PathBuf>,
    download_before: HashSet<PathBuf>,
    automation: Option<IUIAutomation>,
    explorer_op: Option<(isize, ActivityKind, String)>,
    explorer_ui_at: Option<Instant>,
    explorer_seen: Option<Instant>,
    explorer_pending: bool,
    build_exe: Option<String>,
    build_since: Option<Instant>,
    build_pid: Option<u32>,
    build_proc: Option<Proc>,
    build_exit: Option<u32>,
    last: Vec<LiveActivity>,
    finished: Option<LiveActivity>,
}

impl Monitor {
    pub fn new(enabled: bool) -> Self {
        Self { enabled, ..Default::default() }
    }

    pub fn poll(&mut self, now: Instant, full: bool) -> Option<LiveActivity> {
        if !self.enabled {
            self.last.clear();
            self.finished = None;
            return None;
        }
        let scan = full || download_scan_due(self.download_scanned_at, now);
        if scan {
            self.download_scanned_at = Some(now);
        }
        let download = if scan { self.poll_download(now) } else { None };
        let explorer = self.poll_explorer(now);
        let explorer = self.confirm_explorer(explorer, now);
        let mut running: Vec<LiveActivity> = [explorer, download, self.poll_build(now)].into_iter().flatten().collect();
        if let Some(mut a) = ended_activity(std::mem::take(&mut self.last), &running) {
            match a.kind {
                ActivityKind::Download => if let Some(file) = self.download_done.take() {
                    a.title = file.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
                    a.detail = std::fs::metadata(&file).map_or(a.detail, |m| download_detail(m.len(), 0.0));
                    (a.result, a.file) = (Some(true), Some(file));
                },
                ActivityKind::Build => a.result = self.build_exit.take().map(|code| code == 0),
                _ => {}
            }
            self.finished = Some(a);
        }
        self.last = running.clone();
        let more = running.len().saturating_sub(1);
        if let Some(first) = running.first_mut() {
            first.more = more;
        }
        running.into_iter().next()
    }

    pub fn take_finished(&mut self) -> Option<LiveActivity> {
        self.finished.take()
    }

    pub fn waiting(&self) -> bool {
        self.explorer_pending
    }

    fn confirm_explorer(&mut self, found: Option<LiveActivity>, now: Instant) -> Option<LiveActivity> {
        let Some(activity) = found else {
            self.explorer_seen = None;
            self.explorer_pending = false;
            return None;
        };
        let first_seen = *self.explorer_seen.get_or_insert(now);
        let confirmed = file_op_confirmed(first_seen, now, &activity);
        self.explorer_pending = !confirmed;
        confirmed.then_some(activity)
    }

    fn poll_explorer(&mut self, now: Instant) -> Option<LiveActivity> {
        let windows = explorer_windows();
        if windows.is_empty() {
            self.explorer_op = None;
            return None;
        }
        for hwnd in windows {
            let id = hwnd.0 as isize;
            let title = window_title(hwnd);
            let mut activity = parse_explorer_title(&title);
            if activity.as_ref().is_some_and(names_operation) {
                return activity;
            }
            let known = self.explorer_op.as_ref().filter(|(h, ..)| *h == id).map(|(_, kind, name)| (*kind, name.clone()));
            if let (Some(a), Some((kind, name))) = (activity.as_mut(), known) {
                (a.kind, a.title) = (kind, name);
                return activity;
            }
            if self.explorer_ui_at.is_some_and(|t| now.saturating_duration_since(t) < Duration::from_secs(1)) {
                if activity.is_some() { return activity; }
                continue;
            }
            self.explorer_ui_at = Some(now);
            let labels = self.dialog_labels(hwnd);
            if let Some(a) = parse_explorer_labels(&title, &labels) {
                if names_operation(&a) {
                    self.explorer_op = Some((id, a.kind, a.title.clone()));
                }
                return Some(a);
            }
        }
        None
    }

    fn dialog_labels(&mut self, hwnd: HWND) -> Vec<String> {
        if self.automation.is_none() {
            self.automation = new_automation();
        }
        let mut labels = Vec::new();
        let Some(automation) = &self.automation else { return labels };
        unsafe {
            if let (Ok(root), Ok(condition)) = (automation.ElementFromHandle(hwnd), automation.CreateTrueCondition()) {
                if let Ok(elements) = root.FindAll(TreeScope_Descendants, &condition) {
                    for i in 0..elements.Length().unwrap_or(0).clamp(0, 64) {
                        if let Ok(name) = elements.GetElement(i).and_then(|e| e.CurrentName()) {
                            labels.push(name.to_string());
                        }
                    }
                }
            }
        }
        labels
    }

    fn poll_download(&mut self, now: Instant) -> Option<LiveActivity> {
        let selected = self.download_file.as_deref().zip(self.download_changed_at)
            .map(|(path, changed)| (path, self.download_size, changed));
        if self.download_dirs_at.is_none_or(|t| now.saturating_duration_since(t) >= Duration::from_secs(30)) {
            (self.download_dirs, self.download_histories) = download_dirs();
            self.download_dirs_at = Some(now);
        }
        let found = find_download(&self.download_dirs, selected, now, SystemTime::now());
        let Some((path, size)) = found else {
            self.download_done = self.download_file.take().and_then(|part| completed_file(&part, self.download_size, &self.download_before));
            self.download_before.clear();
            self.download_name = None;
            self.download_lookup_at = None;
            self.download_size = 0;
            self.download_at = None;
            self.download_changed_at = None;
            self.download_speed = 0.0;
            return None;
        };
        if self.download_file.as_ref() == Some(&path) {
            if size != self.download_size {
                self.download_changed_at = Some(now);
            }
            let dt = self.download_at.map_or(1.0, |t| now.duration_since(t).as_secs_f64()).max(0.1);
            self.download_speed = size.saturating_sub(self.download_size) as f64 / dt;
        } else {
            self.download_speed = 0.0;
            self.download_file = Some(path.clone());
            self.download_changed_at = Some(now);
            self.download_name = clean_download_name(&path);
            self.download_lookup_at = None;
            self.download_before = finished_files(&path);
        }
        if self.download_name.is_none() && self.download_lookup_at.is_none_or(|t| now.saturating_duration_since(t) >= Duration::from_secs(2)) {
            self.download_lookup_at = Some(now);
            self.download_name = self.download_histories.iter().find_map(|h| {
                let rows = crate::notify::query_unlocked(h, "SELECT current_path, target_path, state FROM downloads ORDER BY id DESC LIMIT 20", 3)?;
                history_name(&rows, &path)
            });
        }
        self.download_size = size;
        self.download_at = Some(now);
        let title = self.download_name.clone().unwrap_or_default();
        Some(LiveActivity { kind: ActivityKind::Download, title, detail: download_detail(size, self.download_speed), progress: None, result: None, file: None, more: 0 })
    }

    fn poll_build(&mut self, now: Instant) -> Option<LiveActivity> {
        let Some((pid, exe)) = pick_build(&process_list()) else {
            self.build_exit = self.build_proc.take().and_then(|p| p.exit_code());
            (self.build_pid, self.build_exe, self.build_since) = (None, None, None);
            return None;
        };
        if self.build_pid != Some(pid) {
            self.build_pid = Some(pid);
            self.build_proc = Proc::open(pid);
        }
        if self.build_exe.as_deref() != Some(exe.as_str()) {
            self.build_exe = Some(exe.clone());
            self.build_since = Some(now);
        }
        let secs = self.build_since.map_or(0, |t| now.duration_since(t).as_secs());
        Some(LiveActivity { kind: ActivityKind::Build, title: build_name(&exe).to_string(), detail: elapsed_text(secs), progress: None, result: None, file: None, more: 0 })
    }
}

fn ended_activity(last: Vec<LiveActivity>, running: &[LiveActivity]) -> Option<LiveActivity> {
    last.into_iter().find(|a| !is_file_op(a.kind) && !running.iter().any(|r| r.kind == a.kind))
}

fn file_op_confirmed(first_seen: Instant, now: Instant, activity: &LiveActivity) -> bool {
    now.saturating_duration_since(first_seen) >= CONFIRM_AFTER && (activity.progress.is_some() || names_operation(activity))
}

fn is_file_op(kind: ActivityKind) -> bool {
    matches!(kind, ActivityKind::FileOp | ActivityKind::Move | ActivityKind::Delete)
}

struct Proc(HANDLE);

impl Proc {
    fn open(pid: u32) -> Option<Self> {
        unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok().map(Self) }
    }

    fn exit_code(&self) -> Option<u32> {
        const STILL_ACTIVE: u32 = 259;
        let mut code = 0;
        unsafe { GetExitCodeProcess(self.0, &mut code).ok()? };
        (code != STILL_ACTIVE).then_some(code)
    }
}

impl Drop for Proc {
    fn drop(&mut self) {
        unsafe { let _ = CloseHandle(self.0); }
    }
}

fn download_detail(size: u64, speed: f64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    if speed > 0.05 * MB {
        format!("{:.1} MB · {:.1} MB/s", size as f64 / MB, speed / MB)
    } else {
        format!("{:.1} MB", size as f64 / MB)
    }
}

fn elapsed_text(secs: u64) -> String {
    if secs >= 60 { format!("{}m {:02}s", secs / 60, secs % 60) } else { format!("{secs}s") }
}

const OPERATION_WINDOW: &str = "OperationStatusWindow";

pub fn is_operation_window(hwnd: HWND) -> bool {
    let mut class = [0u16; OPERATION_WINDOW.len() + 1];
    let n = unsafe { GetClassNameW(hwnd, &mut class) }.max(0) as usize;
    same_name(&class[..n], OPERATION_WINDOW)
}

fn same_name(units: &[u16], ascii: &str) -> bool {
    units.len() == ascii.len() && units.iter().zip(ascii.bytes()).all(|(&u, b)| u < 128 && (u as u8).eq_ignore_ascii_case(&b))
}

fn explorer_windows() -> Vec<HWND> {
    unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
        if IsWindowVisible(hwnd).as_bool() && is_operation_window(hwnd) {
            (*(lparam.0 as *mut Vec<HWND>)).push(hwnd);
        }
        true.into()
    }
    let mut windows = Vec::<HWND>::new();
    unsafe { let _ = EnumWindows(Some(visit), LPARAM(&mut windows as *mut _ as isize)); }
    windows
}

fn window_title(hwnd: HWND) -> String {
    let mut title = [0u16; 256];
    let n = unsafe { GetWindowTextW(hwnd, &mut title) }.max(0) as usize;
    String::from_utf16_lossy(&title[..n])
}

fn new_automation() -> Option<IUIAutomation> {
    let automation: IUIAutomation = unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()? };
    if let Ok(timeout) = automation.cast::<IUIAutomation2>() {
        unsafe {
            let _ = timeout.SetConnectionTimeout(200);
            let _ = timeout.SetTransactionTimeout(200);
        }
    }
    Some(automation)
}

fn names_operation(a: &LiveActivity) -> bool {
    a.title != tr("Đang xử lý tệp", "Working on files") && a.title != tr("Đang chuẩn bị", "Preparing files")
}

fn parse_explorer_labels(title: &str, labels: &[String]) -> Option<LiveActivity> {
    let mut activity = parse_explorer_title(title);
    let generic = tr("Đang xử lý tệp", "Working on files");
    for label in labels {
        if let Some(found) = parse_explorer_title(label) {
            let lower = label.trim().to_lowercase();
            if found.title == generic && !lower.ends_with("complete") && !lower.ends_with("hoàn thành") {
                continue;
            }
            match &mut activity {
                Some(current) => {
                    if current.title == generic && found.title != generic {
                        (current.kind, current.title) = (found.kind, found.title);
                    }
                    if current.progress.is_none() && found.progress.is_some() {
                        current.progress = found.progress;
                        current.detail = found.detail;
                    }
                }
                None => activity = Some(found),
            }
        }
    }
    activity
}

pub fn parse_explorer_title(title: &str) -> Option<LiveActivity> {
    let text = title.trim_start().to_lowercase();
    use ActivityKind::{Delete, FileOp, Move};
    let operation = [
        (&["copying", "đang sao chép", "sao chép"][..], tr("Đang sao chép", "Copying files"), FileOp),
        (&["moving", "đang di chuyển", "di chuyển"][..], tr("Đang di chuyển", "Moving files"), Move),
        (&["deleting", "recycling", "đang xóa", "xóa"][..], tr("Đang xóa", "Deleting files"), Delete),
        (&["preparing", "đang chuẩn bị", "chuẩn bị"][..], tr("Đang chuẩn bị", "Preparing files"), FileOp),
        (&["extracting", "đang giải nén", "giải nén"][..], tr("Đang giải nén", "Extracting files"), FileOp),
    ].into_iter().find(|(prefixes, ..)| prefixes.iter().any(|p| text.starts_with(p)));
    let percent = extract_percent(title);
    if operation.is_none() && percent.is_none() {
        return None;
    }
    Some(LiveActivity {
        kind: operation.map_or(FileOp, |(_, _, kind)| kind),
        title: operation.map_or(tr("Đang xử lý tệp", "Working on files"), |(_, label, _)| label).to_string(),
        detail: percent.map_or_else(|| tr("Đang xử lý…", "In progress…").to_string(), |p| format!("{p}%")),
        progress: percent.map(|p| p as f32 / 100.0),
        result: None, file: None, more: 0,
    })
}

pub fn extract_percent(s: &str) -> Option<u32> {
    let at = s.find('%')?;
    let before = s[..at].trim_end();
    let digits = before.chars().rev().take_while(char::is_ascii_digit).count();
    let start = before.len().checked_sub(digits)?;
    if digits == 0 || before[..start].ends_with(['.', ',', '-']) {
        return None;
    }
    before[start..].parse::<u32>().ok().filter(|&p| p <= 100)
}

fn find_download(dirs: &[PathBuf], current: Option<(&Path, u64, Instant)>, now: Instant, wall_now: SystemTime) -> Option<(PathBuf, u64)> {
    dirs.iter().filter_map(|dir| std::fs::read_dir(dir).ok()).flatten().flatten().filter_map(|entry| {
        let path = entry.path();
        if !is_download_part(&path) { return None; }
        let metadata = std::fs::metadata(&path).ok()?;
        if !metadata.is_file() { return None; }
        let modified = metadata.modified().ok()?;
        let selected = current.is_some_and(|(previous, size, changed)| previous == path
            && (metadata.len() != size || now.saturating_duration_since(changed) < Duration::from_secs(30)));
        if !selected && wall_now.duration_since(modified).unwrap_or_default() >= Duration::from_secs(30) {
            return None;
        }
        Some((path, metadata.len(), selected, modified))
    }).max_by_key(|(_, _, selected, modified)| (*selected, *modified))
        .map(|(path, size, _, _)| (path, size))
}

pub fn is_download_part(path: &Path) -> bool {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    ["crdownload", "part", "opdownload"].iter().any(|e| ext.eq_ignore_ascii_case(e))
}

pub fn clean_download_name(path: &Path) -> Option<String> {
    let stem = path.file_stem().and_then(|n| n.to_str()).unwrap_or("");
    let placeholder = !stem.contains('.') && stem.rsplit_once(' ').is_some_and(|(_, n)| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
    (!stem.is_empty() && !placeholder).then(|| stem.to_string())
}

fn history_name(rows: &[Vec<String>], part: &Path) -> Option<String> {
    let name = |row: &Vec<String>| Path::new(&row[1]).file_name()?.to_str().map(str::to_string);
    let own = rows.iter().find(|r| r.len() == 3 && r[0].eq_ignore_ascii_case(part.to_str().unwrap_or_default()));
    let same_folder = |r: &&Vec<String>| match (Path::new(&r[1]).parent(), part.parent()) {
        (Some(a), Some(b)) => a.as_os_str().eq_ignore_ascii_case(b.as_os_str()),
        _ => false,
    };
    let mut running = rows.iter().filter(|r| r.len() == 3 && r[2] == "0").filter(same_folder);
    own.or_else(|| running.next().filter(|_| running.next().is_none())).and_then(name)
}

fn finished_files(part: &Path) -> HashSet<PathBuf> {
    part.parent().and_then(|dir| std::fs::read_dir(dir).ok()).into_iter().flatten().flatten()
        .map(|entry| entry.path()).filter(|path| !is_download_part(path)).collect()
}

fn completed_file(part: &Path, size: u64, before: &HashSet<PathBuf>) -> Option<PathBuf> {
    if clean_download_name(part).is_some() {
        let named = part.with_extension("");
        return named.is_file().then_some(named);
    }
    std::fs::read_dir(part.parent()?).ok()?.flatten().filter_map(|entry| {
        let path = entry.path();
        let metadata = std::fs::metadata(&path).ok()?;
        let modified = metadata.modified().ok()?;
        (!before.contains(&path) && !is_download_part(&path) && metadata.is_file() && metadata.len() >= size).then_some((path, modified))
    }).max_by_key(|(_, modified)| *modified).map(|(path, _)| path)
}

fn downloads_dir() -> Option<PathBuf> {
    unsafe {
        if let Ok(raw) = SHGetKnownFolderPath(&FOLDERID_Downloads, KF_FLAG_DEFAULT, None) {
            let path = raw.to_string().ok().map(PathBuf::from);
            CoTaskMemFree(Some(raw.0 as _));
            if path.is_some() { return path; }
        }
    }
    std::env::var_os("USERPROFILE").map(|u| PathBuf::from(u).join("Downloads"))
}

const BROWSERS: &[&str] = &[r"Microsoft\Edge\User Data", r"Google\Chrome\User Data", r"BraveSoftware\Brave-Browser\User Data"];

fn download_dirs() -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut dirs: Vec<PathBuf> = downloads_dir().into_iter().collect();
    let mut histories = Vec::new();
    let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    let profiles = local.iter().flat_map(|l| BROWSERS.iter().map(move |b| l.join(b)))
        .filter_map(|root| std::fs::read_dir(root).ok()).flatten().flatten();
    for profile in profiles {
        let Ok(text) = std::fs::read_to_string(profile.path().join("Preferences")) else { continue };
        histories.push(profile.path().join("History"));
        if let Some(dir) = chromium_download_dir(&text) {
            if !dirs.iter().any(|d| d.as_os_str().eq_ignore_ascii_case(dir.as_os_str())) {
                dirs.push(dir);
            }
        }
    }
    (dirs, histories)
}

fn chromium_download_dir(preferences: &str) -> Option<PathBuf> {
    let root = JsonObject::Parse(&HSTRING::from(preferences)).ok()?;
    let dir = root.GetNamedObject(&HSTRING::from("download")).ok()?.GetNamedString(&HSTRING::from("default_directory")).ok()?;
    (!dir.is_empty()).then(|| PathBuf::from(dir.to_string()))
}

const BUILD_TOOLS: &[&str] = &["cargo.exe", "msbuild.exe", "dotnet.exe", "make.exe", "cmake.exe", "ninja.exe", "go.exe"];
const TERMINALS: &[&str] = &["cmd.exe", "powershell.exe", "pwsh.exe", "bash.exe", "wsl.exe", "rtk.exe"];

fn listed(list: &[&str], exe: &str) -> bool {
    list.iter().any(|t| exe.eq_ignore_ascii_case(t))
}

fn build_name(exe: &str) -> &'static str {
    match exe.to_ascii_lowercase().as_str() {
        "cargo.exe" => "Cargo",
        "msbuild.exe" => "MSBuild",
        "dotnet.exe" => ".NET",
        "make.exe" => "make",
        "cmake.exe" => "CMake",
        "ninja.exe" => "Ninja",
        _ => "Go",
    }
}

pub fn pick_build(procs: &[(u32, u32, String)]) -> Option<(u32, String)> {
    procs
        .iter()
        .find(|(_, parent, exe)| listed(BUILD_TOOLS, exe) && procs.iter().any(|(pid, _, p)| pid == parent && listed(TERMINALS, p)))
        .map(|(pid, _, exe)| (*pid, exe.clone()))
}

fn process_list() -> Vec<(u32, u32, String)> {
    let mut out = Vec::new();
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return out };
        let mut entry = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut more = Process32FirstW(snapshot, &mut entry).is_ok();
        while more {
            let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
            let name = &entry.szExeFile[..len];
            if [BUILD_TOOLS, TERMINALS].iter().any(|list| list.iter().any(|t| same_name(name, t))) {
                out.push((entry.th32ProcessID, entry.th32ParentProcessID, String::from_utf16_lossy(name)));
            }
            more = Process32NextW(snapshot, &mut entry).is_ok();
        }
        let _ = CloseHandle(snapshot);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_file_operations_have_specific_labels_before_percent_is_known() {
        for (text, label) in [
            ("Copying 3 items", tr("Đang sao chép", "Copying files")),
            ("Moving 3 items - 42% complete", tr("Đang di chuyển", "Moving files")),
            ("Deleting 3 items", tr("Đang xóa", "Deleting files")),
            ("Recycling 3 items", tr("Đang xóa", "Deleting files")),
            ("Đang sao chép 3 mục", tr("Đang sao chép", "Copying files")),
            ("Đang di chuyển 3 mục", tr("Đang di chuyển", "Moving files")),
            ("Đang xóa 3 mục", tr("Đang xóa", "Deleting files")),
        ] {
            assert_eq!(parse_explorer_title(text).unwrap().title, label, "{text}");
        }
    }

    #[test]
    fn percentages_allow_localized_spacing_without_cutting_utf8() {
        assert_eq!(extract_percent("37 % complete"), Some(37));
        assert_eq!(extract_percent("完成42%"), Some(42));
        assert_eq!(extract_percent("37\u{a0}% complete"), Some(37));
        assert_eq!(extract_percent("12.5% complete"), None);
    }

    #[test]
    fn disabling_does_not_report_success_for_a_running_task() {
        let mut monitor = Monitor::new(false);
        monitor.last = vec![LiveActivity {
            kind: ActivityKind::Download, title: "file.zip".into(),
            detail: "5 MB".into(), progress: None, result: None, file: None, more: 0,
        }];
        assert!(monitor.poll(Instant::now(), true).is_none());
        assert!(monitor.take_finished().is_none());
    }

    #[test]
    fn an_idle_reader_waits_for_events_and_only_checks_in_now_and_then() {
        let now = Instant::now();
        assert_eq!(wait_plan(true, now, now), (false, ACTIVE_POLL), "a running task is sampled often and ignores folder noise");
        assert_eq!(wait_plan(false, now, now), (true, IDLE_CHECK), "idle: listen to the folders, check slowly");
        let quiet = now + Duration::from_millis(400);
        assert_eq!(wait_plan(false, now, quiet), (false, Duration::from_millis(400)), "after a folder change the next ones are folded for a moment");
        assert_eq!(wait_plan(false, quiet, quiet), (true, IDLE_CHECK));
    }

    #[test]
    fn names_are_compared_without_building_a_string_and_without_caring_about_case() {
        let units = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
        assert!(same_name(&units("Cargo.exe"), "cargo.exe") && same_name(&units("CARGO.EXE"), "cargo.exe"));
        assert!(!same_name(&units("cargo.exe2"), "cargo.exe") && !same_name(&units("cargo"), "cargo.exe"));
        assert!(!same_name(&units("cargo.exé"), "cargo.exe"), "a non-ASCII unit is never a match");
        assert!(!same_name(&[], "cargo.exe"));
    }

    #[test]
    fn the_download_folders_are_scanned_on_events_or_every_so_often() {
        let now = Instant::now();
        assert!(download_scan_due(None, now), "the first look");
        assert!(!download_scan_due(Some(now), now + DOWNLOAD_SCAN - Duration::from_millis(1)));
        assert!(download_scan_due(Some(now), now + DOWNLOAD_SCAN));
    }

    #[test]
    fn explorer_uses_the_operation_label_with_the_window_percentage() {
        let activity = parse_explorer_labels("42% complete", &["Moving 3 items from A to B".into()]).unwrap();
        assert_eq!(activity.title, tr("Đang di chuyển", "Moving files"));
        assert_eq!(activity.progress, Some(0.42));
        let moving = parse_explorer_labels("42% complete", &["Moving 3 items from A to B".into()]).unwrap();
        assert_eq!(moving.kind, ActivityKind::Move);
        let deleting = parse_explorer_labels("30% complete", &["Deleting 3 items from A".into()]).unwrap();
        assert_eq!((deleting.kind, deleting.progress), (ActivityKind::Delete, Some(0.30)), "its own kind, for the trash icon");
        assert!(names_operation(&deleting) && !names_operation(&parse_explorer_title("30% complete").unwrap()));
        let preparing = parse_explorer_labels("File Explorer", &["Preparing to copy".into()]).unwrap();
        assert!(!names_operation(&preparing), "preparing is not an operation to remember for the dialog");
        assert_eq!(preparing.progress, None);
        assert!(parse_explorer_labels("File conflict", &["Replace".into(), "Skip".into()]).is_none());
        assert!(parse_explorer_labels("File conflict", &["archive100%.zip".into()]).is_none());
    }

    #[test]
    fn reader_keeps_progress_between_samples_and_forgets_it_when_disabled() {
        let (updates, receiver) = channel();
        let now = Instant::now();
        let activity = LiveActivity { kind: ActivityKind::Download, title: "movie.zip".into(), detail: "5 MB".into(), progress: None, result: None, file: None, more: 0 };
        let mut watch = Watch { enabled: true, reader: Some(Reader {
            stop: Arc::new(AtomicBool::new(false)), updates: receiver, current: None, finished: None, received_at: now,
        }) };
        updates.send((Some(activity.clone()), None)).unwrap();
        assert_eq!(watch.poll(now, 0), Some(activity.clone()));
        assert_eq!(watch.latest(now + Duration::from_millis(500)), Some(activity.clone()));
        assert!(watch.take_finished().is_none(), "a pending scan is not a completed task");
        updates.send((None, Some(activity.clone()))).unwrap();
        updates.send((None, None)).unwrap();
        assert!(watch.latest(now + Duration::from_secs(1)).is_none());
        assert_eq!(watch.take_finished(), Some(activity), "a later empty sample keeps the ending");
        assert!(watch.take_finished().is_none());
        watch.enabled = false;
        assert!(watch.latest(now).is_none());
        assert!(watch.poll(now, 0).is_none());
        assert!(watch.reader.is_none() && watch.take_finished().is_none());
    }

    #[test]
    fn downloads_ignore_stale_partials_and_keep_the_selected_real_file() {
        let now = SystemTime::now();
        let tick = Instant::now();
        let old_tick = tick - Duration::from_secs(31);
        let dir = std::env::temp_dir().join(format!("puw-activity-{}-{}", std::process::id(),
            now.duration_since(SystemTime::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir(&dir).unwrap();
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                for name in ["old.part", "first.crdownload", "second.part"] {
                    let _ = std::fs::remove_file(self.0.join(name));
                }
                let _ = std::fs::remove_dir(self.0.join("directory.part"));
                let _ = std::fs::remove_dir(&self.0);
            }
        }
        let _cleanup = Cleanup(dir.clone());
        let dirs = [dir.clone()];
        let old = dir.join("old.part");
        std::fs::write(&old, b"old").unwrap();
        std::fs::File::options().write(true).open(&old).unwrap()
            .set_modified(now - Duration::from_secs(60)).unwrap();
        std::fs::create_dir(dir.join("directory.part")).unwrap();
        assert!(find_download(&dirs, None, tick, now).is_none());
        assert!(find_download(&dirs, Some((&old, 3, old_tick)), tick, now).is_none(), "an abandoned selection must expire");
        let first = dir.join("first.crdownload");
        std::fs::write(&first, b"12345").unwrap();
        std::fs::File::options().write(true).open(&first).unwrap()
            .set_modified(now - Duration::from_secs(1)).unwrap();
        let second = dir.join("second.part");
        std::fs::write(&second, b"1234567890").unwrap();
        std::fs::File::options().write(true).open(&second).unwrap().set_modified(now).unwrap();
        assert_eq!(find_download(&dirs, None, tick, now), Some((second.clone(), 10)));
        assert_eq!(find_download(&dirs, Some((&first, 5, tick)), tick, now), Some((first, 5)));
        assert_eq!(find_download(&dirs, Some((&old, 3, old_tick)), tick, now), Some((second, 10)));
        assert_eq!(find_download(&dirs, Some((&old, 2, old_tick)), tick, now), Some((old, 3)),
            "a growing download stays active even if LastWriteTime is old");
    }

    #[test]
    #[ignore = "read-only probe of live Explorer windows and the configured Downloads folder"]
    fn live_activity_probe() {
        let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        let (dirs, histories) = download_dirs();
        println!("Download folders: {dirs:?}");
        for h in &histories {
            let rows = crate::notify::query_unlocked(h, "SELECT current_path, target_path, state FROM downloads ORDER BY id DESC LIMIT 2", 3);
            println!("History {h:?}: {rows:?}");
        }
        let mut monitor = Monitor::new(true);
        for _ in 0..3 {
            let t = Instant::now();
            let found = monitor.poll_explorer(t);
            println!("Explorer ({} ms): {found:?}", t.elapsed().as_millis());
        }
        if com.is_ok() { unsafe { CoUninitialize(); } }
    }

    fn visible_top_level_windows() -> Vec<HWND> {
        unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
            if IsWindowVisible(hwnd).as_bool() {
                (*(lparam.0 as *mut Vec<HWND>)).push(hwnd);
            }
            true.into()
        }
        let mut windows = Vec::<HWND>::new();
        unsafe { let _ = EnumWindows(Some(visit), LPARAM(&mut windows as *mut _ as isize)); }
        windows
    }

    fn describe_window(hwnd: HWND) -> String {
        let mut class = [0u16; 256];
        let n = unsafe { GetClassNameW(hwnd, &mut class) }.max(0) as usize;
        let mut pid = 0u32;
        unsafe { windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(hwnd, Some(&mut pid)); }
        format!("class={:?} title={:?} pid={pid}", String::from_utf16_lossy(&class[..n]), window_title(hwnd))
    }

    #[test]
    #[ignore = "read-only trace for 90 s: prints every window that appears or goes away with its class, title and the names inside it; delete, move and copy a file that asks to replace while it runs"]
    fn trace_new_windows_while_you_copy_move_and_delete() {
        let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        let mut monitor = Monitor::new(true);
        let mut known: std::collections::HashMap<isize, String> = visible_top_level_windows().into_iter().map(|h| (h.0 as isize, describe_window(h))).collect();
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(90) {
            let now_windows = visible_top_level_windows();
            for hwnd in &now_windows {
                let id = hwnd.0 as isize;
                if !known.contains_key(&id) {
                    let line = describe_window(*hwnd);
                    let labels = monitor.dialog_labels(*hwnd);
                    println!("[{:>5} ms] OPEN  {line} names={:?}", start.elapsed().as_millis(), &labels[..labels.len().min(14)]);
                    known.insert(id, line);
                }
            }
            known.retain(|id, line| {
                let alive = now_windows.iter().any(|h| h.0 as isize == *id);
                if !alive {
                    println!("[{:>5} ms] CLOSE {line}", start.elapsed().as_millis());
                }
                alive
            });
            std::thread::sleep(Duration::from_millis(250));
        }
        if com.is_ok() { unsafe { CoUninitialize(); } }
    }

    #[test]
    fn a_process_tells_how_it_exited_only_once_it_is_gone() {
        let mut child = std::process::Command::new("cmd").args(["/c", "exit 3"]).spawn().unwrap();
        let proc = Proc::open(child.id()).expect("open own child");
        child.wait().unwrap();
        assert_eq!(proc.exit_code(), Some(3));
        let mut running = std::process::Command::new("cmd").args(["/c", "pause"]).stdin(std::process::Stdio::piped()).spawn().unwrap();
        let proc = Proc::open(running.id()).unwrap();
        assert_eq!(proc.exit_code(), None, "still running");
        running.kill().unwrap();
        running.wait().unwrap();
    }

    #[test]
    fn a_hidden_activity_still_ends_and_counts_toward_the_others() {
        let act = |kind| LiveActivity { kind, title: "x".into(), detail: String::new(), progress: None, result: None, file: None, more: 0 };
        let last = vec![act(ActivityKind::Move), act(ActivityKind::Build)];
        let ended = ended_activity(last.clone(), &[act(ActivityKind::Delete)]);
        assert_eq!(ended.map(|a| a.kind), Some(ActivityKind::Build), "the build ended behind the copy; move to delete is no end");
        assert!(ended_activity(last, &[act(ActivityKind::FileOp), act(ActivityKind::Build)]).is_none(), "nothing ended");
    }

    #[test]
    fn a_file_operation_shows_only_once_it_has_run_a_moment_and_is_named_or_has_progress() {
        let mut monitor = Monitor::default();
        let t0 = Instant::now();
        let copying = parse_explorer_title("Copying 12% complete");
        assert!(monitor.confirm_explorer(copying.clone(), t0).is_none());
        assert!(monitor.waiting());
        assert!(monitor.confirm_explorer(copying.clone(), t0 + CONFIRM_AFTER / 2).is_none());
        assert_eq!(monitor.confirm_explorer(copying, t0 + CONFIRM_AFTER).map(|a| a.kind), Some(ActivityKind::FileOp));
        assert!(!monitor.waiting());
    }

    #[test]
    fn preparing_and_cancelled_operations_never_show() {
        let mut monitor = Monitor::default();
        let t0 = Instant::now();
        let preparing = parse_explorer_title("Preparing to copy");
        for secs in [0, 1, 5, 30] {
            assert!(monitor.confirm_explorer(preparing.clone(), t0 + Duration::from_secs(secs)).is_none(), "{secs} s of preparing");
        }
        assert!(monitor.waiting());
        assert!(monitor.confirm_explorer(None, t0 + Duration::from_secs(31)).is_none());
        assert!(!monitor.waiting());
        let deleting = parse_explorer_title("Deleting 3%");
        assert!(monitor.confirm_explorer(deleting.clone(), t0 + Duration::from_secs(40)).is_none());
        assert!(monitor.confirm_explorer(deleting, t0 + Duration::from_secs(41)).is_some());
    }

    #[test]
    fn percent_from_titles() {
        assert_eq!(extract_percent("35% complete"), Some(35));
        assert_eq!(extract_percent("100% hoàn thành"), Some(100));
        assert_eq!(extract_percent("Đang sao chép - 42%"), Some(42));
        assert_eq!(extract_percent("Copying files..."), None);
        assert_eq!(extract_percent("120% error"), None);
    }

    #[test]
    fn unknown_dialog_text_without_progress_is_not_an_activity() {
        assert_eq!(parse_explorer_title("File conflict"), None);
        let a = parse_explorer_title("37% complete").unwrap();
        assert_eq!((a.kind, a.progress, a.detail.as_str()), (ActivityKind::FileOp, Some(0.37), "37%"));
    }

    #[test]
    fn chromium_profiles_name_their_own_download_folder() {
        let prefs = r#"{"savefile":{"default_directory":"D:\\Pages"},"download":{"default_directory":"E:\\Download","prompt_for_download":false}}"#;
        assert_eq!(chromium_download_dir(prefs), Some(PathBuf::from(r"E:\Download")));
        assert_eq!(chromium_download_dir(r#"{"download":{"prompt_for_download":true}}"#), None, "the browser uses Windows Downloads");
        assert_eq!(chromium_download_dir("not json"), None);
    }

    #[test]
    fn an_unconfirmed_download_takes_its_name_from_the_browser_history() {
        let row = |cur: &str, target: &str, state: &str| vec![cur.to_string(), target.to_string(), state.to_string()];
        let part = Path::new(r"E:\Download\Chưa xác nhận 98941.crdownload");
        let rows = [row(r"E:\Download\Chưa xác nhận 98941.crdownload", r"E:\Download\model.gguf", "0"), row(r"E:\Download\old.zip", r"E:\Download\old.zip", "1")];
        assert_eq!(history_name(&rows, part).as_deref(), Some("model.gguf"));
        let other_path = [row(r"E:\Download\x.crdownload", r"E:\Download\only.iso", "0")];
        assert_eq!(history_name(&other_path, part).as_deref(), Some("only.iso"), "the one download running");
        let elsewhere = [row("x", r"D:\Other\only.iso", "0")];
        assert_eq!(history_name(&elsewhere, part), None, "a download of another folder is not this one");
        let two = [row("a", r"E:\Download\a.iso", "0"), row("b", r"E:\Download\b.iso", "0")];
        assert_eq!(history_name(&two, part), None, "two running: no guessing");
        assert_eq!(history_name(&[], part), None);
    }

    #[test]
    fn a_vanished_partial_succeeded_when_its_file_is_there() {
        let now = SystemTime::now();
        let dir = std::env::temp_dir().join(format!("puw-done-{}-{}", std::process::id(), now.duration_since(SystemTime::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir(&dir).unwrap();
        struct Cleanup(PathBuf);
        impl Drop for Cleanup { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }
        let _cleanup = Cleanup(dir.clone());
        let unconfirmed = dir.join("Unconfirmed 1.crdownload");
        std::fs::write(dir.join("existing.bin"), vec![0u8; 100]).unwrap();
        let before = finished_files(&unconfirmed);
        assert_eq!(completed_file(&unconfirmed, 5, &before), None, "cancelled: nothing new");
        std::fs::write(dir.join("small.txt"), b"1").unwrap();
        assert_eq!(completed_file(&unconfirmed, 5, &before), None, "smaller than what was downloaded");
        std::fs::write(dir.join("model.gguf"), b"123456").unwrap();
        assert_eq!(completed_file(&unconfirmed, 5, &before), Some(dir.join("model.gguf")));
        assert_eq!(completed_file(&dir.join("video.mp4.crdownload"), 1, &before), None, "cancelled named download");
        std::fs::write(dir.join("video.mp4"), b"1").unwrap();
        assert_eq!(completed_file(&dir.join("video.mp4.crdownload"), 1, &before), Some(dir.join("video.mp4")));
    }

    #[test]
    fn download_names_and_parts() {
        assert_eq!(clean_download_name(Path::new("video.mp4.crdownload")).as_deref(), Some("video.mp4"));
        assert_eq!(clean_download_name(Path::new("setup.exe.part")).as_deref(), Some("setup.exe"));
        for unnamed in ["Unconfirmed 98941.crdownload", "Chưa xác nhận 98941.crdownload"] {
            assert_eq!(clean_download_name(Path::new(unnamed)), None, "{unnamed}");
        }
        assert_eq!(clean_download_name(Path::new("model v4 2.gguf.crdownload")).as_deref(), Some("model v4 2.gguf"));
        assert!(is_download_part(Path::new("a.crdownload")) && is_download_part(Path::new("b.PART")));
        assert!(!is_download_part(Path::new("c.mp4")));
    }

    #[test]
    fn download_and_time_text() {
        assert_eq!(download_detail(5 * 1024 * 1024, 0.0), "5.0 MB");
        assert_eq!(download_detail(5 * 1024 * 1024, 2.0 * 1024.0 * 1024.0), "5.0 MB · 2.0 MB/s");
        assert_eq!((elapsed_text(7).as_str(), elapsed_text(75).as_str()), ("7s", "1m 15s"));
    }

    #[test]
    fn only_a_build_tool_a_terminal_started_counts() {
        let p = |pid, parent, exe: &str| (pid, parent, exe.to_string());
        let shell = [p(1, 0, "powershell.exe"), p(2, 1, "cargo.exe"), p(3, 2, "rustc.exe")];
        assert_eq!(pick_build(&shell).map(|(_, exe)| exe).as_deref(), Some("cargo.exe"));
        let wrapped = [p(1, 0, "pwsh.exe"), p(2, 1, "rtk.exe"), p(3, 2, "cargo.exe")];
        assert_eq!(pick_build(&wrapped).map(|(_, exe)| exe).as_deref(), Some("cargo.exe"));
        let ide = [p(1, 0, "Code.exe"), p(2, 1, "cargo.exe")];
        assert_eq!(pick_build(&ide), None, "not from a terminal");
        let nested = [p(1, 0, "cmd.exe"), p(2, 1, "node.exe")];
        assert_eq!(pick_build(&nested), None, "node is not a build tool");
    }
}

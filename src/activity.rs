//! Live background activities shown as their own banner: Explorer file operations (percent from the
//! progress dialog's title), browser downloads (size and speed of the partial file), terminal builds
//! (the build tool a shell started, and how long it has run).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, sync_channel, Receiver, RecvTimeoutError, SyncSender};
use std::time::{Duration, Instant, SystemTime};

use windows::core::{BOOL, HSTRING, Interface};
use windows::Data::Json::JsonObject;
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
use windows::Win32::UI::Accessibility::{CUIAutomation, IUIAutomation, IUIAutomation2, TreeScope_Descendants};
use windows::Win32::UI::Shell::{SHGetKnownFolderPath, FOLDERID_Downloads, KF_FLAG_DEFAULT};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetClassNameW, GetWindowTextW, IsWindowVisible, PostMessageW, WM_APP};

/// A sample with something to show is ready (`Watch::latest`).
pub const WM_ACTIVITY: u32 = WM_APP + 15;

use crate::i18n::tr;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivityKind {
    FileOp,
    Download,
    Build,
}

/// One running activity, as the banner shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct LiveActivity {
    pub kind: ActivityKind,
    pub title: String,
    pub detail: String,
    /// 0.0 ..= 1.0 when the activity says how far it is; None when it does not.
    pub progress: Option<f32>,
    /// An ended activity whose result was seen (the downloaded file is there); false while running
    /// and when it only stopped being seen.
    pub succeeded: bool,
}

/// UI-side reader. Filesystem and accessibility calls never run on the animation thread.
pub struct Watch {
    pub enabled: bool,
    reader: Option<Reader>,
}

struct Reader {
    request: SyncSender<()>,
    updates: Receiver<(Option<LiveActivity>, Option<LiveActivity>)>,
    current: Option<LiveActivity>,
    finished: Option<LiveActivity>,
    received_at: Instant,
}

impl Watch {
    pub fn new(enabled: bool) -> Self {
        Self { enabled, reader: None }
    }

    /// Asks for a new sample and returns the newest one there is. Each sample that has something to
    /// show also posts WM_ACTIVITY to `target`, so it reaches the banner without waiting for this tick.
    pub fn poll(&mut self, now: Instant, target: isize) -> Option<LiveActivity> {
        if !self.enabled {
            // Dropping both channels stops the worker and discards old completion events.
            self.reader = None;
            return None;
        }
        let reader = self.reader.get_or_insert_with(|| Reader::new(target));
        // At most one sample waits behind the current scan.
        let _ = reader.request.try_send(());
        self.latest(now)
    }

    /// The newest sample, without asking for another (for WM_ACTIVITY).
    pub fn latest(&mut self, now: Instant) -> Option<LiveActivity> {
        let reader = self.reader.as_mut().filter(|_| self.enabled)?;
        for (current, finished) in reader.updates.try_iter() {
            reader.current = current;
            // Several samples can arrive at once; a later one must not erase an ending not yet shown.
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
        let (request, requests) = sync_channel::<()>(1);
        let (updates, receiver) = channel();
        crate::system::spawn("live-activity", move || {
            let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
            let mut monitor = Monitor::new(true);
            let mut active = false;
            loop {
                // While something runs, sample on its own 4 times a second: a fast copy moves several
                // percent between the UI's ticks. Idle, wait for the tick (a scan costs ~20-30 ms).
                // The sample is stamped by this thread, not by the UI that asked: a speed is a size
                // difference over the time between two reads of the file.
                if active {
                    if let Err(RecvTimeoutError::Disconnected) = requests.recv_timeout(Duration::from_millis(250)) {
                        break;
                    }
                } else if requests.recv().is_err() {
                    break;
                }
                let current = monitor.poll(Instant::now());
                let finished = monitor.take_finished();
                let show = current.is_some() || finished.is_some();
                active = current.is_some();
                if updates.send((current, finished)).is_err() {
                    break;
                }
                if show {
                    unsafe { let _ = PostMessageW(Some(HWND(target as _)), WM_ACTIVITY, WPARAM(0), LPARAM(0)); }
                }
            }
            if com.is_ok() {
                unsafe { CoUninitialize(); }
            }
        });
        Self { request, updates: receiver, current: None, finished: None, received_at: Instant::now() }
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
    /// The real name of the selected partial file, from the browser's history when the file has none yet.
    download_name: Option<String>,
    download_lookup_at: Option<Instant>,
    /// The file a download that just vanished became, when it is there.
    download_done: Option<PathBuf>,
    /// The folder's finished files when the selected partial appeared (see `completed_file`).
    download_before: HashSet<PathBuf>,
    build_exe: Option<String>,
    build_since: Option<Instant>,
    last: Option<LiveActivity>,
    finished: Option<LiveActivity>,
}

impl Monitor {
    pub fn new(enabled: bool) -> Self {
        Self { enabled, ..Default::default() }
    }

    /// What runs right now (None when nothing, or when switched off). An activity that was there
    /// and is gone becomes `take_finished`, except file operations: Explorer says it is done itself.
    pub fn poll(&mut self, now: Instant) -> Option<LiveActivity> {
        if !self.enabled {
            self.last = None;
            self.finished = None;
            return None;
        }
        let current = poll_explorer().or_else(|| self.poll_download(now)).or_else(|| self.poll_build(now));
        if current.is_none() {
            if let Some(mut a) = self.last.take() {
                if a.kind == ActivityKind::Download {
                    if let Some(file) = self.download_done.take() {
                        a.title = file.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
                        a.detail = std::fs::metadata(&file).map_or(a.detail, |m| download_detail(m.len(), 0.0));
                        a.succeeded = true;
                    }
                }
                self.finished = (a.kind != ActivityKind::FileOp).then_some(a);
            }
        }
        self.last = current.clone();
        current
    }

    /// The activity that just ended, once.
    pub fn take_finished(&mut self) -> Option<LiveActivity> {
        self.finished.take()
    }

    fn poll_download(&mut self, now: Instant) -> Option<LiveActivity> {
        let selected = self.download_file.as_deref().zip(self.download_changed_at)
            .map(|(path, changed)| (path, self.download_size, changed));
        // Browser settings change rarely; reading every profile's Preferences each tick is wasted work.
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
        // Chromium may keep "Unconfirmed 98941" on disk the whole time; its history names the file.
        // Ask again every 2 s until the row is written (never, in InPrivate: the title stays generic).
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
        Some(LiveActivity { kind: ActivityKind::Download, title, detail: download_detail(size, self.download_speed), progress: None, succeeded: false })
    }

    fn poll_build(&mut self, now: Instant) -> Option<LiveActivity> {
        let Some(exe) = pick_build(&process_list()) else {
            self.build_exe = None;
            self.build_since = None;
            return None;
        };
        if self.build_exe.as_deref() != Some(exe.as_str()) {
            self.build_exe = Some(exe.clone());
            self.build_since = Some(now);
        }
        let secs = self.build_since.map_or(0, |t| now.duration_since(t).as_secs());
        Some(LiveActivity { kind: ActivityKind::Build, title: build_name(&exe).to_string(), detail: elapsed_text(secs), progress: None, succeeded: false })
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

/// Explorer's progress windows, including a minimized dialog and its operation label.
fn poll_explorer() -> Option<LiveActivity> {
    unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
        if IsWindowVisible(hwnd).as_bool() {
            let mut class = [0u16; 64];
            let n = GetClassNameW(hwnd, &mut class);
            if n > 0 && String::from_utf16_lossy(&class[..n as usize]) == "OperationStatusWindow" {
                (*(lparam.0 as *mut Vec<HWND>)).push(hwnd);
            }
        }
        true.into()
    }

    let mut windows = Vec::<HWND>::new();
    unsafe { let _ = EnumWindows(Some(visit), LPARAM(&mut windows as *mut _ as isize)); }
    if windows.is_empty() {
        return None;
    }
    let automation: Option<IUIAutomation> = unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok() };
    if let Some(timeout) = automation.as_ref().and_then(|a| a.cast::<IUIAutomation2>().ok()) {
        unsafe {
            let _ = timeout.SetConnectionTimeout(200);
            let _ = timeout.SetTransactionTimeout(200);
        }
    }
    for hwnd in windows {
        let mut title = [0u16; 256];
        let n = unsafe { GetWindowTextW(hwnd, &mut title) }.max(0) as usize;
        let title = String::from_utf16_lossy(&title[..n]);
        let mut labels = Vec::new();
        if let Some(automation) = &automation {
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
        }
        if let Some(activity) = parse_explorer_labels(&title, &labels) {
            return Some(activity);
        }
    }
    None
}

fn parse_explorer_labels(title: &str, labels: &[String]) -> Option<LiveActivity> {
    let mut activity = parse_explorer_title(title);
    let generic = tr("Đang xử lý tệp", "Working on files");
    for label in labels {
        if let Some(found) = parse_explorer_title(label) {
            // File names in the dialog can contain '%' too; only progress labels carry a percent.
            let lower = label.trim().to_lowercase();
            if found.title == generic && !lower.ends_with("complete") && !lower.ends_with("hoàn thành") {
                continue;
            }
            match &mut activity {
                Some(current) => {
                    if current.title == generic && found.title != generic {
                        current.title = found.title;
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

/// The percent is optional while Explorer is preparing, counting, or deleting files.
pub fn parse_explorer_title(title: &str) -> Option<LiveActivity> {
    let text = title.trim_start().to_lowercase();
    let operation = [
        (&["copying", "đang sao chép", "sao chép"][..], tr("Đang sao chép", "Copying files")),
        (&["moving", "đang di chuyển", "di chuyển"][..], tr("Đang di chuyển", "Moving files")),
        (&["deleting", "recycling", "đang xóa", "xóa"][..], tr("Đang xóa", "Deleting files")),
        (&["preparing", "đang chuẩn bị", "chuẩn bị"][..], tr("Đang chuẩn bị", "Preparing files")),
        (&["extracting", "đang giải nén", "giải nén"][..], tr("Đang giải nén", "Extracting files")),
    ].into_iter().find_map(|(prefixes, label)| prefixes.iter().any(|p| text.starts_with(p)).then_some(label));
    let percent = extract_percent(title);
    if operation.is_none() && percent.is_none() {
        return None;
    }
    Some(LiveActivity {
        kind: ActivityKind::FileOp,
        title: operation.unwrap_or(tr("Đang xử lý tệp", "Working on files")).to_string(),
        detail: percent.map_or_else(|| tr("Đang xử lý…", "In progress…").to_string(), |p| format!("{p}%")),
        progress: percent.map(|p| p as f32 / 100.0),
        succeeded: false,
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

/// Keep the selected transfer stable; ignore abandoned partials that have not changed recently.
fn find_download(dirs: &[PathBuf], current: Option<(&Path, u64, Instant)>, now: Instant, wall_now: SystemTime) -> Option<(PathBuf, u64)> {
    dirs.iter().filter_map(|dir| std::fs::read_dir(dir).ok()).flatten().flatten().filter_map(|entry| {
        let path = entry.path();
        if !is_download_part(&path) { return None; }
        // The directory entry's size and time lag behind a file still open for writing; ask the file.
        let metadata = std::fs::metadata(&path).ok()?;
        if !metadata.is_file() { return None; }
        let modified = metadata.modified().ok()?;
        // Size growth remains authoritative even when Windows delays updating LastWriteTime.
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

/// The name of the file a partial one becomes; None while it has none yet.
pub fn clean_download_name(path: &Path) -> Option<String> {
    let stem = path.file_stem().and_then(|n| n.to_str()).unwrap_or("");
    // Chromium writes "Unconfirmed 98941.crdownload" (localized: "Chưa xác nhận 98941") until it knows
    // the real name, and may keep it for the whole download; that placeholder is no name.
    let placeholder = !stem.contains('.') && stem.rsplit_once(' ').is_some_and(|(_, n)| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
    (!stem.is_empty() && !placeholder).then(|| stem.to_string())
}

/// The real name of `part` from a Chromium history's `(current_path, target_path, state)` rows: its own
/// row, else the only download still in progress (state 0) that is headed for the partial's folder.
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

/// What is in the partial's folder besides partial files: a finished download is what is new there later.
fn finished_files(part: &Path) -> HashSet<PathBuf> {
    part.parent().and_then(|dir| std::fs::read_dir(dir).ok()).into_iter().flatten().flatten()
        .map(|entry| entry.path()).filter(|path| !is_download_part(path)).collect()
}

/// The finished file a partial download that just vanished became. A named partial becomes its own name
/// without the extension (and nothing else: no such file means it was cancelled). An "Unconfirmed" one
/// has no such name, so it is the file that appeared in its folder since `before` and is at least as
/// large (not judged by modification time: a browser may keep the server's).
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
    // Respect the Windows Downloads location, including a folder moved to another drive.
    unsafe {
        if let Ok(raw) = SHGetKnownFolderPath(&FOLDERID_Downloads, KF_FLAG_DEFAULT, None) {
            let path = raw.to_string().ok().map(PathBuf::from);
            CoTaskMemFree(Some(raw.0 as _));
            if path.is_some() { return path; }
        }
    }
    std::env::var_os("USERPROFILE").map(|u| PathBuf::from(u).join("Downloads"))
}

/// Chromium browsers keep their own download folder (Edge set to E:\Download saves nothing in Downloads).
const BROWSERS: &[&str] = &[r"Microsoft\Edge\User Data", r"Google\Chrome\User Data", r"BraveSoftware\Brave-Browser\User Data"];

/// Windows Downloads plus every Chromium profile's download folder, without duplicates; and every
/// profile's History database (where the real name of an "Unconfirmed" download is).
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

/// Build tools that count; node, python and the like run all day without building anything.
const BUILD_TOOLS: &[&str] = &["cargo.exe", "msbuild.exe", "dotnet.exe", "make.exe", "cmake.exe", "ninja.exe", "go.exe"];
/// What a terminal's command line runs under (rtk wraps the commands that are typed).
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

/// The build tool a terminal started, from `(pid, parent pid, exe)` of every process; the tools a
/// tool starts itself (rustc under cargo) have a tool as parent and do not count twice.
pub fn pick_build(procs: &[(u32, u32, String)]) -> Option<String> {
    procs
        .iter()
        .find(|(_, parent, exe)| listed(BUILD_TOOLS, exe) && procs.iter().any(|(pid, _, p)| pid == parent && listed(TERMINALS, p)))
        .map(|(_, _, exe)| exe.clone())
}

fn process_list() -> Vec<(u32, u32, String)> {
    let mut out = Vec::new();
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return out };
        let mut entry = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut more = Process32FirstW(snapshot, &mut entry).is_ok();
        while more {
            let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
            out.push((entry.th32ProcessID, entry.th32ParentProcessID, String::from_utf16_lossy(&entry.szExeFile[..len])));
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
        monitor.last = Some(LiveActivity {
            kind: ActivityKind::Download, title: "file.zip".into(),
            detail: "5 MB".into(), progress: None, succeeded: false,
        });
        assert!(monitor.poll(Instant::now()).is_none());
        assert!(monitor.take_finished().is_none());
    }

    #[test]
    fn explorer_uses_the_operation_label_with_the_window_percentage() {
        let activity = parse_explorer_labels("42% complete", &["Moving 3 items from A to B".into()]).unwrap();
        assert_eq!(activity.title, tr("Đang di chuyển", "Moving files"));
        assert_eq!(activity.progress, Some(0.42));
        let preparing = parse_explorer_labels("File Explorer", &["Preparing to copy".into()]).unwrap();
        assert_eq!(preparing.progress, None);
        assert!(parse_explorer_labels("File conflict", &["Replace".into(), "Skip".into()]).is_none());
        assert!(parse_explorer_labels("File conflict", &["archive100%.zip".into()]).is_none());
    }

    #[test]
    fn reader_keeps_progress_between_samples_and_forgets_it_when_disabled() {
        let (request, _requests) = sync_channel(1);
        let (updates, receiver) = channel();
        let now = Instant::now();
        let activity = LiveActivity { kind: ActivityKind::Download, title: "movie.zip".into(), detail: "5 MB".into(), progress: None, succeeded: false };
        let mut watch = Watch { enabled: true, reader: Some(Reader {
            request, updates: receiver, current: None, finished: None, received_at: now,
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
        for _ in 0..3 {
            let t = Instant::now();
            let found = poll_explorer();
            println!("Explorer ({} ms): {found:?}", t.elapsed().as_millis());
        }
        if com.is_ok() { unsafe { CoUninitialize(); } }
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
        // A file that was already there is no finished download, however big and fresh it is.
        std::fs::write(dir.join("existing.bin"), vec![0u8; 100]).unwrap();
        let before = finished_files(&unconfirmed);
        assert_eq!(completed_file(&unconfirmed, 5, &before), None, "cancelled: nothing new");
        std::fs::write(dir.join("small.txt"), b"1").unwrap();
        assert_eq!(completed_file(&unconfirmed, 5, &before), None, "smaller than what was downloaded");
        std::fs::write(dir.join("model.gguf"), b"123456").unwrap();
        assert_eq!(completed_file(&unconfirmed, 5, &before), Some(dir.join("model.gguf")));
        // A named partial is its own file or nothing: another new file does not stand in for a cancelled one.
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
        assert_eq!(pick_build(&shell).as_deref(), Some("cargo.exe"));
        let wrapped = [p(1, 0, "pwsh.exe"), p(2, 1, "rtk.exe"), p(3, 2, "cargo.exe")];
        assert_eq!(pick_build(&wrapped).as_deref(), Some("cargo.exe"));
        let ide = [p(1, 0, "Code.exe"), p(2, 1, "cargo.exe")];
        assert_eq!(pick_build(&ide), None, "not from a terminal");
        let nested = [p(1, 0, "cmd.exe"), p(2, 1, "node.exe")];
        assert_eq!(pick_build(&nested), None, "node is not a build tool");
    }
}

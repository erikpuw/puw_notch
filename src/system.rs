use std::path::PathBuf;
use crate::weather::Place;
use windows::core::{s, w, GUID, PCWSTR};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::Foundation::{CloseHandle, ERROR_SUCCESS};
use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows::Win32::System::Registry::*;
use windows::Win32::Foundation::SYSTEMTIME;
use windows::Win32::System::SystemInformation::{GetLocalTime, GetSystemTime};

const WEEKDAYS: [&str; 7] = ["CN", "T2", "T3", "T4", "T5", "T6", "T7"];
const WEEKDAYS_EN: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

pub fn now_stamp() -> u64 {
    let t = unsafe { GetLocalTime() };
    crate::todo::stamp(t.wYear as u32 * 10_000 + t.wMonth as u32 * 100 + t.wDay as u32, t.wHour as u32, t.wMinute as u32)
}

pub fn utc_offset_min() -> i64 {
    let (local, utc) = unsafe { (GetLocalTime(), GetSystemTime()) };
    let total = |t: SYSTEMTIME| crate::todo::minutes(crate::todo::stamp(t.wYear as u32 * 10_000 + t.wMonth as u32 * 100 + t.wDay as u32, t.wHour as u32, t.wMinute as u32));
    total(local) - total(utc)
}

pub fn today() -> u32 {
    let t = unsafe { GetLocalTime() };
    t.wYear as u32 * 10_000 + t.wMonth as u32 * 100 + t.wDay as u32
}

pub fn clock() -> (String, String) {
    let t = unsafe { GetLocalTime() };
    let days = if crate::i18n::english() { WEEKDAYS_EN } else { WEEKDAYS };
    let day = days[t.wDayOfWeek as usize % 7];
    (format!("{:02}:{:02}", t.wHour, t.wMinute), format!("{day}, {:02}/{:02}", t.wDay, t.wMonth))
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Battery {
    pub percent: u8,
    pub charging: bool,
    pub saver: bool,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum PowerEvent {
    Plugged,
    Unplugged,
    SaverOn,
    SaverOff,
    Low,
    Critical,
    Full,
}

const LOW: u8 = 20;
const CRITICAL: u8 = 10;

pub fn power_events(old: Battery, new: Battery) -> Vec<PowerEvent> {
    let mut out = Vec::new();
    if old.charging != new.charging {
        out.push(if new.charging { PowerEvent::Plugged } else { PowerEvent::Unplugged });
    }
    if old.saver != new.saver {
        out.push(if new.saver { PowerEvent::SaverOn } else { PowerEvent::SaverOff });
    }
    if !new.charging {
        if old.percent > CRITICAL && new.percent <= CRITICAL {
            out.push(PowerEvent::Critical);
        } else if old.percent > LOW && new.percent <= LOW {
            out.push(PowerEvent::Low);
        }
    }
    if new.charging && new.percent == 100 && old.percent < 100 {
        out.push(PowerEvent::Full);
    }
    out
}

fn effective_overlay() -> Option<GUID> {
    type Get = unsafe extern "system" fn(*mut GUID) -> i32;
    static GET: std::sync::OnceLock<Option<Get>> = std::sync::OnceLock::new();
    let get = (*GET.get_or_init(|| unsafe {
        let lib = LoadLibraryW(w!("powrprof.dll")).ok()?;
        GetProcAddress(lib, s!("PowerGetEffectiveOverlayScheme")).map(|p| std::mem::transmute::<_, Get>(p))
    }))?;
    let mut guid = GUID::zeroed();
    (unsafe { get(&mut guid) } == 0).then_some(guid)
}

const BEST_EFFICIENCY: GUID = GUID::from_u128(0x961cc777_2547_4f9d_8174_7d86181b8a7a);

fn saver_on(status_flag: u8, overlay: Option<GUID>) -> bool {
    status_flag == 1 || overlay == Some(BEST_EFFICIENCY)
}

pub fn battery() -> Option<Battery> {
    let mut s = SYSTEM_POWER_STATUS::default();
    if unsafe { GetSystemPowerStatus(&mut s) }.is_err() || s.BatteryLifePercent == 255 || s.BatteryFlag & 128 != 0 {
        return None;
    }
    Some(Battery { percent: s.BatteryLifePercent.min(100), charging: s.ACLineStatus == 1, saver: saver_on(s.SystemStatusFlag, effective_overlay()) })
}

pub fn spawn<T: Send + 'static>(name: &str, f: impl FnOnce() -> T + Send + 'static) -> std::thread::JoinHandle<T> {
    std::thread::Builder::new().name(name.into()).spawn(f).expect("a thread")
}

const BATTERY_REFRESH: std::time::Duration = std::time::Duration::from_secs(30);

type BatteryCache =(std::sync::Arc<std::sync::Mutex<Option<Battery>>>, std::thread::Thread);
static BATTERY: std::sync::OnceLock<BatteryCache> = std::sync::OnceLock::new();

pub fn battery_cached() -> Option<Battery> {
    let (cell, _) = BATTERY.get_or_init(|| {
        let cell = std::sync::Arc::new(std::sync::Mutex::new(battery()));
        let mine = cell.clone();
        let thread = crate::system::spawn("battery", move || loop {
            std::thread::park_timeout(BATTERY_REFRESH);
            let now = battery();
            if let Ok(mut b) = mine.lock() {
                *b = now;
            }
        })
        .thread()
        .clone();
        (cell, thread)
    });
    cell.lock().map_or(None, |b| *b)
}

pub fn battery_changed() {
    if let Some((_, thread)) = BATTERY.get() {
        thread.unpark();
    }
}

fn settings_path() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("APPDATA")?).join("PuwNotch").join("settings.txt"))
}

const LOG_MAX: u64 = 64 * 1024;

fn log_path() -> Option<PathBuf> {
    settings_path().map(|p| p.with_file_name("notch.log"))
}

fn log_header() -> String {
    format!("PUW Notch v{}", env!("CARGO_PKG_VERSION"))
}

pub fn log_open() {
    if let Some(p) = log_path() {
        let _ = std::fs::remove_file(p);
    }
}

pub fn log(line: &str) {
    use std::io::Write;
    if cfg!(test) {
        return;
    }
    let Some(p) = log_path() else { return };
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let fresh = std::fs::metadata(&p).map_or(true, |m| m.len() > LOG_MAX);
    let file = std::fs::OpenOptions::new().create(true).write(true).append(!fresh).truncate(fresh).open(&p);
    let t = unsafe { GetLocalTime() };
    if let Ok(mut f) = file {
        if fresh {
            let _ = writeln!(f, "{}", log_header());
        }
        let _ = writeln!(f, "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03} {line}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond, t.wMilliseconds);
    }
}

pub fn us_measurement() -> bool {
    let mut value = [0u16; 8];
    let mut bytes = std::mem::size_of_val(&value) as u32;
    let r = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, w!("Control Panel\\International"), w!("iMeasure"), RRF_RT_REG_SZ, None, Some(value.as_mut_ptr().cast()), Some(&mut bytes))
    };
    r == ERROR_SUCCESS && value[0] == u16::from(b'1')
}

#[derive(Clone, PartialEq, Debug)]
pub struct Settings {
    pub minimal: bool,
    pub english: bool,
    pub robot: bool,
    pub volume_keys: bool,
    pub style: crate::notch::Style,
    pub approve: bool,
    pub place: Option<Place>,
    pub unit: Option<crate::weather::Unit>,
    pub notes_cleared: i64,
    pub pages: u16,
    pub sun_view: bool,
    pub activity: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { minimal: false, english: false, robot: true, volume_keys: false, style: crate::notch::Style::Ears, approve: true, place: None, unit: None, notes_cleared: 0, pages: ALL_PAGES, sun_view: false, activity: false }
    }
}

const ALL_PAGES: u16 = 0x1FF;
const TASK_PAGE: u16 = 1 << 8;

fn parse_settings(text: &str) -> Settings {
    let mut s = Settings::default();
    let mut knows_task_page = false;
    for line in text.lines() {
        match line.trim() {
            "minimal=1" => s.minimal = true,
            "lang=en" => s.english = true,
            "robot=0" => s.robot = false,
            "volkeys=1" => s.volume_keys = true,
            "approve=0" => s.approve = false,
            "activity=1" => s.activity = true,
            "pages9=1" => knows_task_page = true,
            other => {
                if let Some(style) = other.strip_prefix("style=") {
                    s.style = crate::notch::Style::from_tag(style).unwrap_or_default();
                } else if let Some(place) = other.strip_prefix("city=") {
                    s.place = crate::weather::parse_place(place);
                } else if let Some(on) = other.strip_prefix("sunview=") {
                    s.sun_view = on.trim() == "1";
                } else if let Some(unit) = other.strip_prefix("unit=") {
                    s.unit = crate::weather::Unit::from_tag(unit);
                } else if let Some(mask) = other.strip_prefix("pages=") {
                    s.pages = mask.parse().ok().filter(|m: &u16| *m != 0).map_or(s.pages, |m| if m == 0b11_1111 || m == 0b111_1111 { 0b1111_1111 } else { m });
                } else if let Some(id) = other.strip_prefix("notes_cleared=") {
                    s.notes_cleared = id.parse().unwrap_or(0);
                }
            }
        }
    }
    if !knows_task_page {
        s.pages |= TASK_PAGE;
    }
    s.pages &= ALL_PAGES;
    s
}

pub fn load_settings() -> Settings {
    settings_path().and_then(|p| std::fs::read_to_string(p).ok()).map_or_else(Settings::default, |t| parse_settings(&t))
}

pub fn save_settings(s: Settings) {
    if let Some(p) = settings_path() {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let city = s.place.as_ref().map_or(String::new(), |p| format!("city={}\n", crate::weather::place_line(p)));
        let unit = s.unit.map_or(String::new(), |u| format!("unit={}\n", u.tag()));
        let sun = if s.sun_view { "sunview=1\n" } else { "" };
        let act = if s.activity { "activity=1\n" } else { "" };
        let cleared = if s.notes_cleared > 0 { format!("notes_cleared={}\n", s.notes_cleared) } else { String::new() };
        let _ = std::fs::write(p, format!("minimal={}\nlang={}\nrobot={}\nvolkeys={}\nstyle={}\napprove={}\npages={}\npages9=1\n{city}{unit}{sun}{act}{cleared}", s.minimal as u8, if s.english { "en" } else { "vi" }, s.robot as u8, s.volume_keys as u8, s.style.tag(), s.approve as u8, s.pages));
    }
}

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const RUN_NAME: PCWSTR = w!("PuwNotch");

fn autostart_command(exe: &std::path::Path) -> String {
    format!("\"{}\"", exe.display())
}

fn autostart_value() -> Option<String> {
    let mut buf = [0u16; 1024];
    let mut bytes = (buf.len() * 2) as u32;
    let r = unsafe { RegGetValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_NAME, RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr().cast()), Some(&mut bytes)) };
    (r == ERROR_SUCCESS).then(|| String::from_utf16_lossy(&buf[..(bytes as usize / 2).saturating_sub(1)]))
}

const TASK_NAME: &str = "PuwNotch";

fn task_file() -> PathBuf {
    PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into())).join("System32\\Tasks").join(TASK_NAME)
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn task_xml(exe: &std::path::Path, user: &str) -> String {
    let user = xml_escape(user);
    let dir = exe.parent().map_or(String::new(), |d| d.display().to_string());
    let powershell = PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into()))
        .join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
    let command = xml_escape(&powershell.display().to_string());
    // UIAccess executables need ShellExecute; Task Scheduler starts executables directly.
    let arguments = xml_escape(&format!(
        r#"-NoProfile -NonInteractive -WindowStyle Hidden -Command "Start-Process -FilePath '{}' -WorkingDirectory '{}'""#,
        exe.display().to_string().replace('\'', "''"), dir.replace('\'', "''")
    ));
    let dir = xml_escape(&dir);
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <Triggers><LogonTrigger><Enabled>true</Enabled><UserId>{user}</UserId><Delay>PT15S</Delay></LogonTrigger></Triggers>
  <Principals><Principal id="Author"><UserId>{user}</UserId><LogonType>InteractiveToken</LogonType><RunLevel>HighestAvailable</RunLevel></Principal></Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Enabled>true</Enabled>
  </Settings>
  <Actions Context="Author"><Exec><Command>{command}</Command><Arguments>{arguments}</Arguments><WorkingDirectory>{dir}</WorkingDirectory></Exec></Actions>
</Task>
"#
    )
}

fn schtasks_as_admin(args: &str) -> bool {
    let params: Vec<u16> = args.encode_utf16().chain([0]).collect();
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: w!("runas"),
        lpFile: w!("schtasks.exe"),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    unsafe {
        if ShellExecuteExW(&mut info).is_err() || info.hProcess.is_invalid() {
            return false;
        }
        WaitForSingleObject(info.hProcess, 30_000);
        let mut code = 1;
        let _ = GetExitCodeProcess(info.hProcess, &mut code);
        let _ = CloseHandle(info.hProcess);
        code == 0
    }
}

pub fn autostart_enabled() -> bool {
    if task_file().exists() {
        return true;
    }
    let Ok(exe) = std::env::current_exe() else { return false };
    autostart_value().is_some_and(|v| v.eq_ignore_ascii_case(&autostart_command(&exe)))
}

pub fn refresh_autostart() {
    if autostart_value().is_some() && !autostart_enabled() {
        set_run_entry(true);
    }
}

fn set_run_entry(on: bool) {
    unsafe {
        if !on {
            let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_NAME);
            return;
        }
        let Ok(exe) = std::env::current_exe() else { return };
        let value: Vec<u16> = autostart_command(&exe).encode_utf16().chain([0]).collect();
        let _ = RegSetKeyValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_NAME, REG_SZ.0, Some(value.as_ptr().cast()), (value.len() * 2) as u32);
    }
}

pub fn set_autostart(on: bool) {
    if !on {
        set_run_entry(false);
        if task_file().exists() {
            spawn("autostart", || schtasks_as_admin(&format!("/delete /tn {TASK_NAME} /f")));
        }
        return;
    }
    let Ok(exe) = std::env::current_exe() else { return };
    spawn("autostart", move || {
        let user = format!("{}\\{}", std::env::var("USERDOMAIN").unwrap_or_default(), std::env::var("USERNAME").unwrap_or_default());
        let file = std::env::temp_dir().join("puw_notch_task.xml");
        let utf16: Vec<u8> = [0xFEFFu16].into_iter().chain(task_xml(&exe, &user).encode_utf16()).flat_map(u16::to_le_bytes).collect();
        let made = std::fs::write(&file, utf16).is_ok() && schtasks_as_admin(&format!("/create /tn {TASK_NAME} /xml \"{}\" /f", file.display()));
        let _ = std::fs::remove_file(&file);
        log(&format!("autostart task {}", if made { "created" } else { "not created" }));
        if made {
            set_run_entry(false);
        }
    });
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore = "timing: prints how long the power status calls take (they run on the UI thread)"]
    fn power_call_times() {
        let t = |name: &str, f: &dyn Fn()| {
            let (n, t0) = (200, std::time::Instant::now());
            let mut worst = 0.0f64;
            for _ in 0..n {
                let t1 = std::time::Instant::now();
                f();
                worst = worst.max(t1.elapsed().as_secs_f64() * 1000.0);
            }
            println!("{name}: avg {:.2} ms, worst {worst:.2} ms", t0.elapsed().as_secs_f64() * 1000.0 / n as f64);
        };
        t("GetSystemPowerStatus", &|| { let mut s = SYSTEM_POWER_STATUS::default(); let _ = unsafe { GetSystemPowerStatus(&mut s) }; });
        t("effective_overlay", &|| { let _ = effective_overlay(); });
        t("battery()", &|| { let _ = battery(); });
    }

    use super::*;

    fn b(percent: u8, charging: bool, saver: bool) -> Battery {
        Battery { percent, charging, saver }
    }

    #[test]
    fn autostart_command_quotes_the_path() {
        assert_eq!(autostart_command(std::path::Path::new(r"C:\Users\A B\Telegram Desktop\puw_notch.exe")), r#""C:\Users\A B\Telegram Desktop\puw_notch.exe""#);
    }

    #[test]
    fn task_opens_uiaccess_apps_through_the_shell() {
        let xml = task_xml(std::path::Path::new(r"C:\Apps & Co\O'Brien $HOME `\puw_notch.exe"), r"PC\A");
        let powershell = std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into()))
            .join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
        assert!(xml.contains(&format!("<Command>{}</Command>", xml_escape(&powershell.display().to_string()))));
        assert!(xml.contains(r#"<Arguments>-NoProfile -NonInteractive -WindowStyle Hidden -Command &quot;Start-Process -FilePath 'C:\Apps &amp; Co\O''Brien $HOME `\puw_notch.exe' -WorkingDirectory 'C:\Apps &amp; Co\O''Brien $HOME `'&quot;</Arguments>"#));
        assert!(xml.contains(r"<WorkingDirectory>C:\Apps &amp; Co\O'Brien $HOME `</WorkingDirectory>"));
    }

    #[test]
    fn task_runs_at_logon_with_highest_rights_and_survives_battery() {
        let xml = task_xml(std::path::Path::new(r"C:\Apps & Co\puw_notch.exe"), r"PC\A <b>");
        assert!(xml.contains("<RunLevel>HighestAvailable</RunLevel>") && xml.contains("<LogonTrigger>"));
        assert!(xml.contains("<Delay>PT15S</Delay>"), "not before the startup apps have loaded");
        assert!(xml.contains("<StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>") && xml.contains("<ExecutionTimeLimit>PT0S</ExecutionTimeLimit>"));
        assert!(xml.contains(r"Start-Process -FilePath 'C:\Apps &amp; Co\puw_notch.exe'"), "path quoted and escaped");
        assert!(xml.contains(r"<UserId>PC\A &lt;b&gt;</UserId>"), "user escaped");
    }

    #[test]
    fn saver_is_the_old_flag_or_the_best_efficiency_power_mode() {
        let other = GUID::from_u128(0x3af9b8d9_7c97_431d_ad78_34a8bfea439f);
        assert!(!saver_on(0, None));
        assert!(!saver_on(0, Some(GUID::zeroed())), "default mode");
        assert!(!saver_on(0, Some(other)));
        assert!(saver_on(1, None), "classic battery saver flag");
        assert!(saver_on(1, Some(other)));
        assert!(saver_on(0, Some(BEST_EFFICIENCY)));
        assert_eq!(BEST_EFFICIENCY, GUID::from_u128(0x961cc777_2547_4f9d_8174_7d86181b8a7a));
    }

    #[test]
    fn settings_file() {
        assert_eq!(parse_settings("minimal=1\nlang=en\nrobot=0\nvolkeys=1\n"), Settings { minimal: true, english: true, robot: false, volume_keys: true, style: crate::notch::Style::Ears, approve: true, place: None, unit: None, notes_cleared: 0, pages: 0x1FF, sun_view: false, activity: false });
        assert_eq!(parse_settings("pages=5\n").pages, 5 | 1 << 8, "an old file shows the new Task page");
        assert_eq!(parse_settings("pages=0\n").pages, 0x1FF, "never none");
        assert_eq!(parse_settings("pages=63\n").pages, 0x1FF, "a mask saved before the Focus page gets it too");
        assert_eq!(parse_settings("pages=127\n").pages, 0x1FF, "a mask with every page saved before the Calendar page gets it too");
        assert_eq!(parse_settings("pages=255\n").pages, 0x1FF);
        assert_eq!(parse_settings("pages=255\npages9=1\n").pages, 0xFF, "a file saved after Task keeps Task off");
        assert_eq!(parse_settings("pages9=1\npages=5\n").pages, 5);
        assert!(!parse_settings("approve=0\n").approve && parse_settings("minimal=0\n").approve, "on unless turned off");
        assert_eq!(parse_settings("style=pill\n").style, crate::notch::Style::Pill);
        assert_eq!(parse_settings("style=x\n").style, crate::notch::Style::Ears, "unknown: the original look");
        assert_eq!(parse_settings("notes_cleared=42\n").notes_cleared, 42);
        assert_eq!(parse_settings("notes_cleared=x\n").notes_cleared, 0);
        assert!(parse_settings("sunview=1\n").sun_view && !parse_settings("sunview=0\n").sun_view && !parse_settings("minimal=0\n").sun_view, "off unless turned on");
        assert_eq!(parse_settings("unit=f\n").unit, Some(crate::weather::Unit::F));
        assert_eq!(parse_settings("unit=c\n").unit, Some(crate::weather::Unit::C));
        assert_eq!(parse_settings("unit=x\n").unit, None, "unknown: follow the region");
        assert_eq!(parse_settings("minimal=0\n").unit, None, "old files: follow the region");
        let hue = crate::weather::Place { name: "Huế".into(), lat: 16.4619, lon: 107.59546 };
        assert_eq!(parse_settings("robot=1\ncity=16.46190,107.59546,Huế\n").place, Some(hue));
        assert_eq!(parse_settings("minimal=0\n"), Settings::default(), "old files: Vietnamese, idle robot on, volume keys left to Windows");
        assert!(Settings::default().robot && !Settings::default().volume_keys);
    }

    #[test]
    fn power_changes() {
        assert!(power_events(b(50, false, false), b(49, false, false)).is_empty());
        assert_eq!(power_events(b(50, false, false), b(50, true, false)), vec![PowerEvent::Plugged]);
        assert_eq!(power_events(b(50, true, false), b(50, false, false)), vec![PowerEvent::Unplugged]);
        assert_eq!(power_events(b(50, false, false), b(50, false, true)), vec![PowerEvent::SaverOn]);
        assert_eq!(power_events(b(21, false, false), b(20, false, false)), vec![PowerEvent::Low]);
        assert!(power_events(b(20, false, false), b(19, false, false)).is_empty(), "only when crossing");
        assert_eq!(power_events(b(11, false, false), b(10, false, false)), vec![PowerEvent::Critical]);
        assert!(power_events(b(21, true, false), b(20, true, false)).is_empty(), "charging: not low");
        assert_eq!(power_events(b(99, true, false), b(100, true, false)), vec![PowerEvent::Full]);
    }
}

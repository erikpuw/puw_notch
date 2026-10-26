//! Small OS bits: clock text, battery, settings file, start-with-Windows.

use std::path::PathBuf;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows::Win32::System::Registry::*;
use windows::Win32::Foundation::SYSTEMTIME;
use windows::Win32::System::SystemInformation::{GetLocalTime, GetSystemTime};

const WEEKDAYS: [&str; 7] = ["CN", "T2", "T3", "T4", "T5", "T6", "T7"];
const WEEKDAYS_EN: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

/// Local time as yyyymmddHHMM (to-do reminders).
pub fn now_stamp() -> u64 {
    let t = unsafe { GetLocalTime() };
    crate::todo::stamp(t.wYear as u32 * 10_000 + t.wMonth as u32 * 100 + t.wDay as u32, t.wHour as u32, t.wMinute as u32)
}

/// Local time minus UTC in minutes (notification arrival times are stored in UTC).
pub fn utc_offset_min() -> i64 {
    let (local, utc) = unsafe { (GetLocalTime(), GetSystemTime()) };
    let total = |t: SYSTEMTIME| crate::todo::minutes(crate::todo::stamp(t.wYear as u32 * 10_000 + t.wMonth as u32 * 100 + t.wDay as u32, t.wHour as u32, t.wMinute as u32));
    total(local) - total(utc)
}

/// Local date as yyyymmdd (to-do "done today").
pub fn today() -> u32 {
    let t = unsafe { GetLocalTime() };
    t.wYear as u32 * 10_000 + t.wMonth as u32 * 100 + t.wDay as u32
}

/// ("16:04", "T7, 24/10" / "Sat, 24/10"): time for the pill, short date for the panel header.
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
    /// Windows battery saver / energy saver is on.
    pub saver: bool,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum PowerEvent {
    Plugged,
    Unplugged,
    SaverOn,
    SaverOff,
    Low(u8),
    Critical(u8),
    Full,
}

const LOW: u8 = 20;
const CRITICAL: u8 = 10;

/// Battery changes worth a banner between two readings.
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
            out.push(PowerEvent::Critical(new.percent));
        } else if old.percent > LOW && new.percent <= LOW {
            out.push(PowerEvent::Low(new.percent));
        }
    }
    if new.charging && new.percent == 100 && old.percent < 100 {
        out.push(PowerEvent::Full);
    }
    out
}

impl Battery {
    /// "Pin 80%" / "Đang sạc 80%" (English: "Battery 80%" / "Charging 80%")
    pub fn label(&self) -> String {
        let label = if self.charging { crate::i18n::tr("Đang sạc", "Charging") } else { crate::i18n::tr("Pin", "Battery") };
        format!("{label} {}%", self.percent)
    }
}

/// None on a desktop PC (no system battery): nothing battery-related is shown then.
pub fn battery() -> Option<Battery> {
    let mut s = SYSTEM_POWER_STATUS::default();
    if unsafe { GetSystemPowerStatus(&mut s) }.is_err() || s.BatteryLifePercent == 255 || s.BatteryFlag & 128 != 0 {
        return None;
    }
    Some(Battery { percent: s.BatteryLifePercent.min(100), charging: s.ACLineStatus == 1, saver: s.SystemStatusFlag == 1 })
}

fn settings_path() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("APPDATA")?).join("PuwNotch").join("settings.txt"))
}

/// Tray choices kept in settings.txt.
#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct Settings {
    pub minimal: bool,
    pub english: bool,
}

fn parse_settings(text: &str) -> Settings {
    let mut s = Settings::default();
    for line in text.lines() {
        match line.trim() {
            "minimal=1" => s.minimal = true,
            "lang=en" => s.english = true,
            _ => {}
        }
    }
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
        let _ = std::fs::write(p, format!("minimal={}\nlang={}\n", s.minimal as u8, if s.english { "en" } else { "vi" }));
    }
}

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const RUN_NAME: PCWSTR = w!("PuwNotch");

pub fn autostart_enabled() -> bool {
    let r = unsafe { RegGetValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_NAME, RRF_RT_REG_SZ, None, None, None) };
    r == ERROR_SUCCESS
}

pub fn set_autostart(on: bool) {
    unsafe {
        if !on {
            let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_NAME);
            return;
        }
        let Ok(exe) = std::env::current_exe() else { return };
        let value: Vec<u16> = format!("\"{}\"", exe.display()).encode_utf16().chain([0]).collect();
        let _ = RegSetKeyValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_NAME, REG_SZ.0, Some(value.as_ptr().cast()), (value.len() * 2) as u32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(percent: u8, charging: bool, saver: bool) -> Battery {
        Battery { percent, charging, saver }
    }

    #[test]
    fn settings_file() {
        assert_eq!(parse_settings("minimal=1\nlang=en\n"), Settings { minimal: true, english: true });
        assert_eq!(parse_settings("minimal=0\n"), Settings::default(), "old files without lang stay Vietnamese");
    }

    #[test]
    fn power_changes() {
        assert!(power_events(b(50, false, false), b(49, false, false)).is_empty());
        assert_eq!(power_events(b(50, false, false), b(50, true, false)), vec![PowerEvent::Plugged]);
        assert_eq!(power_events(b(50, true, false), b(50, false, false)), vec![PowerEvent::Unplugged]);
        assert_eq!(power_events(b(50, false, false), b(50, false, true)), vec![PowerEvent::SaverOn]);
        assert_eq!(power_events(b(21, false, false), b(20, false, false)), vec![PowerEvent::Low(20)]);
        assert!(power_events(b(20, false, false), b(19, false, false)).is_empty(), "only when crossing");
        assert_eq!(power_events(b(11, false, false), b(10, false, false)), vec![PowerEvent::Critical(10)]);
        assert!(power_events(b(21, true, false), b(20, true, false)).is_empty(), "charging: not low");
        assert_eq!(power_events(b(99, true, false), b(100, true, false)), vec![PowerEvent::Full]);
    }
}

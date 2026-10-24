//! Small OS bits: clock text, battery, settings file, start-with-Windows.

use std::path::PathBuf;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows::Win32::System::Registry::*;
use windows::Win32::System::SystemInformation::GetLocalTime;

const WEEKDAYS: [&str; 7] = ["Chủ Nhật", "Thứ Hai", "Thứ Ba", "Thứ Tư", "Thứ Năm", "Thứ Sáu", "Thứ Bảy"];

/// ("16:04", "Thứ Bảy, 24 tháng 10")
pub fn clock() -> (String, String) {
    let t = unsafe { GetLocalTime() };
    let day = WEEKDAYS[t.wDayOfWeek as usize % 7];
    (format!("{:02}:{:02}", t.wHour, t.wMinute), format!("{day}, {} tháng {}", t.wDay, t.wMonth))
}

/// "Pin 80%" / "Đang sạc 80%" / "" when there is no battery.
pub fn battery() -> String {
    let mut s = SYSTEM_POWER_STATUS::default();
    if unsafe { GetSystemPowerStatus(&mut s) }.is_err() || s.BatteryLifePercent == 255 || s.BatteryFlag & 128 != 0 {
        return String::new();
    }
    let label = if s.ACLineStatus == 1 { "Đang sạc" } else { "Pin" };
    format!("{label} {}%", s.BatteryLifePercent)
}

fn settings_path() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("APPDATA")?).join("NusaRebuild").join("settings.txt"))
}

pub fn load_minimal() -> bool {
    settings_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .is_some_and(|s| s.lines().any(|l| l.trim() == "minimal=1"))
}

pub fn save_minimal(on: bool) {
    if let Some(p) = settings_path() {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(p, format!("minimal={}\n", on as u8));
    }
}

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const RUN_NAME: PCWSTR = w!("NusaRebuild");

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

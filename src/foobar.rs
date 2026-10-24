//! foobar2000 native timeline supplement.

use windows::core::{BOOL, Interface};
use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomation2, IUIAutomationElement, TreeScope_Children,
    TreeScope_Descendants,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowTextW, IsWindowVisible,
};

/// Timing read from foobar2000's status bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timeline {
    pub duration_ms: u64,
    pub position_ms: u64,
}

unsafe extern "system" fn collect_foobar_windows(hwnd: HWND, data: LPARAM) -> BOOL {
    if !IsWindowVisible(hwnd).as_bool() {
        return BOOL(1);
    }
    let mut title = [0u16; 512];
    let len = GetWindowTextW(hwnd, &mut title);
    if len > 0 && String::from_utf16_lossy(&title[..len as usize]).trim_end().ends_with("[foobar2000]") {
        (*(data.0 as *mut Vec<HWND>)).push(hwnd);
    }
    BOOL(1)
}

fn foobar_window() -> Option<HWND> {
    let mut windows = Vec::new();
    unsafe { EnumWindows(Some(collect_foobar_windows), LPARAM(&mut windows as *mut _ as isize)).ok()?; }
    match windows.as_slice() { [window] => Some(*window), _ => None }
}

fn status_bar_timing(automation: &IUIAutomation, root: &IUIAutomationElement) -> Option<(u64, u64)> {
    unsafe {
        // AutomationId 666 is foobar2000's status bar; searching only direct root children
        // avoids walking its playlist and other large UI subtrees.
        let condition = automation.CreateTrueCondition().ok()?;
        let children = root.FindAll(TreeScope_Children, &condition).ok()?;
        let count = children.Length().ok()?.clamp(0, 64);
        for i in 0..count {
            let child = children.GetElement(i).ok()?;
            if child.CurrentAutomationId().ok()? != "666" {
                continue;
            }
            if let Ok(name) = child.CurrentName() {
                if let Some(timing) = parse_timing(&name.to_string()) {
                    return Some(timing);
                }
            }
            let descendants = child.FindAll(TreeScope_Descendants, &condition).ok()?;
            for j in 0..descendants.Length().ok()?.clamp(0, 64) {
                if let Ok(name) = descendants.GetElement(j).and_then(|e| e.CurrentName()) {
                    if let Some(timing) = parse_timing(&name.to_string()) {
                        return Some(timing);
                    }
                }
            }
            return None;
        }
    }
    None
}

/// Reads real elapsed and total time from foobar2000's UIA status bar.
/// The caller must already have initialized COM on this thread.
pub fn timeline() -> Option<Timeline> {
    unsafe {
        let automation: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()?;
        if let Ok(timeout) = automation.cast::<IUIAutomation2>() {
            let _ = timeout.SetConnectionTimeout(200);
            let _ = timeout.SetTransactionTimeout(200);
        }
        let root = automation.ElementFromHandle(foobar_window()?).ok()?;
        let (position_ms, duration_ms) = status_bar_timing(&automation, &root)?;
        Some(Timeline { duration_ms, position_ms })
    }
}

fn parse_timing(text: &str) -> Option<(u64, u64)> {
    if let Some((left, right)) = text.split_once('/') {
        let elapsed = left.rsplit('|').next()?.trim();
        let duration = right.split('|').next()?.trim();
        if let (Some(position), Some(total)) = (parse_clock(elapsed), parse_clock(duration)) {
            if position <= total {
                return Some((position, total));
            }
        }
    }
    None
}

fn parse_clock(value: &str) -> Option<u64> {
    let parts: Vec<_> = value.trim().split(':').collect();
    if !(2..=3).contains(&parts.len()) {
        return None;
    }
    let numbers: Option<Vec<u64>> = parts.iter().map(|part| part.parse().ok()).collect();
    let n = numbers?;
    let (hours, minutes, seconds) = if n.len() == 3 { (n[0], n[1], n[2]) } else { (0, n[0], n[1]) };
    if seconds >= 60 || minutes >= 60 && n.len() == 3 {
        return None;
    }
    hours.checked_mul(3_600_000)?
        .checked_add(minutes.checked_mul(60_000)?)?
        .checked_add(seconds.checked_mul(1_000)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_foobar_status_bar_timing() {
        assert_eq!(parse_timing("AAC | 343 kbps | 44100 Hz | stereo | 0:57 / 4:24"), Some((57_000, 264_000)));
        assert_eq!(parse_timing("stereo | 1:02:03 / 2:03:04"), Some((3_723_000, 7_384_000)));
    }

    #[test]
    fn rejects_missing_invalid_or_inconsistent_timing() {
        assert_eq!(parse_timing("AAC | unknown"), None);
        assert_eq!(parse_timing("AAC | 0:70 / 4:24"), None);
        assert_eq!(parse_timing("AAC | 5:00 / 4:24"), None);
    }

    #[test]
    #[ignore = "requires foobar2000 running in the interactive desktop"]
    fn live_read_only_timeline() {
        use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED).ok().unwrap();
            println!("foobar timeline: {:?}", timeline().expect("foobar2000 must be open with timing in its status bar"));
            CoUninitialize();
        }
    }
}

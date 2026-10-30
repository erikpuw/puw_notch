//! foobar2000 native timeline supplement.

use windows::core::{BOOL, Interface};
use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomation2, IUIAutomationElement, TreeScope_Children,
    TreeScope_Descendants,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, EnumWindows, GetClassNameW, GetClientRect, GetWindowTextW,
    IsWindowVisible, SendMessageTimeoutW, SMTO_ABORTIFHUNG, SMTO_BLOCK,
    WM_LBUTTONDOWN, WM_LBUTTONUP,
};

/// Timing read from foobar2000's status bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timeline {
    pub duration_ms: u64,
    pub position_ms: u64,
    pub can_seek: bool,
}

// shortcut: Default UI seekbar class, add an adapter if a different skin exposes another control.
const SEEKBAR_CLASS: &str = "{596C5150-D8FF-4ed4-81AB-99B3DDA08AF1}";

unsafe extern "system" fn collect_seekbars(hwnd: HWND, data: LPARAM) -> BOOL {
    let mut class = [0u16; 128];
    let len = GetClassNameW(hwnd, &mut class);
    if String::from_utf16_lossy(&class[..len.max(0) as usize]) == SEEKBAR_CLASS
        && IsWindowVisible(hwnd).as_bool() && IsWindowEnabled(hwnd).as_bool() {
        (*(data.0 as *mut Vec<HWND>)).push(hwnd);
    }
    BOOL(1)
}

fn seekbar(window: HWND) -> Option<HWND> {
    if !unsafe { IsWindowEnabled(window) }.as_bool() { return None; }
    let mut bars = Vec::new();
    unsafe { let _ = EnumChildWindows(Some(window), Some(collect_seekbars), LPARAM(&mut bars as *mut _ as isize)); }
    match bars.as_slice() { [bar] => Some(*bar), _ => None }
}

fn seek_x(width: i32, inset: i32, position_ms: u64, duration_ms: u64) -> Option<i32> {
    (inset >= 0 && width > 2 * inset && duration_ms > 0).then(|| inset +
        (((width - 2 * inset) as u128 * position_ms.min(duration_ms) as u128 + duration_ms as u128 / 2)
            / duration_ms as u128) as i32)
}

pub fn seek(position_ms: u64, duration_ms: u64) -> bool {
    let Some(bar) = foobar_window().and_then(seekbar) else { return false; };
    unsafe {
        let mut rect = RECT::default();
        if GetClientRect(bar, &mut rect).is_err() || rect.bottom <= 0 { return false; }
        // Default UI reserves half of its 16-DIP thumb at each end of the track.
        let inset = (8 * GetDpiForWindow(bar).max(96) / 96) as i32;
        let Some(x) = seek_x(rect.right, inset, position_ms, duration_ms) else { return false; };
        let point = LPARAM(((rect.bottom / 2) << 16 | x) as isize);
        let flags = SMTO_ABORTIFHUNG | SMTO_BLOCK;
        let down = SendMessageTimeoutW(bar, WM_LBUTTONDOWN, WPARAM(1), point, flags, 200, None).0 != 0;
        let up = SendMessageTimeoutW(bar, WM_LBUTTONUP, WPARAM(0), point, flags, 200, None).0 != 0;
        down && up
    }
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
        let window = foobar_window()?;
        let root = automation.ElementFromHandle(window).ok()?;
        let (position_ms, duration_ms) = status_bar_timing(&automation, &root)?;
        Some(Timeline { duration_ms, position_ms, can_seek: seekbar(window).is_some() })
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
    #[test]
    #[ignore = "seeks the live foobar2000 player briefly, then restores its position"]
    fn live_seek_and_restore() {
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(None,
                windows::Win32::System::Com::COINIT_MULTITHREADED);
        }
        let before = super::timeline().expect("foobar2000 must be open with timing");
        assert!(before.can_seek && before.duration_ms > 30_000);
        let mut readings = Vec::new();
        for target in [before.duration_ms / 4, before.duration_ms / 2, before.duration_ms * 3 / 4] {
            let accepted = super::seek(target, before.duration_ms);
            std::thread::sleep(std::time::Duration::from_millis(300));
            readings.push((target, accepted, super::timeline()));
        }
        let restored = super::seek(before.position_ms, before.duration_ms);
        std::thread::sleep(std::time::Duration::from_millis(300));
        let after_restore = super::timeline();
        eprintln!("before={before:?} readings={readings:?} restored={restored} after_restore={after_restore:?}");
        assert!(restored);
        assert!(after_restore.is_some_and(|t| t.position_ms.abs_diff(before.position_ms) <= 2_000));
        assert!(readings.iter().all(|(target, accepted, actual)| *accepted
            && actual.is_some_and(|t| t.position_ms.abs_diff(*target) <= 2_000)), "native seek must reach each requested position");
    }
    #[test]
    fn native_seek_coordinate_is_bounded_and_rejects_unknown_duration() {
        assert_eq!(super::seek_x(100, 8, 60_000, 120_000), Some(50));
        assert_eq!(super::seek_x(100, 8, 0, 120_000), Some(8));
        assert_eq!(super::seek_x(100, 8, u64::MAX, 120_000), Some(92));
        assert_eq!(super::seek_x(100, 8, 1000, 0), None);
        assert_eq!(super::seek_x(16, 8, 1000, 120_000), None);
        assert_eq!(super::seek_x(118, 8, 54_500, 218_000), Some(34));
        assert_eq!(super::seek_x(236, 16, 54_500, 218_000), Some(67));
    }
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

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};

use windows::core::w;
use windows::Win32::Foundation::{GetLastError, ERROR_ACCESS_DENIED, HWND, LPARAM, RECT, WPARAM};
use windows::Win32::UI::Accessibility::{SetWinEventHook, HWINEVENTHOOK};
use windows::Win32::UI::Shell::{SHQueryUserNotificationState, QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::notch;

pub const WM_SCREENEVENT: u32 = WM_APP + 11;
static HOOK_TARGET: AtomicIsize = AtomicIsize::new(0);

unsafe extern "system" fn on_event(_: HWINEVENTHOOK, _: u32, _: HWND, _: i32, _: i32, _: u32, _: u32) {
    let _ = PostMessageW(Some(HWND(HOOK_TARGET.load(Ordering::Relaxed) as _)), WM_SCREENEVENT, WPARAM(0), LPARAM(0));
}

static SHELL_FULLSCREEN: AtomicBool = AtomicBool::new(false);

pub fn watch(hwnd: HWND) {
    HOOK_TARGET.store(hwnd.0 as isize, Ordering::Relaxed);
    crate::system::spawn("shell-state", || {
        unsafe { let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED); }
        loop {
            SHELL_FULLSCREEN.store(ask_shell_fullscreen(), Ordering::Relaxed);
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
    });
    for event in [0x0003, 0x000B] {
        unsafe { SetWinEventHook(event, event, None, Some(on_event), 0, 0, 2) };
    }
}

pub enum Check {
    Fullscreen,
    Busy(bool),
    Unknown,
    Settling,
}

const PILL_H: f32 = 30.0;

fn body_half(pill_w: f32) -> f32 {
    let corner = notch::target(notch::Mode::Collapsed, false)[2];
    (pill_w / 2.0 - corner).max(0.0)
}

fn clip_span(from: i32, to: i32, left: i32, right: i32) -> Option<(i32, i32)> {
    let (from, to) = (from.max(left + 2), to.min(right - 2));
    (to - from >= 4).then_some((from, to))
}

const HTCAPTION: isize = 2;

fn is_empty_title(answer: isize, drag_bar: Option<RECT>, x: i32) -> bool {
    answer == HTCAPTION && drag_bar.is_none_or(|r| x >= r.left && x < r.right)
}

const OFFICE_CLASSES: [&str; 4] = ["OpusApp", "XLMAIN", "PPTFrameClass", "Framework::CFrame"];

fn covers_something(hwnd: HWND, center_x: i32, half: i32, y: i32) -> Option<bool> {
    if OFFICE_CLASSES.contains(&class_name(hwnd).as_str()) {
        return Some(true);
    }
    let drag_bar = unsafe { FindWindowExW(Some(hwnd), None, w!("DRAG_BAR_WINDOW_CLASS"), None) }.ok().and_then(|bar| {
        let mut r = RECT::default();
        unsafe { GetWindowRect(bar, &mut r) }.ok().map(|_| r)
    });
    for k in -2..=2 {
        let x = center_x + k * half / 2;
        let point = ((y as u16 as isize) << 16) | (x as u16 as isize);
        let mut answer = 0usize;
        let sent = unsafe { SendMessageTimeoutW(hwnd, WM_NCHITTEST, WPARAM(0), LPARAM(point), SMTO_ABORTIFHUNG, 20, Some(&mut answer)) };
        let answer = if sent.0 != 0 {
            answer as i16 as isize
        } else if unsafe { GetLastError() } != ERROR_ACCESS_DENIED {
            return None;
        } else if drag_bar.is_some() {
            HTCAPTION
        } else {
            return Some(true);
        };
        if !is_empty_title(answer, drag_bar, x) {
            return Some(true);
        }
    }
    Some(false)
}

pub fn buried(own: HWND) -> bool {
    unsafe {
        let mut w = own;
        while let Ok(above) = GetWindow(w, GW_HWNDPREV) {
            if above.is_invalid() {
                break;
            }
            let topmost = GetWindowLongPtrW(above, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST.0 != 0;
            if !topmost && IsWindowVisible(above).as_bool() {
                return true;
            }
            w = above;
        }
        false
    }
}

const SHELL_CLASSES: [&str; 7] = ["Progman", "WorkerW", "Shell_TrayWnd", "Shell_SecondaryTrayWnd", "XamlWindow", "XamlExplorerHostIslandWindow", "Windows.UI.Core.CoreWindow"];

pub fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 64];
    let n = unsafe { GetClassNameW(hwnd, &mut buf) }.max(0) as usize;
    String::from_utf16_lossy(&buf[..n])
}

fn ask_shell_fullscreen() -> bool {
    unsafe { SHQueryUserNotificationState() }.is_ok_and(|s| s == QUNS_RUNNING_D3D_FULL_SCREEN || s == QUNS_PRESENTATION_MODE)
}

fn shell_says_fullscreen() -> bool {
    SHELL_FULLSCREEN.load(Ordering::Relaxed)
}

pub fn check(own: HWND, center_x: i32, monitor: RECT, scale: f32, pill_w: f32, sample: bool, last: &mut Option<(isize, RECT)>) -> Check {
    if shell_says_fullscreen() {
        return Check::Fullscreen;
    }
    unsafe {
        let fg = GetForegroundWindow();
        if fg.is_invalid() || fg == own || !IsWindowVisible(fg).as_bool() || IsIconic(fg).as_bool() {
            return Check::Busy(false);
        }
        if SHELL_CLASSES.contains(&class_name(fg).as_str()) {
            return Check::Busy(false);
        }
        let mut r = RECT::default();
        if GetWindowRect(fg, &mut r).is_err() {
            return Check::Busy(false);
        }
        let near = |a: i32, b: i32| (a - b).abs() <= 1;
        if near(r.left, monitor.left) && near(r.top, monitor.top) && near(r.right, monitor.right) && near(r.bottom, monitor.bottom) {
            return Check::Fullscreen;
        }
        let seen = Some((fg.0 as isize, r));
        let still = *last == seen;
        *last = seen;
        if !sample {
            return Check::Busy(false);
        }
        if !still {
            return Check::Settling;
        }
        if !reaches_pill(r, center_x, monitor.top, scale, pill_w) {
            return Check::Busy(false);
        }
        let half = (body_half(pill_w) * scale) as i32;
        let y = monitor.top + (PILL_H * scale / 2.0) as i32;
        let asked = std::time::Instant::now();
        let answer = covers_something(fg, center_x, half, y);
        static SAID: AtomicIsize = AtomicIsize::new(0);
        let now = crate::unix_ms() as isize;
        if asked.elapsed().as_millis() > 15 && now - SAID.load(Ordering::Relaxed) > 10_000 {
            SAID.store(now, Ordering::Relaxed);
            crate::system::log(&format!("slow hit test: {} ms, {} ({})", asked.elapsed().as_millis(), class_name(fg), if answer.is_some() { "answered" } else { "timed out" }));
        }
        answer.map_or(Check::Unknown, Check::Busy)
    }
}

fn reaches_pill(r: RECT, center_x: i32, top: i32, scale: f32, pill_w: f32) -> bool {
    let half = (body_half(pill_w) * scale) as i32;
    let sh = (PILL_H * scale) as i32;
    r.top <= top + sh && r.bottom > top + 3 && clip_span(center_x - half, center_x + half, r.left, r.right).is_some()
}

#[cfg(test)]
mod tests {
    use super::{body_half, clip_span, is_empty_title, reaches_pill, HTCAPTION};
    use windows::Win32::Foundation::RECT;

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
        RECT { left, top, right, bottom }
    }

    #[test]
    fn a_bare_bit_of_title_bar_answers_caption_a_tab_does_not() {
        const HTCLIENT: isize = 1;
        assert!(is_empty_title(HTCAPTION, None, 960), "bare title bar (Edge with few tabs, Notepad, Claude)");
        assert!(!is_empty_title(HTCLIENT, None, 960), "a tab or a button answers client");
        let bar = Some(rect(1100, 0, 1920, 32));
        assert!(!is_empty_title(HTCAPTION, bar, 960), "left of the drag bar: a tab");
        assert!(is_empty_title(HTCAPTION, bar, 1500), "inside the drag bar: bare");
        assert!(!is_empty_title(HTCLIENT, bar, 1500), "a button inside the bare part still answers client");
    }

    #[test]
    fn a_window_reaches_the_pill_when_it_covers_the_top_edge_under_it() {
        let on = |r: RECT| reaches_pill(r, 960, 0, 1.0, 150.0);
        assert!(on(rect(0, 0, 1920, 1040)), "maximized");
        assert!(on(rect(0, 0, 960, 1040)), "snapped to the left half, reaching the pill's left half");
        assert!(on(rect(900, 20, 1500, 900)), "a window whose top edge is inside the pill's height");
        assert!(!on(rect(100, 120, 900, 700)), "a window below the pill");
        assert!(!on(rect(0, 0, 500, 700)), "a window beside the pill");
        assert!(!on(rect(0, -800, 1920, 2)), "a window that ends at the very top edge");
    }

    #[test]
    fn the_checked_span_is_the_pill_body_short_or_long() {
        use crate::notch::{target, Mode};
        assert_eq!(body_half(target(Mode::Collapsed, false)[0]), 61.0);
        assert_eq!(body_half(target(Mode::Collapsed, true)[0]), 94.0);
        assert_eq!(body_half(10.0), 0.0, "never negative");
    }

    #[test]
    fn the_span_is_cut_to_the_window_under_it() {
        assert_eq!(clip_span(900, 1020, 0, 1920), Some((900, 1020)), "window covers it all");
        assert_eq!(clip_span(900, 1020, 950, 1920), Some((952, 1020)), "window starts under the pill");
        assert_eq!(clip_span(900, 1020, 0, 960), Some((900, 958)), "window ends under the pill");
        assert_eq!(clip_span(900, 1020, 1000, 1100), Some((1002, 1020)), "only a sliver");
        assert_eq!(clip_span(900, 1020, 1100, 1900), None, "window entirely beside the pill");
        assert_eq!(clip_span(900, 903, 0, 1920), None, "too narrow to tell anything");
    }
}

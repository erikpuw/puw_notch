//! What is under / around the notch: fullscreen apps, and whether the pill sits on tabs. Nothing is captured
//! or drawn: the foreground window is asked what is at the points under the pill (`WM_NCHITTEST`), the
//! question Windows itself asks it for every mouse move. A bare bit of title bar answers "caption" (the window
//! can be dragged by it); a tab or a button answers something else. So the notch is not left out of screen
//! capture and shows in remote desktop and screen sharing.

use std::sync::atomic::{AtomicIsize, Ordering};

use windows::core::w;
use windows::Win32::Foundation::{GetLastError, ERROR_ACCESS_DENIED, HWND, LPARAM, RECT, WPARAM};
use windows::Win32::UI::Accessibility::{SetWinEventHook, HWINEVENTHOOK};
use windows::Win32::UI::Shell::{SHQueryUserNotificationState, QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::notch;

/// Posted to the notch's window when the foreground window changed or a window was moved or resized.
pub const WM_SCREENEVENT: u32 = WM_APP + 11;
static HOOK_TARGET: AtomicIsize = AtomicIsize::new(0);

unsafe extern "system" fn on_event(_: HWINEVENTHOOK, _: u32, _: HWND, _: i32, _: i32, _: u32, _: u32) {
    let _ = PostMessageW(Some(HWND(HOOK_TARGET.load(Ordering::Relaxed) as _)), WM_SCREENEVENT, WPARAM(0), LPARAM(0));
}

/// Like Nusa: Windows tells the notch when the foreground window changes (event 3) or a window has been
/// moved or resized (0xB), so it looks at once instead of at its next tick. Rare events, nothing wakes it
/// at rest. The notch's own windows are skipped (flag 2).
pub fn watch(hwnd: HWND) {
    HOOK_TARGET.store(hwnd.0 as isize, Ordering::Relaxed);
    for event in [0x0003, 0x000B] {
        unsafe { SetWinEventHook(event, event, None, Some(on_event), 0, 0, 2) };
    }
}

pub enum Check {
    /// A fullscreen app (game, video) owns the notch's monitor: hide the notch.
    Fullscreen,
    /// Normal desktop; `true` when the pill sits on a tab or a button of the foreground window.
    /// Not checked -> `false`, and the caller ignores it.
    Busy(bool),
    /// The foreground window did not answer in time: keep what the notch shows, look again at the next tick.
    Unknown,
    /// The foreground window just changed, moved or resized (maximize / restore animation):
    /// no verdict until it holds still.
    Settling,
}

/// What the pill covers: the body of the pill (its resting height, the top of the screen), measured
/// from the pill itself, short or long (music widens it), without its rounded corners.
const PILL_H: f32 = 30.0;

/// Half the width checked: the pill's body, i.e. its half width minus the rounded corner.
fn body_half(pill_w: f32) -> f32 {
    let corner = notch::target(notch::Mode::Collapsed, false)[2];
    (pill_w / 2.0 - corner).max(0.0)
}

/// Cut the span `[from, to)` to where the window `[left, right)` is, with 2 px margin for its frame;
/// None when nothing of at least 4 px is left.
fn clip_span(from: i32, to: i32, left: i32, right: i32) -> Option<(i32, i32)> {
    let (from, to) = (from.max(left + 2), to.min(right - 2));
    (to - from >= 4).then_some((from, to))
}

/// The answer of a window to `WM_NCHITTEST` that means "a bare bit of title bar, you can drag me here".
const HTCAPTION: isize = 2;

/// Is a point of the title bar empty, given what the window answered to `WM_NCHITTEST` there? Most windows
/// (Edge, Notepad, Electron apps) answer "caption" over the bare part and "client" over tabs and buttons.
/// Windows Terminal answers "caption" everywhere, but keeps a child window (`DRAG_BAR_WINDOW_CLASS`, `drag_bar`
/// = its place on screen) exactly over the bare part: a point outside it is on a tab.
fn is_empty_title(answer: isize, drag_bar: Option<RECT>, x: i32) -> bool {
    answer == HTCAPTION && drag_bar.is_none_or(|r| x >= r.left && x < r.right)
}

/// Is the pill's body over a tab or a button of `hwnd`? Asks at 5 points across the body, half way down
/// the pill (`y`). `None` = the window did not answer in time (busy or hung): no verdict. A window that runs
/// as administrator is not allowed to be asked (Windows refuses at once): Windows Terminal's drag bar still
/// says where its bare part is (it answers "caption" everywhere anyway); any other counts as covered, the safe side.
fn covers_something(hwnd: HWND, center_x: i32, half: i32, y: i32) -> Option<bool> {
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

/// Is an ordinary (not topmost) visible window stacked above `own`? A topmost window is never
/// below one while Windows keeps its order, but a fullscreen / immersive app can leave it there
/// with the topmost style still set, so only the stacking order tells.
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

/// Shell windows that are never "an app in the way". `XamlWindow` is the screen-sized host of the
/// Start menu's power / account flyouts: taken for a fullscreen app it hid the notch. The Start and
/// Search panels are `CoreWindow`s as tall as the screen: their acrylic backdrop read as busy pixels
/// under the pill and thinned it to a line.
/// `XamlExplorerHostIslandWindow` is the same kind of host on newer Windows 11 builds (the log from
/// another PC showed it hiding the notch).
const SHELL_CLASSES: [&str; 7] = ["Progman", "WorkerW", "Shell_TrayWnd", "Shell_SecondaryTrayWnd", "XamlWindow", "XamlExplorerHostIslandWindow", "Windows.UI.Core.CoreWindow"];

pub fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 64];
    let n = unsafe { GetClassNameW(hwnd, &mut buf) }.max(0) as usize;
    String::from_utf16_lossy(&buf[..n])
}

/// A game in exclusive full screen or Presentation Mode: Windows says so, no window to measure.
fn shell_says_fullscreen() -> bool {
    unsafe { SHQueryUserNotificationState() }.is_ok_and(|s| s == QUNS_RUNNING_D3D_FULL_SCREEN || s == QUNS_PRESENTATION_MODE)
}

/// `own` = notch window, `center_x` = notch centre in screen px, `monitor` = notch monitor rect,
/// `pill_w` = current resting pill width (logical px). `Busy` when the foreground window has a tab or a
/// button under the pill's body (asked of the window itself, see `covers_something`).
/// `sample` = false skips that verdict: it only means something while the notch rests at its pill size
/// (not hovered, open, bannering or animating).
/// `last` = foreground window and rect seen by the previous call; the verdict waits until they repeat,
/// so a maximize / restore animation is not mistaken for a window that stays.
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
        // The window answers in about 1 ms; one that took long holds the notch's window up that long.
        static SAID: AtomicIsize = AtomicIsize::new(0);
        let now = crate::unix_ms() as isize;
        if asked.elapsed().as_millis() > 15 && now - SAID.load(Ordering::Relaxed) > 10_000 {
            SAID.store(now, Ordering::Relaxed);
            crate::system::log(&format!("slow hit test: {} ms, {} ({})", asked.elapsed().as_millis(), class_name(fg), if answer.is_some() { "answered" } else { "timed out" }));
        }
        answer.map_or(Check::Unknown, Check::Busy)
    }
}

/// Does the window `r` reach under the pill's body (its resting height from the top of the monitor)?
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
        // Windows Terminal answers caption everywhere; its drag-bar child window is the bare part
        let bar = Some(rect(1100, 0, 1920, 32));
        assert!(!is_empty_title(HTCAPTION, bar, 960), "left of the drag bar: a tab");
        assert!(is_empty_title(HTCAPTION, bar, 1500), "inside the drag bar: bare");
        assert!(!is_empty_title(HTCLIENT, bar, 1500), "a button inside the bare part still answers client");
    }

    #[test]
    fn a_window_reaches_the_pill_when_it_covers_the_top_edge_under_it() {
        // 1920 px wide screen, the pill's centre at 960, 150 px pill (body half 61 px), scale 1.
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
        // Measured from the pill's own width, minus its rounded corners: a 150 px pill with 14 px
        // corners checks 61 px each side of the centre, the 216 px music pill 94 px.
        assert_eq!(body_half(target(Mode::Collapsed, false)[0]), 61.0);
        assert_eq!(body_half(target(Mode::Collapsed, true)[0]), 94.0);
        assert_eq!(body_half(10.0), 0.0, "never negative");
    }

    #[test]
    fn the_span_is_cut_to_the_window_under_it() {
        assert_eq!(clip_span(900, 1020, 0, 1920), Some((900, 1020)), "window covers it all");
        // 2 px margin inside the window's own edge
        assert_eq!(clip_span(900, 1020, 950, 1920), Some((952, 1020)), "window starts under the pill");
        assert_eq!(clip_span(900, 1020, 0, 960), Some((900, 958)), "window ends under the pill");
        assert_eq!(clip_span(900, 1020, 1000, 1100), Some((1002, 1020)), "only a sliver");
        assert_eq!(clip_span(900, 1020, 1100, 1900), None, "window entirely beside the pill");
        assert_eq!(clip_span(900, 903, 0, 1920), None, "too narrow to tell anything");
    }
}

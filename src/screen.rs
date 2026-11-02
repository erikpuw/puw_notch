//! What is under / around the notch: fullscreen apps, and windows reaching under the pill. Only what Windows
//! says about windows is used, never the screen's pixels, so the notch is not left out of screen capture and
//! shows in remote desktop and screen sharing.

use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::UI::Shell::{SHQueryUserNotificationState, QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::notch;

pub enum Check {
    /// A fullscreen app (game, video) owns the notch's monitor: hide the notch.
    Fullscreen,
    /// Normal desktop; `true` when the foreground window reaches under the pill (tabs, a title bar).
    /// Not checked -> `false`, and the caller ignores it.
    Busy(bool),
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
/// `pill_w` = current resting pill width (logical px). The verdict comes from the windows alone:
/// `Busy` when the foreground window reaches under the pill's body, i.e. a maximized or snapped window
/// whose tabs or title bar sit where the pill is.
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
        Check::Busy(reaches_pill(r, center_x, monitor.top, scale, pill_w))
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
    use super::{body_half, clip_span, reaches_pill};
    use windows::Win32::Foundation::RECT;

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
        RECT { left, top, right, bottom }
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

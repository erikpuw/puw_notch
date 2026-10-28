//! What is under / around the notch: fullscreen apps and busy content beside the pill.

use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::notch;

pub enum Check {
    /// A fullscreen app (game, video) owns the notch's monitor: hide the notch.
    Fullscreen,
    /// Normal desktop; `true` when the window under the notch has busy pixels beside the pill.
    /// Not sampled -> `false`, and the caller ignores it.
    Busy(bool),
    /// The foreground window just changed, moved or resized (maximize / restore animation):
    /// no verdict until it holds still.
    Settling,
}

/// What the pill covers: the body of the pill (its resting height, the top of the screen), measured
/// from the pill itself, short or long (music widens it), without its rounded corners. The notch
/// window is left out of the capture while this is read, so it sees the tabs under the pill.
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

/// A pixel is "edge" when a channel differs from its left neighbour by at least this much.
const EDGE_DELTA: u8 = 14;
/// Edge pixels under the pill that make it "covering something": a tab title, a tab's edge.
const BUSY_EDGES: usize = 12;

/// Busy when the pill sits on content: tabs or a title under it (Edge, Terminal). `None` = the
/// window does not reach under the pill. Maximize / restore no longer trips it: `check` only samples
/// a window that held still, and `Notch::report_busy` needs two checks in a row.
pub fn is_busy(under_pill: Option<usize>) -> bool {
    under_pill.is_some_and(|n| n >= BUSY_EDGES)
}

/// Count edge pixels in a top-down BGRA buffer `w` pixels wide.
pub fn edge_pixels(px: &[u8], w: usize) -> usize {
    let mut n = 0;
    for row in px.chunks_exact(w * 4) {
        for pair in row.windows(8).step_by(4) {
            let d = (0..3).map(|c| pair[c].abs_diff(pair[c + 4])).max().unwrap_or(0);
            if d >= EDGE_DELTA {
                n += 1;
            }
        }
    }
    n
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

const SHELL_CLASSES: [&str; 4] = ["Progman", "WorkerW", "Shell_TrayWnd", "Shell_SecondaryTrayWnd"];

pub fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 64];
    let n = unsafe { GetClassNameW(hwnd, &mut buf) }.max(0) as usize;
    String::from_utf16_lossy(&buf[..n])
}

/// `own` = notch window, `center_x` = notch centre in screen px, `monitor` = notch monitor rect,
/// `pill_w` = current resting pill width (logical px); the strips follow `strip_span`.
/// `sample` = false skips the pixel strips: the capture includes our own window, so they are only
/// meaningful while the notch rests at its pill size (not hovered, open, bannering or animating).
/// `last` = foreground window and rect seen by the previous call; sampling waits until they repeat.
pub fn check(own: HWND, center_x: i32, monitor: RECT, scale: f32, pill_w: f32, sample: bool, last: &mut Option<(isize, RECT)>) -> Check {
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
        let half = (body_half(pill_w) * scale) as i32;
        let sh = (PILL_H * scale) as i32;
        let top = monitor.top;
        // Only sample where the foreground window actually is, so wallpaper never counts.
        if r.top > top + sh || r.bottom <= top {
            return Check::Busy(false);
        }
        let y = r.top.max(top);
        let h = top + sh - y;
        let under = clip_span(center_x - half, center_x + half, r.left, r.right).filter(|_| h > 3);
        Check::Busy(is_busy(under.map(|(from, to)| grab_edges(own, from, y, to - from, h))))
    }
}

/// Screen capture of the area under the pill. The notch window is dropped from the capture for the
/// moment (display affinity), else it would hide exactly what is being looked for.
unsafe fn grab_edges(own: HWND, x: i32, y: i32, w: i32, h: i32) -> usize {
    let _ = SetWindowDisplayAffinity(own, WDA_EXCLUDEFROMCAPTURE);
    // The compositor drops the window from what it hands to capture on its next frame.
    let _ = windows::Win32::Graphics::Dwm::DwmFlush();
    let edges = grab_edges_raw(x, y, w, h);
    let _ = SetWindowDisplayAffinity(own, WDA_NONE);
    edges
}

unsafe fn grab_edges_raw(x: i32, y: i32, w: i32, h: i32) -> usize {
    let screen = GetDC(None);
    let mem = CreateCompatibleDC(Some(screen));
    let mut bmi = BITMAPINFO::default();
    bmi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
    bmi.bmiHeader.biWidth = w;
    bmi.bmiHeader.biHeight = -h;
    bmi.bmiHeader.biPlanes = 1;
    bmi.bmiHeader.biBitCount = 32;
    let mut bits = std::ptr::null_mut();
    let mut edges = 0;
    if let Ok(bmp) = CreateDIBSection(Some(mem), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
        let old = SelectObject(mem, bmp.into());
        if BitBlt(mem, 0, 0, w, h, Some(screen), x, y, SRCCOPY).is_ok() && !bits.is_null() {
            let px = std::slice::from_raw_parts(bits as *const u8, (w * h * 4) as usize);
            edges = edge_pixels(px, w as usize);
        }
        SelectObject(mem, old);
        let _ = DeleteObject(bmp.into());
    }
    let _ = DeleteDC(mem);
    ReleaseDC(None, screen);
    edges
}

#[cfg(test)]
mod tests {
    use super::{body_half, clip_span, edge_pixels, is_busy};

    #[test]
    fn busy_when_content_sits_under_the_pill() {
        assert!(is_busy(Some(40)), "tab titles under the pill");
        assert!(is_busy(Some(30)), "one tab edge running down the pill's height");
        assert!(!is_busy(Some(11)), "a few stray pixels");
        assert!(!is_busy(Some(0)), "empty title bar");
        assert!(!is_busy(None), "the window does not reach under the pill");
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

    #[test]
    fn flat_is_quiet_stripes_are_busy() {
        let w = 8;
        let flat = vec![40u8; w * 2 * 4];
        assert_eq!(edge_pixels(&flat, w), 0);
        let stripes: Vec<u8> = (0..w * 2).flat_map(|i| if i % 2 == 0 { [0, 0, 0, 255] } else { [200, 200, 200, 255] }).collect();
        assert_eq!(edge_pixels(&stripes, w), 2 * (w - 1));
    }
}

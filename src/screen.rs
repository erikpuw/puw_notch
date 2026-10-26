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

/// Strips sampled left and right of the pill, as Nusa does (read from its code): 18-78 px outside
/// the resting 150 px pill, top 30 px of the screen. The outer edge stays put when music widens the
/// pill, so the strips never reach further out than Nusa's.
const STRIP_GAP: f32 = 18.0;
const STRIP_W: f32 = 60.0;
const STRIP_H: f32 = 30.0;
/// The capture includes our own window: stay this far outside the current pill (its ears reach 10.5).
const EAR_CLEAR: f32 = 14.0;

/// (inner, outer) distance of each strip from the notch centre, logical px, for a pill `pill_w` wide.
fn strip_span(pill_w: f32) -> (f32, f32) {
    let rest_half = notch::target(notch::Mode::Collapsed, false)[0] / 2.0;
    let outer = rest_half + STRIP_GAP + STRIP_W;
    ((rest_half + STRIP_GAP).max(pill_w / 2.0 + EAR_CLEAR), outer)
}
/// A pixel is "edge" when a channel differs from its left neighbour by at least this much.
const EDGE_DELTA: u8 = 14;
/// Edge pixels that make a strip "text": tabs or a title running up to the notch.
const BUSY_EDGES: usize = 12;

/// Busy when text sits right beside the pill on either side: tabs reaching under the notch from
/// the left (Edge, Terminal) count as much as tabs on both sides. `None` = strip outside the window.
/// Maximize / restore no longer trips it: `check` only samples a window that held still, and
/// `Notch::report_busy` needs two checks in a row.
pub fn is_busy(left: Option<usize>, right: Option<usize>) -> bool {
    [left, right].into_iter().flatten().any(|n| n >= BUSY_EDGES)
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

fn class_name(hwnd: HWND) -> String {
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
        let (inner, outer) = strip_span(pill_w);
        let (inner, outer) = ((inner * scale) as i32, (outer * scale) as i32);
        let sw = outer - inner;
        let sh = (STRIP_H * scale) as i32;
        let top = monitor.top;
        // Only sample where the foreground window actually is, so wallpaper never counts.
        if r.top > top + sh || r.bottom <= top {
            return Check::Busy(false);
        }
        let y = r.top.max(top);
        let h = top + sh - y;
        let strip = |x: i32| (x >= r.left + 2 && x + sw <= r.right - 2 && sw >= 4 && h > 3).then(|| grab_edges(x, y, sw, h));
        Check::Busy(is_busy(strip(center_x - outer), strip(center_x + inner)))
    }
}

/// Screen capture of one strip. It includes the notch window too (measured on Windows 11, with or
/// without CAPTUREBLT), hence the `sample` gate in `check`.
unsafe fn grab_edges(x: i32, y: i32, w: i32, h: i32) -> usize {
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
    use super::{edge_pixels, is_busy, strip_span};

    #[test]
    fn busy_with_text_beside_the_notch() {
        assert!(is_busy(Some(40), Some(30)), "tabs on both sides");
        assert!(is_busy(Some(105), Some(0)), "Edge / Terminal tabs reaching the notch from the left");
        assert!(is_busy(None, Some(60)), "the other strip outside the window");
        assert!(!is_busy(Some(11), Some(0)), "a few stray pixels");
        assert!(!is_busy(Some(0), Some(0)), "empty title bar");
        assert!(!is_busy(None, None));
    }

    #[test]
    fn strips_sit_where_nusa_samples() {
        use crate::notch::{target, Mode};
        // Resting pill: 93-153 px from the centre, like Nusa (18-78 px outside a 150 px pill).
        assert_eq!(strip_span(target(Mode::Collapsed, false)[0]), (93.0, 153.0));
        // Music pill (wider): the inner edge moves out past the pill and its ears, the outer edge stays,
        // so Edge's "+" after 3 tabs (~170 px out) never counts and a cut tab title (~130 px) does.
        let (inner, outer) = strip_span(target(Mode::Collapsed, true)[0]);
        assert!(inner > target(Mode::Collapsed, true)[0] / 2.0 + crate::notch::ear(30.0));
        assert_eq!(outer, 153.0);
        assert!(inner < 130.0 && 170.0 > outer);
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

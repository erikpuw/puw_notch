//! What is under / around the notch: fullscreen apps and busy content beside the pill.

use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::*;

pub enum Check {
    /// A fullscreen app (game, video) owns the notch's monitor: hide the notch.
    Fullscreen,
    /// Normal desktop; `true` when the window under the notch has busy pixels beside the pill.
    Busy(bool),
}

/// Strips sampled left and right of the collapsed pill (logical px).
const STRIP_GAP: f32 = 18.0;
const STRIP_W: f32 = 60.0;
const STRIP_H: f32 = 36.0;
/// A pixel is "edge" when a channel differs from its left neighbour by at least this much.
const EDGE_DELTA: u8 = 14;
/// This many edge pixels across both strips = busy (tabs, text, icons).
const BUSY_EDGES: usize = 12;

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

const SHELL_CLASSES: [&str; 4] = ["Progman", "WorkerW", "Shell_TrayWnd", "Shell_SecondaryTrayWnd"];

fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 64];
    let n = unsafe { GetClassNameW(hwnd, &mut buf) }.max(0) as usize;
    String::from_utf16_lossy(&buf[..n])
}

/// `own` = notch window, `center_x` = notch centre in screen px, `monitor` = notch monitor rect.
pub fn check(own: HWND, center_x: i32, monitor: RECT, scale: f32) -> Check {
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

        let half = (crate::notch::target(crate::notch::Mode::Collapsed)[0] / 2.0 * scale) as i32;
        let gap = (STRIP_GAP * scale) as i32;
        let sw = (STRIP_W * scale) as i32;
        let sh = (STRIP_H * scale) as i32;
        let top = monitor.top;
        // Only sample where the foreground window actually is, so wallpaper never counts.
        if r.top > top + sh || r.bottom <= top {
            return Check::Busy(false);
        }
        let y = r.top.max(top);
        let h = top + sh - y;
        let strips = [center_x - half - gap - sw, center_x + half + gap];
        let mut edges = 0;
        for x in strips {
            if x >= r.left + 2 && x + sw <= r.right - 2 && h > 3 {
                edges += grab_edges(x, y, sw, h);
            }
        }
        Check::Busy(edges >= BUSY_EDGES)
    }
}

/// BitBlt without CAPTUREBLT skips layered windows, so our own notch never shows up in the sample.
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
    use super::edge_pixels;

    #[test]
    fn flat_is_quiet_stripes_are_busy() {
        let w = 8;
        let flat = vec![40u8; w * 2 * 4];
        assert_eq!(edge_pixels(&flat, w), 0);
        let stripes: Vec<u8> = (0..w * 2).flat_map(|i| if i % 2 == 0 { [0, 0, 0, 255] } else { [200, 200, 200, 255] }).collect();
        assert_eq!(edge_pixels(&stripes, w), 2 * (w - 1));
    }
}

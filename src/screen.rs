//! What is under / around the notch: fullscreen apps and busy content under the pill. The content is read
//! from the foreground window's own picture (`PrintWindow`), which has no notch in it, so the notch is not
//! left out of screen capture and shows in remote desktop and screen sharing.

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{mpsc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::UI::Accessibility::{SetWinEventHook, HWINEVENTHOOK};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
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
    /// Normal desktop; `true` when the window under the notch has busy pixels (tabs, a title) under the pill.
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

/// The part of a window to read: its rect on screen, its title (a browser's or a terminal's title says which
/// tab is on and how many there are, so a new title is a window to read again), and the region under the pill
/// in the window's own pixels (`x`, `y` from its top left, `w` x `h`).
struct Job {
    hwnd: isize,
    rect: RECT,
    title: u64,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

/// What a read found, for the window (and the place it stood) it was made of.
struct Verdict {
    hwnd: isize,
    rect: RECT,
    title: u64,
    busy: bool,
    /// How many reads in a row found nothing busy since it was last busy.
    quiet: u32,
    /// How many times this window (place and title) has been read, when last, and the tick count then.
    reads: u32,
    at: Instant,
    tick: u32,
}

static VERDICT: Mutex<Option<Verdict>> = Mutex::new(None);
static PENDING: AtomicBool = AtomicBool::new(false);
static JOBS: OnceLock<mpsc::Sender<Job>> = OnceLock::new();

/// The reader thread. `PrintWindow` asks the other window to draw itself and waits for it: an app that
/// hangs would hold the caller, so it is never the UI thread.
fn reader() -> &'static mpsc::Sender<Job> {
    JOBS.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Job>();
        std::thread::spawn(move || {
            for job in rx {
                let found = unsafe { read_band(&job) };
                if let Ok(mut v) = VERDICT.lock() {
                    // A picture that did not come (the window would not draw, or drew nothing) says nothing:
                    // the verdict before it for this window stands.
                    let same = v.as_ref().filter(|v| v.hwnd == job.hwnd && v.rect == job.rect && v.title == job.title);
                    let (before, quiet, reads) = (same.map(|v| v.busy), same.map_or(0, |v| v.quiet), same.map_or(0, |v| v.reads));
                    // A picture that shows tabs is trusted at once; one that shows none, only when the one after it
                    // says so too: a window that draws itself (Notepad) gives an empty picture now and then.
                    let (busy, quiet) = match found {
                        Some(true) => (true, 0),
                        Some(false) => (before.unwrap_or(false) && quiet < 1, quiet + 1),
                        None => (before.unwrap_or(false), quiet),
                    };
                    *v = Some(Verdict { hwnd: job.hwnd, rect: job.rect, title: job.title, busy, quiet, reads: reads + 1, at: Instant::now(), tick: unsafe { GetTickCount() } });
                }
                PENDING.store(false, Ordering::Relaxed);
            }
        });
        tx
    })
}

/// Edge pixels of the region in the window's own picture, as busy or not; `None` when no picture came.
/// The window is drawn into a bitmap that is only as tall as the region reaches down (the rest is cut off).
unsafe fn read_band(job: &Job) -> Option<bool> {
    let (width, rows) = (job.rect.right - job.rect.left, job.y + job.h);
    if width <= 0 || rows <= 0 || width > 16_384 || rows > 512 || job.x < 0 || job.y < 0 || job.x + job.w > width {
        return None;
    }
    let screen = GetDC(None);
    let mem = CreateCompatibleDC(Some(screen));
    let mut bmi = BITMAPINFO::default();
    bmi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
    bmi.bmiHeader.biWidth = width;
    bmi.bmiHeader.biHeight = -rows;
    bmi.bmiHeader.biPlanes = 1;
    bmi.bmiHeader.biBitCount = 32;
    let mut bits = std::ptr::null_mut();
    let mut busy = None;
    if let Ok(bmp) = CreateDIBSection(Some(mem), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
        let old = SelectObject(mem, bmp.into());
        // 2 = PW_RENDERFULLCONTENT: windows drawn by the GPU (Edge, Terminal) too.
        if PrintWindow(HWND(job.hwnd as *mut _), mem, PRINT_WINDOW_FLAGS(2)).as_bool() && !bits.is_null() {
            let px = std::slice::from_raw_parts(bits as *const u8, (width * rows * 4) as usize);
            let mut region = Vec::with_capacity((job.w * job.h * 4) as usize);
            for row in job.y..job.y + job.h {
                let start = ((row * width + job.x) * 4) as usize;
                region.extend_from_slice(&px[start..start + (job.w * 4) as usize]);
            }
            // All zero = nothing drawn (a window the GPU keeps to itself).
            if region.iter().any(|b| *b != 0) {
                busy = Some(is_busy(Some(edge_pixels(&region, job.w as usize))));
            }
        }
        SelectObject(mem, old);
        let _ = DeleteObject(bmp.into());
    }
    let _ = DeleteDC(mem);
    ReleaseDC(None, screen);
    busy
}

/// What is known of a window's last read: busy, quiet reads in a row, reads so far, time since, and the tick
/// count of it.
type Known = (bool, u32, u32, Duration, u32);

/// Read the window now? When it has no verdict yet (new window, new place, new title); while the verdict says
/// "not busy", four more times in its first seconds, soon after each other (a window that has just opened has not drawn its tabs yet); and
/// once after the user has touched the keyboard or mouse since the last read and stopped for a moment (closing
/// a tab leaves the title as it was, but it takes a click or a key); and soon again when a busy window's last
/// read found nothing (one empty picture is not believed alone). `input` = tick counts of the last input and
/// of now.
fn should_read(known: Option<Known>, input: (u32, u32)) -> bool {
    let Some((busy, quiet, reads, age, tick)) = known else { return true };
    let touched = (input.0.wrapping_sub(tick) as i32) > 0 && input.1.wrapping_sub(input.0) >= 600;
    let soon = Duration::from_millis(400);
    (busy && quiet > 0 && age >= soon) || (!busy && reads < 5 && age >= soon) || (touched && age >= Duration::from_millis(1500))
}

/// The tick counts of the last keyboard or mouse input and of now.
fn last_input() -> (u32, u32) {
    let mut info = LASTINPUTINFO { cbSize: size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
    unsafe {
        let _ = GetLastInputInfo(&mut info);
        (info.dwTime, GetTickCount())
    }
}

/// The verdict for this window where it stands now, with this title (false until the first read is back).
/// A window is read when it comes to the front, moves, or changes title, and after the user has used it and
/// stopped (a tab opened, closed or switched); never on a timer, so at rest it costs nothing.
fn busy_under(job: Job) -> bool {
    let known = VERDICT
        .lock()
        .ok()
        .and_then(|v| v.as_ref().filter(|v| v.hwnd == job.hwnd && v.rect == job.rect && v.title == job.title).map(|v| (v.busy, v.quiet, v.reads, v.at.elapsed(), v.tick)));
    let (first, key) = (known.is_none(), (job.hwnd, job.rect, job.title));
    if should_read(known, last_input()) && !PENDING.swap(true, Ordering::Relaxed) {
        if reader().send(job).is_err() {
            PENDING.store(false, Ordering::Relaxed);
        } else if first {
            // A window draws itself in about 20 ms: a new window's first read is waited for (up to 8 x 5 ms), so
            // the notch answers at once. An app that hangs is left to the reader thread.
            for _ in 0..8 {
                std::thread::sleep(Duration::from_millis(5));
                if !PENDING.load(Ordering::Relaxed) {
                    break;
                }
            }
            return VERDICT.lock().ok().and_then(|v| v.as_ref().filter(|v| (v.hwnd, v.rect, v.title) == key).map(|v| v.busy)).unwrap_or(false);
        }
    }
    known.is_some_and(|(busy, ..)| busy)
}

/// The window's title as a number (not kept as text): equal titles, equal numbers.
fn title_of(hwnd: HWND) -> u64 {
    let mut buf = [0u16; 256];
    let n = unsafe { GetWindowTextW(hwnd, &mut buf) }.max(0) as usize;
    buf[..n].iter().fold(0u64, |h, c| h.wrapping_mul(31).wrapping_add(u64::from(*c)))
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
/// `pill_w` = current resting pill width (logical px). `Busy` when the foreground window has busy pixels
/// (tabs, a title) under the pill's body, read from the window's own picture by the reader thread.
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
        // The pill's body in the window's own pixels: from the top of the screen down the pill's height,
        // across the span the window covers.
        let (half, sh) = ((body_half(pill_w) * scale) as i32, (PILL_H * scale) as i32);
        let y = r.top.max(monitor.top);
        let h = monitor.top + sh - y;
        let Some((from, to)) = clip_span(center_x - half, center_x + half, r.left, r.right).filter(|_| h > 3) else {
            return Check::Busy(false);
        };
        Check::Busy(busy_under(Job { hwnd: fg.0 as isize, rect: r, title: title_of(fg), x: from - r.left, y: y - r.top, w: to - from, h }))
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
    use super::{body_half, clip_span, edge_pixels, is_busy, reaches_pill, should_read};
    use std::time::Duration;
    use windows::Win32::Foundation::RECT;

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
        RECT { left, top, right, bottom }
    }

    #[test]
    fn a_window_is_read_when_new_twice_more_while_not_busy_and_after_the_user_used_it() {
        let ms = Duration::from_millis;
        let quiet = (0, 10_000); // no input for a long time
        // known = (busy, quiet reads in a row, reads, time since, tick of the read)
        assert!(should_read(None, quiet), "no verdict yet: a new window, place or title");
                assert!(should_read(Some((false, 1, 1, ms(450), 5_000)), quiet), "not busy: look again soon, its tabs may not be drawn yet");
        assert!(!should_read(Some((false, 1, 1, ms(200), 5_000)), quiet), "but not in a hurry");
        assert!(!should_read(Some((false, 5, 5, ms(60_000), 5_000)), quiet), "then it is left alone");
        assert!(!should_read(Some((true, 0, 1, ms(60_000), 5_000)), quiet), "busy stays busy while nobody touches it");
        // a click after the read, and the user has stopped for 600 ms: a tab may have gone
        assert!(should_read(Some((true, 0, 1, ms(5_000), 5_000)), (6_000, 6_700)), "touched since the read and quiet again");
        assert!(!should_read(Some((true, 0, 1, ms(5_000), 5_000)), (6_000, 6_300)), "still busy using it: wait for the pause");
        assert!(!should_read(Some((true, 0, 1, ms(5_000), 5_000)), (4_000, 9_000)), "the last input was before the read");
        assert!(!should_read(Some((true, 0, 2, ms(1_000), 5_000)), (6_000, 6_700)), "not more than once in 1.5 s");
        // a busy window whose last picture showed nothing is asked again soon, to be sure
        assert!(should_read(Some((true, 1, 4, ms(500), 5_000)), quiet), "one empty picture is not believed alone");
        assert!(!should_read(Some((true, 1, 4, ms(200), 5_000)), quiet), "but not in a hurry");
    }

    #[test]
    fn busy_when_content_sits_under_the_pill() {
        assert!(is_busy(Some(40)), "tab titles under the pill");
        assert!(is_busy(Some(30)), "one tab edge running down the pill's height");
        assert!(!is_busy(Some(11)), "a few stray pixels");
        assert!(!is_busy(Some(0)), "empty title bar");
        assert!(!is_busy(None), "the window does not reach under the pill");
    }

    #[test]
    fn flat_is_quiet_stripes_are_busy() {
        let w = 8;
        let flat = vec![40u8; w * 2 * 4];
        assert_eq!(edge_pixels(&flat, w), 0);
        let stripes: Vec<u8> = (0..w * 2).flat_map(|i| if i % 2 == 0 { [0, 0, 0, 255] } else { [200, 200, 200, 255] }).collect();
        assert_eq!(edge_pixels(&stripes, w), 2 * (w - 1));
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

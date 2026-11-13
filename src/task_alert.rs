use std::time::{Duration, Instant};

use crate::i18n::{english, tr};
use crate::task::{self, Status, Tasks};

pub const LOCK: Duration = Duration::from_secs(1);
pub const CARD_W: f32 = 400.0;
pub const LIST_W: f32 = 440.0;
pub const SINGLE_H: f32 = 236.0;
pub const LIST_TOP: f32 = 64.0;
pub const LIST_ROW_H: f32 = 44.0;
pub const LIST_FOOT: f32 = 32.0;
pub const MAX_ROWS: usize = 4;
const MORE_H: f32 = 20.0;
const VK_ENTER: u16 = 0x0D;
const VK_ESC: u16 = 0x1B;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Choice {
    Done(usize),
    NotYet(usize),
    AllNotYet,
}

pub fn accepts_input(opened: Instant, now: Instant) -> bool {
    now.duration_since(opened) >= LOCK
}

pub fn key_choice(vk: u16, shown: &[usize]) -> Option<Choice> {
    match (vk, shown) {
        (VK_ENTER, [one]) => Some(Choice::Done(*one)),
        (VK_ESC, [_, ..]) => Some(Choice::AllNotYet),
        _ => None,
    }
}

pub fn forget(list: &mut Vec<usize>, removed: usize) {
    list.retain(|&j| j != removed);
    for j in list.iter_mut().filter(|j| **j > removed) {
        *j -= 1;
    }
}

pub fn card_height(rows: usize) -> f32 {
    match rows {
        0 | 1 => SINGLE_H,
        n => LIST_TOP + LIST_ROW_H * n.min(MAX_ROWS) as f32 + LIST_FOOT + if n > MAX_ROWS { MORE_H } else { 0.0 },
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AlertRow {
    pub task: usize,
    pub title: String,
    pub kind: &'static str,
    pub due: String,
    pub when: String,
    pub next: String,
    pub big: String,
    pub status: Status,
    pub told: u32,
    pub snooze: u32,
}

pub fn alert_rows(tasks: &Tasks, shown: &[usize], now: u64) -> Vec<AlertRow> {
    shown
        .iter()
        .filter(|&&i| i < tasks.items.len())
        .map(|&i| {
            let t = &tasks.items[i];
            let status = tasks.status(i, now);
            let date = task::date_of(t.due);
            let (y, m, d) = ((date / 10_000) as i32, date / 100 % 100, date % 100);
            let wd = crate::calendar::weekday_short(crate::calendar::weekday(y, m, d), english());
            let when = format!("{wd} {d}/{m}, {:02}:{:02}", t.due / 100 % 100, t.due % 100);
            let due = format!("{} {when}", tr("Hạn:", "Due"));
            let again = task::later(now, t.snooze);
            let next = format!("{:02}:{:02}", again / 100 % 100, again % 100);
            let left = task::days_left(t.due, now);
            let (kind, big) = match status {
                Status::Late(n) => (tr("trễ hạn", "late"), if english() { format!("{n} day{} late", if n == 1 { "" } else { "s" }) } else { format!("Trễ {n} ngày") }),
                Status::Today => (tr("đến hạn", "due"), tr("Đến hạn hôm nay", "Due today").to_string()),
                _ => (tr("sắp đến hạn", "coming up"), if english() { format!("{left} day{} left", if left == 1 { "" } else { "s" }) } else { format!("Còn {left} ngày") }),
            };
            AlertRow { task: i, title: t.title.clone(), kind, due, when, next, big, status, told: t.told, snooze: t.snooze }
        })
        .collect()
}

pub const CLASS: windows::core::PCWSTR = windows::core::w!("PuwTaskAlert");
pub const T_FRAME: usize = 1;
const FRAME_MS: u32 = 16;
const APPEAR: Duration = Duration::from_millis(250);
const WCA_ACCENT_POLICY: u32 = 19;
const ACCENT_ACRYLIC: u32 = 4;
const DIM_ABGR: u32 = 0x7300_0000;

pub struct Alert {
    pub hwnd: HWND,
    renderer: crate::render::Renderer,
    pub shown: Vec<usize>,
    pub opened: Instant,
    pub hovered: Option<Choice>,
    size: (i32, i32),
    scale: f32,
}

use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, KillTimer, SetForegroundWindow, SetTimer, ShowWindow, SW_SHOW, WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP};

fn blur_behind(hwnd: HWND) {
    #[repr(C)]
    struct Accent { state: u32, flags: u32, color: u32, anim: u32 }
    #[repr(C)]
    struct Data { attr: u32, data: *mut core::ffi::c_void, size: usize }
    use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    unsafe {
        let Ok(user32) = GetModuleHandleW(windows::core::w!("user32.dll")) else { return };
        let Some(proc) = GetProcAddress(user32, windows::core::s!("SetWindowCompositionAttribute")) else { return };
        let set: unsafe extern "system" fn(HWND, *mut Data) -> i32 = std::mem::transmute(proc);
        let mut accent = Accent { state: ACCENT_ACRYLIC, flags: 0, color: DIM_ABGR, anim: 0 };
        set(hwnd, &mut Data { attr: WCA_ACCENT_POLICY, data: &mut accent as *mut _ as _, size: size_of::<Accent>() });
    }
}

impl Alert {
    pub fn open(monitor: RECT, scale: f32, shown: Vec<usize>) -> Option<Self> {
        let (w, h) = (monitor.right - monitor.left, monitor.bottom - monitor.top);
        unsafe {
            let hwnd = CreateWindowExW(WS_EX_NOREDIRECTIONBITMAP | WS_EX_TOPMOST | WS_EX_TOOLWINDOW, CLASS, windows::core::w!("PUW Task"), WS_POPUP, monitor.left, monitor.top, w, h, None, None, None, None).ok()?;
            blur_behind(hwnd);
            let renderer = match crate::render::Renderer::new() {
                Ok(r) => r,
                Err(_) => {
                    let _ = DestroyWindow(hwnd);
                    return None;
                }
            };
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
            SetTimer(Some(hwnd), T_FRAME, FRAME_MS, None);
            Some(Alert { hwnd, renderer, shown, opened: Instant::now(), hovered: None, size: (w, h), scale })
        }
    }

    pub fn locked(&self) -> bool {
        !accepts_input(self.opened, Instant::now())
    }

    pub fn draw(&mut self, tasks: &Tasks, now: u64) {
        let rows = alert_rows(tasks, &self.shown, now);
        let appear = self.opened.elapsed().as_secs_f32() / APPEAR.as_secs_f32();
        let locked = self.locked();
        let view = crate::render::AlertView { rows: &rows, appear, hovered: self.hovered, locked };
        let _ = self.renderer.present_alert(self.hwnd, self.size.0, self.size.1, self.scale, &view);
        if appear >= 1.0 && !locked {
            unsafe {
                let _ = KillTimer(Some(self.hwnd), T_FRAME);
            }
        }
    }

    pub fn hit(&self, x: f32, y: f32) -> Option<Choice> {
        crate::render::alert_buttons(self.size.0 as f32, self.size.1 as f32, self.scale, &self.shown)
            .into_iter()
            .find(|(_, r)| x >= r.left && x < r.right && y >= r.top && y < r.bottom)
            .map(|(c, _)| c)
    }

    pub fn take(&mut self, choice: Choice) -> Vec<usize> {
        match choice {
            Choice::Done(i) | Choice::NotYet(i) => {
                self.shown.retain(|&j| j != i);
                vec![i]
            }
            Choice::AllNotYet => std::mem::take(&mut self.shown),
        }
    }
}

impl Drop for Alert {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn input_locked_first_second() {
        let t = Instant::now();
        assert!(!accepts_input(t, t + Duration::from_millis(999)));
        assert!(accepts_input(t, t + Duration::from_millis(1000)));
    }

    #[test]
    fn keys() {
        assert_eq!(key_choice(0x0D, &[3]), Some(Choice::Done(3)));
        assert_eq!(key_choice(0x0D, &[3, 4]), None);
        assert_eq!(key_choice(0x1B, &[3, 4]), Some(Choice::AllNotYet));
        assert_eq!(key_choice(0x41, &[3]), None);
    }

    #[test]
    fn a_deleted_task_leaves_the_list_and_later_ones_move_up() {
        let mut v = vec![0, 2, 3];
        forget(&mut v, 2);
        assert_eq!(v, vec![0, 2]);
        forget(&mut v, 0);
        assert_eq!(v, vec![1]);
    }

    #[test]
    fn card_grows_with_rows_and_caps_at_four() {
        assert!(card_height(2) < card_height(4));
        assert_eq!(card_height(6), card_height(4) + 20.0);
    }

    #[test]
    fn rows_say_how_late_or_how_soon() {
        use crate::task::{Repeat, Task, Tasks};
        let task = |due: u64| Task { title: "Thuế".into(), repeat: Repeat::Once, day: 1, due, slots: 7, snooze: 30, done: vec![], nag_at: None, told: 2, finished: false };
        let mut t = Tasks::default();
        t.items = vec![task(202611110900), task(202611131600), task(202611160900)];
        let rows = alert_rows(&t, &[0, 1, 2], 202611131000);
        assert_eq!(rows[0].big, "Trễ 2 ngày");
        assert_eq!(rows[1].big, "Đến hạn hôm nay");
        assert_eq!(rows[2].big, "Còn 3 ngày");
        assert_eq!(rows[0].due, "Hạn: T4 11/11, 09:00");
        assert_eq!(rows[0].when, "T4 11/11, 09:00");
        assert_eq!(rows[0].next, "10:30", "Not yet rings again after the task's own minutes");
        assert_eq!(rows[0].told, 2);
    }
}

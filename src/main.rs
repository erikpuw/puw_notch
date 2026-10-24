#![windows_subsystem = "windows"]

mod notch;
mod render;
mod screen;
mod system;

use std::cell::RefCell;
use std::time::Instant;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use notch::{Mode, Notch, DWELL_MS, HOVER_MARGIN, MAX_H, MAX_W};
use render::{Renderer, View};

const WM_TRAY: u32 = WM_APP + 1;
/// Lives in the commctrl headers; not worth a crate feature for one constant.
const WM_MOUSELEAVE: u32 = 0x02A3;
const T_ANIM: usize = 1;
const T_CHECK: usize = 2;
const T_DWELL: usize = 3;
const CHECK_MS: u32 = 500;
const MENU_MINIMAL: usize = 10;
const MENU_AUTOSTART: usize = 11;
const MENU_QUIT: usize = 12;

struct App {
    hwnd: HWND,
    notch: Notch,
    renderer: Renderer,
    scale: f32,
    monitor: RECT,
    /// Window placement in device px: x, y, w, h.
    win: (i32, i32, i32, i32),
    hidden: bool,
    tracking: bool,
    animating: bool,
    last_frame: Instant,
    taskbar_created: u32,
    time: String,
    date: String,
    battery: String,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

impl App {
    fn layout(&mut self) {
        unsafe {
            let mon = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
            let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
            let _ = GetMonitorInfoW(mon, &mut mi);
            self.monitor = mi.rcMonitor;
            let mut dpi_x = 96;
            let mut dpi_y = 96;
            let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
            self.scale = dpi_x as f32 / 96.0;
        }
        let w = (MAX_W * self.scale).ceil() as i32;
        let h = (MAX_H * self.scale).ceil() as i32;
        let m = self.monitor;
        self.win = (m.left + (m.right - m.left - w) / 2, m.top, w, h);
    }

    fn redraw(&mut self) {
        let (x, y, w, h) = self.win;
        let view = View { mode: self.notch.mode, size: self.notch.size(), time: &self.time, date: &self.date, battery: &self.battery };
        let _ = self.renderer.present(self.hwnd, x, y, w, h, self.scale, &view);
    }

    fn kick(&mut self) {
        if !self.animating {
            self.animating = true;
            self.last_frame = Instant::now();
            unsafe { SetTimer(Some(self.hwnd), T_ANIM, 15, None) };
        }
    }

    fn frame(&mut self) {
        let now = Instant::now();
        let dt = (now - self.last_frame).as_secs_f32().min(0.05);
        self.last_frame = now;
        let moving = self.notch.step(dt);
        self.redraw();
        if !moving {
            self.animating = false;
            unsafe { let _ = KillTimer(Some(self.hwnd), T_ANIM); }
        }
    }

    /// Is the cursor over what is drawn right now (pill + hover margin outside mini)?
    fn cursor_inside(&self) -> bool {
        let mut p = POINT::default();
        if unsafe { GetCursorPos(&mut p) }.is_err() {
            return false;
        }
        let [nw, nh, _] = self.notch.size().map(|s| s * self.scale);
        let m = if matches!(self.notch.mode, Mode::Mini | Mode::PeekMini) { 0.0 } else { HOVER_MARGIN * self.scale };
        let cx = (self.win.0 + self.win.2 / 2) as f32;
        let (x, y) = (p.x as f32, (p.y - self.win.1) as f32);
        x >= cx - nw / 2.0 - m && x <= cx + nw / 2.0 + m && y >= 0.0 && y <= nh + m
    }

    fn tick(&mut self) {
        let (time, date) = system::clock();
        let battery = system::battery();
        let changed = (&time, &date, &battery) != (&self.time, &self.date, &self.battery);
        (self.time, self.date, self.battery) = (time, date, battery);

        // While wide the notch covers the sampled strips; keep the last verdict.
        if !self.notch.is_wide() {
            let cx = self.win.0 + self.win.2 / 2;
            match screen::check(self.hwnd, cx, self.monitor, self.scale) {
                screen::Check::Fullscreen => self.set_hidden(true),
                screen::Check::Busy(busy) => {
                    self.set_hidden(false);
                    let before = self.notch.mode;
                    self.notch.report_busy(busy);
                    if self.notch.mode != before {
                        self.kick();
                    }
                }
            }
        }
        if changed && !self.animating {
            self.redraw();
        }
    }

    fn set_hidden(&mut self, hide: bool) {
        if self.hidden != hide {
            self.hidden = hide;
            unsafe { let _ = ShowWindow(self.hwnd, if hide { SW_HIDE } else { SW_SHOWNOACTIVATE }); }
        }
    }

    fn tray(&self, msg: NOTIFY_ICON_MESSAGE) {
        let mut nid = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: 1,
            uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
            uCallbackMessage: WM_TRAY,
            hIcon: unsafe { LoadIconW(None, IDI_APPLICATION) }.unwrap_or_default(),
            ..Default::default()
        };
        for (d, s) in nid.szTip.iter_mut().zip("Nusa Notch".encode_utf16()) {
            *d = s;
        }
        unsafe { let _ = Shell_NotifyIconW(msg, &nid); }
    }

    fn menu(&mut self) {
        unsafe {
            let Ok(menu) = CreatePopupMenu() else { return };
            let check = |on: bool| if on { MF_CHECKED } else { MF_UNCHECKED };
            let _ = AppendMenuW(menu, MF_STRING | check(self.notch.minimal), MENU_MINIMAL, w!("Notch tối giản"));
            let _ = AppendMenuW(menu, MF_STRING | check(system::autostart_enabled()), MENU_AUTOSTART, w!("Khởi động cùng Windows"));
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            let _ = AppendMenuW(menu, MF_STRING, MENU_QUIT, w!("Thoát"));
            let mut p = POINT::default();
            let _ = GetCursorPos(&mut p);
            let _ = SetForegroundWindow(self.hwnd);
            let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, p.x, p.y, None, self.hwnd, None).0 as usize;
            let _ = DestroyMenu(menu);
            match cmd {
                MENU_MINIMAL => {
                    let on = !self.notch.minimal;
                    self.notch.set_minimal(on);
                    system::save_minimal(on);
                    self.kick();
                }
                MENU_AUTOSTART => system::set_autostart(!system::autostart_enabled()),
                MENU_QUIT => { let _ = DestroyWindow(self.hwnd); }
                _ => {}
            }
        }
    }

    fn handle(&mut self, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
        match msg {
            WM_TIMER => match wp.0 {
                T_ANIM => self.frame(),
                T_CHECK => self.tick(),
                T_DWELL => {
                    unsafe { let _ = KillTimer(Some(self.hwnd), T_DWELL); }
                    let inside = self.cursor_inside();
                    self.notch.on_dwell(inside);
                    self.kick();
                }
                _ => return None,
            },
            WM_MOUSEMOVE => {
                if !self.tracking {
                    let mut tme = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: self.hwnd, dwHoverTime: 0 };
                    self.tracking = unsafe { TrackMouseEvent(&mut tme) }.is_ok();
                    if self.notch.on_enter() {
                        unsafe { SetTimer(Some(self.hwnd), T_DWELL, DWELL_MS, None) };
                    }
                    self.kick();
                }
            }
            WM_MOUSELEAVE => {
                self.tracking = false;
                unsafe { let _ = KillTimer(Some(self.hwnd), T_DWELL); }
                self.notch.on_leave();
                self.kick();
            }
            WM_RBUTTONUP => self.menu(),
            WM_TRAY => {
                let event = (lp.0 & 0xffff) as u32;
                if event == WM_RBUTTONUP || event == WM_CONTEXTMENU {
                    self.menu();
                }
            }
            WM_DISPLAYCHANGE | WM_DPICHANGED | WM_SETTINGCHANGE => {
                self.layout();
                self.redraw();
            }
            WM_DESTROY => {
                self.tray(NIM_DELETE);
                unsafe { PostQuitMessage(0) };
            }
            m if m == self.taskbar_created && m != 0 => self.tray(NIM_ADD),
            _ => return None,
        }
        Some(LRESULT(0))
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // Win32 calls made while handling a message can re-enter here; those fall back to the default.
    let handled = APP.with(|a| a.try_borrow_mut().ok().and_then(|mut g| g.as_mut().and_then(|app| app.handle(msg, wp, lp))));
    handled.unwrap_or_else(|| unsafe { DefWindowProcW(hwnd, msg, wp, lp) })
}

fn main() -> windows::core::Result<()> {
    unsafe {
        let _mutex = CreateMutexW(None, false, w!("Local\\NusaRebuild.SingleInstance"))?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return Ok(());
        }
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);

        let instance: HINSTANCE = GetModuleHandleW(None)?.into();
        let class = w!("NusaRebuildNotch");
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: instance,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassExW(&wc);
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class,
            w!("Nusa Notch"),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None,
            Some(instance),
            None,
        )?;

        let (time, date) = system::clock();
        let mut app = App {
            hwnd,
            notch: Notch::new(system::load_minimal()),
            renderer: Renderer::new()?,
            scale: 1.0,
            monitor: RECT::default(),
            win: (0, 0, 1, 1),
            hidden: false,
            tracking: false,
            animating: false,
            last_frame: Instant::now(),
            taskbar_created: RegisterWindowMessageW(w!("TaskbarCreated")),
            time,
            date,
            battery: system::battery(),
        };
        app.layout();
        app.redraw();
        app.tray(NIM_ADD);
        APP.with(|a| *a.borrow_mut() = Some(app));

        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        SetTimer(Some(hwnd), T_CHECK, CHECK_MS, None);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}

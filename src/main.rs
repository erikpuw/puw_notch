#![windows_subsystem = "windows"]

mod audio;
mod bluetooth;
mod music;
mod notch;
mod notify;
mod render;
mod screen;
mod status;
mod system;

use std::cell::RefCell;
use std::time::Instant;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use bluetooth::{Bt, WM_BT};
use notch::{Mode, Notch, ALERT_MS, DWELL_MS, HOVER_MARGIN, MAX_H, MAX_W, STATUS_MS};
use music::WM_MUSIC;
use status::{WM_LOCKKEY, WM_VOLKEY};
use notify::Toast;
use render::{Button, Page, Renderer, View, PAGES};

const WM_TRAY: u32 = WM_APP + 1;
/// Lives in the commctrl headers; not worth a crate feature for one constant.
const WM_MOUSELEAVE: u32 = 0x02A3;
const T_ANIM: usize = 1;
const T_CHECK: usize = 2;
const T_DWELL: usize = 3;
const T_ALERT: usize = 4;
const T_VOLKEY: usize = 5;
const CHECK_MS: u32 = 500;
const VOLUME_STEP: f32 = 0.05;
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
    notifications: Option<notify::Reader>,
    /// What the Alert banner shows right now.
    banner: Option<Toast>,
    /// Newest notification, the card in the panel.
    toast: Option<Toast>,
    brightness: Option<status::Brightness>,
    last_brightness: Option<u8>,
    audio: Option<audio::Audio>,
    volume: Option<(f32, bool)>,
    mic_muted: Option<bool>,
    bt: Bt,
    music: music::Music,
    track: Option<music::Track>,
    started: Instant,
    /// Current T_ANIM interval: fast while the spring moves, slower for the equaliser alone.
    frame_ms: u32,
    page: Page,
}

/// Animation frame intervals.
const FRAME_MS: u32 = 15;
const BARS_FRAME_MS: u32 = 40;

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
        let view = View {
            mode: self.notch.mode,
            size: self.notch.size(),
            time: &self.time,
            date: &self.date,
            battery: &self.battery,
            banner: self.banner.as_ref(),
            toast: self.toast.as_ref(),
            volume: self.volume,
            mic_muted: self.mic_muted,
            bt: self.bt,
            music: self.track.as_ref(),
            phase: self.started.elapsed().as_secs_f32(),
            page: self.page,
        };
        let _ = self.renderer.present(self.hwnd, x, y, w, h, self.scale, &view);
    }

    fn set_frame_interval(&mut self, ms: u32) {
        if self.frame_ms != ms {
            self.frame_ms = ms;
            unsafe { SetTimer(Some(self.hwnd), T_ANIM, ms, None) };
        }
    }

    fn kick(&mut self) {
        if !self.animating {
            self.animating = true;
            self.last_frame = Instant::now();
        }
        self.set_frame_interval(FRAME_MS);
    }

    /// The equaliser in the resting pill keeps frames coming while music plays.
    fn bars_active(&self) -> bool {
        !self.hidden && self.track.as_ref().is_some_and(|t| t.playing) && matches!(self.notch.mode, Mode::Collapsed | Mode::Peek)
    }

    fn frame(&mut self) {
        let now = Instant::now();
        let dt = (now - self.last_frame).as_secs_f32().min(0.05);
        self.last_frame = now;
        let moving = self.notch.step(dt);
        self.redraw();
        if moving {
            self.set_frame_interval(FRAME_MS);
        } else if self.bars_active() {
            self.set_frame_interval(BARS_FRAME_MS);
        } else {
            self.animating = false;
            self.frame_ms = 0;
            unsafe { let _ = KillTimer(Some(self.hwnd), T_ANIM); }
        }
    }

    /// Pull the latest now-playing snapshot; music starting/stopping resizes the pill.
    fn refresh_music(&mut self) {
        self.track = self.music.snapshot();
        if self.notch.set_live(self.track.is_some()) || self.bars_active() {
            self.kick();
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

    /// Re-read speaker/mic; true if anything shown changed.
    fn refresh_audio(&mut self) -> bool {
        let Some(a) = &self.audio else { return false };
        let now = (a.volume(), a.mic_muted());
        let changed = now != (self.volume, self.mic_muted);
        (self.volume, self.mic_muted) = now;
        changed
    }

    fn tick(&mut self) {
        let (time, date) = system::clock();
        let battery = system::battery();
        let mut changed = (&time, &date, &battery) != (&self.time, &self.date, &self.battery);
        (self.time, self.date, self.battery) = (time, date, battery);
        changed |= self.refresh_audio();
        self.refresh_music();
        // Progress bar and lyric line move on their own while the panel is open.
        changed |= self.notch.mode == Mode::Expanded && self.track.as_ref().is_some_and(|t| t.playing);

        // The capture includes our own window: sample only while the pill rests, still and alone.
        let sample = self.notch.is_settled_rest();
        let cx = self.win.0 + self.win.2 / 2;
        let pill_w = notch::target(Mode::Collapsed, self.notch.live)[0];
        match screen::check(self.hwnd, cx, self.monitor, self.scale, pill_w, sample) {
            screen::Check::Fullscreen => self.set_hidden(true),
            screen::Check::Busy(busy) => {
                self.set_hidden(false);
                if sample {
                    let before = self.notch.mode;
                    self.notch.report_busy(busy);
                    if self.notch.mode != before {
                        self.kick();
                    }
                }
            }
        }

        let new = self.notifications.as_mut().map(|r| r.poll()).unwrap_or_default();
        if let Some(t) = new.into_iter().last() {
            self.toast = Some(t.clone());
            self.show_banner(t, ALERT_MS);
            changed = true;
        }

        let level = self.brightness.as_ref().and_then(|b| b.percent());
        if level.is_some() && self.last_brightness.is_some() && level != self.last_brightness {
            let p = level.unwrap_or(0);
            self.show_banner(status_toast("Màn hình", format!("Độ sáng {p}%"), "", Some(p as f32 / 100.0)), STATUS_MS);
        }
        self.last_brightness = level.or(self.last_brightness);

        if changed && !self.animating {
            self.redraw();
        }
    }

    /// Short banner on the notch; skipped over a fullscreen game or while the panel is open.
    fn show_banner(&mut self, t: Toast, ms: u32) {
        if self.hidden || !self.notch.on_alert() {
            return;
        }
        self.banner = Some(t);
        unsafe { SetTimer(Some(self.hwnd), T_ALERT, ms, None) };
        self.kick();
    }

    fn click(&mut self, x: f32, y: f32) {
        if self.notch.mode != Mode::Expanded {
            return;
        }
        let cx = self.win.2 as f32 / 2.0;
        let hit = render::buttons(cx, self.scale, self.page).into_iter().find(|(_, r)| x >= r.left && x <= r.right && y >= r.top && y <= r.bottom);
        let Some((button, r)) = hit else { return };
        let media = match button {
            Button::Bluetooth => return bluetooth::request(self.hwnd, true),
            Button::PagePrev => return self.turn_page(-1),
            Button::PageNext => return self.turn_page(1),
            Button::Previous => Some(music::Cmd::Previous),
            Button::PlayPause => Some(music::Cmd::PlayPause),
            Button::Next => Some(music::Cmd::Next),
            _ => None,
        };
        if let Some(cmd) = media {
            return self.music.send(cmd);
        }
        let Some(a) = &self.audio else { return };
        let level = self.volume.map_or(0.0, |(l, _)| l);
        match button {
            Button::SpeakerMute => a.toggle_speaker_mute(),
            Button::VolDown => a.set_volume(level - VOLUME_STEP),
            Button::VolUp => a.set_volume(level + VOLUME_STEP),
            Button::VolBar => a.set_volume((x - r.left) / (r.right - r.left)),
            Button::Mic => a.toggle_mic(),
            _ => {}
        }
        self.refresh_audio();
        self.redraw();
    }

    fn turn_page(&mut self, step: isize) {
        let i = PAGES.iter().position(|p| *p == self.page).unwrap_or(0) as isize;
        self.page = PAGES[(i + step).rem_euclid(PAGES.len() as isize) as usize];
        self.redraw();
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
        for (d, s) in nid.szTip.iter_mut().zip("PUW Notch".encode_utf16()) {
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
                    if self.notch.mode == Mode::Expanded {
                        // Radio state can change from Windows settings; re-read when the panel opens.
                        bluetooth::request(self.hwnd, false);
                        self.refresh_audio();
                    }
                    self.kick();
                }
                T_VOLKEY => {
                    unsafe { let _ = KillTimer(Some(self.hwnd), T_VOLKEY); }
                    self.refresh_audio();
                    if let Some((level, muted)) = self.volume {
                        let pct = (level * 100.0).round();
                        let title = if muted { "Đã tắt tiếng".to_string() } else { format!("Âm lượng {pct}%") };
                        self.show_banner(status_toast("Loa", title, "", Some(if muted { 0.0 } else { level })), STATUS_MS);
                    }
                    if !self.animating {
                        self.redraw();
                    }
                }
                T_ALERT => {
                    unsafe { let _ = KillTimer(Some(self.hwnd), T_ALERT); }
                    self.notch.on_alert_end();
                    // Mouse resting on the banner when it ends: treat as a fresh visit.
                    if self.tracking && self.notch.on_enter() {
                        unsafe { SetTimer(Some(self.hwnd), T_DWELL, DWELL_MS, None) };
                    }
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
            WM_LBUTTONUP => {
                let x = (lp.0 & 0xffff) as i16 as f32;
                let y = ((lp.0 >> 16) & 0xffff) as i16 as f32;
                self.click(x, y);
            }
            WM_RBUTTONUP => self.menu(),
            WM_MOUSEWHEEL if self.notch.mode == Mode::Expanded => {
                let delta = ((wp.0 >> 16) & 0xffff) as i16;
                self.turn_page(if delta > 0 { -1 } else { 1 });
            }
            WM_LOCKKEY => {
                let (name, on) = status::lock_state(wp.0);
                self.show_banner(status_toast("Bàn phím", name.to_string(), if on { "Bật" } else { "Tắt" }, None), STATUS_MS);
            }
            // Windows applies the key after the hook returns; read the level a moment later.
            WM_VOLKEY => unsafe {
                SetTimer(Some(self.hwnd), T_VOLKEY, 60, None);
            },
            WM_MUSIC => {
                self.refresh_music();
                if !self.animating {
                    self.redraw();
                }
            }
            WM_BT => {
                self.bt = Bt::from_wparam(wp.0);
                if !self.animating {
                    self.redraw();
                }
            }
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

fn status_toast(source: &str, title: String, body: &str, level: Option<f32>) -> Toast {
    Toast { app: source.to_string(), title, body: body.to_string(), level }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // Win32 calls made while handling a message can re-enter here; those fall back to the default.
    let handled = APP.with(|a| a.try_borrow_mut().ok().and_then(|mut g| g.as_mut().and_then(|app| app.handle(msg, wp, lp))));
    handled.unwrap_or_else(|| unsafe { DefWindowProcW(hwnd, msg, wp, lp) })
}

fn main() -> windows::core::Result<()> {
    unsafe {
        let _mutex = CreateMutexW(None, false, w!("Local\\PuwNotch.SingleInstance"))?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return Ok(());
        }
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);

        let instance: HINSTANCE = GetModuleHandleW(None)?.into();
        let class = w!("PuwNotchWindow");
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
            w!("PUW Notch"),
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
        let (notifications, history) = notify::Reader::open().map_or((None, Vec::new()), |(r, h)| (Some(r), h));
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
            notifications,
            banner: None,
            toast: history.into_iter().last(),
            brightness: status::Brightness::new(),
            last_brightness: None,
            audio: audio::Audio::new(),
            volume: None,
            mic_muted: None,
            bt: Bt::Unknown,
            music: music::Music::start(hwnd),
            track: None,
            started: Instant::now(),
            frame_ms: 0,
            page: Page::Music,
        };
        app.refresh_audio();
        bluetooth::request(hwnd, false);
        status::watch_lock_keys(hwnd);
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

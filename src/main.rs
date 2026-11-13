#![windows_subsystem = "windows"]

mod anim;
mod antigravity;
mod approve;
mod audio;
mod calendar;
mod connectivity;
mod music;
mod foobar;
mod focus;
mod health;
mod i18n;
mod icons;
mod itunes;
mod lyrics;
mod notch;
mod notify;
mod render;
mod robot;
mod screen;
mod status;
mod system;
mod task;
mod todo;
mod usage;
mod usb;
mod weather;
mod activity;

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Power::{RegisterPowerSettingNotification, POWERBROADCAST_SETTING};
use windows::Win32::System::Threading::{CreateMutexW, GetCurrentProcess, GetCurrentThread, ProcessPowerThrottling, SetProcessInformation, SetThreadPriority, PROCESS_POWER_THROTTLING_CURRENT_VERSION, PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_STATE, THREAD_PRIORITY_ABOVE_NORMAL};
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use connectivity::{Event, Net, RadioType, WM_CONN};
use notch::{Mode, Notch, ALERT_MS, DWELL_MS, MAX_H, MAX_W, STATUS_MS};
use music::WM_MUSIC;
use i18n::tr;
use audio::{WM_MIC, WM_VOLUME};
use status::{WM_LOCKKEY, WM_VOLUMEKEY};
use screen::WM_SCREENEVENT;
use notify::{Row, RowState, Toast};
use weather::WM_WEATHER;
use focus::Focus;
use render::{Button, Card, HealthView, ListView, Page, Renderer, View, LIST_ROWS, NOTE_ROWS, PAGES};

const WM_TRAY: u32 = WM_APP + 1;
const WM_FRAME: u32 = WM_APP + 7;
const WM_MOUSELEAVE: u32 = 0x02A3;
const T_ANIM: usize = 1;
const T_CHECK: usize = 2;
const T_DWELL: usize = 3;
const T_ALERT: usize = 4;
const T_CARET: usize = 6;
const T_SETTLE: usize = 7;
const T_CONN: usize = 8;
const CONN_MS: u32 = 1500;
const NOTE_MS: u32 = 3000;
const QUEUE_MAX: usize = 8;
const SETTLE_MS: u32 = 120;
const NOTES_MAX: usize = 30;
const CARET_MS: u32 = 530;
const CHECK_MS: u32 = 500;
const ACTIVITY_MS: u32 = 1500;
static ACTIVITY_SHOWN: AtomicBool = AtomicBool::new(true);
const TRIM_TICKS: u32 = 10;
static BEAT: AtomicU64 = AtomicU64::new(0);
static WHERE: AtomicU8 = AtomicU8::new(0);
const STALL_MS: u64 = 6000;

fn unix_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

fn watchdog() {
    crate::system::spawn("watchdog", || {
        let mut said = false;
        loop {
            std::thread::sleep(Duration::from_secs(2));
            let (beat, here) = (BEAT.load(Ordering::Relaxed), WHERE.load(Ordering::Relaxed));
            if beat == 0 {
                continue;
            }
            let late = unix_ms().saturating_sub(beat);
            if late > STALL_MS && !said {
                said = true;
                system::log(&format!("window not answering for {} s (mode {}, page {})", late / 1000, here / 16, PAGES.get((here % 16) as usize).map_or("?", |p| p.name())));
            } else if late <= STALL_MS && said {
                said = false;
                system::log("window answers again");
            }
        }
    });
}
const MENU_MINIMAL: usize = 10;
const MENU_AUTOSTART: usize = 11;
const MENU_QUIT: usize = 12;
const MENU_LANG_VI: usize = 13;
const MENU_LANG_EN: usize = 14;
const MENU_ROBOT: usize = 15;
const MENU_ABOUT: usize = 16;
const MENU_VOLUME_KEYS: usize = 17;
const MENU_STYLE_EARS: usize = 18;
const MENU_STYLE_PILL: usize = 19;
const MENU_APPROVE: usize = 20;
const MENU_ACTIVITY: usize = 21;
const MENU_PAGE: usize = 30;
const REPO_URL: &str = "https://github.com/erikpuw/puw_notch";

struct SeekDrag {
    track: music::Track,
}

impl SeekDrag {
    fn new(track: &music::Track) -> Option<Self> {
        (track.can_seek && track.duration_ms > 0).then(|| Self { track: track.clone() })
    }

    fn update(&mut self, cx: f32, scale: f32, x: f32, wide: bool) {
        self.track.position_ms = render::seek_at(cx, scale, x, self.track.duration_ms, wide);
        self.track.at = None;
    }

    fn current(&self, track: Option<&music::Track>) -> bool {
        track.is_some_and(|t| t.can_seek && t.duration_ms > 0 && t.art_key == self.track.art_key && t.same_media(&self.track))
    }
}

struct App {
    hwnd: HWND,
    notch: Notch,
    renderer: Renderer,
    scale: f32,
    monitor: RECT,
    win: (i32, i32, i32, i32),
    hidden: bool,
    tracking: bool,
    hover_in: bool,
    animating: bool,
    last_frame: Instant,
    taskbar_created: u32,
    time: String,
    date: String,
    battery: Option<system::Battery>,
    notifications: Option<notify::Reader>,
    banner: Option<Toast>,
    banner_at: Instant,
    lyrics_open: bool,
    usb_last: Option<(String, usb::Change, Instant)>,
    last_brightness: Option<u8>,
    audio: Option<audio::Audio>,
    mic_muted: Option<bool>,
    audio_ticks: u8,
    music: music::Music,
    usage: usage::Monitor,
    approve: approve::Server,
    approval: Option<approve::Pending>,
    usage_watch: usage::Watch,
    usage_alerts: Vec<usage::Alert>,
    conn_queue: Vec<Toast>,
    conn_busy: bool,
    conn_list: Option<ConnList>,
    vpn_was_up: bool,
    vpn_lost: Option<Instant>,
    usage_tab: usize,
    usage_tab_chosen: bool,
    track: Option<music::Track>,
    seek_drag: Option<SeekDrag>,
    robot: robot::Robot,
    robot_on: bool,
    style: notch::Style,
    robot_at: Instant,
    paused_since: Option<Instant>,
    started: Instant,
    frame_ms: u32,
    vsync: Vsync,
    hovered_button: Option<Button>,
    last_foreground: Option<(isize, RECT)>,
    rested: u32,
    logged_open: bool,
    last_gap_log: Option<Instant>,
    last_slow_tick: Option<Instant>,
    nav_open: bool,
    page: Page,
    todo: todo::Todo,
    tasks: task::Tasks,
    task_sel: Option<usize>,
    task_scroll: usize,
    today: u32,
    draft: String,
    editing: bool,
    edit_item: Option<usize>,
    caret: bool,
    todo_scroll: usize,
    notes: Vec<notify::Note>,
    note_scroll: usize,
    note_pos: f32,
    health: health::Health,
    activity: activity::Watch,
    focus: Focus,
    settings_open: Option<Card>,
    page_anim: Option<(Page, Instant, i32)>,
    icons_morphing: bool,
    active_page: Option<Page>,
    prev_foreground: HWND,
    high_surrogate: Option<u16>,
    weather: weather::Weather,
    wx: weather::State,
    city_draft: String,
    awaiting_city: bool,
    page_since: Instant,
    unit: weather::Unit,
    sun_view: bool,
    cal_year: i32,
    cal_month: u32,
    cal_sel: u32,
    cal_lunar: bool,
    cal_zone: Option<f64>,
    cal_zone_at: Option<Instant>,
    unit_chosen: bool,
}

const VSYNC: u32 = 1;
const BARS_FRAME_MS: u32 = 50;

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

    fn sync_page_work(&mut self) {
        let page = render::visible_page(self.notch.mode, self.hidden, self.page);
        self.music.set_pill_visible(!self.hidden && matches!(self.notch.mode, Mode::Collapsed | Mode::Peek));
        if self.active_page != page {
            self.active_page = page;
            self.music.set_page_open(page == Some(Page::Music));
            let notes = page.is_some() && render::page_shown(Page::Notify);
            self.renderer.set_page_open(notes);
            self.usage.set_page_open(page == Some(Page::Usage));
            if notes { self.refresh_notes(); }
        }
    }

    fn redraw(&mut self) {
        self.sync_page_work();
        if self.hidden && self.notch.is_hidden() { return; }
        if self.notch.mode != Mode::Expanded {
            self.page_since = Instant::now();
        }
        let eyes = self.robot_eyes();
        let (x, y, w, h) = self.win;
        let now = system::now_stamp();
        let cal = self.cal_view(now);
        let view = View {
            mode: self.notch.mode,
            size: self.notch.size(),
            time: &self.time,
            date: &self.date,
            battery: self.battery,
            banner: self.banner.as_ref(),
            music: self.seek_drag.as_ref().map(|drag| &drag.track).or(self.track.as_ref()),
            live: self.notch.live,
            robot: eyes,
            phase: self.started.elapsed().as_secs_f32(),
            banner_age: self.banner_at.elapsed().as_secs_f32(),
            lyrics: self.lyrics_wide(),
            hovered: self.hovered_button,
            page: self.page,
            leaving: self.page_anim.map(|(from, t0, dir)| (from, (t0.elapsed().as_secs_f32() / anim::PAGE_MORPH_S).min(1.0), dir)),
            list: ListView {
                counts: self.todo.counts(self.today),
                remind_before: self.todo.remind_before,
                now,
                draft: if self.page == Page::Weather { &self.city_draft } else { &self.draft },
                editing: self.editing,
                edit_item: self.edit_item,
                caret: self.caret,
                scroll: self.scroll(),
                note_pos: self.note_pos,
            },
            todos: &self.todo.items,
            notes: &self.notes,
            focus: &self.focus, health: HealthView { h: &self.health, now: todo::minutes(now), minute_of_day: minute_of_day(now), settings: self.settings_open },
            weather: &self.wx,
            page_age: self.page_since.elapsed().as_secs_f32(),
            unit: self.unit, sun_view: self.sun_view, cal,
            task: render::TaskView { tasks: Some(&self.tasks), now, sel: self.task_sel },
        };
        self.icons_morphing = self.renderer.present(self.hwnd, x, y, w, h, self.scale, &view).unwrap_or(false);
        if self.icons_morphing && !self.animating {
            self.kick();
        }
    }

    fn set_frame_interval(&mut self, ms: u32) {
        if self.frame_ms != ms {
            self.frame_ms = ms;
            self.vsync.set(ms == VSYNC);
            unsafe {
                if ms == BARS_FRAME_MS {
                    SetTimer(Some(self.hwnd), T_ANIM, ms, None);
                } else {
                    let _ = KillTimer(Some(self.hwnd), T_ANIM);
                }
            }
        }
    }

    fn kick(&mut self) {
        self.sync_page_work();
        if self.hidden && self.notch.is_hidden() { return; }
        if !self.animating {
            self.animating = true;
            self.last_frame = Instant::now();
        }
        self.set_frame_interval(VSYNC);
    }

    fn bars_active(&self) -> bool {
        let playing = self.track.as_ref().is_some_and(|t| t.playing);
        let bars = playing && matches!(self.notch.mode, Mode::Collapsed | Mode::Peek);
        let aurora = playing && self.notch.mode == Mode::Expanded && self.page == Page::Music;
        !self.hidden && (bars || aurora || self.robot.visible())
    }

    fn settings(&self) -> system::Settings {
        system::Settings { pages: render::pages_on(), minimal: self.notch.minimal, english: i18n::english(), robot: self.robot_on, volume_keys: status::taking_volume_keys(), style: self.style, approve: self.approve.is_enabled(), place: self.wx.place.clone(), unit: self.unit_chosen.then_some(self.unit), sun_view: self.sun_view, notes_cleared: self.notifications.as_ref().map_or(0, |r| r.cleared), activity: self.activity.enabled }
    }

    fn refresh_robot(&mut self) {
        let allowed = self.robot_on && !self.hidden && self.notch.mode == Mode::Collapsed && !self.notch.live;
        if self.robot.update(idle_seconds(), allowed) || (self.robot.visible() && !self.animating) {
            self.kick();
        }
    }

    fn robot_eyes(&mut self) -> Option<robot::Eyes> {
        if !self.robot.visible() {
            self.robot_at = Instant::now();
            return None;
        }
        let dt = self.robot_at.elapsed().as_secs_f32().min(0.1);
        self.robot_at = Instant::now();
        let mut p = POINT::default();
        let cursor = unsafe { GetCursorPos(&mut p) }.is_ok().then(|| {
            let cx = (self.win.0 + self.win.2 / 2) as f32;
            let cy = self.win.1 as f32 + self.top_px() + self.notch.size()[1] * self.scale / 2.0;
            let (half_w, h) = (((self.monitor.right - self.monitor.left) as f32 / 2.0).max(1.0), ((self.monitor.bottom - self.monitor.top) as f32).max(1.0));
            (((p.x as f32 - cx) / half_w).clamp(-1.0, 1.0), ((p.y as f32 - cy) / h).clamp(-1.0, 1.0))
        });
        let mood = match self.battery {
            Some(b) if b.percent <= 20 && !b.charging => robot::Mood::Low,
            _ => robot::Mood::Normal,
        };
        Some(self.robot.step(dt, cursor, mood))
    }

    fn frame(&mut self) {
        let now = Instant::now();
        let gap = (now - self.last_frame).as_secs_f32();
        let dt = gap.min(0.05);
        self.last_frame = now;
        let limit = if self.frame_ms == BARS_FRAME_MS { 0.125 } else { 0.08 };
        if gap > limit && self.animating && self.last_gap_log.is_none_or(|t| t.elapsed().as_secs() >= 10) {
            self.last_gap_log = Some(now);
            system::log(&format!("late frame: {:.0} ms (mode {:?}, page {})", gap * 1000.0, self.notch.mode, self.page.name()));        }
        if self.notch.mode != Mode::Expanded {
            self.lyrics_open = false;
        }
        self.notch.set_wide(self.lyrics_wide());
        if self.nav_open && self.notch.mode != Mode::Expanded {
            self.nav_open = false;
            render::set_nav_open(false);
        }
        let [pw, ph] = render::panel_size(self.page);
        self.notch.set_panel([pw, ph + if self.nav_open { render::NAV_EXTRA } else { 0.0 }]);
        let mut moving = self.notch.step(dt);
        if self.hidden && self.notch.is_hidden() {
            unsafe { let _ = ShowWindow(self.hwnd, SW_HIDE); }
            self.animating = false;
            self.set_frame_interval(0);
            return;
        }
        let goal = self.note_scroll as f32;
        if self.note_pos != goal {
            self.note_pos = anim::follow(self.note_pos, goal, dt, anim::LIST_GLIDE_RATE);
            if (goal - self.note_pos).abs() < 0.01 {
                self.note_pos = goal;
            }
            moving = true;
        }
        let morphing = match self.page_anim {
            Some((_, t0, _)) if t0.elapsed().as_secs_f32() < anim::PAGE_MORPH_S => true,
            Some(_) => {
                self.page_anim = None;
                false
            }
            None => false,
        };
        self.redraw();
        if moving || morphing || self.icons_morphing {
            self.set_frame_interval(VSYNC);
        } else if self.bars_active() || self.renderer.slow_frames {
            self.set_frame_interval(BARS_FRAME_MS);
        } else {
            self.animating = false;
            self.set_frame_interval(0);
            if self.notch.mode != Mode::Alert && self.banner.is_some() {
                self.banner = None;
                self.renderer.forget_morph("banner");
                self.activity.forget();
            }
        }
    }

    fn refresh_music(&mut self) {
        self.track = self.music.snapshot().filter(|_| render::page_shown(Page::Music));
        if self.seek_drag.as_ref().is_some_and(|drag| !drag.current(self.track.as_ref())) {
            self.cancel_seek();
        }
        let playing = self.track.as_ref().is_some_and(|t| t.playing);
        self.paused_since = if self.track.is_some() && !playing { Some(self.paused_since.unwrap_or_else(Instant::now)) } else { None };
        let live = music::shows_pill(playing, self.paused_since.map(|t| t.elapsed()));
        if self.notch.set_live(live) || (self.bars_active() && !self.animating) {
            self.kick();
        }
    }

    fn top_px(&self) -> f32 {
        (self.style.gap_at(self.notch.size()[1]) * self.scale).round()
    }

    fn set_nav(&mut self, open: bool) {
        if self.nav_open != open {
            self.nav_open = open;
            render::set_nav_open(open);
            system::log(if open { "page icons shown" } else { "page icons hidden" });
            self.kick();
        }
    }

    fn cursor_inside(&self) -> bool {
        let mut p = POINT::default();
        if unsafe { GetCursorPos(&mut p) }.is_err() {
            return false;
        }
        let (half, h) = notch::hover_area(self.notch.size());
        let h = if self.nav_open && self.notch.mode == Mode::Expanded { h.max(render::panel_size(self.page)[1] + render::NAV_EXTRA) } else { h };
        let (half, h) = (half * self.scale, h * self.scale);
        let cx = (self.win.0 + self.win.2 / 2) as f32;
        let (x, y) = (p.x as f32, (p.y - self.win.1) as f32);
        (x - cx).abs() <= half && y >= 0.0 && y <= h + self.top_px()
    }

    fn refresh_audio(&mut self) {
        let hwnd = self.hwnd;
        let Some(a) = &mut self.audio else { return };
        if a.watch_speaker(hwnd) {
            connectivity::nudge();
        }
        if a.watch_mic(hwnd) || self.mic_muted.is_none() {
            self.mic_muted = a.mic_muted();
        }
    }

    fn mic_changed(&mut self, muted: bool) {
        if self.mic_muted.replace(muted) == Some(muted) {
            return;
        }
        let (title, icon) = if muted { (tr("Đã tắt mic", "Mic off"), render::ICON_MIC_OFF) } else { (tr("Đã bật mic", "Mic on"), render::ICON_MIC) };
        self.show_banner(status_toast("Mic", title.to_string(), "", None, icon), STATUS_MS);
    }

    fn usb_changed(&mut self, change: usb::Change, path: &str) {
        let Some(id) = usb::instance_id(path) else { return };
        let now = Instant::now();
        if usb::is_root_hub(path) || usb::is_repeat(self.usb_last.as_ref(), &id, change, now) {
            return;
        }
        self.usb_last = Some((id, change, now));
        let (state, icon) = match change {
            usb::Change::Arrived => (tr("Đã kết nối", "Connected"), render::ICON_USB),
            usb::Change::Removed => (tr("Đã ngắt kết nối", "Disconnected"), render::ICON_USB_OUT),
        };
        self.show_banner(status_toast("USB", usb::device_name(path), state, None, icon), ALERT_MS);
    }

    fn release_stale_hover(&mut self) {
        if self.seek_drag.is_some() && unsafe { GetAsyncKeyState(VK_LBUTTON.0 as i32) } >= 0 {
            self.cancel_seek();
        }
        let typing = self.editing && matches!(self.page, Page::Todo | Page::Weather);
        if self.seek_drag.is_some() || !notch::hover_stale(self.notch.mode, self.hover_in, typing, self.cursor_inside()) {
            return;
        }
        self.hover_in = false;
        self.tracking = false;
        self.hovered_button = None;
        unsafe { let _ = KillTimer(Some(self.hwnd), T_DWELL); }
        self.notch.on_leave();
        self.kick();
    }

    fn keep_on_top(&self) {
        if !self.hidden && screen::buried(self.hwnd) {
            unsafe { let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE); }
        }
    }

    fn beat(&mut self) {
        BEAT.store(unix_ms(), Ordering::Relaxed);
        WHERE.store(self.notch.mode as u8 * 16 + PAGES.iter().position(|p| *p == self.page).unwrap_or(0) as u8, Ordering::Relaxed);
        let open = self.notch.mode == Mode::Expanded;
        if open != self.logged_open {
            self.logged_open = open;
            system::log(&if open { format!("panel open: {}", self.page.name()) } else { "panel closed".to_string() });
        }
    }

    fn check_screen(&mut self, sample: bool) {
        let cx = self.win.0 + self.win.2 / 2;
        let pill_w = notch::target(Mode::Collapsed, self.notch.live)[0];
        match screen::check(self.hwnd, cx, self.monitor, self.scale, pill_w, sample, &mut self.last_foreground) {
            screen::Check::Fullscreen => self.set_hidden(true),
            screen::Check::Settling => {
                self.set_hidden(false);
                unsafe { SetTimer(Some(self.hwnd), T_SETTLE, SETTLE_MS, None) };
            }
            screen::Check::Unknown => self.set_hidden(false),
            screen::Check::Busy(busy) => {
                self.set_hidden(false);
                if sample {
                    let before = self.notch.mode;
                    self.notch.report_busy(busy);
                    if self.notch.mode != before {
                        let fg = screen::class_name(unsafe { GetForegroundWindow() });
                        system::log(&if busy { format!("thin line: content under the pill, foreground {fg}") } else { "back to the pill".to_string() });
                        self.kick();
                    }
                    if !busy && self.notch.obstructed() {
                        unsafe { SetTimer(Some(self.hwnd), T_SETTLE, SETTLE_MS, None) };
                    }
                }
            }
        }
    }

    fn tick(&mut self) {
        if self.editing && unsafe { GetForegroundWindow() } != self.hwnd {
            self.end_edit();
        }
        self.beat();
        let mut lap = Instant::now();
        let mut laps = [0u32; 6];
        let mut mark = |i: usize, lap: &mut Instant| {
            laps[i] = lap.elapsed().as_micros() as u32;
            *lap = Instant::now();
        };
        self.keep_on_top();
        self.release_stale_hover();
        let (time, date) = system::clock();
        let battery = system::battery_cached();
        mark(0, &mut lap);
        let mut changed = (&time, &date, battery) != (&self.time, &self.date, self.battery);
        if let (Some(old), Some(new)) = (self.battery, battery) {
            for e in system::power_events(old, new) {
                match e {
                    system::PowerEvent::Full => self.robot.full_battery(true),
                    system::PowerEvent::Unplugged => self.robot.full_battery(false),
                    _ => {}
                }
                self.show_banner(power_toast(e, new), STATUS_MS * 2);
            }
        }
        (self.time, self.date, self.battery) = (time, date, battery);
        let today = system::today();
        if today != self.today {
            self.today = today;
            changed |= self.todo.new_day(today);
            self.health.new_day(today);
            self.clamp_scroll();
        }
        self.audio_ticks = self.audio_ticks.wrapping_add(1);
        if self.audio_ticks % 4 == 0 {
            self.refresh_audio();
        }
        mark(1, &mut lap);
        self.refresh_music();
        self.refresh_robot();
        mark(2, &mut lap);
        changed |= self.notch.mode == Mode::Expanded && self.track.as_ref().is_some_and(|t| t.playing);
        changed |= self.focus.running() && self.audio_ticks % 2 == 0;

        let sample = self.notch.is_settled_rest();
        self.rested = if sample { self.rested.saturating_add(1) } else { 0 };
        if self.rested == TRIM_TICKS {
            unsafe { let _ = windows::Win32::System::Threading::SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX); }
        }
        self.check_screen(sample);

        mark(3, &mut lap);
        self.remind();
        self.focus_tick();
        self.usage_banner();
        mark(4, &mut lap);

        let new = self.notifications.as_mut().filter(|_| render::page_shown(Page::Notify)).map(|r| r.poll()).unwrap_or_default();
        let showing_notes = self.notch.mode == Mode::Expanded && self.page == Page::Notify;
        if showing_notes && !self.hidden && !new.is_empty() {
            self.refresh_notes();
            changed = true;
        }
        let now = Instant::now();
        if self.conn_list.as_mut().is_some_and(|l| l.expire(now)) {
            let t = self.conn_list.as_ref().map(|l| (list_toast(l), l.life_ms(now)));
            if let Some((t, ms)) = t {
                self.show_connection_list(t, ms, false);
            }
        }
        let current = self.activity.poll(Instant::now(), self.hwnd.0 as isize);
        self.activity_banner(current);
        self.conn_queue.extend(new);
        if self.conn_queue.len() > QUEUE_MAX {
            let extra = self.conn_queue.len() - QUEUE_MAX;
            self.conn_queue.drain(..extra);
        }
        if !self.conn_busy && !self.conn_queue.is_empty() {
            self.next_connection_banner();
        }

        mark(5, &mut lap);
        let total: u32 = laps.iter().sum();
        if total > 30_000 && self.last_slow_tick.is_none_or(|t| t.elapsed().as_secs() >= 10) {
            self.last_slow_tick = Some(Instant::now());
            system::log(&format!("slow tick: {} ms (clock+battery {}, audio {}, music+robot {}, screen {}, reminders {}, notifications {} us)", total / 1000, laps[0], laps[1], laps[2], laps[3], laps[4], laps[5]));
        }
        if changed && !self.animating {
            self.redraw();
        }
    }

    fn banner_target(&self) -> Option<&str> {
        self.banner.as_ref().filter(|_| self.notch.mode == Mode::Alert).and_then(|b| b.image.as_deref().or(b.open.as_deref()))
    }

    fn show_banner(&mut self, t: Toast, ms: u32) -> bool {
        if self.approval.is_some() && t.choices.is_empty() {
            return false;
        }
        let lines = if !t.choices.is_empty() {
            self.renderer.banner_lines(&t.body, self.scale, false, true).clamp(1, 3) as u8
        } else if t.icon.is_none() && !t.body.is_empty() {
            self.renderer.banner_lines(&t.body, self.scale, t.image.is_some(), false).min(notch::BANNER_MAX_LINES) as u8
        } else {
            0
        };
        let opening = self.notch.mode != Mode::Alert;
        let shown = if t.choices.is_empty() { self.notch.on_alert(lines) } else { self.notch.on_alert_choice(lines) };
        if self.hidden || !shown {
            return false;
        }
        if opening {
            self.renderer.forget_morph("banner");
        }
        if t.icon.is_some() && t.choices.is_empty() && t.badge.is_none() {
            let width = self.renderer.status_banner_width(&t.title, &t.body, &t.rows, self.scale);
            self.notch.set_alert_width(width);
        }
        self.notch.set_alert_rows(t.rows.len() as u8);
        self.banner = Some(t);
        self.banner_at = Instant::now();
        unsafe { SetTimer(Some(self.hwnd), T_ALERT, ms, None) };
        self.kick();
        true
    }

    fn activity_banner(&mut self, current: Option<activity::LiveActivity>) {
        if let Some(a) = current {
            self.show_activity(activity_toast(&a, false), ACTIVITY_MS);
        } else if let Some(a) = self.activity.take_finished() {
            self.show_activity(activity_toast(&a, true), ALERT_MS);
        } else if self.notch.mode == Mode::Alert && self.banner.as_ref().is_some_and(|b| b.app == tr("Tệp", "Files") && b.choices.is_empty()) {
            unsafe { SetTimer(Some(self.hwnd), T_ALERT, 1, None) };
        }
    }

    fn show_connection_list(&mut self, t: Toast, ms: u32, replaces: bool) {
        if self.approval.is_some() {
            self.wait_for_room(t);
            return;
        }
        if self.notch.mode == Mode::Alert && self.banner.as_ref().is_some_and(|b| (replaces || b.app == t.app) && b.choices.is_empty()) {
            let width = if t.badge.is_none() { self.renderer.status_banner_width(&t.title, &t.body, &t.rows, self.scale) } else { 0.0 };
            self.notch.set_alert_width(width);
            self.notch.set_alert_status(t.rows.len() as u8);
            self.banner = Some(t);
            unsafe { SetTimer(Some(self.hwnd), T_ALERT, ms, None) };
            self.kick();
        } else if !self.show_banner(t.clone(), ms) {
            self.wait_for_room(t);
        }
    }

    fn wait_for_room(&mut self, t: Toast) {
        if self.hidden {
            return;
        }
        self.conn_queue.retain(|q| q.app != t.app);
        self.conn_queue.push(t);
    }

    fn next_connection_banner(&mut self) {
        if self.conn_queue.is_empty() {
            self.conn_busy = false;
            unsafe { let _ = KillTimer(Some(self.hwnd), T_CONN); }
            return;
        }
        let t = self.conn_queue.remove(0);
        let spacing = if t.icon.is_none() { NOTE_MS } else { CONN_MS };
        if !self.show_banner(t.clone(), ALERT_MS) {
            if !self.hidden {
                self.conn_queue.insert(0, t);
            }
            self.conn_busy = false;
            return;
        }
        self.conn_busy = true;
        unsafe { SetTimer(Some(self.hwnd), T_CONN, spacing, None) };
    }

    fn show_activity(&mut self, t: Toast, ms: u32) {
        if self.approval.is_some() {
            return;
        }
        if self.notch.mode != Mode::Alert {
            let mode = self.notch.mode;
            let hidden = self.hidden;
            let shown = self.show_banner(t, ms);
            if ACTIVITY_SHOWN.swap(shown, Ordering::Relaxed) != shown {
                system::log(&format!("live activity banner {}: notch mode {mode:?}, hidden {hidden}", if shown { "shown" } else { "refused" }));
            }
        } else if self.banner.as_ref().is_some_and(|b| (b.app == t.app || (is_activity_app(&b.app) && is_activity_app(&t.app))) && b.icon.is_some() && b.choices.is_empty()) {
            let width = self.renderer.status_banner_width(&t.title, &t.body, &t.rows, self.scale);
            self.notch.set_alert_width(width);
            self.banner = Some(t);
            unsafe { SetTimer(Some(self.hwnd), T_ALERT, ms, None) };
            self.kick();
        }
    }

    fn approve_next(&mut self) {
        if self.approval.is_some() {
            return;
        }
        while let Some(p) = self.approve.next() {
            if !self.approve.is_enabled() || self.approve.take_cancelled(p.id) {
                p.answer(approve::Answer::Ask);
                continue;
            }
            let choices = if p.options.is_empty() { vec![tr("Đồng ý", "Allow").to_string(), tr("Không", "Deny").to_string()] } else { p.options.clone() };
            let toast = Toast { app: p.app.clone(), title: p.title.clone(), body: p.detail.clone(), level: None, icon: Some(render::ICON_WARN), image: None, open: None, badge: None, choices, app_id: None, rows: Vec::new() };
            if self.show_banner(toast, approve::BANNER.as_millis() as u32) {
                self.approval = Some(p);
                return;
            }
            p.answer(approve::Answer::Ask);
        }
    }

    fn finish_approval(&mut self, answer: approve::Answer) {
        let Some(p) = self.approval.take() else { return };
        p.answer(answer);
        unsafe { let _ = KillTimer(Some(self.hwnd), T_ALERT); }
        self.notch.on_alert_end();
        self.kick();
        self.approve_next();
    }

    fn usage_banner(&mut self) {
        if !render::page_shown(Page::Usage) {
            return;
        }
        let Some(alert) = self.usage_alerts.first() else { return };
        let now = usage::now_secs();
        let (title, body) = alert.text(now);
        let toast = status_toast(tr("Giới hạn", "Limits"), title, &body, Some(alert.bar.used / 100.0), render::ICON_WARN);
        if self.show_banner(toast, ALERT_MS) {
            self.usage_alerts.remove(0);
        }
    }

    fn focus_tick(&mut self) {
        if !render::page_shown(Page::Focus) {
            return;
        }
        if let Some(done) = self.focus.tick(Instant::now()) {
            if self.show_banner(focus_toast(done), ALERT_MS) {
                self.page = Page::Focus;
            }
        }
    }

    fn remind(&mut self) {
        let now = system::now_stamp();
        let hhmm = |d: u64| format!("{:02}:{:02}", d / 100 % 100, d % 100);
        let (tasks, health) = (render::page_shown(Page::Todo), render::page_shown(Page::Health));
        for (i, ring) in self.todo.rings(now).into_iter().filter(|_| tasks) {
            let Some(item) = self.todo.items.get(i) else { continue };
            let at = item.at.unwrap_or(now);
            let when = match ring {
                todo::Ring::Soon => format!("{} {} {} · {}", tr("Còn", "In"), todo::minutes(at) - todo::minutes(now), tr("phút", "min"), hhmm(at)),
                todo::Ring::Now => format!("{} · {}", tr("Đến giờ", "It's time"), hhmm(at)),
            };
            let toast = status_toast(tr("Nhắc ghi chú", "Note reminder"), item.text.clone(), &when, None, render::ICON_REMINDER);
            if !self.show_banner(toast, ALERT_MS) {
                return;
            }
            self.todo.mark(i, ring);
            if render::page_shown(Page::Todo) {
                self.page = Page::Todo;
            }
        }
        let h = &self.health;
        let (cup, goal) = (h.cup_ml, h.goal_ml);
        let every = h.sit_every_min;
        if !health {
            return;
        }
        for ring in self.health.tick(todo::minutes(now), minute_of_day(now), idle_seconds()) {
            let taken = self.health.taken_ml;
            let level = Some(taken as f32 / goal.max(1) as f32);
            let toast = match ring {
                health::Ring::Drink => status_toast(tr("Uống nước", "Water"), if i18n::english() { format!("Time to drink {cup} ml") } else { format!("Đến giờ uống {cup} ml nước") }, "", level, render::ICON_DROP),
                health::Ring::GoalReached => status_toast(tr("Uống nước", "Water"), if i18n::english() { format!("Goal of {:.1} L reached today", goal as f32 / 1000.0) } else { format!("Đạt mục tiêu {:.1} L hôm nay", goal as f32 / 1000.0) }, "", Some(1.0), render::ICON_DROP),
                health::Ring::StandUp => status_toast(tr("Đứng dậy đi lại", "Stand up and move"), if i18n::english() { format!("You have been sitting for {every} min") } else { format!("Bạn đã ngồi {every} phút liền") }, tr("Vận động vài phút nhé", "Walk around for a few minutes"), None, render::ICON_WALK),
            };
            if self.show_banner(toast, ALERT_MS) && render::page_shown(Page::Health) {
                self.page = Page::Health;
            }
        }
    }

    fn cal_view(&mut self, now: u64) -> calendar::CalView {
        let date = (now / 10_000) as u32;
        let today = ((date / 10_000) as i32, date / 100 % 100, date % 100);
        if self.cal_year == 0 {
            (self.cal_year, self.cal_month, self.cal_sel) = today;
        }
        if self.page == Page::Calendar && self.cal_zone_at.is_none_or(|t| t.elapsed() >= Duration::from_secs(3)) {
            self.cal_zone = calendar::lunar_zone(&calendar::region());
            self.cal_zone_at = Some(Instant::now());
        }
        calendar::CalView { year: self.cal_year, month: self.cal_month, sel: self.cal_sel, today, zone: self.cal_zone, lunar_on: self.cal_lunar && self.cal_zone.is_some() }
    }

    fn scroll(&self) -> usize {
        match self.page {
            Page::Notify => self.note_scroll,
            Page::Task => self.task_scroll,
            _ => self.todo_scroll,
        }
    }

    fn rows(&self) -> usize {
        match self.page {
            Page::Todo => self.todo.items.len(),
            Page::Task => self.tasks.items.len(),
            Page::Notify => self.notes.len(),
            Page::Weather => usize::from(self.wx.now.is_some()),
            _ => 0,
        }
    }

    fn refresh_notes(&mut self) {
        if let Some(r) = &self.notifications {
            self.notes = r.recent(NOTES_MAX, system::utc_offset_min());
        }
        self.clamp_scroll();
    }

    fn hit(&self, x: f32, y: f32) -> Option<Button> {
        render::hit_button(self.win.2 as f32 / 2.0, self.scale, x, y, self.track.as_ref(), self.page, self.rows(), self.scroll(), self.settings_open, self.lyrics_wide())
    }

    fn lyrics_wide(&self) -> bool {
        self.lyrics_open && self.page == Page::Music && self.notch.mode == Mode::Expanded
    }

    fn cancel_seek(&mut self) {
        if self.seek_drag.take().is_some() {
            unsafe { if GetCapture() == self.hwnd { let _ = ReleaseCapture(); } }
        }
    }

    fn update_seek(&mut self, x: f32) {
        let wide = self.lyrics_wide();
        if let Some(drag) = &mut self.seek_drag {
            drag.update(self.win.2 as f32 / 2.0, self.scale, x, wide);
        }
        self.redraw();
    }

    fn click(&mut self, x: f32, y: f32) {
        if self.notch.mode != Mode::Expanded {
            return;
        }
        let Some(button) = self.hit(x, y) else { return };
        match button {
            Button::Previous => self.music.send(music::Cmd::Previous),
            Button::PlayPause => self.music.send(music::Cmd::PlayPause),
            Button::Next => self.music.send(music::Cmd::Next),
            Button::Shuffle => if let Some(t) = self.music.snapshot() { self.music.send(music::Cmd::Shuffle(!t.shuffle)) },
            Button::Repeat => if let Some(t) = self.music.snapshot() { self.music.send(music::Cmd::Repeat(t.repeat.next())) },
            Button::Seek => return,
            Button::Lyrics => {
                self.lyrics_open = !self.lyrics_open;
                self.kick();
            }
            Button::UsageTab(i) => {
                (self.usage_tab, self.usage_tab_chosen) = (i, true);
                self.renderer.usage_tab = i;
                self.kick();
            }
            Button::Dots => self.set_nav(true),
            Button::GoPage(target) => {
                self.cancel_task_edit();
                self.end_edit();
                if target != self.page {
                    let from = PAGES.iter().position(|p| *p == self.page).unwrap_or(0);
                    let to = PAGES.iter().position(|p| *p == target).unwrap_or(0);
                    self.page_anim = Some((self.page, Instant::now(), if to > from { 1 } else { -1 }));
                    self.page = target;
                    self.settings_open = None;
                }
                self.kick();
            }
            Button::PagePrev | Button::PageNext => {
                self.cancel_task_edit();
                self.end_edit();
                let dir = if button == Button::PageNext { 1 } else { -1 };
                let mut i = PAGES.iter().position(|p| *p == self.page).unwrap_or(0) as i32;
                let next = loop {
                    i = (i + dir).rem_euclid(PAGES.len() as i32);
                    if render::page_shown(PAGES[i as usize]) {
                        break PAGES[i as usize];
                    }
                };
                self.page_anim = Some((self.page, Instant::now(), dir));
                self.page = next;
                self.settings_open = None;
                self.kick();
            }
            Button::ListInput => self.begin_edit(),
            Button::ListClear => {
                if self.page == Page::Weather || self.edit_item.is_none() {
                    self.draft_mut().clear();
                }
                self.begin_edit();
            }
            Button::ListEdit(i) => {
                self.draft = self.todo.edit_text(i, self.today);
                self.edit_item = Some(i);
                self.begin_edit();
            }
            Button::ListToggle(i) => {
                self.cancel_task_edit();
                self.todo.toggle(i, self.today);
            }
            Button::ListDelete(i) => {
                self.cancel_task_edit();
                self.todo.remove(i);
                self.clamp_scroll();
            }
            Button::CityOff => {
                self.weather.clear();
                self.wx = self.weather.snapshot();
                self.awaiting_city = false;
                system::save_settings(self.settings());
            }
            Button::Unit(unit) => {
                (self.unit, self.unit_chosen) = (unit, true);
                system::save_settings(self.settings());
            }
            Button::SunToggle => {
                self.sun_view = !self.sun_view;
                self.page_since = Instant::now();
                system::save_settings(self.settings());
            }
            Button::CalPrev | Button::CalNext => {
                let delta = if button == Button::CalNext { 1 } else { -1 };
                (self.cal_year, self.cal_month) = calendar::shift_month(self.cal_year, self.cal_month, delta);
                let date = system::today();
                let (ty, tm, td) = ((date / 10_000) as i32, date / 100 % 100, date % 100);
                self.cal_sel = if (ty, tm) == (self.cal_year, self.cal_month) { td } else { 1 };
            }
            Button::CalCell(i) => {
                let (cy, cm, cd, _) = calendar::cell_date(self.cal_year, self.cal_month, i);
                (self.cal_year, self.cal_month, self.cal_sel) = (cy, cm, cd);
            }
            Button::CalMode(on) if self.cal_zone.is_some() => self.cal_lunar = on,
            Button::CalMode(_) => {}
            Button::NoteOpen(i) => {
                if let Some(target) = self.notes.get(i).and_then(|n| n.open.as_deref()) {
                    open_target(target);
                }
            }
            Button::NotesClear => {
                if let Some(r) = self.notifications.as_mut() {
                    r.clear();
                }
                system::save_settings(self.settings());
                self.refresh_notes();
            }
            Button::Gear(card) => self.settings_open = if self.settings_open == Some(card) { None } else { Some(card) },
            Button::RemindStep(dir) => self.todo.step_remind(dir),
            Button::Drink => self.health.drink(todo::minutes(system::now_stamp())),
            Button::Snooze => self.health.snooze(todo::minutes(system::now_stamp())),
            Button::Stood => self.health.stood_up(todo::minutes(system::now_stamp())),
            Button::Step(set, dir) => self.health.change(set, dir),
            Button::Toggle(set) => self.health.change(set, 1),
            Button::FocusPlay => self.focus.toggle(Instant::now()),
            Button::FocusReset => self.focus.reset(),
            Button::FocusSkip => self.focus.skip(Instant::now()),
            Button::FocusMode(kind) => self.focus.pick(kind, Instant::now()),
            Button::FocusSeg(seg) => self.focus.jump(seg, Instant::now()),
            Button::TaskSwitch => {
                self.tasks.enabled = !self.tasks.enabled;
                self.tasks.save();
            }
            Button::TaskEdit => self.settings_open = Some(Card::TaskEdit),
            Button::TaskRow(i) => {
                self.task_sel = Some(i);
                self.settings_open = Some(Card::TaskEdit);
            }
        }
        self.hovered_button = self.hit(x, y);
        self.redraw();
    }

    fn cancel_task_edit(&mut self) {
        if self.edit_item.take().is_some() {
            self.draft.clear();
        }
    }

    fn draft_mut(&mut self) -> &mut String {
        if self.page == Page::Weather { &mut self.city_draft } else { &mut self.draft }
    }

    fn submit(&mut self) {
        if self.page == Page::Weather {
            if !self.city_draft.trim().is_empty() {
                self.weather.find(&self.city_draft);
                self.wx = self.weather.snapshot();
                self.awaiting_city = true;
            }
            return;
        }
        let now = system::now_stamp();
        let saved = match self.edit_item {
            Some(i) => self.todo.update(i, &self.draft, now),
            None => self.todo.add(&self.draft, now),
        };
        if saved {
            if self.edit_item.take().is_none() {
                self.todo_scroll = 0;
            }
            self.draft.clear();
            self.clamp_scroll();
        }
    }

    fn clamp_scroll(&mut self) {
        self.todo_scroll = self.todo_scroll.min(self.todo.items.len().saturating_sub(LIST_ROWS));
        self.task_scroll = self.task_scroll.min(self.tasks.items.len().saturating_sub(LIST_ROWS));
        self.note_scroll = self.note_scroll.min(self.notes.len().saturating_sub(NOTE_ROWS));
    }

    fn begin_edit(&mut self) {
        if self.editing {
            return;
        }
        unsafe {
            self.prev_foreground = GetForegroundWindow();
            let ex = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE);
            SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, ex & !(WS_EX_NOACTIVATE.0 as isize));
            let _ = SetForegroundWindow(self.hwnd);
            let _ = SetFocus(Some(self.hwnd));
            SetTimer(Some(self.hwnd), T_CARET, CARET_MS, None);
        }
        self.editing = true;
        self.caret = true;
    }

    fn end_edit(&mut self) {
        if !self.editing {
            return;
        }
        self.editing = false;
        unsafe {
            let _ = KillTimer(Some(self.hwnd), T_CARET);
            let ex = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE);
            SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, ex | WS_EX_NOACTIVATE.0 as isize);
            let prev = self.prev_foreground;
            if !prev.is_invalid() && prev != self.hwnd && IsWindow(Some(prev)).as_bool() && GetForegroundWindow() == self.hwnd {
                let _ = SetForegroundWindow(prev);
            }
        }
        self.prev_foreground = HWND::default();
        if !self.tracking && !self.cursor_inside() {
            self.notch.on_leave();
            self.kick();
        }
    }

    fn type_char(&mut self, unit: u16) {
        match unit {
            0x0D => self.submit(),
            0x1B => {
                self.draft_mut().clear();
                self.edit_item = None;
                self.end_edit();
            }
            0x08 => {
                self.draft_mut().pop();
            }
            0xD800..=0xDBFF => self.high_surrogate = Some(unit),
            0xDC00..=0xDFFF => {
                if let Some(high) = self.high_surrogate.take() {
                    if let Some(c) = char::decode_utf16([high, unit]).next().and_then(|r| r.ok()) {
                        self.push_char(c);
                    }
                }
            }
            u if u >= 0x20 => {
                if let Some(c) = char::from_u32(u as u32) {
                    self.push_char(c);
                }
            }
            _ => {}
        }
        self.caret = true;
        self.redraw();
    }

    fn push_char(&mut self, c: char) {
        let draft = self.draft_mut();
        if draft.chars().count() < todo::MAX_LEN {
            draft.push(c);
        }
    }

    fn set_hidden(&mut self, hide: bool) {
        if self.hidden != hide {
            self.hidden = hide;
            self.notch.set_hidden(hide);
            let fg = unsafe { GetForegroundWindow() };
            system::log(&if hide { format!("hidden, foreground {}", screen::class_name(fg)) } else { "shown again".to_string() });
            if hide {
                self.cancel_seek();
                self.end_edit();
                self.hover_in = false;
                self.tracking = false;
                self.hovered_button = None;
                self.page_anim = None;
                unsafe { let _ = KillTimer(Some(self.hwnd), T_DWELL); }
            } else {
                unsafe { let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE); }
            }
            self.kick();
        }
    }

    fn tray(&self, msg: NOTIFY_ICON_MESSAGE) {
        let mut nid = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: 1,
            uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
            uCallbackMessage: WM_TRAY,
            hIcon: app_icon(true),
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
            let text = |s: &str| windows::core::HSTRING::from(s);
            let english = i18n::english();
            let _ = AppendMenuW(menu, MF_STRING | check(self.notch.minimal), MENU_MINIMAL, &text(tr("Notch tối giản", "Minimal notch")));
            let _ = AppendMenuW(menu, MF_STRING | check(self.robot_on), MENU_ROBOT, &text(tr("Robot khi để yên", "Idle robot")));
            if let Ok(styles) = CreatePopupMenu() {
                let _ = AppendMenuW(styles, MF_STRING | check(self.style == notch::Style::Ears), MENU_STYLE_EARS, &text(tr("Tai thỏ", "Ears")));
                let _ = AppendMenuW(styles, MF_STRING | check(self.style == notch::Style::Pill), MENU_STYLE_PILL, &text(tr("Viên thuốc", "Pill")));
                let _ = AppendMenuW(menu, MF_POPUP, styles.0 as usize, &text(tr("Kiểu notch", "Notch style")));
            }
            if let Ok(pages) = CreatePopupMenu() {
                let mask = render::pages_on();
                for (i, p) in PAGES.iter().enumerate() {
                    let last = mask == p.bit();
                    let _ = AppendMenuW(pages, MF_STRING | check(mask & p.bit() != 0) | if last { MF_GRAYED } else { MF_ENABLED }, MENU_PAGE + i, &text(p.name()));
                }
                let _ = AppendMenuW(menu, MF_POPUP, pages.0 as usize, &text(tr("Trang hiển thị", "Pages shown")));
            }
            if let Ok(more) = CreatePopupMenu() {
                let _ = AppendMenuW(more, MF_STRING | check(self.approve.is_enabled()), MENU_APPROVE, &text(tr("Duyệt ngay trên notch", "Approve on the notch")));
                let _ = AppendMenuW(more, MF_STRING | check(status::taking_volume_keys()), MENU_VOLUME_KEYS, &text(tr("Ẩn bảng âm lượng của Windows", "Hide Windows volume popup")));
                let _ = AppendMenuW(more, MF_STRING | check(self.activity.enabled), MENU_ACTIVITY, &text(tr("Tác vụ nền trực tiếp", "Live activities")));
                let _ = AppendMenuW(menu, MF_POPUP, more.0 as usize, &text(tr("Tính năng khác", "More features")));
            }
            let _ = AppendMenuW(menu, MF_STRING | check(system::autostart_enabled()), MENU_AUTOSTART, &text(tr("Khởi động cùng Windows", "Start with Windows")));
            if let Ok(langs) = CreatePopupMenu() {
                let _ = AppendMenuW(langs, MF_STRING | check(!english), MENU_LANG_VI, w!("Tiếng Việt"));
                let _ = AppendMenuW(langs, MF_STRING | check(english), MENU_LANG_EN, w!("English"));
                let _ = AppendMenuW(menu, MF_POPUP, langs.0 as usize, &text(tr("Ngôn ngữ", "Language")));
            }
            let _ = AppendMenuW(menu, MF_STRING, MENU_ABOUT, &text(tr("Giới thiệu PUW Notch", "About PUW Notch")));
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            let _ = AppendMenuW(menu, MF_STRING, MENU_QUIT, &text(tr("Thoát", "Quit")));
            let mut p = POINT::default();
            let _ = GetCursorPos(&mut p);
            let _ = SetForegroundWindow(self.hwnd);
            let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, p.x, p.y, None, self.hwnd, None).0 as usize;
            let _ = DestroyMenu(menu);
            match cmd {
                MENU_MINIMAL => {
                    let on = !self.notch.minimal;
                    self.notch.set_minimal(on);
                    system::save_settings(self.settings());
                    self.kick();
                }
                MENU_STYLE_EARS | MENU_STYLE_PILL => {
                    self.style = if cmd == MENU_STYLE_PILL { notch::Style::Pill } else { notch::Style::Ears };
                    self.renderer.style = self.style;
                    system::save_settings(self.settings());
                    self.kick();
                }
                c if (MENU_PAGE..MENU_PAGE + PAGES.len()).contains(&c) => {
                    render::set_pages_on(render::pages_on() ^ PAGES[c - MENU_PAGE].bit());
                    if !render::page_shown(self.page) {
                        self.page = PAGES.into_iter().find(|p| render::page_shown(*p)).unwrap_or(Page::Music);
                        self.page_since = Instant::now();
                        self.settings_open = None;
                    }
                    if !render::page_shown(Page::Focus) {
                        self.focus = Focus::default();
                    }
                    if !render::page_shown(Page::Usage) {
                        self.usage_alerts.clear();
                    }
                    if render::page_shown(Page::Weather) {
                        self.weather.refresh();
                    }
                    self.refresh_music();
                    system::save_settings(self.settings());
                    self.kick();
                }
                MENU_ACTIVITY => {
                    self.activity.enabled = !self.activity.enabled;
                    system::save_settings(self.settings());
                    self.kick();
                }
                MENU_APPROVE => {
                    self.approve.set_enabled(!self.approve.is_enabled());
                    system::save_settings(self.settings());
                    self.finish_approval(approve::Answer::Ask);
                }
                MENU_AUTOSTART => system::set_autostart(!system::autostart_enabled()),
                MENU_ROBOT => {
                    self.robot_on = !self.robot_on;
                    system::save_settings(self.settings());
                    self.refresh_robot();
                }
                MENU_VOLUME_KEYS => {
                    status::take_volume_keys(!status::taking_volume_keys());
                    system::save_settings(self.settings());
                }
                MENU_LANG_VI | MENU_LANG_EN => {
                    i18n::set_english(cmd == MENU_LANG_EN);
                    system::save_settings(self.settings());
                    (self.time, self.date) = system::clock();
                    self.redraw();
                }
                MENU_ABOUT => {
                    let body = windows::core::HSTRING::from(format!("{}\n\n{}", about_text(env!("CARGO_PKG_VERSION")), tr("Mở trang GitHub?", "Open the GitHub page?")));
                    let title = windows::core::HSTRING::from(tr("Giới thiệu PUW Notch", "About PUW Notch"));
                    if MessageBoxW(Some(self.hwnd), &body, &title, MB_YESNO | MB_ICONINFORMATION) == IDYES {
                        let url = windows::core::HSTRING::from(REPO_URL);
                        ShellExecuteW(None, w!("open"), &url, None, None, SW_SHOWNORMAL);
                    }
                }
                MENU_QUIT => { let _ = DestroyWindow(self.hwnd); }
                _ => self.kick(),
            }
        }
    }

    fn handle(&mut self, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
        match msg {
            WM_FRAME => {
                FRAME_PENDING.store(false, Ordering::Relaxed);
                if self.frame_ms == VSYNC {
                    self.frame();
                }
            }
            WM_TIMER => match wp.0 {
                T_ANIM => self.frame(),
                T_CHECK => self.tick(),
                T_DWELL => {
                    unsafe { let _ = KillTimer(Some(self.hwnd), T_DWELL); }
                    let inside = self.cursor_inside();
                    self.hover_in &= inside;
                    self.notch.on_dwell(inside);
                    self.kick();
                }
                T_SETTLE => {
                    unsafe { let _ = KillTimer(Some(self.hwnd), T_SETTLE); }
                    let sample = self.notch.is_settled_rest();
                    self.check_screen(sample);
                }
                T_CARET => {
                    self.caret = !self.caret;
                    self.redraw();
                }
                T_CONN => self.next_connection_banner(),
                T_ALERT => {
                    unsafe { let _ = KillTimer(Some(self.hwnd), T_ALERT); }
                    self.notch.on_alert_end();
                    self.finish_approval(approve::Answer::Ask);
                    if self.hover_in && self.notch.on_enter() {
                        unsafe { SetTimer(Some(self.hwnd), T_DWELL, DWELL_MS, None) };
                    }
                    self.kick();
                }
                _ => return None,
            },
            WM_MOUSEMOVE => {
                if self.seek_drag.is_some() {
                    self.update_seek((lp.0 & 0xffff) as i16 as f32);
                    return Some(LRESULT(0));
                }
                if !self.tracking {
                    let mut tme = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: self.hwnd, dwHoverTime: 0 };
                    self.tracking = unsafe { TrackMouseEvent(&mut tme) }.is_ok();
                }
                if !self.hover_in && self.cursor_inside() {
                    self.hover_in = true;
                    if self.notch.on_enter() && self.banner_target().is_none() && self.approval.is_none() {
                        unsafe { SetTimer(Some(self.hwnd), T_DWELL, DWELL_MS, None) };
                    }
                    self.kick();
                }
                let hover = if self.notch.mode == Mode::Expanded {
                    let x = (lp.0 & 0xffff) as i16 as f32;
                    let y = ((lp.0 >> 16) & 0xffff) as i16 as f32 - self.top_px();
                    self.hit(x, y)
                } else { None };
                if hover != self.hovered_button { self.hovered_button = hover; self.redraw(); }
                if self.notch.mode == Mode::Expanded {
                    let (x, y) = ((lp.0 & 0xffff) as i16 as f32, ((lp.0 >> 16) & 0xffff) as i16 as f32 - self.top_px());
                    if !self.nav_open && hover == Some(Button::Dots) {
                        self.set_nav(true);
                    } else if self.nav_open && !render::in_nav_zone(self.win.2 as f32 / 2.0, self.scale, render::panel_size(self.page)[1], x, y) {
                        self.set_nav(false);
                    }
                }
                let hot = self.notch.mode == Mode::Alert && self.banner_target().is_some() && {
                    let (x, y) = ((lp.0 & 0xffff) as i16 as f32, ((lp.0 >> 16) & 0xffff) as i16 as f32 - self.top_px());
                    let b = render::banner_close(self.win.2 as f32 / 2.0, self.scale, self.notch.size()[0], self.style == notch::Style::Pill);
                    x >= b.left && x <= b.right && y >= b.top && y <= b.bottom
                };
                if hot != self.renderer.close_hot { self.renderer.close_hot = hot; self.redraw(); }
            }
            WM_SETCURSOR if self.banner_target().is_some() || self.approval.is_some() => {
                unsafe { let _ = SetCursor(LoadCursorW(None, IDC_HAND).ok()); }
                return Some(LRESULT(1));
            }
            WM_SETCURSOR if self.hovered_button.is_some_and(|b| b.enabled(self.track.as_ref())) => {
                let cursor = if self.hovered_button == Some(Button::ListInput) { IDC_IBEAM } else { IDC_HAND };
                unsafe { let _ = SetCursor(LoadCursorW(None, cursor).ok()); }
                return Some(LRESULT(1));
            }
            WM_MOUSELEAVE => {
                if self.seek_drag.is_some() {
                    self.tracking = false;
                    return Some(LRESULT(0));
                }
                self.hovered_button = None;
                if std::mem::take(&mut self.renderer.close_hot) { self.redraw(); }
                self.tracking = false;
                self.hover_in = false;
                unsafe { let _ = KillTimer(Some(self.hwnd), T_DWELL); }
                if !self.editing && self.seek_drag.is_none() {
                    self.notch.on_leave();
                }
                self.kick();
            }
            WM_CHAR if self.editing => self.type_char(wp.0 as u16),
            WM_ACTIVATE if self.editing && (wp.0 & 0xffff) as u32 == WA_INACTIVE => self.end_edit(),
            WM_MOUSEWHEEL if self.notch.mode == Mode::Expanded && matches!(self.page, Page::Todo | Page::Notify | Page::Task) => {
                let delta = ((wp.0 >> 16) & 0xffff) as i16;
                let max = self.rows().saturating_sub(if self.page == Page::Notify { NOTE_ROWS } else { LIST_ROWS });
                let scroll = match self.page {
                    Page::Notify => &mut self.note_scroll,
                    Page::Task => &mut self.task_scroll,
                    _ => &mut self.todo_scroll,
                };
                *scroll = if delta > 0 { scroll.saturating_sub(1) } else { (*scroll + 1).min(max) };
                if self.page == Page::Notify { self.kick(); } else { self.redraw(); }
            }
            WM_LBUTTONDOWN => {
                let x = (lp.0 & 0xffff) as i16 as f32;
                let y = ((lp.0 >> 16) & 0xffff) as i16 as f32 - self.top_px();
                if self.notch.mode == Mode::Expanded && self.hit(x, y) == Some(Button::Seek) {
                    self.seek_drag = self.track.as_ref().and_then(SeekDrag::new);
                    if self.seek_drag.is_some() {
                        unsafe { SetCapture(self.hwnd); }
                        if unsafe { GetCapture() } != self.hwnd { self.seek_drag = None; }
                        self.hovered_button = Some(Button::Seek);
                        self.update_seek(x);
                    }
                }
            }
            WM_CAPTURECHANGED | WM_CANCELMODE => {
                self.cancel_seek();
                self.release_stale_hover();
                self.redraw();
            }
            WM_LBUTTONUP => {
                let x = (lp.0 & 0xffff) as i16 as f32;
                let y = ((lp.0 >> 16) & 0xffff) as i16 as f32 - self.top_px();
                if self.seek_drag.is_some() {
                    self.update_seek(x);
                    let drag = self.seek_drag.take().unwrap();
                    if drag.current(self.music.snapshot().as_ref()) {
                        self.music.seek(&drag.track);
                        let mut preview = drag.track;
                        preview.at = Some(Instant::now());
                        self.track = Some(preview);
                    }
                    unsafe { if GetCapture() == self.hwnd { let _ = ReleaseCapture(); } }
                    self.release_stale_hover();
                    self.redraw();
                    return Some(LRESULT(0));
                }
                if let Some(p) = &self.approval {
                    let plain = p.options.is_empty();
                    let labels = self.banner.as_ref().map(|b| b.choices.clone()).unwrap_or_default();
                    let size = self.notch.size();
                    let hit = render::approve_buttons(self.win.2 as f32 / 2.0, self.scale, size[0], size[1], &labels)
                        .iter().position(|b| x >= b.left && x <= b.right && y >= b.top && y <= b.bottom);
                    if let Some(i) = hit {
                        self.finish_approval(match (plain, i) {
                            (true, 0) => approve::Answer::Allow,
                            (true, _) => approve::Answer::Deny,
                            (false, i) => approve::Answer::Choice(i),
                        });
                    }
                    return Some(LRESULT(0));
                }
                let close = render::banner_close(self.win.2 as f32 / 2.0, self.scale, self.notch.size()[0], self.style == notch::Style::Pill);
                match self.banner_target() {
                    Some(_) if x >= close.left && x <= close.right && y >= close.top && y <= close.bottom => {
                        unsafe {
                            let _ = KillTimer(Some(self.hwnd), T_ALERT);
                            let _ = KillTimer(Some(self.hwnd), T_DWELL);
                        }
                        self.notch.on_alert_end();
                        self.kick();
                    }
                    Some(target) => open_target(target),
                    None => self.click(x, y),
                }
            }
            WM_SCREENEVENT => unsafe { SetTimer(Some(self.hwnd), T_SETTLE, SETTLE_MS, None); },
            WM_LOCKKEY => {
                let (name, on) = status::lock_state(wp.0);
                self.show_banner(status_toast(tr("Bàn phím", "Keyboard"), name.to_string(), on_off(on), None, render::ICON_KEYBOARD), STATUS_MS);
            }
            WM_VOLUMEKEY => {
                if let Some(a) = &self.audio {
                    a.volume_key(wp.0 as u16);
                }
            }
            WM_VOLUME => {
                let (pct, muted) = (wp.0 as u8, lp.0 != 0);
                let title = if muted { tr("Đã tắt tiếng", "Muted").to_string() } else { format!("{} {pct}%", tr("Âm lượng", "Volume")) };
                let level = if muted { 0.0 } else { pct as f32 / 100.0 };
                self.show_banner(status_toast(tr("Loa", "Speaker"), title, "", Some(level), if muted { render::ICON_MUTE } else { render::ICON_VOLUME }), STATUS_MS);
                if !self.animating {
                    self.redraw();
                }
            }
            WM_MIC => self.mic_changed(lp.0 != 0),
            WM_POWERBROADCAST if wp.0 as u32 == PBT_APMPOWERSTATUSCHANGE => system::battery_changed(),
            WM_POWERBROADCAST if wp.0 as u32 == PBT_POWERSETTINGCHANGE => {
                let setting = unsafe { (lp.0 as *const POWERBROADCAST_SETTING).as_ref() };
                let level = setting.and_then(|ps| {
                    let data = unsafe { std::slice::from_raw_parts(ps.Data.as_ptr(), ps.DataLength as usize) };
                    status::brightness_setting(&ps.PowerSetting, data)
                });
                if let Some(p) = level {
                    if self.last_brightness.is_some_and(|before| before != p) {
                        self.show_banner(status_toast(tr("Màn hình", "Display"), format!("{} {p}%", tr("Độ sáng", "Brightness")), "", Some(p as f32 / 100.0), render::ICON_BRIGHTNESS), STATUS_MS);
                    }
                    self.last_brightness = Some(p);
                }
                return Some(LRESULT(1));
            }
            WM_MUSIC => {
                self.refresh_music();
                if !self.animating {
                    self.redraw();
                }
            }
            activity::WM_ACTIVITY => {
                let current = self.activity.latest(Instant::now());
                self.activity_banner(current);
            }
            render::WM_ICONS => {
                if self.active_page == Some(Page::Notify) || (!self.hidden && self.notch.mode == Mode::Alert) {
                    self.redraw();
                }
            }
            WM_CONN => {
                self.weather.refresh();
                let mut shown_at = None;
                let mut replaces = None;
                for e in connectivity::take_events() {
                    let now = Instant::now();
                    match &e {
                        Event::Device { up: true, .. } | Event::DeviceBattery { .. } if self.conn_list.as_ref().is_some_and(|l| l.hushes_devices(now)) => {}
                        Event::Device { up: false, .. } => self.conn_queue.push(connection_toast(e)),
                        Event::DeviceBattery { name, percent, .. } => {
                            if self.conn_list.as_mut().is_some_and(|l| device_battery(l, name, *percent, now)) {
                                shown_at = Some(now);
                            } else {
                                self.conn_queue.push(connection_toast(e));
                            }
                        }
                        _ => {
                            let expected = vpn_expected(self.vpn_was_up, self.vpn_lost, &e, self.conn_list.as_ref(), now);
                            if matches!(e, Event::Vpn(false)) && self.vpn_was_up && !expected {
                                self.vpn_lost = Some(now);
                            }
                            let devices = connectivity::expected_devices();
                            replaces.get_or_insert(self.conn_list.as_ref().is_some_and(|l| l.fresh(now)));
                            let list = conn_list_add(self.conn_list.take(), &e, now, Expect { vpn: self.vpn_was_up, devices: &devices });
                            self.vpn_was_up = expected;
                            self.conn_list = Some(list);
                            shown_at = Some(now);
                        }
                    }
                }
                if let Some(now) = shown_at {
                    let t = self.conn_list.as_ref().map(|l| (list_toast(l), l.life_ms(now)));
                    if let Some((t, ms)) = t {
                        self.show_connection_list(t, ms, replaces.unwrap_or(false));
                    }
                }
                if !self.conn_busy && !self.conn_queue.is_empty() {
                    self.next_connection_banner();
                }
            }
            approve::WM_APPROVE => {
                if self.approval.as_ref().is_some_and(|p| self.approve.take_cancelled(p.id)) {
                    self.finish_approval(approve::Answer::Ask);
                }
                self.approve_next();
                return Some(LRESULT(0));
            }
            usage::WM_USAGE if !render::page_shown(Page::Usage) => return Some(LRESULT(0)),
            usage::WM_USAGE => {
                let snap = self.usage.snapshot();
                render::set_usage_apps(snap.mask());
                if !self.usage_tab_chosen || !snap.shown(usage::APPS[self.usage_tab]) {
                    self.usage_tab = (0..usage::APPS.len()).find(|&i| snap.get(usage::APPS[i]).is_some())
                        .or_else(|| (0..usage::APPS.len()).find(|&i| snap.shown(usage::APPS[i]))).unwrap_or(0);
                    self.renderer.usage_tab = self.usage_tab;
                }
                let day = (usage::now_secs() + system::utc_offset_min() * 60).div_euclid(86_400);
                self.usage_alerts.extend(self.usage_watch.check(&snap, day));
                self.renderer.usage = snap;
                self.usage_banner();
                self.kick();
                return Some(LRESULT(0));
            }
            WM_WEATHER => {
                let before = self.wx.place.clone();
                self.wx = self.weather.snapshot();
                if self.wx.place != before {
                    system::save_settings(self.settings());
                }
                if self.awaiting_city && !self.wx.looking {
                    self.awaiting_city = false;
                    if self.wx.not_found.is_none() {
                        self.city_draft.clear();
                        self.end_edit();
                    }
                }
                if !self.animating {
                    self.redraw();
                }
            }
            WM_POWERBROADCAST if wp.0 as u32 == PBT_APMRESUMEAUTOMATIC => self.weather.refresh(),
            WM_DEVICECHANGE => {
                if let Some((change, path)) = usb::parse_message(wp, lp) {
                    self.usb_changed(change, &path);
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
            m if m == self.taskbar_created && m != 0 => self.tray(NIM_ADD),
            _ => return None,
        }
        Some(LRESULT(0))
    }
}

fn about_text(version: &str) -> String {
    let what = tr(
        "Notch kiểu Dynamic Island cho Windows: giờ, pin, nhạc, việc cần làm, thông báo, sức khỏe và banner trạng thái. Viết bằng Rust và Direct2D, không dùng WebView.",
        "A Dynamic Island-style notch for Windows: clock, battery, music, tasks, notifications, health and status banners. Written in Rust and Direct2D, no WebView.",
    );
    format!("PUW Notch {version}\n\n{what}\n\n{REPO_URL}")
}

fn minute_of_day(stamp: u64) -> u32 {
    ((stamp / 100 % 100) * 60 + stamp % 100) as u32
}

fn idle_seconds() -> u32 {
    let mut lii = LASTINPUTINFO { cbSize: size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
    unsafe {
        if GetLastInputInfo(&mut lii).as_bool() {
            windows::Win32::System::SystemInformation::GetTickCount().wrapping_sub(lii.dwTime) / 1000
        } else {
            0
        }
    }
}

fn app_icon(small: bool) -> HICON {
    unsafe {
        let module = GetModuleHandleW(None).ok().map(HINSTANCE::from);
        let (cx, cy) = if small { (GetSystemMetrics(SM_CXSMICON), GetSystemMetrics(SM_CYSMICON)) } else { (GetSystemMetrics(SM_CXICON), GetSystemMetrics(SM_CYICON)) };
        LoadImageW(module, PCWSTR(1 as _), IMAGE_ICON, cx, cy, LR_DEFAULTCOLOR)
            .map(|h| HICON(h.0))
            .or_else(|_| LoadIconW(None, IDI_APPLICATION))
            .unwrap_or_default()
    }
}

fn status_toast(source: &str, title: String, body: &str, level: Option<f32>, icon: &'static str) -> Toast {
    Toast { app: source.to_string(), title, body: body.to_string(), level, icon: Some(icon), image: None, open: None, badge: None, choices: Vec::new(), app_id: None, rows: Vec::new() }
}

fn is_activity_app(app: &str) -> bool {
    [tr("Tệp", "Files"), tr("Tải xuống", "Download"), tr("Build", "Build")].contains(&app)
}

fn activity_toast(a: &activity::LiveActivity, done: bool) -> Toast {
    use activity::ActivityKind::*;
    let (app, icon) = match a.kind {
        FileOp => (tr("Tệp", "Files"), render::ICON_COPY),
        Move => (tr("Tệp", "Files"), render::ICON_MOVE),
        Delete => (tr("Tệp", "Files"), render::ICON_TRASH),
        Download => (tr("Tải xuống", "Download"), render::ICON_DOWNLOAD),
        Build => (tr("Build", "Build"), render::ICON_BUILD),
    };
    let named = |what: &str| if a.title.is_empty() { what.to_string() } else { format!("{what}: {}", a.title) };
    match (a.kind, done, a.result) {
        (Download, true, Some(true)) => {
            let open = a.file.as_ref().map(|f| format!("select:{}", f.display()));
            return Toast { open, ..status_toast(app, tr("Tải xuống hoàn tất", "Download complete").to_string(), &format!("{} · {}", a.title, a.detail), None, render::ICON_DONE) };
        }
        (Build, true, Some(ok)) => {
            let (what, icon) = if ok { (tr("hoàn tất", "finished"), render::ICON_DONE) } else { (tr("thất bại", "failed"), render::ICON_FAILED) };
            return status_toast(app, format!("{} {what}", a.title), &a.detail, None, icon);
        }
        _ => {}
    }
    let mut title = match (a.kind, done) {
        (Download, true) => named(tr("Đã dừng tải xuống", "Download stopped")),
        (Download, false) => named(tr("Đang tải xuống", "Downloading")),
        (_, true) => format!("{} {}", tr("Đã kết thúc:", "Ended:"), a.title),
        _ if a.progress.is_some() => format!("{} · {}", a.title, a.detail),
        _ => a.title.clone(),
    };
    if a.more > 0 && !done {
        title = format!("{title} · +{}", a.more);
    }
    status_toast(app, title, &a.detail, if done { None } else { a.progress }, icon)
}

fn focus_toast(done: focus::Done) -> Toast {
    let min = |k: focus::Kind| k.minutes();
    let (title, body) = match (done.ended, done.next) {
        (focus::Kind::Long, _) if i18n::english() => ("4 rounds done".to_string(), "Press start for a new cycle".to_string()),
        (focus::Kind::Long, _) => ("Xong 4 vòng".to_string(), "Bấm bắt đầu để làm chu kỳ mới".to_string()),
        (focus::Kind::Focus, next) if i18n::english() => (format!("{} min of focus done", min(focus::Kind::Focus)),
            format!("Take a {} min {}", min(next), if next == focus::Kind::Long { "long break" } else { "break" })),
        (focus::Kind::Focus, next) => (format!("Hết {} phút tập trung", min(focus::Kind::Focus)),
            format!("{} {} phút", if next == focus::Kind::Long { "Nghỉ dài" } else { "Nghỉ" }, min(next))),
        (_, next) if i18n::english() => ("Break is over".to_string(), format!("Ready for {} min of focus", min(next))),
        (_, next) => ("Hết giờ nghỉ".to_string(), format!("Sẵn sàng tập trung {} phút", min(next))),
    };
    status_toast(tr("Tập trung", "Focus"), title, &body, None, render::ICON_REMINDER)
}

fn on_off(on: bool) -> &'static str {
    if on { tr("Bật", "On") } else { tr("Tắt", "Off") }
}

fn open_target(target: &str) {
    unsafe {
        let _ = AllowSetForegroundWindow(ASFW_ANY);
        match target.strip_prefix("select:") {
            Some(file) => ShellExecuteW(None, w!("open"), w!("explorer.exe"), &windows::core::HSTRING::from(format!("/select,\"{file}\"")), None, SW_SHOWNORMAL),
            None => ShellExecuteW(None, w!("open"), &windows::core::HSTRING::from(target), None, None, SW_SHOWNORMAL),
        };
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Domain {
    Air,
    Net,
    Bt,
}

fn subject(e: &Event) -> Option<&'static str> {
    match e {
        Event::Airplane(_) => Some("air"),
        Event::Radio(RadioType::Wifi, _) => Some("wifi"),
        Event::Radio(RadioType::Bluetooth, _) => Some("bt"),
        Event::Radio(RadioType::Mobile, _) => Some("mobile"),
        Event::Net(_) => Some("net"),
        Event::Vpn(_) => Some("vpn"),
        Event::Device { .. } | Event::DeviceBattery { .. } => None,
    }
}

fn domain_of(e: &Event) -> Domain {
    match e {
        Event::Airplane(_) => Domain::Air,
        Event::Radio(RadioType::Bluetooth, _) | Event::Device { .. } | Event::DeviceBattery { .. } => Domain::Bt,
        _ => Domain::Net,
    }
}

struct ConnList {
    head: Toast,
    head_row: String,
    rows: Vec<Row>,
    at: Instant,
    single: bool,
    devices_head: bool,
    devices: usize,
    net: Option<Row>,
    vpn: Option<Row>,
    deadline: Instant,
    domain: Domain,
    bt: Option<Row>,
    head_key: Option<&'static str>,
    before: Option<Box<ConnList>>,
}

impl ConnList {
    fn new(head: Toast, head_row: String, rows: Vec<Row>, at: Instant, domain: Domain) -> Self {
        ConnList { head, head_row, rows, at, single: false, devices_head: false, devices: 0, net: None, vpn: None, deadline: at, domain, bt: None, head_key: None, before: None }
    }

    fn take_in(&mut self, old: ConnList, keep_steps: bool) {
        let ConnList { head_row, head_key, rows, at, single, devices_head, net, vpn, deadline, domain, before, .. } = old;
        if let Some(b) = before {
            self.take_in(*b, keep_steps);
        }
        let bt_on_text = row_text(&Event::Radio(RadioType::Bluetooth, true));
        let mut bt_head = domain == Domain::Bt && head_row == bt_on_text;
        let mut told = Vec::new();
        if !single && !devices_head && !bt_head && !head_row.is_empty() {
            told.push(Row::line(head_row, at).keyed(head_key));
        }
        let mut had_device = single;
        for r in rows {
            if r.icon.is_some() {
                had_device |= r.state == RowState::Done;
            } else if r.text == bt_on_text {
                bt_head = true;
            } else {
                told.push(r);
            }
        }
        for r in told {
            if !self.rows.iter().any(|x| x.text == r.text) {
                self.rows.push(r);
            }
        }
        if keep_steps {
            if bt_head || had_device {
                self.bt.get_or_insert_with(|| Row::line(bt_on_text, at));
            }
            self.net = net.or(self.net.take());
            self.vpn = vpn.or(self.vpn.take());
            self.deadline = self.deadline.max(deadline);
        }
    }

    fn rows_shown(&self) -> Vec<Row> {
        let (devices, told): (Vec<Row>, Vec<Row>) = self.rows.iter().cloned().partition(|r| r.icon.is_some());
        told.into_iter().chain(self.bt.iter().cloned()).chain(self.net.iter().cloned()).chain(self.vpn.iter().cloned()).chain(devices).collect()
    }

    fn hushes_devices(&self, now: Instant) -> bool {
        self.domain == Domain::Air && self.fresh(now)
    }

    fn start_devices(&mut self, devices: &[(String, connectivity::DeviceKind)], now: Instant) {
        let missing: Vec<_> = devices.iter().filter(|(name, _)| !self.rows.iter().any(|r| r.icon.is_some() && r.text == *name)).take(MAX_WAIT_DEVICES).collect();
        for (name, kind) in missing {
            let mut row = device_row(name, render::device_icon(*kind), None, now);
            row.state = RowState::Wait;
            self.rows.push(row);
        }
        if !devices.is_empty() {
            self.deadline = now + Duration::from_millis(WAIT_MAX_MS as u64);
        }
    }

    fn start_waiting(&mut self, vpn_expected: bool, now: Instant) {
        if self.net.is_none() {
            self.net = Some(Row::step(tr("Kết nối mạng", "Network connection").to_string(), RowState::Wait, now));
        }
        if self.vpn.is_none() && vpn_expected {
            let state = if self.net.as_ref().is_some_and(|n| n.state == RowState::Done) { RowState::Wait } else { RowState::Todo };
            self.vpn = Some(Row::step("VPN".to_string(), state, now));
        }
        self.deadline = now + Duration::from_millis(WAIT_MAX_MS as u64);
    }

    fn pending(&self) -> bool {
        self.net.iter().chain(self.vpn.iter()).any(|r| matches!(r.state, RowState::Wait | RowState::Todo))
            || self.rows.iter().any(|r| r.icon.is_some() && r.state == RowState::Wait)
    }

    fn life_ms(&self, now: Instant) -> u32 {
        if self.pending() {
            (self.deadline.saturating_duration_since(now).as_millis() as u32).saturating_add(1500)
        } else {
            ALERT_MS
        }
    }

    fn done_coming_back(&self, e: &Event, now: Instant) -> bool {
        let awaiting_radios = self.net.is_none() && now.saturating_duration_since(self.at) < Duration::from_millis(RADIOS_MS as u64);
        matches!(e, Event::Radio(..)) && self.domain == Domain::Air && !self.pending() && !awaiting_radios && self.head_row == row_text(&Event::Airplane(false))
    }

    fn fresh(&self, now: Instant) -> bool {
        if self.pending() { now < self.deadline } else { now.saturating_duration_since(self.at) < Duration::from_millis(ALERT_MS as u64) }
    }

    fn expire(&mut self, now: Instant) -> bool {
        if !self.pending() || now < self.deadline {
            return false;
        }
        if self.net.as_ref().is_some_and(|n| n.state == RowState::Wait) {
            self.net = Some(Row::step(tr("Không kết nối được", "Could not connect").to_string(), RowState::Failed, now));
        }
        if self.vpn.as_ref().is_some_and(|v| v.state != RowState::Done) {
            self.vpn = None;
        }
        self.rows.retain(|r| !(r.icon.is_some() && r.state == RowState::Wait));
        self.at = now;
        true
    }

    fn step_event(&mut self, e: &Event, now: Instant) -> bool {
        match e {
            Event::Net(Net::Offline) if self.net.as_ref().is_some_and(|n| n.state != RowState::Done) => true,
            Event::Vpn(false) if self.vpn.as_ref().is_some_and(|v| v.state != RowState::Done) => true,
            Event::Net(Net::Online { .. }) if self.net.is_some() => {
                self.rows.retain(|r| r.key != Some("net"));
                self.net = Some(Row::line(row_text(e), now));
                if let Some(v) = self.vpn.as_mut().filter(|v| v.state == RowState::Todo) {
                    (v.state, v.at) = (RowState::Wait, now);
                }
                true
            }
            Event::Net(Net::Limited { .. }) if self.net.as_ref().is_some_and(|n| n.state == RowState::Wait) => {
                self.net = Some(Row::step(row_text(e), RowState::Failed, now));
                self.vpn = None;
                true
            }
            Event::Vpn(true) if self.vpn.is_some() || self.net.is_some() => {
                self.rows.retain(|r| r.key != Some("vpn"));
                self.vpn = Some(Row::line(row_text(e), now));
                true
            }
            Event::Radio(RadioType::Bluetooth, true) if self.domain == Domain::Air => {
                self.bt.get_or_insert_with(|| Row::line(row_text(e), now));
                true
            }
            Event::Radio(RadioType::Wifi, false) => {
                (self.net, self.vpn) = (None, None);
                self.rows.retain(|r| r.key != Some("net"));
                false
            }
            _ => false,
        }
    }

    fn takes_devices(&self) -> bool {
        self.devices_head || (self.domain == Domain::Bt && self.rows.iter().any(|r| r.icon.is_some()))
    }
}

const WAIT_MAX_MS: u32 = 15_000;
const RADIOS_MS: u32 = 3_000;

const MAX_ROWS: usize = 8;
const MAX_WAIT_DEVICES: usize = 5;

#[derive(Clone, Copy, Default)]
struct Expect<'a> {
    vpn: bool,
    devices: &'a [(String, connectivity::DeviceKind)],
}

fn devices_title(n: usize) -> String {
    format!("{n} {}", tr("thiết bị Bluetooth", "Bluetooth devices"))
}

fn low_battery(value: Option<&str>) -> bool {
    value.and_then(|v| v.trim_end_matches('%').parse::<u8>().ok()).is_some_and(|p| p <= connectivity::LOW_BATTERY)
}

fn device_row(name: &str, icon: &'static str, value: Option<String>, at: Instant) -> Row {
    Row { text: name.to_string(), at, icon: Some(icon), low: low_battery(value.as_deref()), value, state: RowState::Done, key: None }
}

fn device_connected(fresh: Option<ConnList>, e: &Event, now: Instant) -> ConnList {
    let Event::Device { name, kind, battery, .. } = e else { unreachable!("a device event") };
    let value = battery.map(|p| format!("{p}%"));
    match fresh {
        Some(mut l) if l.single => {
            if l.head.title == *name {
                l.head.badge = l.head.badge.take().or(value);
            } else {
                let first = device_row(&l.head.title, l.head.icon.unwrap_or(render::ICON_BT), l.head.badge.clone(), l.at);
                l.rows = vec![first, device_row(name, render::device_icon(*kind), value, now)];
                (l.single, l.devices_head, l.devices) = (false, true, 2);
                l.head_row = String::new();
                l.head = status_toast("Bluetooth", devices_title(2), "", None, render::ICON_BT);
            }
            l.at = now;
            l
        }
        Some(mut l) if l.takes_devices() => {
            match l.rows.iter_mut().find(|r| r.icon.is_some() && r.text == *name) {
                Some(row) => {
                    row.value = value.or(row.value.take());
                    row.low = low_battery(row.value.as_deref());
                    if row.state == RowState::Wait {
                        (row.state, row.at) = (RowState::Done, now);
                    }
                }
                None => {
                    l.devices += 1;
                    if l.rows.len() < MAX_ROWS {
                        l.rows.push(device_row(name, render::device_icon(*kind), value, now));
                    }
                }
            }
            if l.devices_head {
                l.head.title = devices_title(l.devices);
            }
            l.at = now;
            l
        }
        other => ConnList { single: true, before: other.map(Box::new), ..ConnList::new(connection_toast(e.clone()), name.clone(), Vec::new(), now, Domain::Bt) },
    }
}

fn device_battery(l: &mut ConnList, name: &str, percent: u8, now: Instant) -> bool {
    if !l.fresh(now) {
        return false;
    }
    let value = format!("{percent}%");
    if l.single && l.head.title == name {
        l.head.badge = Some(value);
        if percent <= connectivity::LOW_BATTERY {
            l.head.body = tr("Pin yếu, nên sạc", "Low battery, charge soon").to_string();
        }
    } else if let Some(row) = l.rows.iter_mut().find(|r| r.icon.is_some() && r.text == name) {
        row.low = percent <= connectivity::LOW_BATTERY;
        row.value = Some(value);
    } else {
        return false;
    }
    l.at = now;
    true
}

fn row_text(e: &Event) -> String {
    let radio = |name: &str, on: bool| format!("{name} {}", if on { tr("bật", "on") } else { tr("tắt", "off") });
    match e {
        Event::Radio(RadioType::Wifi, on) => radio("Wi-Fi", *on),
        Event::Radio(RadioType::Bluetooth, on) => radio("Bluetooth", *on),
        Event::Radio(RadioType::Mobile, on) => radio(tr("Mạng di động", "Mobile data"), *on),
        Event::Airplane(on) => format!("{} {}", tr("Chế độ máy bay", "Airplane mode"), on_off(*on)),
        Event::Net(Net::Online { name, .. }) => format!("{} {name}", tr("Đã kết nối", "Connected to")),
        Event::Net(Net::Limited { .. }) => tr("Không có internet", "No internet").to_string(),
        Event::Net(Net::Offline) => tr("Mất kết nối mạng", "Disconnected").to_string(),
        Event::Vpn(up) => tr(if *up { "VPN kết nối" } else { "VPN tắt" }, if *up { "VPN connected" } else { "VPN off" }).to_string(),
        Event::Device { name, .. } | Event::DeviceBattery { name, .. } => name.clone(),
    }
}

fn conn_list_add(prev: Option<ConnList>, e: &Event, now: Instant, expect: Expect) -> ConnList {
    let fresh = prev.filter(|l| l.fresh(now));
    if matches!(e, Event::Device { up: true, .. }) {
        return device_connected(fresh, e, now);
    }
    let domain = domain_of(e);
    match fresh {
        Some(mut l) if !l.single && !matches!(e, Event::Airplane(_)) && (l.domain == domain || (l.domain == Domain::Air && !l.done_coming_back(e, now))) => {
            let key = subject(e);
            let wifi_on = matches!(e, Event::Radio(RadioType::Wifi, true));
            let bt_on = matches!(e, Event::Radio(RadioType::Bluetooth, true));
            if wifi_on {
                l.rows.retain(|r| !matches!(r.key, Some("vpn" | "net")));
            }
            if !l.step_event(e, now) {
                let text = row_text(e);
                if (wifi_on || bt_on) && l.domain != Domain::Air {
                    l.rows.retain(|r| r.key != key);
                    (l.head, l.head_row, l.head_key) = (connection_toast(e.clone()), text, key);
                } else if key.is_some() && key == l.head_key {
                    (l.head, l.head_row) = (connection_toast(e.clone()), text);
                } else if let Some(r) = l.rows.iter_mut().find(|r| key.is_some() && r.key == key) {
                    (r.text, r.at) = (text, now);
                } else if text != l.head_row && !l.rows.iter().any(|r| r.text == text) && l.rows.len() < MAX_ROWS {
                    l.rows.push(Row::line(text, now).keyed(key));
                }
            }
            if wifi_on {
                l.start_waiting(expect.vpn, now);
            }
            if bt_on && l.domain == Domain::Bt && expect.devices.len() >= 2 {
                l.start_devices(expect.devices, now);
            }
            l.at = now;
            l
        }
        prev => {
            let airplane = matches!(e, Event::Airplane(_));
            let fold = prev.as_ref().is_some_and(|l| l.head.icon != Some(render::ICON_AIRPLANE) && (airplane || (l.single && l.domain == domain)));
            let (old, before) = if fold { (prev, None) } else { (None, prev.map(Box::new)) };
            let mut l = match old {
                Some(old) if airplane => {
                    let mut l = ConnList::new(connection_toast(e.clone()), row_text(e), Vec::new(), now, Domain::Air);
                    l.take_in(old, matches!(e, Event::Airplane(false)));
                    l
                }
                Some(old) => {
                    let first = device_row(&old.head.title, old.head.icon.unwrap_or(render::ICON_BT), old.head.badge.clone(), old.at);
                    ConnList::new(connection_toast(e.clone()), row_text(e), vec![first], now, domain)
                }
                None => ConnList::new(connection_toast(e.clone()), row_text(e), Vec::new(), now, domain),
            };
            l.head_key = subject(e);
            l.before = before.filter(|b| b.domain != Domain::Air);
            if matches!(e, Event::Radio(RadioType::Wifi, true)) {
                l.start_waiting(expect.vpn, now);
            }
            if matches!(e, Event::Airplane(true)) && expect.vpn {
                l.rows.push(Row::line(row_text(&Event::Vpn(false)), now).keyed(Some("vpn")));
            }
            if matches!(e, Event::Radio(RadioType::Bluetooth, true)) && expect.devices.len() >= 2 {
                l.start_devices(expect.devices, now);
            }
            l
        }
    }
}

const VPN_LOST: Duration = Duration::from_secs(3);

fn vpn_expected(was: bool, lost: Option<Instant>, e: &Event, list: Option<&ConnList>, now: Instant) -> bool {
    match e {
        Event::Vpn(true) => true,
        Event::Radio(RadioType::Wifi, false) | Event::Airplane(true) | Event::Net(Net::Offline) => was || lost.is_some_and(|t| now.saturating_duration_since(t) < VPN_LOST),
        Event::Vpn(false) => list.is_some_and(|l| l.fresh(now) && matches!(l.head.icon, Some(render::ICON_AIRPLANE | render::ICON_WIFI_OFF))) && was,
        _ => was,
    }
}

fn list_toast(l: &ConnList) -> Toast {
    Toast { rows: l.rows_shown(), ..l.head.clone() }
}

fn connection_toast(e: Event) -> Toast {
    match e {
        Event::Radio(RadioType::Wifi, on) => status_toast("Wi-Fi", format!("{} Wi-Fi", if on { tr("Đã bật", "Turned on") } else { tr("Đã tắt", "Turned off") }), "", None, if on { render::ICON_WIFI } else { render::ICON_WIFI_OFF }),
        Event::Radio(RadioType::Bluetooth, on) => status_toast("Bluetooth", format!("{} Bluetooth", if on { tr("Đã bật", "Turned on") } else { tr("Đã tắt", "Turned off") }), "", None, render::ICON_BT),
        Event::Radio(RadioType::Mobile, on) => status_toast(tr("Mạng di động", "Mobile data"), format!("{} {}", tr("Mạng di động", "Mobile data"), on_off(on)), "", None, render::ICON_MOBILE),
        Event::Airplane(on) => status_toast(tr("Chế độ máy bay", "Airplane mode"), format!("{} {}", tr("Chế độ máy bay", "Airplane mode"), on_off(on)), "", None, render::ICON_AIRPLANE),
        Event::Vpn(up) => status_toast("VPN", tr(if up { "VPN đã kết nối" } else { "VPN đã tắt" }, if up { "VPN connected" } else { "VPN off" }).to_string(), "", None, render::ICON_VPN),
        Event::Net(Net::Online { name, wifi }) => {
            let (source, icon) = if wifi { ("Wi-Fi", render::ICON_WIFI) } else { (tr("Mạng", "Network"), render::ICON_ETHERNET) };
            status_toast(source, format!("{} {name}", tr("Đã kết nối", "Connected to")), "", None, icon)
        }
        Event::Net(Net::Limited { name, .. }) => status_toast(tr("Mạng", "Network"), tr("Không có internet", "No internet").to_string(), &name, None, render::ICON_NO_INTERNET),
        Event::Net(Net::Offline) => status_toast(tr("Mạng", "Network"), tr("Mất kết nối mạng", "Disconnected").to_string(), "", None, render::ICON_NO_INTERNET),
        Event::Device { name, kind, up, battery } => {
            let state = if up { tr("Đã kết nối", "Connected") } else { tr("Đã ngắt kết nối", "Disconnected") };
            Toast { badge: battery.map(|p| format!("{p}%")), ..status_toast("Bluetooth", name, state, None, render::device_icon(kind)) }
        }
        Event::DeviceBattery { name, kind, percent } => {
            let state = if percent <= connectivity::LOW_BATTERY { tr("Pin yếu, nên sạc", "Low battery, charge soon") } else { tr("Pin", "Battery") };
            Toast { badge: Some(format!("{percent}%")), ..status_toast("Bluetooth", name, state, None, render::device_icon(kind)) }
        }
    }
}

fn power_toast(e: system::PowerEvent, b: system::Battery) -> Toast {
    use system::PowerEvent::*;
    let battery = |title: &str, body: &str, icon| Toast { badge: Some(format!("{}%", b.percent)), ..status_toast(tr("Pin", "Battery"), title.to_string(), body, None, icon) };
    match e {
        Plugged => battery(tr("Đang sạc", "Charging"), tr("Đã cắm sạc", "Charger connected"), render::ICON_PLUG),
        Unplugged => battery(tr("Đã rút sạc", "Unplugged"), tr("Đang dùng pin", "On battery"), render::ICON_UNPLUG),
        SaverOn => status_toast(tr("Tiết kiệm pin", "Battery saver"), format!("{} {}", tr("Tiết kiệm pin", "Battery saver"), on_off(true)), "", None, render::ICON_SAVER),
        SaverOff => status_toast(tr("Tiết kiệm pin", "Battery saver"), format!("{} {}", tr("Tiết kiệm pin", "Battery saver"), on_off(false)), "", None, render::ICON_SAVER),
        Low => battery(tr("Pin yếu", "Low battery"), tr("Nên cắm sạc", "Plug in soon"), render::ICON_BATTERY_LOW),
        Critical => battery(tr("Pin rất yếu", "Battery critical"), tr("Cắm sạc ngay", "Plug in now"), render::ICON_BATTERY_LOW),
        Full => battery(tr("Đã sạc đầy", "Fully charged"), tr("Có thể rút sạc", "You can unplug"), render::ICON_BATTERY),
    }
}

static FRAME_PENDING: AtomicBool = AtomicBool::new(false);

struct Vsync {
    on: Arc<AtomicBool>,
    thread: std::thread::Thread,
}

impl Vsync {
    fn start(hwnd: HWND) -> Self {
        let on = Arc::new(AtomicBool::new(false));
        let (on2, target) = (on.clone(), hwnd.0 as isize);
        let thread = crate::system::spawn("vsync", move || {
            unsafe { let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_ABOVE_NORMAL); }
            loop {
            if !on2.load(Ordering::Relaxed) {
                std::thread::park();
                continue;
            }
            if unsafe { DwmFlush() }.is_err() {
                std::thread::sleep(std::time::Duration::from_millis(16));
            }
            if on2.load(Ordering::Relaxed) && !FRAME_PENDING.swap(true, Ordering::Relaxed) {
                unsafe { let _ = PostMessageW(Some(HWND(target as _)), WM_FRAME, WPARAM(0), LPARAM(0)); }
            }
            }
        })
        .thread()
        .clone();
        Vsync { on, thread }
    }

    fn set(&self, on: bool) {
        self.on.store(on, Ordering::Relaxed);
        if on {
            self.thread.unpark();
        }
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_DESTROY {
        unsafe { PostQuitMessage(0) };
        return LRESULT(0);
    }
    let handled = APP.with(|a| a.try_borrow_mut().ok().and_then(|mut g| g.as_mut().and_then(|app| app.handle(msg, wp, lp))));
    if handled.is_none() && msg == WM_FRAME {
        FRAME_PENDING.store(false, Ordering::Relaxed);
    }
    handled.unwrap_or_else(|| unsafe { DefWindowProcW(hwnd, msg, wp, lp) })
}

fn main() -> windows::core::Result<()> {
    watchdog();
    std::panic::set_hook(Box::new(|info| system::log(&format!("panic: {info}"))));
    let result = run();
    match &result {
        Ok(()) => system::log("exit"),
        Err(e) => system::log(&format!("failed: {e} ({:?})", e.code())),
    }
    result
}

fn run() -> windows::core::Result<()> {
    unsafe {
        let _mutex = CreateMutexW(None, false, w!("Local\\PuwNotch.SingleInstance"))?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            system::log("already running, leaving");
            return Ok(());
        }
        system::log_open();
        let cwd = std::env::current_dir().map(|d| d.display().to_string()).unwrap_or_default();
        system::log(&format!("start v{} pid {} cwd {cwd}", env!("CARGO_PKG_VERSION"), std::process::id()));
        system::refresh_autostart();
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let eco = PROCESS_POWER_THROTTLING_STATE { Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION, ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED, StateMask: 0 };
        let _ = SetProcessInformation(GetCurrentProcess(), ProcessPowerThrottling, &eco as *const _ as *const std::ffi::c_void, size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32);
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);

        let instance: HINSTANCE = GetModuleHandleW(None)?.into();
        let class = w!("PuwNotchWindow");
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: instance,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            hIcon: app_icon(false),
            hIconSm: app_icon(true),
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassExW(&wc);
        let hwnd = CreateWindowExW(
            WS_EX_NOREDIRECTIONBITMAP | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
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

        let settings = system::load_settings();
        render::set_pages_on(settings.pages);
        i18n::set_english(settings.english);
        let (time, date) = system::clock();
        let mut app = App {
            hwnd,
            notch: Notch::new(settings.minimal),
            renderer: { let mut r = Renderer::new()?; r.style = settings.style; r },
            scale: 1.0,
            monitor: RECT::default(),
            win: (0, 0, 1, 1),
            hidden: false,
            tracking: false,
            hover_in: false,
            animating: false,
            last_frame: Instant::now(),
            taskbar_created: RegisterWindowMessageW(w!("TaskbarCreated")),
            time,
            date,
            battery: system::battery(),
            notifications: notify::Reader::open(),
            banner: None,
            banner_at: Instant::now(),
            lyrics_open: false,
            usb_last: None,
            last_brightness: None,
            audio: audio::Audio::new(),
            mic_muted: None,
            audio_ticks: 0,
            music: music::Music::start(hwnd),
            usage: usage::Monitor::start(hwnd),
            approve: {
                let server = approve::Server::start(hwnd);
                server.set_enabled(settings.approve);
                antigravity::watch(server.queue(), server.cancelled(), server.enabled(), hwnd);
                server
            },
            approval: None,
            usage_watch: usage::Watch::default(),
            usage_alerts: Vec::new(),
            conn_queue: Vec::new(),
            conn_busy: false,
            conn_list: None,
            vpn_was_up: false,
            vpn_lost: None,
            usage_tab: 0,
            usage_tab_chosen: false,
            track: None,
            seek_drag: None,
            robot: robot::Robot::default(),
            robot_on: settings.robot,
            style: settings.style,
            robot_at: Instant::now(),
            paused_since: None,
            started: Instant::now(),
            frame_ms: 0,
            vsync: Vsync::start(hwnd),
            hovered_button: None,
            last_foreground: None,
            rested: 0,
            logged_open: false,
            last_gap_log: None,
            last_slow_tick: None,
            nav_open: false,
            page: Page::Music,
            todo: todo::Todo::load(system::now_stamp()),
            tasks: task::Tasks::load(system::now_stamp()),
            task_sel: None,
            task_scroll: 0,
            today: system::today(),
            draft: String::new(),
            editing: false,
            edit_item: None,
            caret: false,
            todo_scroll: 0,
            notes: Vec::new(),
            note_scroll: 0,
            note_pos: 0.0,
            health: health::Health::load(system::today()),
            activity: activity::Watch::new(settings.activity),
            focus: Focus::default(),
            settings_open: None,
            page_anim: None,
            icons_morphing: false,
            active_page: None,
            prev_foreground: HWND::default(),
            high_surrogate: None,
            weather: weather::Weather::start(hwnd, settings.place.clone()),
            wx: weather::State { place: settings.place.clone(), ..weather::State::default() },
            city_draft: String::new(),
            awaiting_city: false,
            page_since: Instant::now(),
            unit: settings.unit.unwrap_or_else(weather::Unit::automatic),
            unit_chosen: settings.unit.is_some(),
            sun_view: settings.sun_view,
            cal_year: 0,
            cal_month: 0,
            cal_sel: 0,
            cal_lunar: false,
            cal_zone: None,
            cal_zone_at: None,
        };
        app.refresh_audio();
        if let Some(r) = app.notifications.as_mut() {
            r.cleared = settings.notes_cleared;
        }
        let _ = RegisterPowerSettingNotification(HANDLE(hwnd.0), &status::VIDEO_BRIGHTNESS, DEVICE_NOTIFY_WINDOW_HANDLE);
        status::take_volume_keys(settings.volume_keys);
        status::watch_lock_keys(hwnd);
        connectivity::watch(hwnd);
        usb::register(hwnd);
        app.layout();
        app.redraw();
        app.kick();
        app.tray(NIM_ADD);
        APP.with(|a| *a.borrow_mut() = Some(app));

        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        screen::watch(hwnd);
        SetTimer(Some(hwnd), T_CHECK, CHECK_MS, None);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let app = APP.with(|a| a.borrow_mut().take());
        if let Some(app) = app {
            app.tray(NIM_DELETE);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn live_activity_percentage_is_visible_alongside_the_bar() {
        let activity = activity::LiveActivity {
            kind: activity::ActivityKind::FileOp,
            title: "Copying files".into(), detail: "42%".into(), progress: Some(0.42), result: None, file: None, more: 0,
        };
        let toast = activity_toast(&activity, false);
        assert!(toast.title.contains("42%"), "the status layout hides body text when a bar is present");
        assert_eq!(toast.level, Some(0.42));
    }

    #[test]
    fn a_build_ends_with_a_check_or_a_cross_and_others_running_are_counted() {
        let mut a = activity::LiveActivity { kind: activity::ActivityKind::Build, title: "Cargo".into(), detail: "1m 15s".into(), progress: None, result: Some(true), file: None, more: 0 };
        assert_eq!(activity_toast(&a, true).icon, Some(render::ICON_DONE));
        a.result = Some(false);
        let failed = activity_toast(&a, true);
        assert_eq!((failed.icon, failed.title.as_str()), (Some(render::ICON_FAILED), format!("Cargo {}", tr("thất bại", "failed")).as_str()));
        a.result = None;
        assert_eq!(activity_toast(&a, true).icon, Some(render::ICON_BUILD), "exit code unknown: only 'ended'");
        (a.result, a.more) = (None, 2);
        assert!(activity_toast(&a, false).title.ends_with("· +2"));
        assert!(!activity_toast(&a, true).title.contains('+'), "an ending counts nothing");
    }

    #[test]
    fn network_events_that_come_together_are_one_banner_with_lines() {
        let t0 = Instant::now();
        let later = |ms: u64| t0 + Duration::from_millis(ms);
        let texts = |l: &ConnList| l.rows.iter().map(|r| r.text.clone()).collect::<Vec<String>>();
        let wifi_off = Event::Radio(RadioType::Wifi, false);
        let l = conn_list_add(None, &wifi_off, t0, Expect::default());
        assert_eq!((l.head.icon, l.rows.len()), (Some(render::ICON_WIFI_OFF), 0), "alone it is the banner it always was");
        let l = conn_list_add(Some(l), &Event::Vpn(false), later(600), Expect::default());
        assert_eq!((l.head.title.as_str(), texts(&l)), (format!("{} Wi-Fi", tr("Đã tắt", "Turned off")).as_str(), vec![tr("VPN tắt", "VPN off").to_string()]));
        let mut l = conn_list_add(None, &Event::Airplane(true), t0, Expect::default());
        for (ms, e) in [(100, wifi_off.clone()), (200, Event::Radio(RadioType::Bluetooth, false)), (300, Event::Vpn(false)), (400, wifi_off.clone())] {
            l = conn_list_add(Some(l), &e, later(ms), Expect::default());
        }
        assert_eq!(l.head.icon, Some(render::ICON_AIRPLANE));
        assert_eq!(texts(&l), [tr("Wi-Fi tắt", "Wi-Fi off"), tr("Bluetooth tắt", "Bluetooth off"), tr("VPN tắt", "VPN off")]);
        let l = conn_list_add(Some(conn_list_add(None, &wifi_off, t0, Expect::default())), &Event::Airplane(true), later(300), Expect::default());
        assert_eq!((l.head.icon, texts(&l)), (Some(render::ICON_AIRPLANE), vec![tr("Wi-Fi tắt", "Wi-Fi off").to_string()]));
        let l = conn_list_add(Some(l), &Event::Vpn(true), later(300 + ALERT_MS as u64 + 1), Expect::default());
        assert_eq!((l.head.title.as_str(), l.rows.len()), (tr("VPN đã kết nối", "VPN connected"), 0));
    }

    #[test]
    fn a_banner_that_waits_lists_its_steps_and_ticks_them_off() {
        use RowState::*;
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let online = Event::Net(Net::Online { name: "JihoonKim".into(), wifi: true });
        let shown = |l: &ConnList| l.rows_shown().into_iter().map(|r| (r.text, r.state)).collect::<Vec<_>>();
        let (net, connected) = (tr("Kết nối mạng", "Network connection").to_string(), format!("{} JihoonKim", tr("Đã kết nối", "Connected to")));
        let (wifi_on, bt_on, vpn_on) = (tr("Wi-Fi bật", "Wi-Fi on").to_string(), tr("Bluetooth bật", "Bluetooth on").to_string(), tr("VPN kết nối", "VPN connected").to_string());
        let mut l = conn_list_add(None, &Event::Airplane(false), t0, Expect { vpn: true, ..Expect::default() });
        assert!(shown(&l).is_empty() && !l.pending(), "airplane off settles alone first: no network or VPN step yet");
        l = conn_list_add(Some(l), &Event::Radio(RadioType::Wifi, true), at(100), Expect { vpn: true, ..Expect::default() });
        assert_eq!(shown(&l), [(wifi_on.clone(), Done), (net.clone(), Wait), ("VPN".to_string(), Todo)], "the network and VPN steps come with Wi-Fi");
        l = conn_list_add(Some(l), &Event::Radio(RadioType::Bluetooth, true), at(120), Expect { vpn: true, ..Expect::default() });
        assert_eq!(shown(&l), [(wifi_on.clone(), Done), (bt_on.clone(), Done), (net.clone(), Wait), ("VPN".to_string(), Todo)]);
        assert!(l.pending() && l.fresh(at(14_000)) && l.life_ms(at(14_000)) > 1_000);
        l = conn_list_add(Some(l), &online, at(7000), Expect { vpn: true, ..Expect::default() });
        assert_eq!(shown(&l), [(wifi_on.clone(), Done), (bt_on.clone(), Done), (connected.clone(), Done), ("VPN".to_string(), Wait)]);
        l = conn_list_add(Some(l), &Event::Vpn(true), at(9000), Expect { vpn: true, ..Expect::default() });
        assert_eq!(l.head.icon, Some(render::ICON_AIRPLANE), "one banner, not a second one");
        assert!(shown(&l).iter().all(|(_, s)| *s == Done) && shown(&l).last().map(|r| r.0.clone()) == Some(vpn_on.clone()));
        assert!(!l.pending() && l.life_ms(at(9000)) == ALERT_MS);
        assert!(l.fresh(at(9000 + ALERT_MS as u64 - 1)) && !l.fresh(at(9000 + ALERT_MS as u64 + 1)));
        let mut l = conn_list_add(None, &Event::Airplane(false), t0, Expect::default());
        assert!(shown(&l).is_empty());
        l = conn_list_add(Some(l), &online, at(3000), Expect::default());
        assert!(!l.pending());
        assert!(conn_list_add(None, &Event::Radio(RadioType::Wifi, true), t0, Expect::default()).vpn.is_none());
        l = conn_list_add(Some(l), &Event::Vpn(true), at(3500), Expect::default());
        assert_eq!(shown(&l), [(connected.clone(), Done), (vpn_on.clone(), Done)]);
    }

    #[test]
    fn a_step_that_does_not_come_fails_at_the_deadline_and_then_the_banner_ends() {
        use RowState::*;
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let shown = |l: &ConnList| l.rows_shown().into_iter().map(|r| (r.text, r.state)).collect::<Vec<_>>();
        let mut l = conn_list_add(None, &Event::Radio(RadioType::Wifi, true), t0, Expect { vpn: true, ..Expect::default() });
        assert!(!l.expire(at(WAIT_MAX_MS as u64 - 1)));
        assert!(l.expire(at(WAIT_MAX_MS as u64)));
        assert_eq!(shown(&l), [(tr("Không kết nối được", "Could not connect").to_string(), Failed)], "the VPN cannot come without it");
        assert!(!l.pending() && l.fresh(at(WAIT_MAX_MS as u64 + ALERT_MS as u64 - 1)) && !l.fresh(at(WAIT_MAX_MS as u64 + ALERT_MS as u64 + 1)));
        let mut l = conn_list_add(None, &Event::Radio(RadioType::Wifi, true), t0, Expect { vpn: true, ..Expect::default() });
        l = conn_list_add(Some(l), &Event::Net(Net::Limited { name: "Cafe".into(), wifi: true }), at(9000), Expect { vpn: true, ..Expect::default() });
        assert_eq!(shown(&l), [(tr("Không có internet", "No internet").to_string(), Failed)]);
        l = conn_list_add(Some(l), &Event::Net(Net::Online { name: "Cafe".into(), wifi: true }), at(12_000), Expect { vpn: true, ..Expect::default() });
        assert_eq!(shown(&l), [(format!("{} Cafe", tr("Đã kết nối", "Connected to")), Done)]);
        let mut l = conn_list_add(None, &Event::Radio(RadioType::Wifi, true), t0, Expect { vpn: true, ..Expect::default() });
        l = conn_list_add(Some(l), &Event::Radio(RadioType::Wifi, false), at(500), Expect { vpn: true, ..Expect::default() });
        assert!(!l.pending());
    }

    #[test]
    fn devices_that_were_connected_are_listed_and_ticked_off_as_they_connect() {
        use connectivity::DeviceKind::{Gamepad, Headphones, Mouse};
        use RowState::*;
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let known = vec![("WH-1000XM5".to_string(), Headphones), ("MX Master 3".to_string(), Mouse), ("Tay cầm".to_string(), Gamepad)];
        let expect = Expect { devices: &known, ..Expect::default() };
        let dev = |name: &str, kind, battery| Event::Device { name: name.into(), kind, up: true, battery };
        let shown = |l: &ConnList| l.rows_shown().into_iter().map(|r| (r.text, r.state, r.value)).collect::<Vec<_>>();
        let mut l = conn_list_add(None, &Event::Radio(RadioType::Bluetooth, true), t0, expect);
        assert!(l.pending() && l.rows_shown().iter().all(|r| r.state == Wait && r.icon.is_some()));
        l = conn_list_add(Some(l), &dev("MX Master 3", Mouse, Some(64)), at(1500), expect);
        l = conn_list_add(Some(l), &dev("WH-1000XM5", Headphones, None), at(2000), expect);
        assert!(device_battery(&mut l, "WH-1000XM5", 80, at(2600)));
        assert_eq!(shown(&l), [("WH-1000XM5".into(), Done, Some("80%".into())), ("MX Master 3".into(), Done, Some("64%".into())), ("Tay cầm".into(), Wait, None)]);
        assert!(l.pending() && l.fresh(at(14_000)), "it stays while one is waited for");
        l = conn_list_add(Some(l), &dev("Tay cầm", Gamepad, Some(18)), at(4000), expect);
        assert!(!l.pending() && l.life_ms(at(4000)) == ALERT_MS, "all connected: the usual time, no more");
        assert!(l.rows_shown().last().is_some_and(|r| r.low));
        let mut l = conn_list_add(None, &Event::Radio(RadioType::Bluetooth, true), t0, expect);
        l = conn_list_add(Some(l), &dev("WH-1000XM5", Headphones, Some(80)), at(1000), expect);
        assert!(l.expire(at(WAIT_MAX_MS as u64)));
        assert_eq!(l.rows_shown().len(), 1);
        assert!(!l.pending());
        let l = conn_list_add(None, &Event::Radio(RadioType::Bluetooth, true), t0, Expect::default());
        assert!(!l.pending() && l.rows_shown().is_empty());
        let many: Vec<_> = (0..8).map(|i| (format!("Thiết bị {i}"), Mouse)).collect();
        let l = conn_list_add(None, &Event::Radio(RadioType::Bluetooth, true), t0, Expect { devices: &many, ..Expect::default() });
        assert_eq!(l.rows_shown().len(), MAX_WAIT_DEVICES);
    }

    #[test]
    fn airplane_off_tells_bluetooth_on_and_leaves_the_devices_out() {
        use connectivity::DeviceKind::{Headphones, Mouse};
        use RowState::*;
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let known = vec![("WH-1000XM5".to_string(), Headphones), ("MX Master 3".to_string(), Mouse)];
        let air = Expect { vpn: true, devices: &known };
        let states = |l: &ConnList| l.rows_shown().into_iter().map(|r| (r.text, r.state)).collect::<Vec<_>>();
        let (wifi_on, bt_on, net) = (tr("Wi-Fi bật", "Wi-Fi on").to_string(), tr("Bluetooth bật", "Bluetooth on").to_string(), tr("Kết nối mạng", "Network connection").to_string());
        let mut l = conn_list_add(None, &Event::Airplane(false), t0, air);
        assert!(states(&l).is_empty(), "airplane off alone first: no line waits for the radios or the remembered devices");
        l = conn_list_add(Some(l), &Event::Radio(RadioType::Wifi, true), at(100), air);
        l = conn_list_add(Some(l), &Event::Radio(RadioType::Bluetooth, true), at(120), air);
        assert_eq!(states(&l), [(wifi_on, Done), (bt_on, Done), (net, Wait), ("VPN".to_string(), Todo)]);
        assert!(l.hushes_devices(at(500)), "devices that come back while it shows are not told");
        l = conn_list_add(Some(l), &Event::Net(Net::Online { name: "W".into(), wifi: true }), at(2000), air);
        l = conn_list_add(Some(l), &Event::Vpn(true), at(2500), air);
        assert!(!l.pending() && l.hushes_devices(at(2500 + ALERT_MS as u64 - 1)));
        assert!(!l.hushes_devices(at(2500 + ALERT_MS as u64)), "once it is gone a device has its own banner again");
        let bt = conn_list_add(None, &Event::Radio(RadioType::Bluetooth, true), t0, Expect::default());
        assert!(!bt.hushes_devices(t0), "only the airplane banner keeps the devices out");
    }

    #[test]
    fn many_devices_back_after_airplane_mode_are_left_out_while_it_shows_then_one_list() {
        use connectivity::DeviceKind::{Headphones, Keyboard, Mouse};
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let air = Expect { vpn: true, devices: &[] };
        let dev = |n: &str, k| Event::Device { name: n.into(), kind: k, up: true, battery: Some(70) };
        let names = [("Tai nghe", Headphones), ("Chuột", Mouse), ("Bàn phím", Keyboard), ("Loa", Headphones), ("Bút", Mouse), ("Tay cầm", Mouse)];
        let mut l = conn_list_add(None, &Event::Airplane(false), t0, air);
        for (ms, e) in [(1000, Event::Radio(RadioType::Wifi, true)), (1300, Event::Radio(RadioType::Bluetooth, true))] {
            l = conn_list_add(Some(l), &e, at(ms), air);
        }
        let rows_before = l.rows_shown().len();
        for i in 0..3 {
            assert!(l.hushes_devices(at(2000 + 1000 * i)), "a device that comes back while the airplane banner shows is left out");
        }
        assert_eq!(l.rows_shown().len(), rows_before, "the airplane list does not grow with the devices");
        for (ms, e) in [(6000, Event::Net(Net::Online { name: "W".into(), wifi: true })), (6300, Event::Vpn(true))] {
            l = conn_list_add(Some(l), &e, at(ms), air);
        }
        assert!(!l.pending());
        let end = at(6300 + ALERT_MS as u64);
        assert!(!l.hushes_devices(end), "once the airplane banner is over the devices are told again");
        let mut next: Option<ConnList> = None;
        for (i, (name, kind)) in names.iter().enumerate() {
            next = Some(conn_list_add(next.take(), &dev(name, *kind), end + Duration::from_millis(1000 * i as u64), Expect::default()));
        }
        let list = next.unwrap();
        assert_eq!((list.domain, list.devices_head, list.devices), (Domain::Bt, true, names.len()), "six devices in a row are one list that counts them");
        assert!(list.rows_shown().len() <= MAX_ROWS);
        assert_eq!(list.head.title, devices_title(names.len()));
    }

    #[test]
    fn airplane_off_is_one_list_whatever_order_the_events_come_in() {
        use connectivity::DeviceKind::{Gamepad, Headphones, Keyboard, Mouse};
        let t0 = Instant::now();
        let known = vec![("Tai nghe".to_string(), Headphones), ("Chuột".to_string(), Mouse), ("Bàn phím".to_string(), Keyboard), ("Tay cầm".to_string(), Gamepad)];
        let expect = Expect { vpn: true, devices: &known };
        let dev = |n: &str, k| Event::Device { name: n.into(), kind: k, up: true, battery: Some(70) };
        let online = Event::Net(Net::Online { name: "W".into(), wifi: true });
        let wifi = || Event::Radio(RadioType::Wifi, true);
        let bt = || Event::Radio(RadioType::Bluetooth, true);
        let orders: Vec<Vec<Event>> = vec![
            vec![Event::Airplane(false), wifi(), bt(), dev("Tai nghe", Headphones), dev("Chuột", Mouse), dev("Bàn phím", Keyboard), dev("Tay cầm", Gamepad), online.clone(), Event::Vpn(true)],
            vec![dev("Tai nghe", Headphones), dev("Chuột", Mouse), bt(), wifi(), Event::Airplane(false), dev("Bàn phím", Keyboard), dev("Tay cầm", Gamepad), online.clone(), Event::Vpn(true)],
            vec![bt(), dev("Tai nghe", Headphones), Event::Airplane(false), dev("Chuột", Mouse), dev("Bàn phím", Keyboard), dev("Tay cầm", Gamepad), wifi(), online.clone(), Event::Vpn(true)],
            vec![dev("Tai nghe", Headphones), dev("Chuột", Mouse), Event::Airplane(false), wifi(), bt(), dev("Bàn phím", Keyboard), dev("Tay cầm", Gamepad), online.clone(), Event::Vpn(true)],
            vec![wifi(), dev("Tai nghe", Headphones), dev("Chuột", Mouse), Event::Airplane(false), bt(), dev("Bàn phím", Keyboard), dev("Tay cầm", Gamepad), online, Event::Vpn(true)],
        ];
        for (n, events) in orders.iter().enumerate() {
            let mut l: Option<ConnList> = None;
            for (i, e) in events.iter().enumerate() {
                let now = t0 + Duration::from_millis(100 * i as u64);
                if matches!(e, Event::Device { .. }) && l.as_ref().is_some_and(|l| l.hushes_devices(now)) {
                    continue;
                }
                l = Some(conn_list_add(l.take(), e, now, expect));
            }
            let l = l.unwrap();
            let rows = l.rows_shown();
            assert_eq!(l.head.icon, Some(render::ICON_AIRPLANE), "order {n}");
            let texts: Vec<String> = rows.iter().map(|r| r.text.clone()).collect();
            let expected = [
                tr("Wi-Fi bật", "Wi-Fi on").to_string(),
                tr("Bluetooth bật", "Bluetooth on").to_string(),
                format!("{} W", tr("Đã kết nối", "Connected to")),
                tr("VPN kết nối", "VPN connected").to_string(),
            ];
            assert_eq!(texts, expected, "order {n}");
            assert!(rows.iter().all(|r| r.state == RowState::Done && r.icon.is_none()) && !l.pending(), "order {n}: {rows:?}");
        }
    }

    #[test]
    fn wifi_bluetooth_and_airplane_each_have_their_own_banner_and_airplane_takes_the_others() {
        use connectivity::DeviceKind::Mouse;
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let none = Expect::default();
        let dev = Event::Device { name: "Chuột".into(), kind: Mouse, up: true, battery: Some(64) };
        let wifi = conn_list_add(None, &Event::Radio(RadioType::Wifi, false), t0, none);
        let wifi = conn_list_add(Some(wifi), &Event::Vpn(false), at(100), none);
        assert_eq!((wifi.domain, wifi.rows.len()), (Domain::Net, 1));
        let bt = conn_list_add(Some(wifi), &Event::Radio(RadioType::Bluetooth, true), at(200), none);
        assert_eq!((bt.domain, bt.head.icon, bt.rows_shown().len()), (Domain::Bt, Some(render::ICON_BT), 0));
        let wifi_on = conn_list_add(Some(bt), &Event::Radio(RadioType::Wifi, true), at(300), none);
        assert_eq!(wifi_on.domain, Domain::Net);
        let one = conn_list_add(None, &dev, t0, none);
        assert!(one.single && one.head.badge.as_deref() == Some("64%"));
        let next = conn_list_add(Some(one), &Event::Net(Net::Online { name: "W".into(), wifi: true }), at(100), none);
        assert_eq!((next.domain, next.rows.len(), next.single), (Domain::Net, 0, false));
        let mut air = conn_list_add(None, &Event::Airplane(true), t0, none);
        for e in [Event::Radio(RadioType::Wifi, false), Event::Radio(RadioType::Bluetooth, false), Event::Vpn(false)] {
            air = conn_list_add(Some(air), &e, at(100), none);
        }
        assert_eq!((air.domain, air.rows_shown().len()), (Domain::Air, 3));
    }

    #[test]
    fn one_bluetooth_device_is_the_banner_with_the_battery_large_two_or_more_a_list() {
        use connectivity::DeviceKind::{Headphones, Mouse};
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let one = vec![("UGREEN".to_string(), Headphones)];
        let two = vec![("UGREEN".to_string(), Headphones), ("Chuột".to_string(), Mouse)];
        let dev = |n: &str, k, b| Event::Device { name: n.into(), kind: k, up: true, battery: b };
        let e1 = Expect { devices: &one, ..Expect::default() };
        let l = conn_list_add(None, &Event::Radio(RadioType::Bluetooth, true), t0, e1);
        assert!(!l.pending() && l.rows_shown().is_empty());
        let l = conn_list_add(Some(l), &dev("UGREEN", Headphones, Some(80)), at(1000), e1);
        assert!(l.single && l.head.title == "UGREEN" && l.head.badge.as_deref() == Some("80%") && l.rows_shown().is_empty());
        let mut l = conn_list_add(None, &Event::Radio(RadioType::Bluetooth, true), t0, e1);
        l = conn_list_add(Some(l), &dev("UGREEN", Headphones, None), at(1000), e1);
        assert!(device_battery(&mut l, "UGREEN", 80, at(1500)) && l.head.badge.as_deref() == Some("80%"));
        let e2 = Expect { devices: &two, ..Expect::default() };
        let mut l = conn_list_add(None, &Event::Radio(RadioType::Bluetooth, true), t0, e2);
        assert!(l.pending() && l.rows_shown().len() == 2);
        l = conn_list_add(Some(l), &dev("UGREEN", Headphones, Some(80)), at(1000), e2);
        assert!(!l.single && l.rows_shown().iter().filter(|r| r.state == RowState::Done).count() == 1);
    }

    #[test]
    fn switched_off_and_on_again_quickly_is_just_on() {
        use RowState::*;
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let vpn = Expect { vpn: true, ..Expect::default() };
        let shown = |l: &ConnList| l.rows_shown().into_iter().map(|r| (r.text, r.state)).collect::<Vec<_>>();
        let mut l = conn_list_add(None, &Event::Radio(RadioType::Bluetooth, false), t0, Expect::default());
        l = conn_list_add(Some(l), &Event::Radio(RadioType::Bluetooth, true), at(300), Expect::default());
        assert_eq!(l.head.title, format!("{} Bluetooth", tr("Đã bật", "Turned on")));
        assert!(l.rows_shown().is_empty());
        let mut l = conn_list_add(None, &Event::Radio(RadioType::Wifi, false), t0, vpn);
        l = conn_list_add(Some(l), &Event::Vpn(false), at(100), vpn);
        l = conn_list_add(Some(l), &Event::Radio(RadioType::Wifi, true), at(400), vpn);
        assert_eq!(l.head.title, format!("{} Wi-Fi", tr("Đã bật", "Turned on")));
        assert_eq!(shown(&l), [(tr("Kết nối mạng", "Network connection").to_string(), Wait), ("VPN".to_string(), Todo)]);
        l = conn_list_add(Some(l), &Event::Net(Net::Online { name: "W".into(), wifi: true }), at(2000), vpn);
        l = conn_list_add(Some(l), &Event::Vpn(true), at(3000), vpn);
        assert!(!l.pending() && shown(&l).iter().all(|(_, s)| *s == Done));
        let mut l = conn_list_add(None, &Event::Net(Net::Offline), t0, vpn);
        for (ms, e) in [(100, Event::Vpn(false)), (200, Event::Radio(RadioType::Wifi, false)), (300, Event::Radio(RadioType::Wifi, true))] {
            l = conn_list_add(Some(l), &e, at(ms), vpn);
        }
        assert_eq!(l.head.title, format!("{} Wi-Fi", tr("Đã bật", "Turned on")));
        assert_eq!(shown(&l), [(tr("Kết nối mạng", "Network connection").to_string(), Wait), ("VPN".to_string(), Todo)]);
        let mut l = conn_list_add(None, &Event::Radio(RadioType::Wifi, true), t0, vpn);
        l = conn_list_add(Some(l), &Event::Net(Net::Online { name: "W".into(), wifi: true }), at(1000), vpn);
        l = conn_list_add(Some(l), &Event::Radio(RadioType::Wifi, false), at(2000), vpn);
        assert_eq!(l.head.title, format!("{} Wi-Fi", tr("Đã tắt", "Turned off")));
        assert!(l.rows_shown().is_empty());
    }

    #[test]
    fn a_radio_switched_after_airplane_mode_is_done_has_its_own_banner() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let vpn = Expect { vpn: true, ..Expect::default() };
        let mut l = conn_list_add(None, &Event::Airplane(false), t0, vpn);
        for (ms, e) in [(100, Event::Radio(RadioType::Wifi, true)), (200, Event::Radio(RadioType::Bluetooth, true)), (1000, Event::Net(Net::Online { name: "W".into(), wifi: true })), (1500, Event::Vpn(true))] {
            l = conn_list_add(Some(l), &e, at(ms), vpn);
        }
        assert!(!l.pending() && l.domain == Domain::Air);
        let w = conn_list_add(Some(l), &Event::Radio(RadioType::Wifi, false), at(2000), vpn);
        assert_eq!((w.domain, w.head.title.clone()), (Domain::Net, format!("{} Wi-Fi", tr("Đã tắt", "Turned off"))));
        let b = conn_list_add(Some(w), &Event::Radio(RadioType::Bluetooth, false), at(2200), vpn);
        assert_eq!(b.domain, Domain::Bt);
        let l = conn_list_add(None, &Event::Airplane(false), t0, vpn);
        let l = conn_list_add(Some(l), &Event::Radio(RadioType::Wifi, true), at(100), vpn);
        assert_eq!(l.domain, Domain::Air);
    }

    #[test]
    fn offline_and_vpn_off_told_late_while_wifi_comes_on_leave_no_old_lines() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let vpn = Expect { vpn: true, ..Expect::default() };
        let online = Event::Net(Net::Online { name: "W".into(), wifi: true });
        let mut l = conn_list_add(None, &Event::Radio(RadioType::Wifi, true), t0, vpn);
        for (ms, e) in [(100, Event::Net(Net::Offline)), (200, Event::Vpn(false)), (3000, online.clone()), (4000, Event::Vpn(true))] {
            l = conn_list_add(Some(l), &e, at(ms), vpn);
        }
        let texts: Vec<String> = l.rows_shown().into_iter().map(|r| r.text).collect();
        assert_eq!(texts, vec![row_text(&online), row_text(&Event::Vpn(true))]);
        let mut l = conn_list_add(None, &Event::Radio(RadioType::Wifi, true), t0, vpn);
        l = conn_list_add(Some(l), &online, at(100), vpn);
        l = conn_list_add(Some(l), &Event::Vpn(true), at(200), vpn);
        l = conn_list_add(Some(l), &Event::Vpn(false), at(300), vpn);
        assert!(l.rows_shown().iter().any(|r| r.text == row_text(&Event::Vpn(false))), "a VPN that really drops after it was up is still told");
    }

    #[test]
    fn a_step_told_again_after_it_is_done_is_not_a_second_line() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let vpn = Expect { vpn: true, ..Expect::default() };
        let online = Event::Net(Net::Online { name: "W".into(), wifi: true });
        for start in [Event::Radio(RadioType::Wifi, true), Event::Airplane(false)] {
            let mut l = conn_list_add(None, &start, t0, vpn);
            for (ms, e) in [(500, &online), (900, &Event::Vpn(true)), (1000, &online), (1100, &Event::Vpn(true)), (1200, &Event::Radio(RadioType::Wifi, true)), (1300, &online)] {
                l = conn_list_add(Some(l), e, at(ms), vpn);
            }
            let texts: Vec<String> = l.rows_shown().into_iter().map(|r| r.text).filter(|t| t.starts_with(tr("Đã kết nối", "Connected to")) || t.contains("VPN")).collect();
            assert_eq!(texts.len(), 2, "{texts:?}");
        }
    }

    #[test]
    fn airplane_mode_on_tells_the_vpn_off_even_when_it_was_down_already() {
        let t0 = Instant::now();
        let vpn = Expect { vpn: true, ..Expect::default() };
        let texts = |l: &ConnList| l.rows_shown().into_iter().map(|r| r.text).collect::<Vec<_>>();
        let vpn_off = row_text(&Event::Vpn(false));
        let mut l = conn_list_add(None, &Event::Airplane(false), t0, vpn);
        l = conn_list_add(Some(l), &Event::Airplane(true), t0 + Duration::from_millis(300), vpn);
        assert_eq!(texts(&l).iter().filter(|t| **t == vpn_off).count(), 1);
        l = conn_list_add(Some(l), &Event::Vpn(false), t0 + Duration::from_millis(500), vpn);
        assert_eq!(texts(&l).iter().filter(|t| **t == vpn_off).count(), 1);
        let l = conn_list_add(None, &Event::Airplane(true), t0, Expect::default());
        assert!(!texts(&l).contains(&vpn_off));
    }

    #[test]
    fn a_vpn_is_expected_back_only_when_it_went_down_with_the_network() {
        let t0 = Instant::now();
        let vpn_up = Event::Vpn(true);
        assert!(vpn_expected(false, None, &vpn_up, None, t0), "it came up: expected from now on");
        let wifi_off = conn_list_add(None, &Event::Radio(RadioType::Wifi, false), t0, Expect { vpn: true, ..Expect::default() });
        assert!(vpn_expected(true, None, &Event::Vpn(false), Some(&wifi_off), t0));
        assert!(!vpn_expected(true, None, &Event::Vpn(false), None, t0));
        let other = conn_list_add(None, &Event::Net(Net::Offline), t0, Expect { vpn: true, ..Expect::default() });
        assert!(!vpn_expected(true, None, &Event::Vpn(false), Some(&other), t0));
        assert!(vpn_expected(true, None, &Event::Radio(RadioType::Wifi, true), None, t0), "other events change nothing");
        assert!(vpn_expected(false, Some(t0), &Event::Radio(RadioType::Wifi, false), None, t0 + Duration::from_secs(1)));
        assert!(!vpn_expected(false, Some(t0), &Event::Radio(RadioType::Wifi, false), None, t0 + Duration::from_secs(10)));
    }

    #[test]
    fn bluetooth_devices_are_one_banner_that_becomes_a_list() {
        use connectivity::DeviceKind::{Headphones, Mouse};
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let dev = |name: &str, kind, battery| Event::Device { name: name.into(), kind, up: true, battery };
        let mut l = conn_list_add(None, &dev("WH-1000XM5", Headphones, None), t0, Expect::default());
        assert_eq!((l.head.title.as_str(), l.head.badge.clone(), l.rows.len()), ("WH-1000XM5", None, 0));
        assert!(device_battery(&mut l, "WH-1000XM5", 80, at(700)));
        assert_eq!(l.head.badge.as_deref(), Some("80%"));
        let l = conn_list_add(Some(l), &dev("MX Master 3", Mouse, Some(64)), at(2800), Expect::default());
        assert_eq!(l.head.title, devices_title(2));
        let lines: Vec<_> = l.rows.iter().map(|r| (r.text.as_str(), r.value.as_deref(), r.icon.is_some())).collect();
        assert_eq!(lines, [("WH-1000XM5", Some("80%"), true), ("MX Master 3", Some("64%"), true)]);
        let mut l = l;
        assert!(device_battery(&mut l, "MX Master 3", 18, at(3000)) && l.rows[1].low && !l.rows[0].low);
        assert!(!device_battery(&mut l, "Tay cầm", 50, at(3100)));
        assert!(!device_battery(&mut l, "MX Master 3", 18, at(3000 + ALERT_MS as u64 + 1)), "its banner has gone");
        let mut l = conn_list_add(None, &Event::Radio(RadioType::Bluetooth, true), t0, Expect::default());
        for i in 0..8u64 {
            l = conn_list_add(Some(l), &dev(&format!("Thiết bị {i}"), Mouse, Some(50)), at(100 * i), Expect::default());
        }
        assert_eq!((l.head.icon, l.rows.len(), l.devices), (Some(render::ICON_BT), MAX_ROWS, 8));
        let wifi = conn_list_add(None, &Event::Radio(RadioType::Wifi, false), t0, Expect::default());
        let l = conn_list_add(Some(wifi), &dev("Chuột", Mouse, Some(70)), at(200), Expect::default());
        assert_eq!((l.head.title.as_str(), l.rows.len(), l.single), ("Chuột", 0, true));
    }

    #[test]
    fn a_finished_download_opens_its_folder_with_the_file_selected_on_click() {
        let a = activity::LiveActivity { kind: activity::ActivityKind::Download, title: "model.gguf".into(), detail: "1 MB".into(), progress: None, result: Some(true), file: Some(r"E:\Download\model.gguf".into()), more: 0 };
        assert_eq!(activity_toast(&a, true).open.as_deref(), Some(r"select:E:\Download\model.gguf"));
        assert_eq!(activity_toast(&a, false).open, None, "a running download opens nothing");
    }

    #[test]
    fn a_download_banner_names_the_file_and_ends_with_a_tick() {
        let mut a = activity::LiveActivity { kind: activity::ActivityKind::Download, title: String::new(), detail: "196.6 MB".into(), progress: None, result: None, file: None, more: 0 };
        assert_eq!(activity_toast(&a, false).title, tr("Đang tải xuống", "Downloading"));
        a.title = "model.gguf".into();
        assert_eq!(activity_toast(&a, false).title, format!("{}: model.gguf", tr("Đang tải xuống", "Downloading")));
        assert_eq!(activity_toast(&a, true).icon, Some(render::ICON_DOWNLOAD), "vanished without a file: no tick");
        a.result = Some(true);
        let done = activity_toast(&a, true);
        assert_eq!((done.title.as_str(), done.icon), (tr("Tải xuống hoàn tất", "Download complete"), Some(render::ICON_DONE)));
        assert_eq!(done.body, "model.gguf · 196.6 MB");
    }

    use super::*;

    #[test]
    fn seek_drag_previews_clamps_and_cancels_when_the_track_changes() {
        let track = music::Track { source_id: "player".into(), title: "Song".into(), art_key: 7,
            duration_ms: 120_000, can_seek: true, playing: true, ..Default::default() };
        let mut drag = SeekDrag::new(&track).unwrap();
        for wide in [false, true] {
            for scale in [1.0, 1.5, 2.0] {
                let cx = 410.0 * scale;
                let bar = render::seek_bar(cx, scale, wide);
                drag.update(cx, scale, (bar.left + bar.right) / 2.0, wide);
                assert_eq!(drag.track.position_ms(), 60_000);
                assert!(drag.track.playing && drag.track.at.is_none());
                drag.update(cx, scale, bar.left - 100.0, wide);
                assert_eq!(drag.track.position_ms(), 0);
                drag.update(cx, scale, bar.right + 100.0, wide);
                assert_eq!(drag.track.position_ms(), 120_000);
            }
        }
        assert!(drag.current(Some(&track)));
        let mut changed = track.clone();
        changed.art_key += 1;
        assert!(!drag.current(Some(&changed)));
        changed = track.clone();
        changed.source_id = "other player".into();
        assert!(!drag.current(Some(&changed)));
        changed = track.clone();
        changed.can_seek = false;
        assert!(!drag.current(Some(&changed)));
        assert!(SeekDrag::new(&changed).is_none());
        assert!(!drag.current(None));
    }

    #[test]
    fn about_text_names_the_app_its_version_and_where_to_get_it() {
        for text in [about_text("1.0.10")] {
            assert!(text.starts_with("PUW Notch 1.0.10\n"), "{text}");
            assert!(text.contains(REPO_URL) && text.contains("Rust"), "{text}");
            assert!(!text.contains("Nusa"), "{text}");
        }
        assert_ne!(about_text("1.0.10"), about_text("1.0.11"));
    }

    #[test]
    fn destroy_posts_quit_while_app_is_borrowed() {
        unsafe {
            let instance: HINSTANCE = GetModuleHandleW(None).unwrap().into();
            let class = w!("PuwNotchShutdownTest");
            let wc = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(wndproc),
                hInstance: instance,
                lpszClassName: class,
                ..Default::default()
            };
            assert_ne!(RegisterClassExW(&wc), 0);
            let hwnd = CreateWindowExW(WINDOW_EX_STYLE::default(), class, w!(""),
                WS_POPUP, 0, 0, 1, 1, None, None, Some(instance), None).unwrap();
            APP.with(|app| {
                let _busy = app.borrow_mut();
                DestroyWindow(hwnd).unwrap();
            });
            assert!(!IsWindow(Some(hwnd)).as_bool());
            let mut msg = MSG::default();
            let quit = PeekMessageW(&mut msg, None, WM_QUIT, WM_QUIT, PM_REMOVE).as_bool();
            UnregisterClassW(class, Some(instance)).unwrap();
            assert!(quit, "destroying the window must stop the message loop");
            assert_eq!(msg.wParam.0, 0);
        }
    }
}

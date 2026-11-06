#![windows_subsystem = "windows"]

mod anim;
mod antigravity;
mod approve;
mod audio;
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
use notify::{Row, Toast};
use weather::WM_WEATHER;
use focus::Focus;
use render::{Button, Card, HealthView, ListView, Page, Renderer, View, LIST_ROWS, NOTE_ROWS, PAGES};

const WM_TRAY: u32 = WM_APP + 1;
const WM_FRAME: u32 = WM_APP + 7;
/// Lives in the commctrl headers; not worth a crate feature for one constant.
const WM_MOUSELEAVE: u32 = 0x02A3;
const T_ANIM: usize = 1;
const T_CHECK: usize = 2;
const T_DWELL: usize = 3;
const T_ALERT: usize = 4;
const T_CARET: usize = 6;
/// The foreground window changed or moved: look again once it has had a moment to settle.
const T_SETTLE: usize = 7;
/// Network banners queue: each stays at least this long before the next one in line replaces it.
const T_CONN: usize = 8;
const CONN_MS: u32 = 1500;
const SETTLE_MS: u32 = 120;
/// Page switch morph length.
/// Notifications kept on the Notifications page.
const NOTES_MAX: usize = 30;
const CARET_MS: u32 = 530;
const CHECK_MS: u32 = 500;
/// A live activity's banner lasts this long past the last tick that renewed it.
const ACTIVITY_MS: u32 = 1500;
/// A pill that has rested this long (5 s) gives its pages back to Windows: Task Manager then shows
/// a few MB instead of what the last panel touched. They come back on demand. (The other notch app
/// trims as it enters its resting state, too.)
const TRIM_TICKS: u32 = 10;
/// When `tick` last ran (unix ms) and where the notch was then (mode * 16 + page): the watchdog
/// writes to the log when the window stops answering, which the log could not say before.
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
/// The page choices of the "Pages" submenu: this + the page's index in `PAGES`.
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
    /// Window placement in device px: x, y, w, h.
    win: (i32, i32, i32, i32),
    hidden: bool,
    tracking: bool,
    /// Cursor reached the pill body this visit (ears alone do not count, see `notch::hover_area`).
    hover_in: bool,
    animating: bool,
    last_frame: Instant,
    taskbar_created: u32,
    time: String,
    /// Short date for the panel header.
    date: String,
    battery: Option<system::Battery>,
    notifications: Option<notify::Reader>,
    /// What the Alert banner shows right now.
    banner: Option<Toast>,
    /// When the banner appeared.
    banner_at: Instant,
    /// The lyrics button was pressed in this visit of the open panel.
    lyrics_open: bool,
    /// The 24-hour forecast shows in place of the details (Weather page).
    /// The last USB change shown (device, change, when), to drop a repeat of it.
    usb_last: Option<(String, usb::Change, Instant)>,
    /// Last brightness Windows reported; the first report (sent on registering) is not a change.
    last_brightness: Option<u8>,
    audio: Option<audio::Audio>,
    mic_muted: Option<bool>,
    /// Ticks so far, to look at the default audio devices every few.
    audio_ticks: u8,
    music: music::Music,
    /// The Limits page's readings, what was already announced, alerts waiting for a free moment,
    /// and the app tab (followed by itself until the user picks one).
    usage: usage::Monitor,
    /// Approval requests from other apps (see approve.rs) and the one on the banner now.
    approve: approve::Server,
    approval: Option<approve::Pending>,
    usage_watch: usage::Watch,
    usage_alerts: Vec<usage::Alert>,
    /// Network banners waiting their turn, in the order the events came; `conn_busy` while one holds the slot.
    conn_queue: Vec<Toast>,
    conn_busy: bool,
    /// The checklist banner of the network events that came together, while it still shows.
    conn_list: Option<ConnList>,
    usage_tab: usize,
    usage_tab_chosen: bool,
    track: Option<music::Track>,
    seek_drag: Option<SeekDrag>,
    /// Idle robot eyes, the tray switch for them and when they last moved.
    robot: robot::Robot,
    robot_on: bool,
    /// Ears (v1) or floating pill (v2), see `notch::Style`.
    style: notch::Style,
    robot_at: Instant,
    /// When the current track was paused (None while playing or with no player).
    paused_since: Option<Instant>,
    started: Instant,
    /// Frame pacing: VSYNC while something moves, T_ANIM at BARS_FRAME_MS for the equaliser alone, 0 = idle.
    frame_ms: u32,
    vsync: Vsync,
    hovered_button: Option<Button>,
    /// Foreground window + rect at the last check: busy sampling waits for it to hold still.
    last_foreground: Option<(isize, RECT)>,
    /// Ticks the pill has rested without a move; at `TRIM_TICKS` the working set is handed back.
    rested: u32,
    /// Whether the log last said the panel is open (so it says it once per change).
    logged_open: bool,
    last_gap_log: Option<Instant>,
    last_slow_tick: Option<Instant>,
    /// The row of page icons is open under the page dots.
    nav_open: bool,
    page: Page,
    todo: todo::Todo,
    today: u32,
    /// Text in the add box; kept if the box loses focus.
    draft: String,
    /// The add box has the keyboard (window activated for it).
    editing: bool,
    /// Task being edited in the box; None = the box adds.
    edit_item: Option<usize>,
    caret: bool,
    todo_scroll: usize,
    /// Notifications page: what Windows still keeps, newest first, and its first visible row.
    notes: Vec<notify::Note>,
    note_scroll: usize,
    /// Where the Notifications list is drawn, in rows: eases toward `note_scroll`.
    note_pos: f32,
    health: health::Health,
    activity: activity::Watch,
    focus: Focus,
    /// Health card showing its settings.
    settings_open: Option<Card>,
    /// Page switch in progress: (page leaving, start, direction).
    page_anim: Option<(Page, Instant, i32)>,
    icons_morphing: bool,
    active_page: Option<Page>,
    /// Window that had the keyboard before typing started; it gets it back after.
    prev_foreground: HWND,
    /// First half of a UTF-16 pair arriving in WM_CHAR (emoji).
    high_surrogate: Option<u16>,
    weather: weather::Weather,
    /// The weather as of the last WM_WEATHER (city and current conditions).
    wx: weather::State,
    /// Text in the Weather page's city box, apart from the tasks' draft.
    city_draft: String,
    /// A city typed in the box is being looked up: the box keeps its text until the answer says it
    /// was found (a typo can then be mended instead of retyped).
    awaiting_city: bool,
    /// When the open panel or its page appeared (the Weather page's icon moves from then).
    page_since: Instant,
    /// °C or °F, and whether it was chosen (else it follows the region setting and is not saved).
    unit: weather::Unit,
    /// The Weather page shows the sun's path in place of the hours ahead.
    sun_view: bool,
    unit_chosen: bool,
}

/// Animation frame intervals.
/// `frame_ms` value for frames paced by the display (see `Vsync`).
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
            // The notification icons load as soon as the panel opens (on any page), so the Notifications
            // page already has them when it slides in.
            let notes = page.is_some() && render::page_shown(Page::Notify);
            self.renderer.set_page_open(notes);
            self.usage.set_page_open(page == Some(Page::Usage));
            if notes { self.refresh_notes(); }
        }
    }

    fn redraw(&mut self) {
        self.sync_page_work();
        if self.hidden && self.notch.is_hidden() { return; }
        // The page's own motion plays from the moment the panel opens.
        if self.notch.mode != Mode::Expanded {
            self.page_since = Instant::now();
        }
        let eyes = self.robot_eyes();
        let (x, y, w, h) = self.win;
        let now = system::now_stamp();
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
            unit: self.unit, sun_view: self.sun_view,
        };
        // An icon that just changed keeps frames coming until its morph has played.
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

    /// The equaliser in the resting pill keeps frames coming while music plays, and so does the
    /// aurora on the open Music page; so do the idle robot's eyes while they show or fade.
    fn bars_active(&self) -> bool {
        let playing = self.track.as_ref().is_some_and(|t| t.playing);
        let bars = playing && matches!(self.notch.mode, Mode::Collapsed | Mode::Peek);
        let aurora = playing && self.notch.mode == Mode::Expanded && self.page == Page::Music;
        !self.hidden && (bars || aurora || self.robot.visible())
    }

    fn settings(&self) -> system::Settings {
        system::Settings { pages: render::pages_on(), minimal: self.notch.minimal, english: i18n::english(), robot: self.robot_on, volume_keys: status::taking_volume_keys(), style: self.style, approve: self.approve.is_enabled(), place: self.wx.place.clone(), unit: self.unit_chosen.then_some(self.unit), sun_view: self.sun_view, notes_cleared: self.notifications.as_ref().map_or(0, |r| r.cleared), activity: self.activity.enabled }
    }

    /// Idle robot: shown only on the plain resting pill (no music, banner, open panel, mini or
    /// fullscreen game), from the time since the last key or mouse input.
    fn refresh_robot(&mut self) {
        let allowed = self.robot_on && !self.hidden && self.notch.mode == Mode::Collapsed && !self.notch.live;
        if self.robot.update(idle_seconds(), allowed) || (self.robot.visible() && !self.animating) {
            self.kick();
        }
    }

    /// The robot's eyes for this frame (None while it is away), with the cursor relative to the
    /// pill centre for the glance.
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
            // Across the half width of the monitor and down its height, so the eyes follow the cursor over the whole screen.
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
        // A frame that came this late while moving is what the eye reads as a stutter: leave a line
        // (at most one every 10 s) so a report of one says how late and in which mode.
        // The equaliser alone comes by timer every BARS_FRAME_MS: a few timer steps of slack are not a stutter.
        let limit = if self.frame_ms == BARS_FRAME_MS { 0.125 } else { 0.08 };
        if gap > limit && self.animating && self.last_gap_log.is_none_or(|t| t.elapsed().as_secs() >= 10) {
            self.last_gap_log = Some(now);
            system::log(&format!("late frame: {:.0} ms (mode {:?}, page {})", gap * 1000.0, self.notch.mode, self.page.name()));
        }
        // The lyrics and the 24-hour forecast belong to one visit of the open panel.
        if self.notch.mode != Mode::Expanded {
            self.lyrics_open = false;
        }
        self.notch.set_wide(self.lyrics_wide());
        // The row of page icons only lives while the panel is open; it makes the panel a little taller.
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
        // The Notifications list glides to the row the wheel asked for.
        let goal = self.note_scroll as f32;
        if self.note_pos != goal {
            self.note_pos = anim::follow(self.note_pos, goal, dt, anim::LIST_GLIDE_RATE);
            if (goal - self.note_pos).abs() < 0.01 {
                self.note_pos = goal;
            }
            moving = true;
        }
        // The page morph runs on the same frames; drop it once it has played out.
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
        }
    }

    /// Pull the latest now-playing snapshot; music starting/stopping resizes the pill. A paused
    /// player keeps the pill for a few seconds (`music::shows_pill`); the panel's Music page keeps it.
    fn refresh_music(&mut self) {
        self.track = self.music.snapshot().filter(|_| render::page_shown(Page::Music));
        if self.seek_drag.as_ref().is_some_and(|drag| !drag.current(self.track.as_ref())) {
            self.cancel_seek();
        }
        let playing = self.track.as_ref().is_some_and(|t| t.playing);
        self.paused_since = if self.track.is_some() && !playing { Some(self.paused_since.unwrap_or_else(Instant::now)) } else { None };
        let live = music::shows_pill(playing, self.paused_since.map(|t| t.elapsed()));
        // Frames already coming (the equaliser's timer) need no kick: it would switch them to the screen's
        // rhythm and back, and restart their timer, at every tick.
        if self.notch.set_live(live) || (self.bars_active() && !self.animating) {
            self.kick();
        }
    }

    /// Device px between the screen's top edge and the top of the drawn notch (a floating pill
    /// sits lower in the window): mouse y is measured from the notch, not from the window.
    fn top_px(&self) -> f32 {
        (self.style.gap_at(self.notch.size()[1]) * self.scale).round()
    }

    /// Is the cursor over the area that reacts to the mouse right now (`notch::hover_area`)?
    /// Open or close the row of page icons under the page dots.
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

    /// Keep the speaker and mic callbacks on the default devices (a few seconds late is fine, so not on
    /// every tick); the mic's state is read once when it is found, `WM_MIC` reports the rest.
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

    /// The default mic was muted or unmuted, from anywhere (laptop mic key, Windows settings, a call app).
    fn mic_changed(&mut self, muted: bool) {
        if self.mic_muted.replace(muted) == Some(muted) {
            return;
        }
        let (title, icon) = if muted { (tr("Đã tắt mic", "Mic off"), render::ICON_MIC_OFF) } else { (tr("Đã bật mic", "Mic on"), render::ICON_MIC) };
        self.show_banner(status_toast("Mic", title.to_string(), "", None, icon), STATUS_MS);
    }

    /// A USB device came or went: a banner with its name (once per plug, none for root hubs).
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

    /// Let go of a panel the mouse has left without Windows saying so (see `notch::hover_stale`).
    fn release_stale_hover(&mut self) {
        // A seek drag ends with the button: one that outlived it (the release went elsewhere) would
        // hold the panel open for good.
        if self.seek_drag.is_some() && unsafe { GetAsyncKeyState(VK_LBUTTON.0 as i32) } >= 0 {
            self.cancel_seek();
        }
        // Only the pages with a text box are held open by typing.
        let typing = self.editing && matches!(self.page, Page::Todo | Page::Weather);
        if self.seek_drag.is_some() || !notch::hover_stale(self.notch.mode, self.hover_in, typing, self.cursor_inside()) {
            return;
        }
        self.hover_in = false;
        // Ask Windows for the next leave again: the old request may be gone.
        self.tracking = false;
        self.hovered_button = None;
        unsafe { let _ = KillTimer(Some(self.hwnd), T_DWELL); }
        self.notch.on_leave();
        self.kick();
    }

    /// After a fullscreen / immersive app (Photos) Windows can leave a topmost window stacked under
    /// ordinary ones with its style intact: stack it on top again.
    fn keep_on_top(&self) {
        if !self.hidden && screen::buried(self.hwnd) {
            unsafe { let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE); }
        }
    }

    /// Tells the watchdog the window still answers, and leaves a line in the log when the panel
    /// opens or closes: a panel that "does not retract" then shows in the log with its page.
    fn beat(&mut self) {
        BEAT.store(unix_ms(), Ordering::Relaxed);
        WHERE.store(self.notch.mode as u8 * 16 + PAGES.iter().position(|p| *p == self.page).unwrap_or(0) as u8, Ordering::Relaxed);
        let open = self.notch.mode == Mode::Expanded;
        if open != self.logged_open {
            self.logged_open = open;
            system::log(&if open { format!("panel open: {}", self.page.name()) } else { "panel closed".to_string() });
        }
    }

    /// What is under the pill, and the verdict: hide for a fullscreen app, thin line or pill. While the window is
    /// still moving (no verdict yet), or the notch is on its way back to the pill (it takes two quiet checks), the
    /// next look is soon (`T_SETTLE`) instead of at the next tick.
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
                        // A notch that "does not show" may be resting as the thin line: say why.
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
        // Typing needs the keyboard: when the notch is not the foreground window (Windows refused
        // the focus, or took it back without a WM_ACTIVATE) it cannot be typing, and `editing`
        // would keep the open panel from ever closing.
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
        // Progress bar and lyric line move on their own while the panel is open.
        changed |= self.notch.mode == Mode::Expanded && self.track.as_ref().is_some_and(|t| t.playing);
        // The Pomodoro clock and the line along the pill move on their own once a second while it runs.
        changed |= self.focus.running() && self.audio_ticks % 2 == 0;

        // The verdict about what is under the pill only counts while the pill rests, still and alone.
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

        // New ones get a banner; the Notifications page lists what Windows still keeps.
        let new = self.notifications.as_mut().filter(|_| render::page_shown(Page::Notify)).map(|r| r.poll()).unwrap_or_default();
        let showing_notes = self.notch.mode == Mode::Expanded && self.page == Page::Notify;
        // The list is read again when something arrived (and when the page opens), not on every tick.
        if showing_notes && !self.hidden && !new.is_empty() {
            self.refresh_notes();
            changed = true;
        }
        let current = self.activity.poll(Instant::now(), self.hwnd.0 as isize);
        self.activity_banner(current);
        if let Some(t) = new.into_iter().last() {
            // A newer one restarts the 5 s from the start (show_banner re-arms the timer).
            self.show_banner(t, ALERT_MS);
        }

        mark(5, &mut lap);
        // A tick that holds the window up for long is a stutter while something moves: say which part.
        let total: u32 = laps.iter().sum();
        if total > 30_000 && self.last_slow_tick.is_none_or(|t| t.elapsed().as_secs() >= 10) {
            self.last_slow_tick = Some(Instant::now());
            system::log(&format!("slow tick: {} ms (clock+battery {}, audio {}, music+robot {}, screen {}, reminders {}, notifications {} us)", total / 1000, laps[0], laps[1], laps[2], laps[3], laps[4], laps[5]));
        }
        if changed && !self.animating {
            self.redraw();
        }
    }

    /// What clicking the banner on show opens: its picture (a screenshot), else the app or link of
    /// the notification. None for status banners.
    fn banner_target(&self) -> Option<&str> {
        self.banner.as_ref().filter(|_| self.notch.mode == Mode::Alert).and_then(|b| b.image.as_deref().or(b.open.as_deref()))
    }

    /// Short banner on the notch; skipped over a fullscreen game or while the panel is open.
    /// Returns whether it is showing.
    fn show_banner(&mut self, t: Toast, ms: u32) -> bool {
        // Notifications (no status icon) with text grow to fit it, up to BANNER_MAX_LINES lines.
        // One banner at a time speaks for an approval; nothing else replaces it while it waits.
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
        let shown = if t.choices.is_empty() { self.notch.on_alert(lines) } else { self.notch.on_alert_choice(lines) };
        if self.hidden || !shown {
            return false;
        }
        // A status banner without the big figure (battery, Bluetooth) is as wide as its lines.
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

    /// The banner of the running activity, else of the one that just ended.
    fn activity_banner(&mut self, current: Option<activity::LiveActivity>) {
        if let Some(a) = current {
            self.show_activity(activity_toast(&a, false), ACTIVITY_MS);
        } else if let Some(a) = self.activity.take_finished() {
            // The end stays long enough to be read, like a notification.
            self.show_activity(activity_toast(&a, true), ALERT_MS);
        }
    }

    /// The checklist banner: shown at once, and while it shows a new line makes it taller in place
    /// (the dot of the new line pops in) and keeps it on for another full time.
    fn show_connection_list(&mut self, t: Toast) {
        if self.approval.is_some() {
            return;
        }
        if self.notch.mode == Mode::Alert && self.banner.as_ref().is_some_and(|b| b.app == t.app && b.choices.is_empty()) {
            let width = if t.badge.is_none() { self.renderer.status_banner_width(&t.title, &t.body, &t.rows, self.scale) } else { 0.0 };
            self.notch.set_alert_width(width);
            self.notch.set_alert_rows(t.rows.len() as u8);
            self.banner = Some(t);
            unsafe { SetTimer(Some(self.hwnd), T_ALERT, ALERT_MS, None) };
            self.kick();
        } else {
            self.show_banner(t, ALERT_MS);
        }
    }

    /// The next network banner in line, now: the first event shows at once, the ones behind it after
    /// `CONN_MS` each, and the last stays its full time. The slot frees when the timer finds the line empty.
    fn next_connection_banner(&mut self) {
        if self.conn_queue.is_empty() {
            self.conn_busy = false;
            unsafe { let _ = KillTimer(Some(self.hwnd), T_CONN); }
            return;
        }
        let t = self.conn_queue.remove(0);
        self.show_banner(t, ALERT_MS);
        self.conn_busy = true;
        unsafe { SetTimer(Some(self.hwnd), T_CONN, CONN_MS, None) };
    }

    /// The live activity's banner: it stays while the activity runs (each tick renews it and updates its
    /// text) and ends by itself a moment after the last one. Another banner is not pushed aside.
    fn show_activity(&mut self, t: Toast, ms: u32) {
        if self.approval.is_some() {
            return;
        }
        if self.notch.mode != Mode::Alert {
            self.show_banner(t, ms);
        // The same activity's banner is updated in place, also when its icon changes (the arrow
        // becoming the green tick morphs on screen).
        } else if self.banner.as_ref().is_some_and(|b| b.app == t.app && b.icon.is_some() && b.choices.is_empty()) {
            let width = self.renderer.status_banner_width(&t.title, &t.body, &t.rows, self.scale);
            self.notch.set_alert_width(width);
            self.banner = Some(t);
            unsafe { SetTimer(Some(self.hwnd), T_ALERT, ms, None) };
            self.kick();
        }
    }

    /// The next approval request as a banner with Allow / Deny. One that cannot show now (the panel
    /// is open, a fullscreen game) is told to ask as usual.
    fn approve_next(&mut self) {
        if self.approval.is_some() {
            return;
        }
        while let Some(p) = self.approve.next() {
            // Switched off in the tray, or answered in its app while it waited: back to the app.
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

    /// The banner's request gets its answer (a click, or Ask when time ran out); the banner goes.
    fn finish_approval(&mut self, answer: approve::Answer) {
        let Some(p) = self.approval.take() else { return };
        p.answer(answer);
        unsafe { let _ = KillTimer(Some(self.hwnd), T_ALERT); }
        self.notch.on_alert_end();
        self.kick();
        self.approve_next();
    }

    /// The oldest waiting limit alert, as a banner when one can show now (else it waits for the next tick).
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

    /// Task reminders (early and on time) whose moment has come, then water / stand-up. One that
    /// cannot show now (fullscreen game, panel open) rings at the next chance. A banner that shows
    /// switches the panel to its page, so resting on it opens the right one.
    /// A Pomodoro segment that ran out: a banner says so and what comes next; the next one has started by itself (after the long break the cycle waits for a new start).
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
        // A page the user hid does nothing: no reminders, no water, no stand-up.
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

    /// First visible row of the list page on screen.
    fn scroll(&self) -> usize {
        if self.page == Page::Notify { self.note_scroll } else { self.todo_scroll }
    }

    fn rows(&self) -> usize {
        match self.page {
            Page::Todo => self.todo.items.len(),
            Page::Notify => self.notes.len(),
            Page::Weather => usize::from(self.wx.now.is_some()),
            _ => 0,
        }
    }

    /// Re-read the Notifications page from Windows (cleared there = gone here).
    fn refresh_notes(&mut self) {
        if let Some(r) = &self.notifications {
            self.notes = r.recent(NOTES_MAX, system::utc_offset_min());
        }
        self.clamp_scroll();
    }

    fn hit(&self, x: f32, y: f32) -> Option<Button> {
        render::hit_button(self.win.2 as f32 / 2.0, self.scale, x, y, self.track.as_ref(), self.page, self.rows(), self.scroll(), self.settings_open, self.lyrics_wide())
    }

    /// The lyrics show beside the player: asked for on the Music page of the open panel.
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
            Button::Seek => return, // Committed only by the captured press/drag/release gesture.
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
                // The pages the user keeps (there is always one).
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
            // The x shows only while there is text (and no row being edited); over an empty box it is just the box.
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
            // Ticking or deleting reorders the list: an edit in progress would point at the wrong row.
            Button::ListToggle(i) => {
                self.cancel_task_edit();
                self.todo.toggle(i, self.today);
            }
            Button::ListDelete(i) => {
                self.cancel_task_edit();
                self.todo.remove(i);
                self.clamp_scroll();
            }
            // The x beside the city: no city, no weather (the pill goes back to clock and battery).
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
                // The sun starts its walk (and the big icon its pop) again from here.
                self.page_since = Instant::now();
                system::save_settings(self.settings());
            }
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
        }
        self.hovered_button = self.hit(x, y);
        self.redraw();
    }

    fn cancel_task_edit(&mut self) {
        if self.edit_item.take().is_some() {
            self.draft.clear();
        }
    }

    /// The text of the box on the page now: the city on the Weather page, else the task.
    fn draft_mut(&mut self) -> &mut String {
        if self.page == Page::Weather { &mut self.city_draft } else { &mut self.draft }
    }

    /// Enter in the box: on the Weather page look the city up, keeping the text and the keyboard
    /// until it is found (see `awaiting_city`); elsewhere save the edited task, or add a new one,
    /// and stay in the box for the next task.
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
        self.note_scroll = self.note_scroll.min(self.notes.len().saturating_sub(NOTE_ROWS));
    }

    /// Typing needs the keyboard: let the window activate for once, remembering who had it.
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

    /// Stop typing (the draft stays in the box) and hand the keyboard back.
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
        // The mouse may have left while typing kept the panel open.
        if !self.tracking && !self.cursor_inside() {
            self.notch.on_leave();
            self.kick();
        }
    }

    /// One WM_CHAR unit while typing in the add box.
    fn type_char(&mut self, unit: u16) {
        match unit {
            // Enter: save and stay in the box for the next task.
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
            // Which window made it hide: a notch missing after start-up may be hidden, not gone.
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
                    // The last page kept cannot be switched off.
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
                    // A page switched off leaves nothing waiting; one switched on starts fresh.
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
                    // What is on the banner now goes back to its app.
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
                    // Everything shown is rebuilt from the new language on the next frame.
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
                // Menu dismissed: frames missed while it was open start again, and a stale hover is let go.
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
                    // Slid off the body onto an ear before the dwell: entering again needs the body.
                    self.hover_in &= inside;
                    self.notch.on_dwell(inside);
                    self.kick();
                }
                // After a foreground change: look, and again in a moment while the window is still moving or the notch is coming back.
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
                    // Mouse resting on the banner when it ends: treat as a fresh visit.
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
                // The ears report the mouse too (they are drawn); only the pill body counts.
                if !self.hover_in && self.cursor_inside() {
                    self.hover_in = true;
                    // Resting on a banner opens the panel, except a notification's: that one is
                    // there to be clicked (opens its app or picture).
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
                // The page dots open into the row of page icons while the mouse is on them; the row goes
                // when the mouse leaves it.
                if self.notch.mode == Mode::Expanded {
                    let (x, y) = ((lp.0 & 0xffff) as i16 as f32, ((lp.0 >> 16) & 0xffff) as i16 as f32 - self.top_px());
                    if !self.nav_open && hover == Some(Button::Dots) {
                        self.set_nav(true);
                    } else if self.nav_open && !render::in_nav_zone(self.win.2 as f32 / 2.0, self.scale, render::panel_size(self.page)[1], x, y) {
                        self.set_nav(false);
                    }
                }
                // The x of a notification banner lights up under the mouse.
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
                // Typing keeps the panel open; it closes when the box lets go of the keyboard.
                if !self.editing && self.seek_drag.is_none() {
                    self.notch.on_leave();
                }
                self.kick();
            }
            WM_CHAR if self.editing => self.type_char(wp.0 as u16),
            // Lost the keyboard (clicked elsewhere): stop typing, keep the draft.
            WM_ACTIVATE if self.editing && (wp.0 & 0xffff) as u32 == WA_INACTIVE => self.end_edit(),
            // The wheel only scrolls a long to-do list; pages switch with the arrows.
            WM_MOUSEWHEEL if self.notch.mode == Mode::Expanded && matches!(self.page, Page::Todo | Page::Notify) => {
                let delta = ((wp.0 >> 16) & 0xffff) as i16;
                let max = self.rows().saturating_sub(if self.page == Page::Notify { NOTE_ROWS } else { LIST_ROWS });
                let scroll = if self.page == Page::Notify { &mut self.note_scroll } else { &mut self.todo_scroll };
                *scroll = if delta > 0 { scroll.saturating_sub(1) } else { (*scroll + 1).min(max) };
                // The Notifications list eases there frame by frame (see `frame`).
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
                    // Its x: back to the pill now, nothing opens.
                    Some(_) if x >= close.left && x <= close.right && y >= close.top && y <= close.bottom => {
                        unsafe {
                            let _ = KillTimer(Some(self.hwnd), T_ALERT);
                            let _ = KillTimer(Some(self.hwnd), T_DWELL);
                        }
                        self.notch.on_alert_end();
                        self.kick();
                    }
                    // A notification banner: clicking it opens its picture, link or app.
                    Some(target) => open_target(target),
                    None => self.click(x, y),
                }
            }
            WM_SCREENEVENT => unsafe { SetTimer(Some(self.hwnd), T_SETTLE, SETTLE_MS, None); },
            WM_LOCKKEY => {
                let (name, on) = status::lock_state(wp.0);
                self.show_banner(status_toast(tr("Bàn phím", "Keyboard"), name.to_string(), on_off(on), None, render::ICON_KEYBOARD), STATUS_MS);
            }
            // A volume key taken from Windows: apply it here; WM_VOLUME then shows it.
            WM_VOLUMEKEY => {
                if let Some(a) = &self.audio {
                    a.volume_key(wp.0 as u16);
                }
            }
            // From the speaker callback: any change, keys, taskbar slider or an app.
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
            // Charger plugged or unplugged, battery percent changed: read the battery now.
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
                // A fresh sample from the worker: shown now, not at the next tick.
                let current = self.activity.latest(Instant::now());
                self.activity_banner(current);
            }
            render::WM_ICONS => {
                if self.active_page == Some(Page::Notify) || (!self.hidden && self.notch.mode == Mode::Alert) {
                    self.redraw();
                }
            }
            WM_CONN => {
                // The network may just have come back: the weather catches up (a recent one is kept).
                self.weather.refresh();
                for e in connectivity::take_events() {
                    let now = Instant::now();
                    match &e {
                        // A device that goes has its own banner; those wait their turn.
                        Event::Device { up: false, .. } => self.conn_queue.push(connection_toast(e)),
                        Event::DeviceBattery { name, percent, .. } => {
                            if self.conn_list.as_mut().is_some_and(|l| device_battery(l, name, *percent, now)) {
                                let t = self.conn_list.as_ref().map(list_toast);
                                if let Some(t) = t {
                                    self.show_connection_list(t);
                                }
                            } else {
                                // Not on show (a battery that just ran low): a banner of its own.
                                self.conn_queue.push(connection_toast(e));
                            }
                        }
                        // The airplane switch, radios, network, VPN and devices that come together are one banner.
                        _ => {
                            let list = conn_list_add(self.conn_list.take(), &e, now);
                            self.show_connection_list(list_toast(&list));
                            self.conn_list = Some(list);
                        }
                    }
                }
                if !self.conn_busy && !self.conn_queue.is_empty() {
                    self.next_connection_banner();
                }
            }
            // From the limits worker: a reading changed.
            approve::WM_APPROVE => {
                // A question answered in its own app: its banner goes now.
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
                // The tab: the user's choice while that app is still on the PC, else the first with a reading.
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
            // From the weather thread: a city was found (or not) or new conditions came.
            WM_WEATHER => {
                let before = self.wx.place.clone();
                self.wx = self.weather.snapshot();
                if self.wx.place != before {
                    system::save_settings(self.settings());
                }
                // The lookup is over: found, the box is done; not found, it keeps the typo to mend.
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
            // The PC woke up: the old conditions may be hours stale.
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

/// Text of the About box.
fn about_text(version: &str) -> String {
    let what = tr(
        "Notch kiểu Dynamic Island cho Windows: giờ, pin, nhạc, việc cần làm, thông báo, sức khỏe và banner trạng thái. Viết bằng Rust và Direct2D, không dùng WebView.",
        "A Dynamic Island-style notch for Windows: clock, battery, music, tasks, notifications, health and status banners. Written in Rust and Direct2D, no WebView.",
    );
    format!("PUW Notch {version}\n\n{what}\n\n{REPO_URL}")
}

/// yyyymmddHHMM -> minute of the day.
fn minute_of_day(stamp: u64) -> u32 {
    ((stamp / 100 % 100) * 60 + stamp % 100) as u32
}

/// Seconds since the last keyboard / mouse input anywhere (away from the PC -> sitting clock resets).
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

/// The PUW icon embedded by build.rs (resource id 1), at the small (tray) or large size.
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

/// The banner of a live activity (or, `done`, of one that just ended).
fn activity_toast(a: &activity::LiveActivity, done: bool) -> Toast {
    use activity::ActivityKind::*;
    let (app, icon) = match a.kind {
        FileOp => (tr("Tệp", "Files"), render::ICON_COPY),
        Move => (tr("Tệp", "Files"), render::ICON_MOVE),
        Delete => (tr("Tệp", "Files"), render::ICON_TRASH),
        Download => (tr("Tải xuống", "Download"), render::ICON_DOWNLOAD),
        Build => (tr("Build", "Build"), render::ICON_BUILD),
    };
    // A download's title says what happens and names the file once it is known ("" until then).
    let named = |what: &str| if a.title.is_empty() { what.to_string() } else { format!("{what}: {}", a.title) };
    // An ending whose outcome is known: a green check in a ring, or a red cross, takes the icon's place
    // (the banner icon morphs into it).
    match (a.kind, done, a.result) {
        (Download, true, Some(true)) => {
            // A click on it opens the folder with the file selected.
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
        // No finished file: cancelled, failed, or moved away before it was seen.
        (Download, true) => named(tr("Đã dừng tải xuống", "Download stopped")),
        (Download, false) => named(tr("Đang tải xuống", "Downloading")),
        // A vanished process can also mean failure or cancellation.
        (_, true) => format!("{} {}", tr("Đã kết thúc:", "Ended:"), a.title),
        _ if a.progress.is_some() => format!("{} · {}", a.title, a.detail),
        _ => a.title.clone(),
    };
    // Others run beside it: a count at the end of the title (a badge would take width from the text).
    if a.more > 0 && !done {
        title = format!("{title} · +{}", a.more);
    }
    status_toast(app, title, &a.detail, if done { None } else { a.progress }, icon)
}

/// The banner for a Pomodoro segment that ended.
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

/// Open what a notification points at (picture, link or app) from a click on the notch. The
/// notch never takes focus, so hand the click's foreground right on (as Windows does for a toast
/// click); otherwise the app opens behind the window in front.
fn open_target(target: &str) {
    unsafe {
        let _ = AllowSetForegroundWindow(ASFW_ANY);
        match target.strip_prefix("select:") {
            // A finished download: its folder with the file selected.
            Some(file) => ShellExecuteW(None, w!("open"), w!("explorer.exe"), &windows::core::HSTRING::from(format!("/select,\"{file}\"")), None, SW_SHOWNORMAL),
            None => ShellExecuteW(None, w!("open"), &windows::core::HSTRING::from(target), None, None, SW_SHOWNORMAL),
        };
    }
}

/// The banner of a burst of network events. The first is its head (icon and title, as when it comes
/// alone); what comes while it shows is a line under it. Windows turns the airplane switch, the
/// radios, the network and the VPN one after another, and one banner tells all of it.
struct ConnList {
    head: Toast,
    /// What the head's event reads as a line, for when it has to become one (see `conn_list_add`).
    head_row: String,
    rows: Vec<Row>,
    at: Instant,
    /// The banner of one Bluetooth device (name, state, its battery large): a second device turns it into a list.
    single: bool,
    /// The head says how many devices there are ("3 Bluetooth devices"), the lines are those devices.
    devices_head: bool,
    devices: usize,
}

impl ConnList {
    fn new(head: Toast, head_row: String, rows: Vec<Row>, at: Instant) -> Self {
        ConnList { head, head_row, rows, at, single: false, devices_head: false, devices: 0 }
    }

    /// Whether its banner still shows (it stays `ALERT_MS` after the last line).
    fn fresh(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.at) < Duration::from_millis(ALERT_MS as u64)
    }

    /// Devices that connect while it shows are lines in it: after "Bluetooth on", after airplane mode, or
    /// in a list of devices.
    fn takes_devices(&self) -> bool {
        self.devices_head || matches!(self.head.icon, Some(render::ICON_BT | render::ICON_AIRPLANE))
    }
}

/// A list shows at most this many lines (the airplane switch, the radios, the network and the devices).
const MAX_ROWS: usize = 6;

fn devices_title(n: usize) -> String {
    format!("{n} {}", tr("thiết bị Bluetooth", "Bluetooth devices"))
}

fn device_row(name: &str, icon: &'static str, value: Option<String>, at: Instant) -> Row {
    let low = value.as_deref().and_then(|v| v.trim_end_matches('%').parse::<u8>().ok()).is_some_and(|p| p <= connectivity::LOW_BATTERY);
    Row { text: name.to_string(), at, icon: Some(icon), value, low }
}

/// A Bluetooth device connected. Alone it is its own banner (the battery large, the point of it); one
/// more while that shows, and the banner becomes a list with a line each; after "Bluetooth on" or
/// airplane mode off the devices are lines in that banner.
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
                l.head = status_toast("Bluetooth", devices_title(2), "", None, render::ICON_BT);
            }
            l.at = now;
            l
        }
        Some(mut l) if l.takes_devices() => {
            match l.rows.iter_mut().find(|r| r.icon.is_some() && r.text == *name) {
                Some(row) => row.value = value.or(row.value.take()),
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
        _ => ConnList { single: true, ..ConnList::new(connection_toast(e.clone()), name.clone(), Vec::new(), now) },
    }
}

/// A device's battery reading came after it connected: it goes in its banner or line, not a banner of
/// its own. False when the device is not on show.
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

/// The line an event makes under a head.
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

/// `prev` (the list on show, if any) with the event told. A list lasts as long as its banner shows
/// (`ALERT_MS` after the last line); an event after that starts a new one.
fn conn_list_add(prev: Option<ConnList>, e: &Event, now: Instant) -> ConnList {
    let fresh = prev.filter(|l| l.fresh(now));
    if matches!(e, Event::Device { up: true, .. }) {
        return device_connected(fresh, e, now);
    }
    match fresh {
        Some(mut l) if !matches!(e, Event::Airplane(_)) => {
            let text = row_text(e);
            if text != l.head_row && !l.rows.iter().any(|r| r.text == text) && l.rows.len() < MAX_ROWS {
                l.rows.push(Row::line(text, now));
            }
            l.at = now;
            l
        }
        prev => {
            // Windows can turn the radios off before it flips the airplane switch: what was told stays,
            // as lines under the switch.
            let rows = prev
                .filter(|l| l.head.icon != Some(render::ICON_AIRPLANE) && matches!(e, Event::Airplane(_)))
                .map(|l| std::iter::once(Row::line(l.head_row, l.at)).chain(l.rows).collect())
                .unwrap_or_default();
            ConnList::new(connection_toast(e.clone()), row_text(e), rows, now)
        }
    }
}

fn list_toast(l: &ConnList) -> Toast {
    Toast { rows: l.rows.clone(), ..l.head.clone() }
}

/// Banner text for a connectivity change.
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
        // The battery stands large at the banner's right end.
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

/// Banner text for a battery change.
fn power_toast(e: system::PowerEvent, b: system::Battery) -> Toast {
    use system::PowerEvent::*;
    // Title and a line under it on the left, the percent large at the right.
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

/// Paces animation frames to the display: a helper thread waits for each DWM composition and
/// posts WM_FRAME while on. WM_TIMER ticks every 15.6 ms and gets coalesced (frames 3-36 ms
/// apart measured), which made the spring stutter.
/// A WM_FRAME is queued and not yet handled: never stack more than one. A message that finds the
/// app borrowed (a menu or a message box is open) is not handled and must clear this, or the
/// vsync thread waits for it for ever and nothing moves again.
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
    // DestroyWindow re-enters here while the tray menu still holds APP's mutable borrow.
    if msg == WM_DESTROY {
        unsafe { PostQuitMessage(0) };
        return LRESULT(0);
    }
    // Win32 calls made while handling a message can re-enter here; those fall back to the default.
    let handled = APP.with(|a| a.try_borrow_mut().ok().and_then(|mut g| g.as_mut().and_then(|app| app.handle(msg, wp, lp))));
    if handled.is_none() && msg == WM_FRAME {
        FRAME_PENDING.store(false, Ordering::Relaxed);
    }
    handled.unwrap_or_else(|| unsafe { DefWindowProcW(hwnd, msg, wp, lp) })
}

/// Starts, exits and failures go to notch.log: a start with Windows that dies early shows nothing
/// on screen and leaves nothing in the event log otherwise.
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
        // Only the run that keeps the window starts the log over: a second start must not wipe it.
        system::log_open();
        let cwd = std::env::current_dir().map(|d| d.display().to_string()).unwrap_or_default();
        system::log(&format!("start v{} pid {} cwd {cwd}", env!("CARGO_PKG_VERSION"), std::process::id()));
        system::refresh_autostart();
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        // Never run at Windows' "efficiency" speed (EcoQoS): battery saver slows what it takes for
        // background work, and a topmost window that is not the foreground one looks like that.
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
        // Grow from the mini line into the pill (see Notch::new).
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
        // Wi-Fi off, and the VPN that was on it: the head is Wi-Fi, the VPN a line under it.
        let l = conn_list_add(None, &wifi_off, t0);
        assert_eq!((l.head.icon, l.rows.len()), (Some(render::ICON_WIFI_OFF), 0), "alone it is the banner it always was");
        let l = conn_list_add(Some(l), &Event::Vpn(false), later(600));
        assert_eq!((l.head.title.as_str(), texts(&l)), (format!("{} Wi-Fi", tr("Đã tắt", "Turned off")).as_str(), vec![tr("VPN tắt", "VPN off").to_string()]));
        // Airplane mode on: the switch is the head, each thing that went off a line, none told twice.
        let mut l = conn_list_add(None, &Event::Airplane(true), t0);
        for (ms, e) in [(100, wifi_off.clone()), (200, Event::Radio(RadioType::Bluetooth, false)), (300, Event::Vpn(false)), (400, wifi_off.clone())] {
            l = conn_list_add(Some(l), &e, later(ms));
        }
        assert_eq!(l.head.icon, Some(render::ICON_AIRPLANE));
        assert_eq!(texts(&l), [tr("Wi-Fi tắt", "Wi-Fi off"), tr("Bluetooth tắt", "Bluetooth off"), tr("VPN tắt", "VPN off")]);
        // Radios first, the switch after: what was told becomes the first lines under the switch.
        let l = conn_list_add(Some(conn_list_add(None, &wifi_off, t0)), &Event::Airplane(true), later(300));
        assert_eq!((l.head.icon, texts(&l)), (Some(render::ICON_AIRPLANE), vec![tr("Wi-Fi tắt", "Wi-Fi off").to_string()]));
        // The banner has gone by the time the next one comes: a new list.
        let l = conn_list_add(Some(l), &Event::Vpn(true), later(300 + ALERT_MS as u64 + 1));
        assert_eq!((l.head.title.as_str(), l.rows.len()), (tr("VPN đã kết nối", "VPN connected"), 0));
    }

    #[test]
    fn bluetooth_devices_are_one_banner_that_becomes_a_list() {
        use connectivity::DeviceKind::{Headphones, Mouse};
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let dev = |name: &str, kind, battery| Event::Device { name: name.into(), kind, up: true, battery };
        // One device: the banner it always was, its battery large; a late battery arrives in place.
        let mut l = conn_list_add(None, &dev("WH-1000XM5", Headphones, None), t0);
        assert_eq!((l.head.title.as_str(), l.head.badge.clone(), l.rows.len()), ("WH-1000XM5", None, 0));
        assert!(device_battery(&mut l, "WH-1000XM5", 80, at(700)));
        assert_eq!(l.head.badge.as_deref(), Some("80%"));
        // A second one turns it into a list: a line each, the battery at the right.
        let l = conn_list_add(Some(l), &dev("MX Master 3", Mouse, Some(64)), at(2800));
        assert_eq!(l.head.title, devices_title(2));
        let lines: Vec<_> = l.rows.iter().map(|r| (r.text.as_str(), r.value.as_deref(), r.icon.is_some())).collect();
        assert_eq!(lines, [("WH-1000XM5", Some("80%"), true), ("MX Master 3", Some("64%"), true)]);
        // A later battery fills its line in; a low one is red; a device that is not on show is not taken.
        let mut l = l;
        assert!(device_battery(&mut l, "MX Master 3", 18, at(3000)) && l.rows[1].low && !l.rows[0].low);
        assert!(!device_battery(&mut l, "Tay cầm", 50, at(3100)));
        assert!(!device_battery(&mut l, "MX Master 3", 18, at(3000 + ALERT_MS as u64 + 1)), "its banner has gone");
        // After "Bluetooth on" the devices are lines under it, however many; at most MAX_ROWS are drawn.
        let mut l = conn_list_add(None, &Event::Radio(RadioType::Bluetooth, true), t0);
        for i in 0..8u64 {
            l = conn_list_add(Some(l), &dev(&format!("Thiết bị {i}"), Mouse, Some(50)), at(100 * i));
        }
        assert_eq!((l.head.icon, l.rows.len(), l.devices), (Some(render::ICON_BT), MAX_ROWS, 8));
        // Under a Wi-Fi banner a device is not a line: it has its own banner.
        let wifi = conn_list_add(None, &Event::Radio(RadioType::Wifi, false), t0);
        let l = conn_list_add(Some(wifi), &dev("Chuột", Mouse, Some(70)), at(200));
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

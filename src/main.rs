#![windows_subsystem = "windows"]

mod audio;
mod banner_guard;
mod connectivity;
mod music;
mod foobar;
mod health;
mod i18n;
mod itunes;
mod notch;
mod notify;
mod render;
mod robot;
mod screen;
mod status;
mod system;
mod todo;

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Power::{RegisterPowerSettingNotification, POWERBROADCAST_SETTING};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use connectivity::{Event, Net, RadioType, WM_CONN};
use notch::{Mode, Notch, ALERT_MS, DWELL_MS, MAX_H, MAX_W, STATUS_MS};
use music::WM_MUSIC;
use i18n::tr;
use audio::WM_VOLUME;
use status::WM_LOCKKEY;
use notify::Toast;
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
/// Page switch morph length.
const PAGE_MORPH_S: f32 = 0.32;
/// Notifications kept on the Notifications page.
const NOTES_MAX: usize = 30;
const CARET_MS: u32 = 530;
const CHECK_MS: u32 = 500;
const MENU_MINIMAL: usize = 10;
const MENU_AUTOSTART: usize = 11;
const MENU_QUIT: usize = 12;
const MENU_LANG_VI: usize = 13;
const MENU_LANG_EN: usize = 14;
const MENU_ROBOT: usize = 15;

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
    /// Last brightness Windows reported; the first report (sent on registering) is not a change.
    last_brightness: Option<u8>,
    audio: Option<audio::Audio>,
    mic_muted: Option<bool>,
    music: music::Music,
    track: Option<music::Track>,
    /// Idle robot eyes, the tray switch for them and when they last moved.
    robot: robot::Robot,
    robot_on: bool,
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
    health: health::Health,
    /// Health card showing its settings.
    settings_open: Option<Card>,
    /// Page switch in progress: (page leaving, start, direction).
    page_anim: Option<(Page, Instant, i32)>,
    icons_morphing: bool,
    /// Window that had the keyboard before typing started; it gets it back after.
    prev_foreground: HWND,
    /// First half of a UTF-16 pair arriving in WM_CHAR (emoji).
    high_surrogate: Option<u16>,
}

/// Animation frame intervals.
/// `frame_ms` value for frames paced by the display (see `Vsync`).
const VSYNC: u32 = 1;
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
            music: self.track.as_ref(),
            live: self.notch.live,
            robot: eyes,
            phase: self.started.elapsed().as_secs_f32(),
            hovered: self.hovered_button,
            page: self.page,
            leaving: self.page_anim.map(|(from, t0, dir)| (from, (t0.elapsed().as_secs_f32() / PAGE_MORPH_S).min(1.0), dir)),
            list: ListView {
                counts: self.todo.counts(self.today),
                remind_before: self.todo.remind_before,
                now,
                draft: &self.draft,
                editing: self.editing,
                edit_item: self.edit_item,
                caret: self.caret,
                scroll: self.scroll(),
            },
            todos: &self.todo.items,
            notes: &self.notes,
            health: HealthView { h: &self.health, now: todo::minutes(now), minute_of_day: minute_of_day(now), settings: self.settings_open },
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
        if !self.animating {
            self.animating = true;
            self.last_frame = Instant::now();
        }
        self.set_frame_interval(VSYNC);
    }

    /// The equaliser in the resting pill keeps frames coming while music plays; so do the idle
    /// robot's eyes while they show or fade.
    fn bars_active(&self) -> bool {
        let bars = self.track.as_ref().is_some_and(|t| t.playing) && matches!(self.notch.mode, Mode::Collapsed | Mode::Peek);
        !self.hidden && (bars || self.robot.visible())
    }

    fn settings(&self) -> system::Settings {
        system::Settings { minimal: self.notch.minimal, english: i18n::english(), robot: self.robot_on }
    }

    /// Idle robot: shown only on the plain resting pill (no music, banner, open panel, mini or
    /// fullscreen game), from the time since the last key or mouse input.
    fn refresh_robot(&mut self) {
        let allowed = self.robot_on && !self.hidden && self.notch.mode == Mode::Collapsed && !self.notch.live;
        if self.robot.update(idle_seconds(), allowed) || self.robot.visible() {
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
            let cy = self.win.1 as f32 + self.notch.size()[1] * self.scale / 2.0;
            ((p.x as f32 - cx) / self.scale, (p.y as f32 - cy) / self.scale)
        });
        let mood = match self.battery {
            Some(b) if b.charging => robot::Mood::Charging,
            Some(b) if b.percent <= 20 => robot::Mood::Low,
            _ => robot::Mood::Normal,
        };
        Some(self.robot.step(dt, cursor, mood))
    }

    fn frame(&mut self) {
        let now = Instant::now();
        let dt = (now - self.last_frame).as_secs_f32().min(0.05);
        self.last_frame = now;
        let moving = self.notch.step(dt);
        // The page morph runs on the same frames; drop it once it has played out.
        let morphing = match self.page_anim {
            Some((_, t0, _)) if t0.elapsed().as_secs_f32() < PAGE_MORPH_S => true,
            Some(_) => {
                self.page_anim = None;
                false
            }
            None => false,
        };
        self.redraw();
        if moving || morphing || self.icons_morphing {
            self.set_frame_interval(VSYNC);
        } else if self.bars_active() {
            self.set_frame_interval(BARS_FRAME_MS);
        } else {
            self.animating = false;
            self.set_frame_interval(0);
        }
    }

    /// Pull the latest now-playing snapshot; music starting/stopping resizes the pill. A paused
    /// player keeps the pill for a few seconds (`music::shows_pill`); the panel's Music page keeps it.
    fn refresh_music(&mut self) {
        self.track = self.music.snapshot();
        let playing = self.track.as_ref().is_some_and(|t| t.playing);
        self.paused_since = if self.track.is_some() && !playing { Some(self.paused_since.unwrap_or_else(Instant::now)) } else { None };
        let live = music::shows_pill(playing, self.paused_since.map(|t| t.elapsed()));
        if self.notch.set_live(live) || self.bars_active() {
            self.kick();
        }
    }

    /// Is the cursor over the area that reacts to the mouse right now (`notch::hover_area`)?
    fn cursor_inside(&self) -> bool {
        let mut p = POINT::default();
        if unsafe { GetCursorPos(&mut p) }.is_err() {
            return false;
        }
        let (half, h) = notch::hover_area(self.notch.size());
        let (half, h) = (half * self.scale, h * self.scale);
        let cx = (self.win.0 + self.win.2 / 2) as f32;
        let (x, y) = (p.x as f32, (p.y - self.win.1) as f32);
        (x - cx).abs() <= half && y >= 0.0 && y <= h
    }

    /// Keep the speaker callback on the default device and re-read the mic. A mic mute change
    /// from anywhere (laptop mic key, Windows settings, a call app) gets a banner.
    fn refresh_audio(&mut self) {
        let hwnd = self.hwnd;
        let Some(a) = &mut self.audio else { return };
        a.watch_speaker(hwnd);
        let now = a.mic_muted();
        let mic_flip = match (self.mic_muted, now) {
            (Some(before), Some(after)) if before != after => Some(after),
            _ => None,
        };
        self.mic_muted = now;
        if let Some(muted) = mic_flip {
            let (title, icon) = if muted { (tr("Đã tắt mic", "Mic off"), render::ICON_MIC_OFF) } else { (tr("Đã bật mic", "Mic on"), render::ICON_MIC) };
            self.show_banner(status_toast("Mic", title.to_string(), "", None, icon), STATUS_MS);
        }
    }

    fn tick(&mut self) {
        let (time, date) = system::clock();
        let battery = system::battery();
        let mut changed = (&time, &date, battery) != (&self.time, &self.date, self.battery);
        if let (Some(old), Some(new)) = (self.battery, battery) {
            for e in system::power_events(old, new) {
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
        self.refresh_audio();
        self.refresh_music();
        self.refresh_robot();
        // Progress bar and lyric line move on their own while the panel is open.
        changed |= self.notch.mode == Mode::Expanded && self.track.as_ref().is_some_and(|t| t.playing);

        // The capture includes our own window: sample only while the pill rests, still and alone.
        let sample = self.notch.is_settled_rest();
        let cx = self.win.0 + self.win.2 / 2;
        let pill_w = notch::target(Mode::Collapsed, self.notch.live)[0];
        match screen::check(self.hwnd, cx, self.monitor, self.scale, pill_w, sample, &mut self.last_foreground) {
            screen::Check::Fullscreen => self.set_hidden(true),
            screen::Check::Settling => self.set_hidden(false),
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

        self.remind();

        // New ones get a banner; the Notifications page lists what Windows still keeps.
        let new = self.notifications.as_mut().map(|r| r.poll()).unwrap_or_default();
        let showing_notes = self.notch.mode == Mode::Expanded && self.page == Page::Notify;
        if !new.is_empty() || showing_notes {
            self.refresh_notes();
            changed |= showing_notes;
        }
        if let Some(t) = new.into_iter().last() {
            self.show_banner(t, ALERT_MS);
        }

        if changed && !self.animating {
            self.redraw();
        }
    }

    /// Short banner on the notch; skipped over a fullscreen game or while the panel is open.
    /// Returns whether it is showing.
    fn show_banner(&mut self, t: Toast, ms: u32) -> bool {
        // Notifications (no status icon) with text grow to fit it, up to BANNER_MAX_LINES lines.
        let lines = if t.icon.is_none() && !t.body.is_empty() {
            self.renderer.banner_lines(&t.body, self.scale).min(notch::BANNER_MAX_LINES) as u8
        } else {
            0
        };
        if self.hidden || !self.notch.on_alert(lines) {
            return false;
        }
        self.banner = Some(t);
        unsafe { SetTimer(Some(self.hwnd), T_ALERT, ms, None) };
        self.kick();
        true
    }

    /// Task reminders (early and on time) whose moment has come, then water / stand-up. One that
    /// cannot show now (fullscreen game, panel open) rings at the next chance. A banner that shows
    /// switches the panel to its page, so resting on it opens the right one.
    fn remind(&mut self) {
        let now = system::now_stamp();
        let hhmm = |d: u64| format!("{:02}:{:02}", d / 100 % 100, d % 100);
        for (i, ring) in self.todo.rings(now) {
            let Some(item) = self.todo.items.get(i) else { continue };
            let at = item.at.unwrap_or(now);
            let when = match ring {
                todo::Ring::Soon => format!("{} {} {} · {}", tr("Còn", "In"), todo::minutes(at) - todo::minutes(now), tr("phút", "min"), hhmm(at)),
                todo::Ring::Now => format!("{} · {}", tr("Đến giờ", "It's time"), hhmm(at)),
            };
            let toast = status_toast(tr("Nhắc việc", "Reminder"), item.text.clone(), &when, None, render::ICON_REMINDER);
            if !self.show_banner(toast, ALERT_MS * 2) {
                return;
            }
            self.todo.mark(i, ring);
            self.page = Page::Todo;
        }
        let h = &self.health;
        let (cup, goal) = (h.cup_ml, h.goal_ml);
        let every = h.sit_every_min;
        for ring in self.health.tick(todo::minutes(now), minute_of_day(now), idle_seconds()) {
            let taken = self.health.taken_ml;
            let level = Some(taken as f32 / goal.max(1) as f32);
            let toast = match ring {
                health::Ring::Drink => status_toast(tr("Uống nước", "Water"), if i18n::english() { format!("Time to drink {cup} ml") } else { format!("Đến giờ uống {cup} ml nước") }, "", level, render::ICON_DROP),
                health::Ring::GoalReached => status_toast(tr("Uống nước", "Water"), if i18n::english() { format!("Goal of {:.1} L reached today", goal as f32 / 1000.0) } else { format!("Đạt mục tiêu {:.1} L hôm nay", goal as f32 / 1000.0) }, "", Some(1.0), render::ICON_DROP),
                health::Ring::StandUp => status_toast(tr("Đứng dậy đi lại", "Stand up and move"), if i18n::english() { format!("You have been sitting for {every} min") } else { format!("Bạn đã ngồi {every} phút liền") }, tr("Vận động vài phút nhé", "Walk around for a few minutes"), None, render::ICON_WALK),
            };
            if self.show_banner(toast, ALERT_MS * 2) {
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
        render::hit_button(self.win.2 as f32 / 2.0, self.scale, x, y, self.track.as_ref(), self.page, self.rows(), self.scroll(), self.settings_open)
    }

    fn click(&mut self, x: f32, y: f32) {
        if self.notch.mode != Mode::Expanded {
            return;
        }
        let cx = self.win.2 as f32 / 2.0;
        let Some(button) = self.hit(x, y) else { return };
        let duration = self.track.as_ref().map_or(0, |t| t.duration_ms);
        match button {
            Button::Previous => self.music.send(music::Cmd::Previous),
            Button::PlayPause => self.music.send(music::Cmd::PlayPause),
            Button::Next => self.music.send(music::Cmd::Next),
            Button::Seek => self.music.send(music::Cmd::Seek(render::seek_at(cx, self.scale, x, duration))),
            Button::PagePrev | Button::PageNext => {
                self.cancel_task_edit();
                self.end_edit();
                let dir = if button == Button::PageNext { 1 } else { -1 };
                let i = PAGES.iter().position(|p| *p == self.page).unwrap_or(0) as i32;
                let next = PAGES[(i + dir).rem_euclid(PAGES.len() as i32) as usize];
                self.page_anim = Some((self.page, Instant::now(), dir));
                self.page = next;
                self.settings_open = None;
                if next == Page::Notify {
                    self.refresh_notes();
                }
                self.kick();
            }
            Button::ListInput => self.begin_edit(),
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
            Button::Gear(card) => self.settings_open = if self.settings_open == Some(card) { None } else { Some(card) },
            Button::RemindStep(dir) => self.todo.step_remind(dir),
            Button::Drink => self.health.drink(todo::minutes(system::now_stamp())),
            Button::Snooze => self.health.snooze(todo::minutes(system::now_stamp())),
            Button::Stood => self.health.stood_up(todo::minutes(system::now_stamp())),
            Button::Step(set, dir) => self.health.change(set, dir),
            Button::Toggle(set) => self.health.change(set, 1),
        }
        self.hovered_button = self.hit(x, y);
        self.redraw();
    }

    fn cancel_task_edit(&mut self) {
        if self.edit_item.take().is_some() {
            self.draft.clear();
        }
    }

    /// Enter in the box: save the edited task, or add a new one. The box stays for the next task.
    fn submit(&mut self) {
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
                self.draft.clear();
                self.edit_item = None;
                self.end_edit();
            }
            0x08 => {
                self.draft.pop();
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
        if self.draft.chars().count() < todo::MAX_LEN {
            self.draft.push(c);
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
            let _ = AppendMenuW(menu, MF_STRING | check(system::autostart_enabled()), MENU_AUTOSTART, &text(tr("Khởi động cùng Windows", "Start with Windows")));
            if let Ok(langs) = CreatePopupMenu() {
                let _ = AppendMenuW(langs, MF_STRING | check(!english), MENU_LANG_VI, w!("Tiếng Việt"));
                let _ = AppendMenuW(langs, MF_STRING | check(english), MENU_LANG_EN, w!("English"));
                let _ = AppendMenuW(menu, MF_POPUP, langs.0 as usize, &text(tr("Ngôn ngữ", "Language")));
            }
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
                MENU_AUTOSTART => system::set_autostart(!system::autostart_enabled()),
                MENU_ROBOT => {
                    self.robot_on = !self.robot_on;
                    system::save_settings(self.settings());
                    self.refresh_robot();
                }
                MENU_LANG_VI | MENU_LANG_EN => {
                    i18n::set_english(cmd == MENU_LANG_EN);
                    system::save_settings(self.settings());
                    // Everything shown is rebuilt from the new language on the next frame.
                    (self.time, self.date) = system::clock();
                    self.redraw();
                }
                MENU_QUIT => { let _ = DestroyWindow(self.hwnd); }
                _ => {}
            }
        }
    }

    fn handle(&mut self, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
        match msg {
            WM_FRAME => {
                self.vsync.pending.store(false, Ordering::Relaxed);
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
                T_CARET => {
                    self.caret = !self.caret;
                    self.redraw();
                }
                T_ALERT => {
                    unsafe { let _ = KillTimer(Some(self.hwnd), T_ALERT); }
                    self.notch.on_alert_end();
                    // Mouse resting on the banner when it ends: treat as a fresh visit.
                    if self.hover_in && self.notch.on_enter() {
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
                }
                // The ears report the mouse too (they are drawn); only the pill body counts.
                if !self.hover_in && self.cursor_inside() {
                    self.hover_in = true;
                    if self.notch.on_enter() {
                        unsafe { SetTimer(Some(self.hwnd), T_DWELL, DWELL_MS, None) };
                    }
                    self.kick();
                }
                let hover = if self.notch.mode == Mode::Expanded {
                    let x = (lp.0 & 0xffff) as i16 as f32;
                    let y = ((lp.0 >> 16) & 0xffff) as i16 as f32;
                    self.hit(x, y)
                } else { None };
                if hover != self.hovered_button { self.hovered_button = hover; self.redraw(); }
            }
            WM_SETCURSOR if self.hovered_button.is_some_and(|b| b.enabled(self.track.as_ref())) => {
                let cursor = if self.hovered_button == Some(Button::ListInput) { IDC_IBEAM } else { IDC_HAND };
                unsafe { let _ = SetCursor(LoadCursorW(None, cursor).ok()); }
                return Some(LRESULT(1));
            }
            WM_MOUSELEAVE => {
                self.hovered_button = None;
                self.tracking = false;
                self.hover_in = false;
                unsafe { let _ = KillTimer(Some(self.hwnd), T_DWELL); }
                // Typing keeps the panel open; it closes when the box lets go of the keyboard.
                if !self.editing {
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
                self.redraw();
            }
            WM_LBUTTONUP => {
                let x = (lp.0 & 0xffff) as i16 as f32;
                let y = ((lp.0 >> 16) & 0xffff) as i16 as f32;
                self.click(x, y);
            }
            WM_RBUTTONUP => self.menu(),
            WM_LOCKKEY => {
                let (name, on) = status::lock_state(wp.0);
                self.show_banner(status_toast(tr("Bàn phím", "Keyboard"), name.to_string(), on_off(on), None, render::ICON_KEYBOARD), STATUS_MS);
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
            WM_CONN => {
                for e in connectivity::take_events() {
                    self.show_banner(connection_toast(e), ALERT_MS);
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
    Toast { app: source.to_string(), title, body: body.to_string(), level, icon: Some(icon) }
}

fn on_off(on: bool) -> &'static str {
    if on { tr("Bật", "On") } else { tr("Tắt", "Off") }
}

/// Banner text for a connectivity change.
fn connection_toast(e: Event) -> Toast {
    match e {
        Event::Radio(RadioType::Wifi, on) => status_toast("Wi-Fi", format!("{} Wi-Fi", if on { tr("Đã bật", "Turned on") } else { tr("Đã tắt", "Turned off") }), "", None, if on { render::ICON_WIFI } else { render::ICON_WIFI_OFF }),
        Event::Radio(RadioType::Bluetooth, on) => status_toast("Bluetooth", format!("{} Bluetooth", if on { tr("Đã bật", "Turned on") } else { tr("Đã tắt", "Turned off") }), "", None, render::ICON_BT),
        Event::Radio(RadioType::Mobile, on) => status_toast(tr("Mạng di động", "Mobile data"), on_off(on).to_string(), "", None, render::ICON_MOBILE),
        Event::Airplane(on) => status_toast(tr("Chế độ máy bay", "Airplane mode"), on_off(on).to_string(), "", None, render::ICON_AIRPLANE),
        Event::Net(Net::Online { name, wifi }) => {
            let (source, icon) = if wifi { ("Wi-Fi", render::ICON_WIFI) } else { (tr("Mạng", "Network"), render::ICON_ETHERNET) };
            status_toast(source, format!("{} {name}", tr("Đã kết nối", "Connected to")), "", None, icon)
        }
        Event::Net(Net::Limited { name, .. }) => status_toast(tr("Mạng", "Network"), tr("Không có internet", "No internet").to_string(), &name, None, render::ICON_NO_INTERNET),
        Event::Net(Net::Offline) => status_toast(tr("Mạng", "Network"), tr("Mất kết nối mạng", "Disconnected").to_string(), "", None, render::ICON_NO_INTERNET),
        Event::Device { name, kind, up } => status_toast("Bluetooth", name, if up { tr("Đã kết nối", "Connected") } else { tr("Đã ngắt kết nối", "Disconnected") }, None, render::device_icon(kind)),
    }
}

/// Banner text for a battery change.
fn power_toast(e: system::PowerEvent, b: system::Battery) -> Toast {
    use system::PowerEvent::*;
    let level = Some(b.percent as f32 / 100.0);
    match e {
        Plugged => status_toast(tr("Pin", "Battery"), format!("{} {}%", tr("Đang sạc", "Charging"), b.percent), "", level, render::ICON_PLUG),
        Unplugged => status_toast(tr("Pin", "Battery"), format!("{} · {}%", tr("Đã rút sạc", "Unplugged"), b.percent), "", level, render::ICON_BATTERY),
        SaverOn => status_toast(tr("Tiết kiệm pin", "Battery saver"), on_off(true).to_string(), "", None, render::ICON_SAVER),
        SaverOff => status_toast(tr("Tiết kiệm pin", "Battery saver"), on_off(false).to_string(), "", None, render::ICON_SAVER),
        Low(p) => status_toast(tr("Pin yếu", "Low battery"), format!("{p}% · {}", tr("nên cắm sạc", "plug in soon")), "", level, render::ICON_BATTERY_LOW),
        Critical(p) => status_toast(tr("Pin rất yếu", "Battery critical"), format!("{p}% · {}", tr("cắm sạc ngay", "plug in now")), "", level, render::ICON_BATTERY_LOW),
        Full => status_toast(tr("Pin", "Battery"), tr("Đã sạc đầy", "Fully charged").to_string(), "", level, render::ICON_BATTERY),
    }
}

/// Paces animation frames to the display: a helper thread waits for each DWM composition and
/// posts WM_FRAME while on. WM_TIMER ticks every 15.6 ms and gets coalesced (frames 3-36 ms
/// apart measured), which made the spring stutter.
struct Vsync {
    on: Arc<AtomicBool>,
    /// A WM_FRAME is queued and not yet handled: never stack more than one.
    pending: Arc<AtomicBool>,
    thread: std::thread::Thread,
}

impl Vsync {
    fn start(hwnd: HWND) -> Self {
        let (on, pending) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
        let (on2, pending2, target) = (on.clone(), pending.clone(), hwnd.0 as isize);
        let thread = std::thread::spawn(move || loop {
            if !on2.load(Ordering::Relaxed) {
                std::thread::park();
                continue;
            }
            if unsafe { DwmFlush() }.is_err() {
                std::thread::sleep(std::time::Duration::from_millis(16));
            }
            if on2.load(Ordering::Relaxed) && !pending2.swap(true, Ordering::Relaxed) {
                unsafe { let _ = PostMessageW(Some(HWND(target as _)), WM_FRAME, WPARAM(0), LPARAM(0)); }
            }
        })
        .thread()
        .clone();
        Vsync { on, pending, thread }
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
    handled.unwrap_or_else(|| unsafe { DefWindowProcW(hwnd, msg, wp, lp) })
}

fn main() -> windows::core::Result<()> {
    if banner_guard::run_if_helper() {
        return Ok(());
    }
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
            hIcon: app_icon(false),
            hIconSm: app_icon(true),
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

        let settings = system::load_settings();
        i18n::set_english(settings.english);
        let (time, date) = system::clock();
        let mut app = App {
            hwnd,
            notch: Notch::new(settings.minimal),
            renderer: Renderer::new()?,
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
            last_brightness: None,
            audio: audio::Audio::new(),
            mic_muted: None,
            music: music::Music::start(hwnd),
            track: None,
            robot: robot::Robot::default(),
            robot_on: settings.robot,
            robot_at: Instant::now(),
            paused_since: None,
            started: Instant::now(),
            frame_ms: 0,
            vsync: Vsync::start(hwnd),
            hovered_button: None,
            last_foreground: None,
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
            health: health::Health::load(system::today()),
            settings_open: None,
            page_anim: None,
            icons_morphing: false,
            prev_foreground: HWND::default(),
            high_surrogate: None,
        };
        // Keep native banners if notification delivery to Notch is unavailable.
        let _banner_guard = banner_guard::Guard::start(app.notifications.is_some())
            .map_err(|error| banner_guard::report(&error)).ok();
        app.refresh_audio();
        let _ = RegisterPowerSettingNotification(HANDLE(hwnd.0), &status::VIDEO_BRIGHTNESS, DEVICE_NOTIFY_WINDOW_HANDLE);
        status::watch_lock_keys(hwnd);
        connectivity::watch(hwnd);
        app.layout();
        app.redraw();
        // Grow from the mini line into the pill (see Notch::new).
        app.kick();
        app.tray(NIM_ADD);
        APP.with(|a| *a.borrow_mut() = Some(app));

        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
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
    use super::*;

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

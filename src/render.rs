//! Direct2D drawing on the GPU into a DXGI swap chain shown through DirectComposition: frames never
//! come back to the CPU (UpdateLayeredWindow needed a GPU-to-CPU read-back per frame, which stalled
//! the UI thread while the GPU was busy, e.g. decoding a video). The window region is the drawn
//! shape's bounds, so the mouse goes through everywhere else.

use windows::core::{w, Interface, Result, HSTRING, PCWSTR};
use windows::Win32::UI::Shell::{IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_BIGGERSIZEOK, SIIGBF_ICONONLY};
use windows::Win32::Foundation::{D2DERR_RECREATE_TARGET, GENERIC_READ, HMODULE, HWND, SIZE};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE, D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::DirectComposition::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows_numerics::{Matrix3x2, Vector2};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, AtomicU8, Ordering};
use std::time::Instant;

use crate::anim::{self, ease_out, ease_out_back, page_fades};
use crate::approve;
use crate::connectivity::DeviceKind;
use crate::icons::{self, Seg};
use crate::lyrics;
use crate::music::{Repeat, Track};
use crate::notch::{self, target, Mode, LYRICS_EXTRA_W};
use crate::notify::{BannerKind, Note, Toast};
use crate::system::Battery;

mod aurora;
mod battery;
mod focus_page;
mod health_page;
mod weather_page;
mod notify_page;
mod todo_page;
mod music_page;
mod pill;
mod banner;
mod header;
mod nav;
mod pages;
mod sun_path;
mod usage_page;

pub const WM_ICONS: u32 = WM_APP + 10;

pub fn visible_page(mode: Mode, hidden: bool, page: Page) -> Option<Page> {
    (mode == Mode::Expanded && !hidden).then_some(page)
}
use crate::focus::{self, Focus};
use crate::health::{self, Health, Setting};
use crate::i18n::tr;
use crate::robot::{curious_eyes, Eyes, Face, EYE_H};
use crate::todo::{self, Item};
use crate::weather;

/// Everything the painter needs for one frame (logical px).
pub struct View<'a> {
    pub mode: Mode,
    pub size: [f32; 3],
    pub time: &'a str,
    /// Short date for the panel header ("CN, 25/10").
    pub date: &'a str,
    /// None on a desktop PC: no battery icon, no label.
    pub battery: Option<Battery>,
    /// Banner content while in Alert (a notification or a status change).
    pub banner: Option<&'a Toast>,
    /// Now playing (also paused), for the panel's Music page.
    pub music: Option<&'a Track>,
    /// Resting pill uses the music layout (cover, bars); see `music::shows_pill`.
    pub live: bool,
    /// Idle robot eyes in the resting pill (they replace clock and battery while shown).
    pub robot: Option<Eyes>,
    /// Seconds, drives the equaliser bars.
    pub phase: f32,
    /// Seconds since the current banner appeared (drives its icon's motion).
    pub banner_age: f32,
    /// The lyrics are open beside the player (Music page, open panel): the panel is wider.
    pub lyrics: bool,
    pub hovered: Option<Button>,
    pub page: Page,
    /// Page switch in progress: (page sliding out, progress 0..1, direction +1 next / -1 previous).
    pub leaving: Option<(Page, f32, i32)>,
    /// Box, scroll and counts of the list page on screen (tasks or notifications).
    pub list: ListView<'a>,
    pub todos: &'a [Item],
    /// Notifications page, newest first.
    pub notes: &'a [Note],
    pub health: HealthView<'a>,
    /// The Pomodoro timer: its page and the line along the pill's bottom.
    pub focus: &'a Focus,
    /// The weather's city and current conditions (shown on the pill and the Weather page).
    pub weather: &'a weather::State,
    /// Seconds since the open panel or its page appeared: the Weather page's icon moves for a moment.
    pub page_age: f32,
    /// °C or °F, for every temperature shown.
    pub unit: weather::Unit,
    /// The Weather page shows the sun's path in place of the hours ahead.
    pub sun_view: bool,
}

/// Pages of the open panel, switched with the side arrows.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Page {
    Music,
    /// Tasks and appointments in one list.
    Todo,
    /// Pomodoro: the timer of focus and break segments.
    Focus,
    /// Windows notifications, to read back.
    Notify,
    Health,
    /// The city (typed here) and its current weather.
    Weather,
    /// Plan limits of the AI apps open on this PC.
    Usage,
}

pub const PAGES: [Page; 7] = [Page::Music, Page::Todo, Page::Focus, Page::Notify, Page::Health, Page::Weather, Page::Usage];

/// Which pages (a bit each, see `Page::bit`) the user keeps: the arrows and dots only go through those.
/// Never empty.
static PAGES_ON: AtomicU8 = AtomicU8::new(ALL_PAGES);
const ALL_PAGES: u8 = (1 << PAGES.len()) - 1;

pub fn set_pages_on(mask: u8) {
    let mask = mask & ALL_PAGES;
    PAGES_ON.store(if mask == 0 { ALL_PAGES } else { mask }, Ordering::Relaxed);
}

pub fn pages_on() -> u8 {
    PAGES_ON.load(Ordering::Relaxed)
}

pub fn page_shown(page: Page) -> bool {
    pages_on() & page.bit() != 0
}

impl Page {
    /// The page's bit in the saved mask. Fixed per page (not its place in `PAGES`), so a page added in the
    /// middle of the order does not move the choices already saved.
    pub fn bit(self) -> u8 {
        1 << match self {
            Page::Music => 0,
            Page::Todo => 1,
            Page::Notify => 2,
            Page::Health => 3,
            Page::Weather => 4,
            Page::Usage => 5,
            Page::Focus => 6,
        }
    }

    /// The name in the panel's header (and in the tray's list of pages).
    pub fn name(self) -> &'static str {
        match self {
            Page::Music => tr("Nhạc", "Music"),
            Page::Todo => tr("Ghi chú", "Notes"),
            Page::Focus => tr("Tập trung", "Focus"),
            Page::Notify => tr("Thông báo", "Notifications"),
            Page::Health => tr("Sức khỏe", "Health"),
            Page::Weather => tr("Thời tiết", "Weather"),
            Page::Usage => tr("Giới hạn", "Limits"),
        }
    }
}

/// The add / edit box and the visible rows of a list page.
pub struct ListView<'a> {
    /// Counts shown above the task rows: (open for today, open on later days).
    pub counts: (usize, usize),
    /// Early reminder, minutes (the tasks' settings row).
    pub remind_before: u32,
    /// Local time yyyymmddHHMM, for the time labels.
    pub now: u64,
    /// Text being typed (kept when the box loses focus).
    pub draft: &'a str,
    pub editing: bool,
    /// Row being edited in the box (None = the box adds a new one).
    pub edit_item: Option<usize>,
    /// Caret blink phase.
    pub caret: bool,
    /// First visible row.
    pub scroll: usize,
    /// Notifications list position in rows while it glides to `scroll` (a wheel step eases).
    pub note_pos: f32,
}

pub struct HealthView<'a> {
    pub h: &'a Health,
    /// Total minutes (todo::minutes) and minute of the day, now.
    pub now: i64,
    pub minute_of_day: u32,
    /// Card whose settings are open.
    pub settings: Option<Card>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Card {
    Water,
    Sit,
    /// The task list's settings (early reminder).
    Tasks,
}

/// Clickable things of the open panel.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Button {
    Previous,
    PlayPause,
    Next,
    /// Shuffle on / off, and repeat off, all, one (the next mode on each click).
    Shuffle,
    Repeat,
    Seek,
    /// Show / hide the lyrics beside the player.
    Lyrics,
    /// Show / hide the 24-hour forecast beside the weather.
    PagePrev,
    PageNext,
    /// The page dots: open the row of page icons under them.
    Dots,
    /// An icon of that row: go to the page.
    GoPage(Page),
    ListInput,
    /// Tick / untick task at this index (the whole row, text included). To-do only.
    ListToggle(usize),
    ListEdit(usize),
    ListDelete(usize),
    /// Open the notification at this index (its app, link or picture).
    NoteOpen(usize),
    /// Show the temperature in this unit (Weather page).
    Unit(weather::Unit),
    /// Show this app (index into `usage::APPS`) on the Limits page.
    UsageTab(usize),
    /// Switch the weather off (the x beside the city).
    CityOff,
    /// The x at the end of the add / city box, wiping what is typed.
    ListClear,
    /// "Clear all" under the Notifications list: hides what has arrived so far.
    NotesClear,
    Gear(Card),
    Drink,
    /// Water reminder later (10 min).
    Snooze,
    Stood,
    /// −1 / +1 on a setting.
    Step(Setting, i32),
    /// −1 / +1 on the tasks' early reminder.
    RemindStep(i32),
    Toggle(Setting),
    /// The Focus page: start / pause, back to the segment's start, on to the next segment, a chip (go to the
    /// next segment of that kind), a segment of the strip.
    FocusPlay,
    FocusReset,
    FocusSkip,
    FocusMode(focus::Kind),
    FocusSeg(usize),
    /// The Weather page: the sun's path in place of the hours ahead, and back.
    SunToggle,
}

impl Button {
    fn label(self) -> &'static str {
        match self {
            Self::Previous => tr("Bài trước", "Previous"), Self::PlayPause => tr("Phát / Tạm dừng", "Play / Pause"),
            Self::Next => tr("Bài tiếp theo", "Next"), Self::Seek => tr("Bấm để tua", "Click to seek"),
            Self::Lyrics => tr("Lời bài hát", "Lyrics"),
            Self::Shuffle => tr("Trộn bài", "Shuffle"), Self::Repeat => tr("Lặp lại", "Repeat"),
            Self::FocusPlay => tr("Bắt đầu / Tạm dừng", "Start / Pause"), Self::FocusReset => tr("Đặt lại đoạn này", "Restart this segment"),
            Self::FocusSkip => tr("Sang đoạn kế", "Next segment"),
            Self::SunToggle => tr("Đường đi của mặt trời", "The sun's path"),
            _ => "",
        }
    }
    pub fn enabled(self, track: Option<&Track>) -> bool {
        match self {
            Self::Previous => track.is_some_and(|t| t.can_previous),
            Self::PlayPause => track.is_some_and(|t| t.can_toggle),
            Self::Next => track.is_some_and(|t| t.can_next),
            Self::Seek => track.is_some_and(|t| t.can_seek && t.duration_ms > 0),
            Self::Lyrics => track.is_some_and(|t| !t.lyrics.is_empty()),
            Self::Shuffle => track.is_some_and(|t| t.can_shuffle),
            Self::Repeat => track.is_some_and(|t| t.can_repeat),
            _ => true,
        }
    }
}

const PAD: f32 = 24.0;
/// Music page: cover on the left, a column of text, bar and controls on its right.
const COVER: f32 = 120.0;
const COVER_TOP: f32 = 40.0;
const COVER_GAP: f32 = 16.0;
const PLAY_BTN: f32 = 36.0;
const SKIP_BTN: f32 = 28.0;
const LYRICS_BTN: f32 = 28.0;
/// Distance between the centres of neighbouring player buttons: shuffle, previous, play, next, repeat.
const SKIP_GAP: f32 = 40.0;
const CONTROLS_Y: f32 = 148.0;
const BAR_Y: f32 = 108.0;
/// Page content sits between the side arrows (logical px from the panel edge).
const SIDE: f32 = 44.0;
/// The Focus page (logical px from the panel's top-left): the chips under the big time, the strip of the cycle's segments
/// and the line of text under it.
const FOCUS_CHIP_Y: f32 = 92.0;
const FOCUS_CHIP_H: f32 = 22.0;
const FOCUS_STRIP_Y: f32 = 128.0;
const FOCUS_STRIP_H: f32 = 5.0;
const FOCUS_SEG_GAP: f32 = 3.0;
const FOCUS_PLAY: f32 = 44.0;
const FOCUS_SMALL: f32 = 28.0;

/// The width of a chip: its label plus room each side.
fn focus_chip_w(label: &str) -> f32 {
    22.0 + 6.6 * label.chars().count() as f32
}

fn focus_chip_label(kind: focus::Kind) -> &'static str {
    match kind {
        focus::Kind::Focus => tr("Tập trung", "Focus"),
        focus::Kind::Short => tr("Nghỉ ngắn", "Short break"),
        focus::Kind::Long => tr("Nghỉ dài", "Long break"),
    }
}

/// The cycle's segments across `width` (logical px): each as long as its minutes, `FOCUS_SEG_GAP` apart.
fn focus_segments(width: f32) -> Vec<(f32, f32)> {
    let total: f32 = focus::CYCLE.iter().map(|k| k.minutes() as f32).sum();
    let room = width - FOCUS_SEG_GAP * (focus::CYCLE.len() - 1) as f32;
    let mut x = 0.0;
    focus::CYCLE.iter().map(|k| {
        let w = room * k.minutes() as f32 / total;
        let seg = (x, x + w);
        x += w + FOCUS_SEG_GAP;
        seg
    }).collect()
}
const ARROW: f32 = 24.0;
/// Pages are clipped this far in from the panel's sides (just inside the arrows).
const PAGE_CLIP: f32 = 34.0;
/// The arrows sit level with the panel's middle and the dots near its bottom, whatever its height.
fn arrow_y(h: f32) -> f32 {
    h / 2.0 - 13.0
}

fn dots_y(h: f32) -> f32 {
    h - 12.0
}

/// What the open panel measures (logical px) while `page` shows. One place decides it; the notch
/// glides to it, and the layout and hit areas of the page read the same height.
pub fn panel_size(page: Page) -> [f32; 2] {
    let [w, h, _] = target(Mode::Expanded, false);
    match page {
        // Every page shares one standard size, 480 x 186; a page that needs more asks for it here.
        _ => [w, h],
    }
}
/// Task list page.
const INPUT_Y: f32 = 36.0;
const INPUT_H: f32 = 24.0;
/// The left column of the page (the ring of the day's progress and what it counts), then the
/// right column (the add box and the list) from `LIST_X` on, as far as the content reaches.
const HERO_W: f32 = 92.0;
const LIST_X: f32 = HERO_W + 6.0;
const ROW_Y: f32 = 66.0;
/// A note is a plain row with a hairline under it, and its tick, edit and delete buttons: 26 px high,
/// four of them from `ROW_Y` down to just above the page dots.
const ROW_H: f32 = 26.0;
pub const LIST_ROWS: usize = 4;
/// Weather page, logical px from the content's left / the panel's top. City line: the name, then at
/// its right end the unit chips, 24h and x. Left block: the big icon with the temperature beside it, the
/// sky's words and the feels-like under that. Right block (from `STRIP_X` to the content's right): the
/// next hours as columns. Under both, across the whole width, a hairline and one line of figures.
const CITY_Y: f32 = 62.0;
const ICON_Y: f32 = 90.0;
const WX_ICON: f32 = 56.0;
const TEMP_X: f32 = 64.0;
const STRIP_X: f32 = 160.0;
/// The hours' columns: the top of the time, and the line of figures under the hairline.
const STRIP_Y: f32 = 88.0;
const FIGURES_Y: f32 = 152.0;
const UNIT_W: f32 = 28.0;
const UNIT_H: f32 = 18.0;
/// The Weather page's sun chip (beside the unit capsule) and the gap between them.
const SUN_W: f32 = 28.0;
const SUN_GAP: f32 = 6.0;

/// Notifications page: a narrow column on the left with the count and "Clear all"; on the right, from
/// `NOTE_X` to the content's end, a row for each notification: its app's tile, then two lines.
const NOTE_X: f32 = 72.0;
const NOTE_Y: f32 = 36.0;
const NOTE_H: f32 = 33.0;
pub const NOTE_ROWS: usize = 4;
/// "Clear all" under the count, in the left column (the click area).
const CLEAR_W: f32 = 64.0;
const CLEAR_H: f32 = 18.0;
const CLEAR_Y: f32 = 118.0;
/// Health page cards.
const CARD_TOP: f32 = 34.0;
/// The cards share the content's width; they end 28 px above the panel's bottom.
const CARD_GAP: f32 = 8.0;
fn card_bottom(panel_h: f32) -> f32 {
    panel_h - 28.0
}
/// The row of page icons is open where the page dots are (the panel is `NAV_EXTRA` taller meanwhile): its cells
/// start at the dots' top, 7 px under the controls, and 5 px of panel is left under them (the glyph is smaller
/// than its cell, so the clear space under it reads as about 8).
static NAV_OPEN: AtomicBool = AtomicBool::new(false);
pub const NAV_EXTRA: f32 = 15.0;

pub fn set_nav_open(open: bool) {
    NAV_OPEN.store(open, Ordering::Relaxed);
}

pub fn nav_open() -> bool {
    NAV_OPEN.load(Ordering::Relaxed)
}

/// The icon of a page in the row.
fn nav_icon(page: Page) -> &'static str {
    match page {
        Page::Music => "\u{E8D6}",
        Page::Todo => "\u{E9D5}",
        Page::Focus => "\u{E916}",
        Page::Notify => ICON_BELL,
        Page::Health => "\u{E95E}",
        // The Weather page draws its own line sun (see `draw`); this is only a stand-in.
        Page::Weather => "\u{E706}",
        Page::Usage => "\u{E9D9}",
    }
}

/// The row of page icons: its cells are at most this far apart (logical px) with `NAV_GAP` between neighbours. The row is
/// at most `NAV_ROW_W` wide (the panel's 480 less its 44 px margins), so with many pages the cells close up instead of
/// running into the arrows or the credit at the corner.
const NAV_PITCH: f32 = 34.0;
const NAV_GAP: f32 = 4.0;
const NAV_ROW_W: f32 = 392.0;

fn nav_pitch(pages: usize) -> f32 {
    NAV_PITCH.min(NAV_ROW_W / pages.max(1) as f32)
}

/// The cells of the row (window px) for a panel `cx` is the middle of: `nav_pitch` apart, centred, just
/// below the panel's resting height `ph`.
fn nav_cells(cx: f32, scale: f32, ph: f32) -> Vec<(Page, D2D_RECT_F)> {
    let shown: Vec<Page> = PAGES.into_iter().filter(|p| page_shown(*p)).collect();
    let (n, pitch) = (shown.len() as f32, nav_pitch(shown.len()));
    let half = (pitch - NAV_GAP) / 2.0;
    shown.into_iter().enumerate().map(|(i, p)| {
        let x = cx + (i as f32 - (n - 1.0) / 2.0) * pitch * scale;
        (p, rect(x - half * scale, dots_y(ph) * scale, x + half * scale, (dots_y(ph) + 22.0) * scale))
    }).collect()
}

/// Is the mouse (window px) over the row of page icons, or just above it? The row shows while it is, and
/// the page dots come back once it leaves.
pub fn in_nav_zone(cx: f32, scale: f32, ph: f32, x: f32, y: f32) -> bool {
    let cells = nav_cells(cx, scale, ph);
    let (Some((_, first)), Some((_, last))) = (cells.first(), cells.last()) else { return false };
    x >= first.left - 16.0 * scale && x <= last.right + 16.0 * scale
        && y >= (dots_y(ph) - 4.0) * scale && y <= (ph + NAV_EXTRA + 10.0) * scale
}

/// Which apps (bit per `usage::APPS` index) have a tab on the Limits page: those found on this PC.
static USAGE_APPS: AtomicU8 = AtomicU8::new(0b111);

pub fn set_usage_apps(mask: u8) {
    USAGE_APPS.store(mask, Ordering::Relaxed);
}

/// The Limits page's app tabs: one segmented control.
/// A Limits tab is a chip this wide.
const USAGE_CHIP_W: f32 = 110.0;
const USAGE_TAB_H: f32 = 22.0;
const SET_ROW: f32 = 18.0;
const SET_Y: f32 = 62.0;

/// Width of the settled open panel; `wide` = the lyrics show beside the player.
pub fn panel_w(wide: bool) -> f32 {
    target(Mode::Expanded, false)[0] + if wide { LYRICS_EXTRA_W } else { 0.0 }
}

/// Media controls of the music page, in window px for the settled panel centred at `cx`. The
/// player is the left 480 px of the panel; with the lyrics open (`wide`) the panel is wider and
/// the lyrics take the part on the right.
pub fn buttons(cx: f32, scale: f32, wide: bool) -> [(Button, D2D_RECT_F); 7] {
    buttons_from(cx / scale - panel_w(wide) / 2.0, scale)
}

/// The same controls for a panel whose left edge is at `left` (logical px): follows the panel
/// while it grows or shrinks.
fn buttons_from(left: f32, scale: f32) -> [(Button, D2D_RECT_F); 7] {
    let (x0, x1) = (SIDE + COVER + COVER_GAP, target(Mode::Expanded, false)[0] - SIDE);
    let mid = (x0 + x1) / 2.0;
    let round = |x: f32, size: f32| rect((left + x - size / 2.0) * scale, (CONTROLS_Y - size / 2.0) * scale, (left + x + size / 2.0) * scale, (CONTROLS_Y + size / 2.0) * scale);
    let seek = rect((left + x0) * scale, (BAR_Y - 8.0) * scale, (left + x1) * scale, (BAR_Y + 8.0) * scale);
    [
        (Button::Shuffle, round(mid - 2.0 * SKIP_GAP, SKIP_BTN)),
        (Button::Previous, round(mid - SKIP_GAP, SKIP_BTN)),
        (Button::PlayPause, round(mid, PLAY_BTN)),
        (Button::Next, round(mid + SKIP_GAP, SKIP_BTN)),
        (Button::Repeat, round(mid + 2.0 * SKIP_GAP, SKIP_BTN)),
        (Button::Seek, seek),
        (Button::Lyrics, round(x1 - LYRICS_BTN / 2.0, LYRICS_BTN)),
    ]
}

/// Settings rows of a health card: (setting, is a switch).
fn setting_rows(card: Card) -> &'static [(Setting, bool)] {
    match card {
        Card::Water => &[(Setting::WaterOn, true), (Setting::Cup, false), (Setting::Goal, false), (Setting::From, false), (Setting::To, false)],
        Card::Sit => &[(Setting::SitOn, true), (Setting::SitEvery, false)],
        Card::Tasks => &[],
    }
}

/// Every clickable rect of the open panel on `page`. `rows` / `scroll` place list rows,
/// `settings` = health card showing its settings.
pub fn page_buttons(cx: f32, scale: f32, page: Page, rows: usize, scroll: usize, settings: Option<Card>, wide: bool) -> Vec<(Button, D2D_RECT_F)> {
    let panel_h = panel_size(page)[1];
    // Only the Music page can be wide.
    let wide = wide && page == Page::Music;
    let w = panel_size(page)[0] + if wide { LYRICS_EXTRA_W } else { 0.0 };
    let left = cx / scale - w / 2.0;
    let r = |x: f32, y: f32, bw: f32, bh: f32| rect((left + x) * scale, y * scale, (left + x + bw) * scale, (y + bh) * scale);
    let mut out = Vec::new();
    // The arrows are hidden while the lyrics are open.
    if !wide {
        out.push((Button::PagePrev, r(10.0, arrow_y(panel_h), ARROW, ARROW)));
        out.push((Button::PageNext, r(w - 10.0 - ARROW, arrow_y(panel_h), ARROW, ARROW)));
    }
    let (cl, cw) = (SIDE, w - 2.0 * SIDE);
    match page {
        Page::Music => out.extend(buttons(cx, scale, wide)),
        Page::Todo => {
            let (x0, rw) = (cl + LIST_X, cw - LIST_X);
            out.push((Button::ListInput, r(x0, INPUT_Y, rw, INPUT_H)));
            out.push((Button::ListClear, r(x0 + rw - 26.0, INPUT_Y, 26.0, INPUT_H)));
            // The settings gear, small, in the left column under the count.
            out.push((Button::Gear(Card::Tasks), r(cl + HERO_W / 2.0 - 10.0, 152.0, 20.0, 18.0)));
            if settings == Some(Card::Tasks) {
                out.push((Button::RemindStep(-1), r(x0 + rw - 112.0, ROW_Y, 22.0, 22.0)));
                out.push((Button::RemindStep(1), r(x0 + rw - 24.0, ROW_Y, 22.0, 22.0)));
            } else {
                for (row, i) in (scroll..rows.min(scroll + LIST_ROWS)).enumerate() {
                    let y = ROW_Y + row as f32 * ROW_H;
                    out.push((Button::ListToggle(i), r(x0, y, rw - 56.0, ROW_H)));
                    out.push((Button::ListEdit(i), r(x0 + rw - 54.0, y, 26.0, ROW_H)));
                    out.push((Button::ListDelete(i), r(x0 + rw - 26.0, y, 26.0, ROW_H)));
                }
            }
        }
        Page::Weather => {
            out.push((Button::ListInput, r(cl, INPUT_Y, cw, INPUT_H)));
            out.push((Button::ListClear, r(cl + cw - 26.0, INPUT_Y, 26.0, INPUT_H)));
            // The x that switches the weather off, and the °C | °F choice beside the temperature,
            // once there is one (`rows` = 1).
            if rows > 0 {
                out.push((Button::CityOff, r(cl + cw - 24.0, CITY_Y, 24.0, 24.0)));
                // The sun's path: a chip left of the unit capsule.
                out.push((Button::SunToggle, r(cl + cw - 30.0 - 8.0 - 2.0 * UNIT_W - SUN_GAP - SUN_W, CITY_Y + 3.0, SUN_W, UNIT_H)));
                for (k, unit) in [weather::Unit::C, weather::Unit::F].into_iter().enumerate() {
                    out.push((Button::Unit(unit), r(cl + cw - 30.0 - 8.0 - 2.0 * UNIT_W + k as f32 * UNIT_W, CITY_Y + 3.0, UNIT_W, UNIT_H)));
                }
            }
        }
        Page::Notify => {
            for (row, i) in (scroll..rows.min(scroll + NOTE_ROWS)).enumerate() {
                out.push((Button::NoteOpen(i), r(cl + NOTE_X, NOTE_Y + row as f32 * NOTE_H, cw - NOTE_X, NOTE_H - 2.0)));
            }
            if rows > 0 {
                out.push((Button::NotesClear, r(cl, CLEAR_Y, CLEAR_W, CLEAR_H)));
            }
        }
        Page::Usage => {
            // Only the apps found on this PC get a tab: chips side by side, from the content's left.
            let shown: Vec<usize> = (0..3).filter(|i| USAGE_APPS.load(Ordering::Relaxed) >> i & 1 == 1).collect();
            for (place, &i) in shown.iter().enumerate() {
                out.push((Button::UsageTab(i), r(cl + place as f32 * (USAGE_CHIP_W + 4.0), CARD_TOP, USAGE_CHIP_W, USAGE_TAB_H)));
            }
        }
        Page::Focus => {
            let mut x = cl;
            for kind in [focus::Kind::Focus, focus::Kind::Short, focus::Kind::Long] {
                let w = focus_chip_w(focus_chip_label(kind));
                out.push((Button::FocusMode(kind), r(x, FOCUS_CHIP_Y, w, FOCUS_CHIP_H)));
                x += w + 6.0;
            }
            // Right-aligned like the Music page's controls: restart, the white disc, next.
            let right = cl + cw;
            let (skip_x, play_x) = (right - FOCUS_SMALL, right - FOCUS_SMALL - 14.0 - FOCUS_PLAY);
            out.push((Button::FocusSkip, r(skip_x, 72.0 - FOCUS_SMALL / 2.0, FOCUS_SMALL, FOCUS_SMALL)));
            out.push((Button::FocusPlay, r(play_x, 72.0 - FOCUS_PLAY / 2.0, FOCUS_PLAY, FOCUS_PLAY)));
            out.push((Button::FocusReset, r(play_x - 14.0 - FOCUS_SMALL, 72.0 - FOCUS_SMALL / 2.0, FOCUS_SMALL, FOCUS_SMALL)));
            // The strip's hit area is taller than its 5 px so it can be hit.
            for (i, (x0, x1)) in focus_segments(cw).into_iter().enumerate() {
                out.push((Button::FocusSeg(i), r(cl + x0, FOCUS_STRIP_Y - 7.0, x1 - x0, FOCUS_STRIP_H + 14.0)));
            }
        }
        Page::Health => {
            let (card_w, bottom) = ((cw - CARD_GAP) / 2.0, card_bottom(panel_h));
            for (card, x) in [(Card::Water, cl), (Card::Sit, cl + card_w + CARD_GAP)] {
                out.push((Button::Gear(card), r(x + card_w - 28.0, CARD_TOP + 4.0, 22.0, 22.0)));
                if settings == Some(card) {
                    for (k, &(set, switch)) in setting_rows(card).iter().enumerate() {
                        let y = SET_Y + k as f32 * SET_ROW;
                        if switch {
                            out.push((Button::Toggle(set), r(x + card_w - 46.0, y + 1.0, 34.0, 16.0)));
                        } else {
                            out.push((Button::Step(set, -1), r(x + card_w - 100.0, y, 18.0, 18.0)));
                            out.push((Button::Step(set, 1), r(x + card_w - 30.0, y, 18.0, 18.0)));
                        }
                    }
                } else if card == Card::Water {
                    // The round + at the tile's bottom right, "later" beside it.
                    out.push((Button::Drink, r(x + card_w - 48.0, bottom - 44.0, 34.0, 34.0)));
                    out.push((Button::Snooze, r(x + card_w - 72.0, CARD_TOP + 28.0, 60.0, 18.0)));
                } else {
                    out.push((Button::Stood, r(x + card_w - 48.0, bottom - 44.0, 34.0, 34.0)));
                }
            }
        }
    }
    if nav_open() {
        out.extend(nav_cells(cx, scale, panel_h).into_iter().map(|(p, b)| (Button::GoPage(p), b)));
    } else {
        // Under the last row, over the dots.
        out.push((Button::Dots, r(w / 2.0 - 40.0, dots_y(panel_h) - 2.0, 80.0, 14.0)));
    }
    out
}

/// The seek bar's hit area (window px).
pub fn seek_bar(cx: f32, scale: f32, wide: bool) -> D2D_RECT_F {
    buttons(cx, scale, wide).into_iter().find(|(b, _)| *b == Button::Seek).map(|(_, r)| r).unwrap_or_default()
}

pub fn seek_at(cx: f32, scale: f32, x: f32, duration: u64, wide: bool) -> u64 {
    let r = seek_bar(cx, scale, wide);
    (((x - r.left) / (r.right - r.left)).clamp(0.0, 1.0) as f64 * duration as f64) as u64
}

pub fn hit_button(cx: f32, scale: f32, x: f32, y: f32, track: Option<&Track>, page: Page, rows: usize, scroll: usize, settings: Option<Card>, wide: bool) -> Option<Button> {
    page_buttons(cx, scale, page, rows, scroll, settings, wide).into_iter().find(|(b, r)| b.enabled(track)
        && x >= r.left && x <= r.right && y >= r.top && y <= r.bottom).map(|(b, _)| b)
}

/// The x in a notification banner's top-right corner (window px) for a banner `width` px wide
/// (logical) centred on `cx`: the glyph is 8 px, the rect around it is what a click must hit.
/// The pill's top corners are round, so its x sits further in than the ears' square ones.
pub fn banner_close(cx: f32, scale: f32, width: f32, pill: bool) -> D2D_RECT_F {
    let (inset, down) = if pill { (20.0, 18.0) } else { (12.0, 12.0) };
    let (x, y) = (cx + (width / 2.0 - inset) * scale, down * scale);
    let half = 10.0 * scale;
    rect(x - half, y - half, x + half, y + half)
}

/// The buttons of an approval banner (window px) for a banner `width` x `height` px (logical)
/// centred on `cx`, one per label in order, along the bottom edge and right-aligned.
pub fn approve_buttons(cx: f32, scale: f32, width: f32, height: f32, labels: &[String]) -> Vec<D2D_RECT_F> {
    let (top, bottom) = ((height - 34.0) * scale, (height - 8.0) * scale);
    let gap = 4.0 * scale;
    let mut right = cx + (width / 2.0 - 14.0) * scale;
    let mut out: Vec<D2D_RECT_F> = labels.iter().rev().map(|l| {
        let w = (24.0 + 7.0 * l.chars().count() as f32).clamp(56.0, 120.0) * scale;
        let b = rect(right - w, top, right, bottom);
        right -= w + gap;
        b
    }).collect();
    out.reverse();
    out
}

/// Side in px of a banner's line icon (its grid is scaled to this).
const LINE_ICON: f32 = 26.0;

// Matrices in Direct2D's row-vector convention: `a * b` applies `a` first.
fn translate(x: f32, y: f32) -> Matrix3x2 {
    Matrix3x2 { M11: 1.0, M12: 0.0, M21: 0.0, M22: 1.0, M31: x, M32: y }
}

fn scaling(k: f32) -> Matrix3x2 {
    Matrix3x2 { M11: k, M12: 0.0, M21: 0.0, M22: k, M31: 0.0, M32: 0.0 }
}

fn rotation(degrees: f32) -> Matrix3x2 {
    let (sin, cos) = degrees.to_radians().sin_cos();
    Matrix3x2 { M11: cos, M12: sin, M21: -sin, M22: cos, M31: 0.0, M32: 0.0 }
}

/// Text styles, indexed by these constants.
const F_SMALL: usize = 0;
const F_BODY: usize = 1;
const F_LABEL: usize = 2;
const F_ICON: usize = 3;
const F_TITLE: usize = 4;
const F_TINY_ICON: usize = 5;
const F_BODY_WRAP: usize = 6;
/// Text column of a notification banner: its 420 px width minus icon (18 + 36 + 12) and right pad 22.
const BANNER_TEXT_W: f32 = 420.0 - 88.0;
const BANNER_MIN_W: f32 = 220.0;
const F_SMALL_ICON: usize = 7;
const F_BIG: usize = 8;
/// Icon of a banner: larger than the 16 px of the buttons (the banner has no ring around it).
const F_BANNER_ICON: usize = 9;
/// A banner that carries a picture (a screenshot) shows it in a box this wide and `BANNER_IMAGE_H`
/// tall at the right edge, with this gap before the text.
const BANNER_IMAGE_W: f32 = 64.0;
const BANNER_IMAGE_H: f32 = 40.0;
const BANNER_IMAGE_GAP: f32 = 10.0;
/// Longest side the loader scales a banner picture to (px): the banner box at 2x.
const BANNER_IMAGE_PX: (f32, f32) = (2.0 * BANNER_IMAGE_W, 2.0 * BANNER_IMAGE_H);
/// A line of lyrics: wraps onto more lines instead of being cut short.
const F_LYRIC: usize = 10;
/// A banner's badge: the large plain number at its right end (a Bluetooth device's battery).
const F_BADGE: usize = 11;
/// The panel header: the page's name, and the date and time in the middle (the time bolder).
const F_HEAD: usize = 12;
const F_CLOCK: usize = 13;
const F_CLOCK_BOLD: usize = 14;
/// A page's big figure: the number in its ring.
const F_FIGURE: usize = 15;
/// A big faint picture behind a tile (a walking figure).
const F_MARK: usize = 16;
/// The temperature on the Weather page.
const F_TEMP: usize = 17;
/// The song's title on the Music page: large and bold, it is what the page is about.
const F_SONG: usize = 18;
/// The time left on the Focus page.
const F_TIME: usize = 19;
/// Lyrics area beside the player, logical px: its top and bottom in the panel, the space each side
/// of the thin divider after the player's controls, its right margin and its width. The right
/// margin equals the gap at the divider; the pages' clip gives way on that side while the lyrics
/// show (`LYRIC_CLIP`), leaving room for a last letter's ink past the box.
const LYRIC_TOP: f32 = 30.0;
const LYRIC_BOTTOM: f32 = 168.0;
const LYRIC_GAP: f32 = 14.0;
const LYRIC_RIGHT: f32 = LYRIC_GAP;
/// How far in from the panel's right edge the pages are clipped while the lyrics show (else `PAGE_CLIP`).
const LYRIC_CLIP: f32 = 8.0;
const LYRIC_W: f32 = LYRICS_EXTRA_W + SIDE - 2.0 * LYRIC_GAP - LYRIC_RIGHT;

pub const ICON_VOLUME: &str = "\u{E767}";
pub const ICON_MUTE: &str = "\u{E74F}";
pub const ICON_MIC: &str = "\u{E720}";
pub const ICON_MIC_OFF: &str = "\u{F781}";
pub const ICON_BT: &str = "\u{E702}";
pub const ICON_BRIGHTNESS: &str = "\u{E706}";
pub const ICON_KEYBOARD: &str = "\u{E765}";
pub const ICON_WIFI: &str = "\u{E701}";
pub const ICON_WIFI_OFF: &str = "\u{EB5E}";
pub const ICON_NO_INTERNET: &str = "\u{F384}";
pub const ICON_AIRPLANE: &str = "\u{E709}";
pub const ICON_ETHERNET: &str = "\u{E839}";
pub const ICON_MOBILE: &str = "\u{EC3B}";
pub const ICON_PLUG: &str = "\u{EBB3}";
pub const ICON_BATTERY: &str = "\u{E83F}";
pub const ICON_BATTERY_LOW: &str = "\u{E850}";
pub const ICON_SAVER: &str = "\u{EBA6}";
pub const ICON_REMINDER: &str = "\u{EC92}";
pub const ICON_WALK: &str = "\u{E805}";
pub const ICON_HEADPHONES: &str = "\u{E7F6}";
pub const ICON_SPEAKER: &str = "\u{E7F5}";
pub const ICON_PHONE_DEVICE: &str = "\u{E8EA}";
pub const ICON_MOUSE: &str = "\u{E962}";
pub const ICON_GAMEPAD: &str = "\u{E7FC}";
/// A USB device plugged in / pulled out: line icons (not font glyphs).
pub const ICON_USB: &str = "\u{1}usb";
pub const ICON_USB_OUT: &str = "\u{1}usb-out";
/// The charger was pulled out: drawn as a plug coming apart (not a font glyph).
pub const ICON_UNPLUG: &str = "\u{1}unplug";
/// Not a glyph: the banner draws the water drop shape for this.
pub const ICON_DROP: &str = "\u{1}drop";
const ICON_GEAR: &str = "\u{E713}";
const ICON_MINUS: &str = "\u{E738}";
const ICON_PLUS: &str = "\u{E710}";
const ICON_PLAY: &str = "\u{E768}";
const ICON_PAUSE: &str = "\u{E769}";
const ICON_NOTE: &str = "\u{E8D6}";
pub const ICON_WARN: &str = "\u{E7BA}";
pub const ICON_BELL: &str = "\u{EA8F}";
const ICON_LEFT: &str = "\u{E76B}";
const ICON_RIGHT: &str = "\u{E76C}";
const ICON_ADD: &str = "\u{E710}";
const ICON_CHECK: &str = "\u{E73E}";
const ICON_CLOSE: &str = "\u{E711}";
const ICON_EDIT: &str = "\u{E70F}";
const YELLOW: (f32, f32, f32) = (1.0, 0.8, 0.2);
const GREEN: (f32, f32, f32) = (0.2, 0.84, 0.29);
const BLUE: (f32, f32, f32) = (0.3, 0.62, 1.0);
const ORANGE: (f32, f32, f32) = (1.0, 0.6, 0.2);
/// The Focus page's colour for a focus segment (breaks are GREEN).
const CORAL: (f32, f32, f32) = (1.0, 0.42, 0.30);
/// Idle robot's LED eyes: near-white, like a small LED face.
const LED: (f32, f32, f32) = (0.93, 0.97, 1.0);
/// The robot's red eyes and plug when the battery is low: the battery ring's red.
const LOW_RED: (f32, f32, f32) = (1.0, 0.27, 0.23);
/// Height of the countdown bar on the bottom edge of an approval banner.
const COUNTDOWN_H: f32 = 3.0;
/// The Pomodoro line along the pill's bottom, thinner than the approval countdown.
const FOCUS_LINE_H: f32 = 2.0;

/// Banner icon for a Bluetooth device.
pub fn device_icon(kind: DeviceKind) -> &'static str {
    use DeviceKind::*;
    match kind {
        Headphones => ICON_HEADPHONES,
        Speaker => ICON_SPEAKER,
        Phone => ICON_PHONE_DEVICE,
        Mouse => ICON_MOUSE,
        Keyboard => ICON_KEYBOARD,
        Gamepad => ICON_GAMEPAD,
        Other => ICON_BT,
    }
}

struct CoverCache {
    track_key: u64,
    bytes: Option<Arc<Vec<u8>>>,
    /// Pixel sizes the bitmaps were made for.
    sizes: (u32, u32),
    /// Page-size and pill-size copies of the cover.
    bitmaps: Option<(ID2D1Bitmap, ID2D1Bitmap)>,
    /// Colour the Music page fades into while this cover is up (None: no cover).
    glow: Option<(f32, f32, f32)>,
    /// The aurora band's two colours for this cover.
    aurora: [(f32, f32, f32); 2],
}

/// A track change in progress on the Music page: the cover that is leaving (None: there was no art, so the note
/// placeholder is under it) and how far the new one has faded in, 0..1 over `anim::COVER_FADE_S`.
struct CoverFade {
    from: Option<(ID2D1Bitmap, ID2D1Bitmap)>,
    t: f32,
}

/// Shadow of the open panel: reaches SHADOW_STEPS * SHADOW_STEP px past the edge (the window has
/// 24+ px of margin below), SHADOW_ALPHA dark at the edge, shifted SHADOW_DY px down.
/// Ten wide strokes instead of fourteen narrow ones: the reach is the same 12.6 px, so the
/// falloff keeps its shape, for 4 fewer stroked outlines a frame. Fewer steps than this put more
/// than SHADOW_ALPHA * 0.2 on the innermost stroke, which reads as a line (see the shadow test).
const SHADOW_STEPS: u32 = 10;
const SHADOW_STEP: f32 = 1.26;
const SHADOW_ALPHA: f32 = 0.20;
const SHADOW_DY: f32 = 2.0;

/// Opacity of the k-th (1 = narrowest) shadow stroke. Together they make the darkness at distance
/// d from the edge fall off as SHADOW_ALPHA * (1 - d / reach)^2: soft all the way out, with no
/// visible line where the shadow starts.
fn shadow_layer_alpha(k: u32) -> f32 {
    let n = SHADOW_STEPS as f32;
    let inside = |j: u32| (1.0 - j as f32 / n).max(0.0).powi(2);
    SHADOW_ALPHA * (inside(k - 1) - inside(k))
}

/// Largest centred square of a `w` x `h` image: (x, y, side).
fn square_crop(w: u32, h: u32) -> (u32, u32, u32) {
    let side = w.min(h);
    ((w - side) / 2, (h - side) / 2, side)
}

/// Relative luminance of an sRGB colour (WCAG).
fn luminance(r: f32, g: f32, b: f32) -> f32 {
    let f = |v: f32| if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
    0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b)
}

/// The cover's shadow, a blurred dark square (`Renderer::cover_shadow`): how far it reaches past the
/// cover, how far it drops, how dark it is, and how much smaller it is drawn before being stretched
/// (the blur). Short enough to end above the pages' bottom clip (see the test).
const COVER_SHADOW_PAD: f32 = 8.0;
const COVER_SHADOW_DROP: f32 = 2.0;
const COVER_SHADOW_ALPHA: f32 = 0.55;
const COVER_SHADOW_SHRINK: f32 = 4.0;

/// Brightest the glow may get at the bottom of the panel: white text on it stays above 4.5:1.
const GLOW_MAX_LUM: f32 = 0.17;

/// `c` dimmed until its luminance is at most `max`.
fn dim_to_lum((r, g, b): (f32, f32, f32), max: f32) -> (f32, f32, f32) {
    let mut k = 0.75;
    while k > 0.05 && luminance(r * k, g * k, b * k) > max {
        k -= 0.02;
    }
    (r * k, g * k, b * k)
}

/// Secondary text: grey, shifting with `level` (0..1, the glow's strength) to white leaning
/// towards the cover's tint, so it sits in the glow instead of looking pasted on.
fn soft_text((r, g, b): (f32, f32, f32), level: f32, grey_v: f32, a: f32) -> D2D1_COLOR_F {
    let mix = |tinted: f32| grey_v + (0.8 + 0.2 * tinted - grey_v) * level;
    color(mix(r), mix(g), mix(b), a)
}

/// One animation step of the Music-page glow towards `target` (None: fade out, keeping the last
/// colour). Returns the new (colour, level) and whether it still has somewhere to go.
fn glow_step(color: (f32, f32, f32), level: f32, target: Option<(f32, f32, f32)>, dt: f32) -> ((f32, f32, f32), f32, bool) {
    let k = anim::approach(dt, anim::GLOW_FADE_S);
    let lerp = |a: f32, b: f32| a + (b - a) * k;
    match target {
        Some(t) => {
            // From invisible the colour is simply set; otherwise it slides to the new one.
            let from = if level < 0.02 { t } else { color };
            let c = (lerp(from.0, t.0), lerp(from.1, t.1), lerp(from.2, t.2));
            let l = lerp(level, 1.0);
            let done = l > 0.995 && (c.0 - t.0).abs().max((c.1 - t.1).abs()).max((c.2 - t.2).abs()) < 0.005;
            if done { (t, 1.0, false) } else { (c, l, true) }
        }
        None => {
            let l = level * (1.0 - k);
            if l < 0.005 { (color, 0.0, false) } else { (color, l, true) }
        }
    }
}


/// Tint for covers with no colour to speak of (black, white, grey): a light grey.
const GLOW_NEUTRAL: (f32, f32, f32) = (0.8, 0.8, 0.8);

/// A small copy of the cover (premultiplied BGRA pixels) -> glow colour: its most prominent hue
/// at full brightness. Pixels vote for their 30-degree hue bin by how vivid and bright they are;
/// the winning bin's colours are averaged, so one strong colour beats a muddy overall average.
/// Covers that are nearly all black or grey get GLOW_NEUTRAL.
fn glow_tint(pixels: &[u8]) -> (f32, f32, f32) {
    let bins = hue_bins(pixels);
    let (weight, sum) = bins.into_iter().fold((0.0, [0.0; 3]), |best, bin| if bin.0 > best.0 { bin } else { best });
    // Under ~1% of the pixels at a pale tint: not enough colour to bother with.
    if weight <= pixels.len() as f32 / 4.0 * 0.0005 {
        return GLOW_NEUTRAL;
    }
    full_bright(sum.map(|c| c / weight))
}

fn full_bright([r, g, b]: [f32; 3]) -> (f32, f32, f32) {
    let max = r.max(g).max(b).max(1e-6);
    (r / max, g / max, b / max)
}



/// Pixels (premultiplied BGRA) voting for their 30-degree hue bin by how vivid and bright they
/// are: (weight, weighted colour sum) per bin.
fn hue_bins(pixels: &[u8]) -> [(f32, [f32; 3]); 12] {
    let mut bins = [(0.0f32, [0.0f32; 3]); 12];
    for px in pixels.chunks_exact(4).filter(|p| p[3] > 0) {
        let un = |c: u8| (c as f32 / px[3] as f32).min(1.0);
        let (r, g, b) = (un(px[2]), un(px[1]), un(px[0]));
        let max = r.max(g).max(b);
        let chroma = max - r.min(g).min(b);
        let weight = chroma * chroma * max;
        if chroma < 0.1 || weight <= 0.0 {
            continue;
        }
        let hue = if max == r { ((g - b) / chroma).rem_euclid(6.0) } else if max == g { (b - r) / chroma + 2.0 } else { (r - g) / chroma + 4.0 };
        let bin = &mut bins[((hue * 2.0) as usize).min(11)];
        bin.0 += weight;
        for (sum, c) in bin.1.iter_mut().zip([r, g, b]) {
            *sum += weight * c;
        }
    }
    bins
}

/// Fetches app icons on a worker thread: the shell takes tens of ms for an app's first icon, which
/// showed as a stutter when the Notifications page opened. Answers are (id, width, height, premultiplied BGRA).
struct IconLoader {
    ask: Sender<String>,
    got: Receiver<LoadedIcon>,
    /// Asked for, not answered yet.
    pending: HashSet<String>,
    page_open: Arc<AtomicBool>,
    hwnd: Arc<AtomicIsize>,
}

struct LoadedIcon {
    id: String,
    pixels: Option<(u32, u32, Vec<u8>)>,
    deferred: bool,
}

impl IconLoader {
    fn start() -> Self {
        let (ask, asked) = channel::<String>();
        let (answer, got) = channel();
        let page_open = Arc::new(AtomicBool::new(false));
        let hwnd = Arc::new(AtomicIsize::new(0));
        let (open, target) = (page_open.clone(), hwnd.clone());
        crate::system::spawn("covers", move || unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let wic: Option<IWICImagingFactory> = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok();
            for id in asked {
                let deferred = !id.starts_with("img:") && !id.starts_with("now:") && !open.load(Ordering::Relaxed);
                let pixels = wic.as_ref().filter(|_| !deferred).and_then(|wic| match id.strip_prefix("img:") {
                    Some(path) => image_pixels(wic, path),
                    None => app_icon_pixels(wic, id.strip_prefix("now:").unwrap_or(&id)),
                });
                if answer.send(LoadedIcon { id, pixels, deferred }).is_err() {
                    break;
                }
                let hwnd = target.load(Ordering::Relaxed);
                if hwnd != 0 {
                    let _ = PostMessageW(Some(HWND(hwnd as _)), WM_ICONS, Default::default(), Default::default());
                }
            }
        });
        IconLoader { ask, got, pending: HashSet::new(), page_open, hwnd }
    }
}

/// The icon Windows shows for this app id (the shell's AppsFolder knows packaged and Start-menu apps alike).
unsafe fn app_icon_pixels(wic: &IWICImagingFactory, id: &str) -> Option<(u32, u32, Vec<u8>)> {
    let item: IShellItemImageFactory = SHCreateItemFromParsingName(&HSTRING::from(format!("shell:AppsFolder\\{id}")), None).ok()?;
    let hbmp = item.GetImage(SIZE { cx: 64, cy: 64 }, SIIGBF_BIGGERSIZEOK | SIIGBF_ICONONLY).ok()?;
    let src = wic.CreateBitmapFromHBITMAP(hbmp, HPALETTE::default(), WICBitmapUseAlpha);
    let _ = DeleteObject(hbmp.into());
    bgra_pixels(wic, &src.ok()?.into())
}

/// A picture file scaled down to fit the banner's picture box (at 2x), for the loader.
unsafe fn image_pixels(wic: &IWICImagingFactory, path: &str) -> Option<(u32, u32, Vec<u8>)> {
    let decoder = wic.CreateDecoderFromFilename(&HSTRING::from(path), None, GENERIC_READ, WICDecodeMetadataCacheOnDemand).ok()?;
    let frame = decoder.GetFrame(0).ok()?;
    let (mut w, mut h) = (0, 0);
    frame.GetSize(&mut w, &mut h).ok()?;
    let k = (BANNER_IMAGE_PX.0 / w.max(1) as f32).min(BANNER_IMAGE_PX.1 / h.max(1) as f32).min(1.0);
    let scaler = wic.CreateBitmapScaler().ok()?;
    scaler.Initialize(&frame, ((w as f32 * k).round() as u32).max(1), ((h as f32 * k).round() as u32).max(1), WICBitmapInterpolationModeFant).ok()?;
    bgra_pixels(wic, &scaler.into())
}

/// Any WIC bitmap as premultiplied BGRA bytes.
unsafe fn bgra_pixels(wic: &IWICImagingFactory, src: &IWICBitmapSource) -> Option<(u32, u32, Vec<u8>)> {
    let conv = wic.CreateFormatConverter().ok()?;
    conv.Initialize(src, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeMedianCut).ok()?;
    let (mut w, mut h) = (0, 0);
    conv.GetSize(&mut w, &mut h).ok()?;
    let mut pixels = vec![0u8; (w * h * 4) as usize];
    conv.CopyPixels(std::ptr::null(), w * 4, &mut pixels).ok()?;
    Some((w, h, pixels))
}

/// What the frame prepared before drawing: line icons with their geometry, the lyrics' layout and the cover's colours.
#[derive(Clone, Copy)]
struct Prep<'a> {
    weather_line: &'a Option<(&'static icons::Icon, Vec<ID2D1PathGeometry>)>,
    weather_empty: &'a Option<(&'static icons::Icon, Vec<ID2D1PathGeometry>)>,
    hour_lines: &'a [(weather::Hour, &'static icons::Icon, Vec<ID2D1PathGeometry>)],
    focus_icons: &'a [(&'static icons::Icon, Vec<ID2D1PathGeometry>)],
    player_icons: &'a [(&'static icons::Icon, Vec<ID2D1PathGeometry>)],
    note_icons: &'a [Option<ID2D1Bitmap>],
    lyric_heights: &'a [f32],
    lyric_tops: &'a [f32],
    moon_line: &'a Option<(&'static icons::Icon, Vec<ID2D1PathGeometry>)>,
    tint: (f32, f32, f32),
    level: f32,
    fonts: &'a [IDWriteTextFormat],
}

/// What each part of a frame (the pill, a banner, the panel) is drawn with: the view, the notch's geometry in window px
/// and the shared drawing helpers.
#[derive(Clone, Copy)]
struct Ctx<'a> {
    v: &'a View<'a>,
    s: f32,
    l: f32,
    r: f32,
    cx: f32,
    nh: f32,
    /// How far down a floating pill is drawn.
    gy: f32,
    shape: &'a ID2D1PathGeometry,
    text: &'a dyn Fn(&str, usize, D2D_RECT_F, DWRITE_TEXT_ALIGNMENT, D2D1_COLOR_F) -> Result<()>,
    width: &'a dyn Fn(&str, usize) -> f32,
    micon: &'a dyn Fn(&'static str, &'static str, usize, D2D_RECT_F, D2D1_COLOR_F) -> Result<()>,
    wx_icon: &'a dyn Fn(&icons::Icon, &[ID2D1PathGeometry], f32, f32, f32, Option<((f32, f32, f32), (f32, f32, f32))>, f32, f32) -> Result<()>,
    cover: &'a dyn Fn(D2D_RECT_F, f32, f32) -> Result<()>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Owner {
    Pill,
    Panel,
    Banner,
}

pub struct Renderer {
    /// What owns the notch now: it stays the panel (or the banner) while the pill shrinks back, until it is a pill again.
    /// Only the panel draws pages and the glow, so a banner that shrinks back shows none of them, however tall it was.
    owner: Owner,
    factory: ID2D1Factory,
    d3d: ID3D11Device,
    /// The Direct2D context on `d3d`: binds the swap chain's buffer as the target.
    ctx: ID2D1DeviceContext,
    /// The same context seen as a plain render target, which all the drawing uses.
    rt: ID2D1RenderTarget,
    dw: IDWriteFactory,
    wic: Option<IWICImagingFactory>,
    /// The window's swap chain and its composition tree (None until the first frame, and after the
    /// GPU was lost).
    surface: Option<Surface>,
    w: i32,
    h: i32,
    /// Where the window was last put and its region, to touch them only when they change.
    placed: (i32, i32, i32, i32),
    region: (i32, i32, i32, i32),
    /// What the Limits page shows and the app it is on (set by the window when either changes).
    pub usage: crate::usage::Snapshot,
    pub usage_tab: usize,
    /// The mouse is over the x of a notification banner (a soft disc shows behind it).
    pub close_hot: bool,
    /// Ears (v1) or floating pill (v2).
    pub style: notch::Style,
    /// Copy each frame back to `grabbed` (preview tests only).
    #[cfg(test)]
    grab: bool,
    #[cfg(test)]
    grabbed: Vec<u8>,
    fonts: Option<(f32, Vec<IDWriteTextFormat>)>,
    /// Decoded cover of the current track (keyed by Track::art_key).
    cover: Option<CoverCache>,
    /// The cover change being faded, if any (see `CoverFade`).
    cover_fade: Option<CoverFade>,
    /// The cover's blurred shadow, made once per scale: (scale, bitmap).
    cover_shadow: Option<(f32, ID2D1Bitmap)>,
    /// The aurora's two blur targets (small, mid) for one small-target size, kept across frames:
    /// a new GPU texture pair every frame stalls integrated GPUs.
    aurora_rts: RefCell<Option<((u32, u32), ID2D1BitmapRenderTarget, ID2D1BitmapRenderTarget)>>,
    /// The Music page's glow: (colour, strength 0..1, time of the last frame), see glow_step.
    glow: ((f32, f32, f32), f32, Instant),
    /// How far the pill's equaliser is "up" (0 flat .. 1 dancing): eases both ways on play / pause.
    bars: f32,
    /// The last frame showed the lyrics scrolling with the song: frames at a relaxed pace will do.
    pub slow_frames: bool,
    /// Geometry of each line-icon path, by its path data.
    line_geoms: HashMap<&'static str, ID2D1PathGeometry>,
    /// Height of each lyric line wrapped to the lyrics area, for one song (the lyrics' address)
    /// and width: measured once, not every frame.
    lyric_heights: Option<(usize, u32, Vec<f32>)>,
    lyric_motion: lyrics::Motion,
    /// Start-menu icon per app id (None: Windows has none for it); asked once per app.
    app_icons: HashMap<String, Option<ID2D1Bitmap>>,
    icon_loader: IconLoader,
    /// Icon slots that just changed glyph: (current, previous, when) for the morph.
    morphs: RefCell<HashMap<&'static str, (&'static str, &'static str, Instant)>>,
    /// The Weather page's sun view: whether the sun was up at the last frame, and when it last rose or set while the view stayed
    /// open (None: not yet, or the view has been closed since). See `sun_swap`.
    sun_seen: std::cell::Cell<Option<(bool, Option<Instant>)>>,
    /// Round caps and joins for the robot face's strokes.
    round: ID2D1StrokeStyle,
}

/// Colours of the battery icon, (bolt or first path, the rest): a green bolt on a white case while
/// charging, all yellow in battery saver (like iPhone's low power mode), all red at 20 % or less.
fn color(r: f32, g: f32, b: f32, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r, g, b, a }
}

fn grey(v: f32, a: f32) -> D2D1_COLOR_F {
    color(v, v, v, a)
}

/// First letter or digit of an app name, upper-cased; "?" when it has none.
fn app_initial(app: &str) -> String {
    app.chars().find(|c| c.is_alphanumeric()).map_or_else(|| "?".to_string(), |c| c.to_uppercase().collect())
}

/// Tile colour of an app: hue from its name, lightness fixed so white text stays readable.
fn app_tint(app: &str) -> (f32, f32, f32) {
    let hash = app.to_lowercase().bytes().fold(2166136261u32, |h, b| (h ^ b as u32).wrapping_mul(16777619));
    hue_tint((hash % 360) as f32)
}

/// HSL (saturation 0.55, lightness 0.30) at `hue` degrees.
fn hue_tint(hue: f32) -> (f32, f32, f32) {
    let (s, l) = (0.55, 0.30);
    let c = (1.0 - (2.0 * l - 1.0f32).abs()) * s;
    let x = c * (1.0 - ((hue / 60.0) % 2.0 - 1.0).abs());
    let (r, g, b) = match (hue / 60.0) as u32 { 0 => (c, x, 0.0), 1 => (x, c, 0.0), 2 => (0.0, c, x), 3 => (0.0, x, c), 4 => (x, 0.0, c), _ => (c, 0.0, x) };
    let m = l - c / 2.0;
    (r + m, g + m, b + m)
}

fn rect(l: f32, t: f32, r: f32, b: f32) -> D2D_RECT_F {
    D2D_RECT_F { left: l, top: t, right: r, bottom: b }
}

/// 83_000 ms -> "01:23"
fn clock(ms: u64) -> String {
    let s = ms / 1000;
    format!("{:02}:{:02}", s / 60, s % 60)
}

fn rounded(r: D2D_RECT_F, radius: f32) -> D2D1_ROUNDED_RECT {
    D2D1_ROUNDED_RECT { rect: r, radiusX: radius, radiusY: radius }
}

/// A window's swap chain, shown by a DirectComposition visual.
struct Surface {
    hwnd: HWND,
    swap: IDXGISwapChain1,
    _dcomp: IDCompositionDevice,
    _target: IDCompositionTarget,
    _visual: IDCompositionVisual,
}

const PIXELS: D2D1_PIXEL_FORMAT = D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED };

/// A D3D11 device on Windows' default GPU (WARP, the CPU rasteriser, when the driver gives none) and
/// a Direct2D context drawing with it. The default is the GPU showing the primary monitor, where the
/// notch sits: on a laptop with integrated + discrete GPUs that is the integrated one, so nothing is
/// copied across GPUs and the discrete one stays asleep. Only a "High performance" choice for the app
/// (Windows graphics settings, NVIDIA/AMD panel) moves it to the discrete GPU, and Windows then
/// reports the monitor there too, so no app can pick otherwise (checked 2026-10-29, UHD + RTX 4060).
fn gpu_device(factory: &ID2D1Factory) -> Result<(ID3D11Device, ID2D1DeviceContext, ID2D1RenderTarget)> {
    unsafe {
        let make = |driver: D3D_DRIVER_TYPE| -> Result<ID3D11Device> {
            let mut dev = None;
            D3D11CreateDevice(None, driver, HMODULE::default(), D3D11_CREATE_DEVICE_BGRA_SUPPORT, None, D3D11_SDK_VERSION, Some(&mut dev), None, None)?;
            dev.ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_FAIL))
        };
        let d3d = make(D3D_DRIVER_TYPE_HARDWARE).or_else(|_| make(D3D_DRIVER_TYPE_WARP))?;
        let dxgi: IDXGIDevice1 = d3d.cast()?;
        if let Ok(desc) = dxgi.GetAdapter().and_then(|a| a.GetDesc()) {
            let name = String::from_utf16_lossy(&desc.Description);
            crate::system::log(&format!("GPU: {}", name.trim_end_matches('\0')));
        }
        // One frame in flight: a frame shows on the next refresh instead of queueing behind others.
        dxgi.SetMaximumFrameLatency(1)?;
        let ctx = factory.cast::<ID2D1Factory1>()?.CreateDevice(&dxgi)?.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
        ctx.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
        let rt = ctx.cast()?;
        Ok((d3d, ctx, rt))
    }
}

impl Renderer {
    pub fn new() -> Result<Self> {
        unsafe {
            let factory: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let (d3d, ctx, rt) = gpu_device(&factory)?;
            let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let wic = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok();
            let round = factory.CreateStrokeStyle(&D2D1_STROKE_STYLE_PROPERTIES {
                startCap: D2D1_CAP_STYLE_ROUND, endCap: D2D1_CAP_STYLE_ROUND, dashCap: D2D1_CAP_STYLE_ROUND,
                lineJoin: D2D1_LINE_JOIN_ROUND, miterLimit: 1.0, dashStyle: D2D1_DASH_STYLE_SOLID, dashOffset: 0.0,
            }, None)?;
            Ok(Renderer { factory, d3d, ctx, rt, dw, wic, surface: None, w: 0, h: 0, placed: (0, 0, 0, 0), region: (0, 0, 0, 0), usage: Default::default(), usage_tab: 0, close_hot: false, style: notch::Style::Ears,
                owner: Owner::Pill,
                #[cfg(test)] grab: false, #[cfg(test)] grabbed: Vec::new(),
                fonts: None, cover: None, cover_fade: None, cover_shadow: None, aurora_rts: RefCell::new(None), glow: (GLOW_NEUTRAL, 0.0, Instant::now()), bars: 0.0, slow_frames: false, line_geoms: HashMap::new(), lyric_heights: None, lyric_motion: lyrics::Motion::default(), app_icons: HashMap::new(), icon_loader: IconLoader::start(), morphs: RefCell::new(HashMap::new()), sun_seen: std::cell::Cell::new(None), round })
        }
    }

    /// The GPU was reset or removed (driver update, sleep): start over on a new device. Every bitmap
    /// belonged to the old one, so they are made again as they are needed.
    fn lose_device(&mut self) -> Result<()> {
        (self.d3d, self.ctx, self.rt) = gpu_device(&self.factory)?;
        self.surface = None;
        self.cover = None;
        self.cover_shadow = None;
        self.cover_fade = None;
        *self.aurora_rts.get_mut() = None;
        self.app_icons.clear();
        Ok(())
    }

    /// A `w` x `h` swap chain composed onto `hwnd`, bound as the drawing target.
    fn ensure_surface(&mut self, hwnd: HWND, w: i32, h: i32) -> Result<()> {
        let same_window = self.surface.as_ref().is_some_and(|s| s.hwnd == hwnd);
        if same_window && (w, h) == (self.w, self.h) {
            return Ok(());
        }
        unsafe {
            self.ctx.SetTarget(None);
            if same_window {
                self.surface.as_ref().unwrap().swap.ResizeBuffers(0, w as u32, h as u32, DXGI_FORMAT_UNKNOWN, DXGI_SWAP_CHAIN_FLAG(0))?;
            } else {
                self.surface = None;
                let dxgi: IDXGIDevice = self.d3d.cast()?;
                let factory: IDXGIFactory2 = dxgi.GetAdapter()?.GetParent()?;
                let desc = DXGI_SWAP_CHAIN_DESC1 {
                    Width: w as u32,
                    Height: h as u32,
                    Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                    BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                    BufferCount: 2,
                    Scaling: DXGI_SCALING_STRETCH,
                    SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
                    AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
                    Flags: 0,
                    ..Default::default()
                };
                let swap = factory.CreateSwapChainForComposition(&self.d3d, &desc, None)?;
                let dcomp: IDCompositionDevice = DCompositionCreateDevice(&dxgi)?;
                let target = dcomp.CreateTargetForHwnd(hwnd, true)?;
                let visual = dcomp.CreateVisual()?;
                visual.SetContent(&swap)?;
                target.SetRoot(&visual)?;
                dcomp.Commit()?;
                self.surface = Some(Surface { hwnd, swap, _dcomp: dcomp, _target: target, _visual: visual });
            }
            let buffer: IDXGISurface = self.surface.as_ref().unwrap().swap.GetBuffer(0)?;
            let props = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: PIXELS,
                dpiX: 96.0,
                dpiY: 96.0,
                bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
                ..Default::default()
            };
            let bitmap = self.ctx.CreateBitmapFromDxgiSurface(&buffer, Some(&props))?;
            self.ctx.SetTarget(&bitmap);
        }
        self.w = w;
        self.h = h;
        Ok(())
    }

    /// Put the window at (x, y, w, h) and limit it to `region` (window px), each only on a change.
    fn place(&mut self, hwnd: HWND, at: (i32, i32, i32, i32), region: (i32, i32, i32, i32)) {
        unsafe {
            if at != self.placed {
                self.placed = at;
                let _ = SetWindowPos(hwnd, None, at.0, at.1, at.2, at.3, SWP_NOZORDER | SWP_NOACTIVATE);
            }
            if region != self.region {
                self.region = region;
                // The window owns the region from here on.
                SetWindowRgn(hwnd, Some(CreateRectRgn(region.0, region.1, region.2, region.3)), false);
            }
        }
    }

    /// The frame just drawn, as top-down BGRA rows (preview tests).
    #[cfg(test)]
    fn grab_frame(&mut self) -> Result<()> {
        unsafe {
            let back: ID3D11Texture2D = self.surface.as_ref().unwrap().swap.GetBuffer(0)?;
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            back.GetDesc(&mut desc);
            desc.Usage = D3D11_USAGE_STAGING;
            desc.BindFlags = 0;
            desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            desc.MiscFlags = 0;
            let mut staging = None;
            self.d3d.CreateTexture2D(&desc, None, Some(&mut staging))?;
            let staging = staging.unwrap();
            let ctx = self.d3d.GetImmediateContext()?;
            ctx.CopyResource(&staging, &back);
            let mut map = D3D11_MAPPED_SUBRESOURCE::default();
            ctx.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut map))?;
            let row = desc.Width as usize * 4;
            self.grabbed.clear();
            for y in 0..desc.Height as usize {
                self.grabbed.extend_from_slice(std::slice::from_raw_parts((map.pData as *const u8).add(y * map.RowPitch as usize), row));
            }
            ctx.Unmap(&staging, 0);
        }
        Ok(())
    }

    fn fonts(&mut self, scale: f32) -> Result<Vec<IDWriteTextFormat>> {
        if self.fonts.as_ref().map(|f| f.0) != Some(scale) {
            let make = |family: PCWSTR, size: f32, weight: DWRITE_FONT_WEIGHT| -> Result<IDWriteTextFormat> {
                unsafe {
                    let f = self.dw.CreateTextFormat(family, None, weight, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, size * scale, w!("vi-VN"))?;
                    f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
                    f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
                    let sign = self.dw.CreateEllipsisTrimmingSign(&f)?;
                    let trim = DWRITE_TRIMMING { granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER, delimiter: 0, delimiterCount: 0 };
                    f.SetTrimming(&trim, &sign)?;
                    Ok(f)
                }
            };
            // Same body text, but wrapping onto more lines (top-aligned). Text past the box's last
            // line ends with "…" after a whole word, never mid-word.
            let wrap = |f: IDWriteTextFormat| -> Result<IDWriteTextFormat> {
                unsafe {
                    f.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP)?;
                    f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR)?;
                    let sign = self.dw.CreateEllipsisTrimmingSign(&f)?;
                    let trim = DWRITE_TRIMMING { granularity: DWRITE_TRIMMING_GRANULARITY_WORD, delimiter: 0, delimiterCount: 0 };
                    f.SetTrimming(&trim, &sign)?;
                }
                Ok(f)
            };
            let ui = w!("Segoe UI");
            // The header uses Windows 11's display cut of Segoe UI (drawn for large sizes, finer than the text
            // cut at 18 px) when it is installed, else the plain one.
            let display = unsafe {
                let (mut collection, mut index, mut found) = (None, 0u32, false.into());
                let has = self.dw.GetSystemFontCollection(&mut collection, false).is_ok()
                    && collection.is_some_and(|c| c.FindFamilyName(w!("Segoe UI Variable Display"), &mut index, &mut found).is_ok() && found.as_bool());
                if has { w!("Segoe UI Variable Display") } else { ui }
            };
            let set = vec![
                make(ui, 15.0, DWRITE_FONT_WEIGHT_BOLD)?,
                make(ui, 13.0, DWRITE_FONT_WEIGHT_NORMAL)?,
                make(ui, 12.0, DWRITE_FONT_WEIGHT_NORMAL)?,
                make(w!("Segoe Fluent Icons"), 16.0, DWRITE_FONT_WEIGHT_NORMAL)?,
                make(ui, 14.0, DWRITE_FONT_WEIGHT_SEMI_BOLD)?,
                make(w!("Segoe Fluent Icons"), 8.0, DWRITE_FONT_WEIGHT_NORMAL)?,
                wrap(make(ui, 13.0, DWRITE_FONT_WEIGHT_NORMAL)?)?,
                make(w!("Segoe Fluent Icons"), 12.0, DWRITE_FONT_WEIGHT_NORMAL)?,
                make(ui, 24.0, DWRITE_FONT_WEIGHT_SEMI_BOLD)?,
                make(w!("Segoe Fluent Icons"), 22.0, DWRITE_FONT_WEIGHT_NORMAL)?,
                {
                    let f = make(ui, 14.0, DWRITE_FONT_WEIGHT_SEMI_BOLD)?;
                    unsafe {
                        f.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP)?;
                        f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR)?;
                        f.SetTrimming(&DWRITE_TRIMMING { granularity: DWRITE_TRIMMING_GRANULARITY_NONE, delimiter: 0, delimiterCount: 0 }, None)?;
                    }
                    f
                },
                make(ui, 30.0, DWRITE_FONT_WEIGHT_NORMAL)?,
                make(display, 18.0, DWRITE_FONT_WEIGHT_SEMI_BOLD)?,
                make(display, 14.0, DWRITE_FONT_WEIGHT_NORMAL)?,
                make(display, 14.0, DWRITE_FONT_WEIGHT_SEMI_BOLD)?,
                make(display, 30.0, DWRITE_FONT_WEIGHT_SEMI_BOLD)?,
                make(w!("Segoe Fluent Icons"), 96.0, DWRITE_FONT_WEIGHT_NORMAL)?,
                make(display, 40.0, DWRITE_FONT_WEIGHT_SEMI_BOLD)?,
                make(display, 21.0, DWRITE_FONT_WEIGHT_BOLD)?,
                make(display, 46.0, DWRITE_FONT_WEIGHT_MEDIUM)?,
            ];
            self.fonts = Some((scale, set));
        }
        Ok(self.fonts.as_ref().unwrap().1.clone())
    }

    /// Decode again when the track's asynchronous artwork arrives.
    /// The cover as two square bitmaps resampled to exactly `big` and `small` px (the Music page
    /// and the pill), so it is drawn 1:1 instead of being shrunk by the GPU's plain linear filter.
    fn cover(&mut self, t: &Track, big: u32, small: u32) -> Option<(ID2D1Bitmap, ID2D1Bitmap)> {
        let cached = self.cover.as_ref().is_some_and(|cache| cache.track_key == t.art_key && cache.sizes == (big, small)
            && match (&cache.bytes, &t.art) { (None, None) => true, (Some(a), Some(b)) => Arc::ptr_eq(a, b), _ => false });
        if !cached {
            let decoded = t.art.as_ref().and_then(|bytes| unsafe {
                let wic = self.wic.as_ref()?;
                let stream = wic.CreateStream().ok()?;
                stream.InitializeFromMemory(bytes).ok()?;
                let decoder = wic.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand).ok()?;
                let frame = decoder.GetFrame(0).ok()?;
                let conv = wic.CreateFormatConverter().ok()?;
                conv.Initialize(&frame, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeMedianCut).ok()?;
                // Glow colour: from the cover scaled down to 16x16.
                let tiny = wic.CreateBitmapScaler().ok()?;
                tiny.Initialize(&conv, 16, 16, WICBitmapInterpolationModeFant).ok()?;
                let mut px = [0u8; 16 * 16 * 4];
                tiny.CopyPixels(std::ptr::null(), 16 * 4, &mut px).ok()?;
                // Centre square, then Fant (area averaging) down or cubic up to the drawn size.
                let (mut w, mut h) = (0, 0);
                conv.GetSize(&mut w, &mut h).ok()?;
                let (x, y, side) = square_crop(w, h);
                let clip = wic.CreateBitmapClipper().ok()?;
                clip.Initialize(&conv, &WICRect { X: x as i32, Y: y as i32, Width: side as i32, Height: side as i32 }).ok()?;
                let fit = |px: u32| -> Option<ID2D1Bitmap> {
                    let scaler = wic.CreateBitmapScaler().ok()?;
                    let mode = if px < side { WICBitmapInterpolationModeFant } else { WICBitmapInterpolationModeHighQualityCubic };
                    scaler.Initialize(&clip, px, px, mode).ok()?;
                    self.rt.CreateBitmapFromWicBitmap(&scaler, None).ok()
                };
                Some(((fit(big)?, fit(small)?), glow_tint(&px), aurora::tints(&px)))
            });
            let (bitmaps, glow, aurora) = match decoded { Some((b, g, au)) => (Some(b), Some(g), au), None => (None, None, [GLOW_NEUTRAL; 2]) };
            // Fade when the picture changes: a new track, or art that arrives late for the same track (over the note until
            // then); re-making the same picture at another size does not.
            let same_art = |old: &CoverCache| match (&old.bytes, &t.art) { (None, None) => true, (Some(a), Some(b)) => Arc::ptr_eq(a, b), _ => false };
            self.cover_fade = match (&self.cover, &bitmaps) {
                (Some(old), Some(_)) if old.track_key != t.art_key || !same_art(old) => Some(CoverFade { from: old.bitmaps.clone(), t: 0.0 }),
                _ => None,
            };
            self.cover = Some(CoverCache { track_key: t.art_key, bytes: t.art.clone(), sizes: (big, small), bitmaps, glow, aurora });
        }
        self.cover.as_ref().and_then(|c| c.bitmaps.clone())
    }

    /// The path `d` of a line icon as geometry (stroked, never filled), built once. None when it
    /// cannot be parsed or built.
    fn line_geometry(&mut self, d: &'static str) -> Option<ID2D1PathGeometry> {
        if let Some(g) = self.line_geoms.get(d) {
            return Some(g.clone());
        }
        let segs = icons::parse(d)?;
        let p = |x: f32, y: f32| Vector2 { X: x, Y: y };
        let geo = unsafe {
            let geo = self.factory.CreatePathGeometry().ok()?;
            let sink = geo.Open().ok()?;
            let mut open = false;
            for seg in segs {
                match seg {
                    Seg::Move(x, y) => {
                        if open {
                            sink.EndFigure(D2D1_FIGURE_END_OPEN);
                        }
                        sink.BeginFigure(p(x, y), D2D1_FIGURE_BEGIN_HOLLOW);
                        open = true;
                    }
                    Seg::Line(x, y) => sink.AddLine(p(x, y)),
                    Seg::Cubic([x1, y1, x2, y2, x, y]) => sink.AddBezier(&D2D1_BEZIER_SEGMENT { point1: p(x1, y1), point2: p(x2, y2), point3: p(x, y) }),
                    Seg::Arc { rx, ry, rot, large, sweep, x, y } => sink.AddArc(&D2D1_ARC_SEGMENT {
                        point: p(x, y),
                        size: D2D_SIZE_F { width: rx, height: ry },
                        rotationAngle: rot,
                        sweepDirection: if sweep { D2D1_SWEEP_DIRECTION_CLOCKWISE } else { D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE },
                        arcSize: if large { D2D1_ARC_SIZE_LARGE } else { D2D1_ARC_SIZE_SMALL },
                    }),
                    Seg::Close => {
                        sink.EndFigure(D2D1_FIGURE_END_CLOSED);
                        open = false;
                    }
                }
            }
            if open {
                sink.EndFigure(D2D1_FIGURE_END_OPEN);
            }
            sink.Close().ok()?;
            geo
        };
        self.line_geoms.insert(d, geo.clone());
        Some(geo)
    }

    /// Height in px of each lyric line wrapped to `width` px (at least `min_h`, for blank lines).
    fn lyric_heights(&mut self, lines: &Arc<Vec<(u64, String)>>, width: f32, min_h: f32, fonts: &[IDWriteTextFormat]) -> Vec<f32> {
        let (key, w) = (Arc::as_ptr(lines) as usize, width.round() as u32);
        if let Some((k, kw, heights)) = &self.lyric_heights {
            if *k == key && *kw == w {
                return heights.clone();
            }
        }
        let heights: Vec<f32> = lines
            .iter()
            .map(|(_, text)| unsafe {
                let units: Vec<u16> = text.encode_utf16().collect();
                let mut m = DWRITE_TEXT_METRICS::default();
                match self.dw.CreateTextLayout(&units, &fonts[F_LYRIC], width, 4000.0) {
                    Ok(layout) if layout.GetMetrics(&mut m).is_ok() => m.height.max(min_h),
                    _ => min_h,
                }
            })
            .collect();
        self.lyric_motion = lyrics::Motion::default();
        self.lyric_heights = Some((key, w, heights.clone()));
        heights
    }

    /// The icon Windows shows for this app id, once its loader has it (None until then, and for
    /// apps Windows has no icon for). First asking starts the fetch.
    fn app_icon(&mut self, id: &str) -> Option<ID2D1Bitmap> {
        self.prefetch_icon(id);
        self.app_icons.get(id).cloned().flatten()
    }

    /// The icon of the app a notification banner comes from: asked for when the banner shows, whichever page is open
    /// (the banner keeps its bell until it arrives).
    fn banner_icon(&mut self, aumid: &str) -> Option<ID2D1Bitmap> {
        let key = format!("now:{aumid}");
        if !self.app_icons.contains_key(&key) && self.icon_loader.pending.insert(key.clone()) {
            let _ = self.icon_loader.ask.send(key.clone());
        }
        self.app_icons.get(&key).cloned().flatten()
    }

    /// Ask for an app's icon ahead of time, so the Notifications page finds it ready.
    pub fn set_page_open(&self, open: bool) {
        self.icon_loader.page_open.store(open, Ordering::Relaxed);
    }

    fn prefetch_icon(&mut self, id: &str) {
        if !id.starts_with("img:") && !self.icon_loader.page_open.load(Ordering::Relaxed) { return; }
        if !self.app_icons.contains_key(id) && self.icon_loader.pending.insert(id.to_string()) {
            let _ = self.icon_loader.ask.send(id.to_string());
        }
    }

    /// Turn the icons the loader has finished into bitmaps.
    fn take_icons(&mut self) {
        while let Ok(LoadedIcon { id, pixels, deferred }) = self.icon_loader.got.try_recv() {
            self.icon_loader.pending.remove(&id);
            if deferred { continue; }
            let bitmap = pixels.and_then(|(w, h, px)| unsafe {
                let props = D2D1_BITMAP_PROPERTIES { pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED }, dpiX: 96.0, dpiY: 96.0 };
                self.rt.CreateBitmap(D2D_SIZE_U { width: w, height: h }, Some(px.as_ptr() as *const _), w * 4, &props).ok()
            });
            self.app_icons.insert(id, bitmap);
        }
    }

    /// A solid brush of this colour. D2D makes them cheaply (measured: caching them saved
    /// nothing), so nothing is kept between frames.
    fn brush(&self, c: D2D1_COLOR_F) -> Result<ID2D1SolidColorBrush> {
        unsafe { self.rt.CreateSolidColorBrush(&c, None) }
    }

    /// iPhone-style notch: flat top on the screen edge, concave "ears" flaring into it,
    /// straight sides, round bottom corners.
    fn notch_shape(&self, l: f32, r: f32, h: f32, radius: f32, ear: f32) -> Result<ID2D1PathGeometry> {
        if self.style == notch::Style::Pill {
            return self.pill_shape(l, r, h, radius);
        }
        let p = |x: f32, y: f32| Vector2 { X: x, Y: y };
        let rad = radius.min(h - ear).min((r - l) / 2.0).max(0.0);
        let arc = |to: Vector2, size: f32, dir: D2D1_SWEEP_DIRECTION| D2D1_ARC_SEGMENT {
            point: to,
            size: D2D_SIZE_F { width: size, height: size },
            rotationAngle: 0.0,
            sweepDirection: dir,
            arcSize: D2D1_ARC_SIZE_SMALL,
        };
        unsafe {
            let geo = self.factory.CreatePathGeometry()?;
            let sink = geo.Open()?;
            sink.BeginFigure(p(l - ear, 0.0), D2D1_FIGURE_BEGIN_FILLED);
            sink.AddArc(&arc(p(l, ear), ear, D2D1_SWEEP_DIRECTION_CLOCKWISE));
            sink.AddLine(p(l, h - rad));
            sink.AddArc(&arc(p(l + rad, h), rad, D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE));
            sink.AddLine(p(r - rad, h));
            sink.AddArc(&arc(p(r, h - rad), rad, D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE));
            sink.AddLine(p(r, ear));
            sink.AddArc(&arc(p(r + ear, 0.0), ear, D2D1_SWEEP_DIRECTION_CLOCKWISE));
            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            sink.Close()?;
            Ok(geo)
        }
    }

    /// The floating pill: all four corners round, a capsule while it is short; `radius` (the
    /// notch's own, a little under half the pill's height) caps the rounding once it opens.
    fn pill_shape(&self, l: f32, r: f32, h: f32, radius: f32) -> Result<ID2D1PathGeometry> {
        let p = |x: f32, y: f32| Vector2 { X: x, Y: y };
        let rad = (h / 2.0).min(radius * 1.1).min((r - l) / 2.0).max(0.0);
        let arc = |to: Vector2| D2D1_ARC_SEGMENT {
            point: to,
            size: D2D_SIZE_F { width: rad, height: rad },
            rotationAngle: 0.0,
            sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
            arcSize: D2D1_ARC_SIZE_SMALL,
        };
        unsafe {
            let geo = self.factory.CreatePathGeometry()?;
            let sink = geo.Open()?;
            sink.BeginFigure(p(l + rad, 0.0), D2D1_FIGURE_BEGIN_FILLED);
            sink.AddLine(p(r - rad, 0.0));
            sink.AddArc(&arc(p(r, rad)));
            sink.AddLine(p(r, h - rad));
            sink.AddArc(&arc(p(r - rad, h)));
            sink.AddLine(p(l + rad, h));
            sink.AddArc(&arc(p(l, h - rad)));
            sink.AddLine(p(l, rad));
            sink.AddArc(&arc(p(l + rad, 0.0)));
            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            sink.Close()?;
            Ok(geo)
        }
    }

    /// Idle robot's LED face centred on (cx, cy): eyes 14 px either side, the whole face shifted along
    /// `e.gaze`. Strokes have round ends like the glowing lines of an LED face, and each eye sits in a halo
    /// of three widening, fainter layers that reads as a blur. Asleep a pixel "z" drifts up beside the
    /// face, cheering two sparkles twinkle there.
    fn draw_face(&self, e: &Eyes, cx: f32, cy: f32, s: f32, a: f32) -> Result<()> {
        let rt = &self.rt;
        // The cheering face is green, like the battery ring while it charges; the sad one red, like it at 20 %.
        let ink = match e.face {
            Face::Happy => GREEN,
            Face::Sad => LOW_RED,
            _ => LED,
        };
        let led = |alpha: f32| unsafe { rt.CreateSolidColorBrush(&color(ink.0, ink.1, ink.2, alpha), None) };
        let lit = led(a)?;
        // (how far the layer grows past the shape in logical px, its opacity), the widest and faintest last.
        let halo: Vec<(f32, ID2D1SolidColorBrush)> = [(1.2, 0.16), (2.6, 0.09), (4.4, 0.045)].into_iter().map(|(grow, alpha)| led(a * alpha).map(|b| (grow, b))).collect::<Result<_>>()?;
        let p = |x: f32, y: f32| Vector2 { X: x, Y: y };
        // Quadratic curve from `a` through control `c` to `b`, as points.
        let quad = |a: Vector2, c: Vector2, b: Vector2| -> Vec<Vector2> {
            (0..=8).map(|i| {
                let t = i as f32 / 8.0;
                let u = 1.0 - t;
                p(u * u * a.X + 2.0 * u * t * c.X + t * t * b.X, u * u * a.Y + 2.0 * u * t * c.Y + t * t * b.Y)
            }).collect()
        };
        let path = |pts: &[Vector2], closed: bool| -> Result<ID2D1PathGeometry> {
            unsafe {
                let geo = self.factory.CreatePathGeometry()?;
                let sink = geo.Open()?;
                sink.BeginFigure(pts[0], if closed { D2D1_FIGURE_BEGIN_FILLED } else { D2D1_FIGURE_BEGIN_HOLLOW });
                sink.AddLines(&pts[1..]);
                sink.EndFigure(if closed { D2D1_FIGURE_END_CLOSED } else { D2D1_FIGURE_END_OPEN });
                sink.Close()?;
                Ok(geo)
            }
        };
        let stroke = |pts: &[Vector2], width: f32| -> Result<()> {
            let geo = path(pts, false)?;
            unsafe {
                for (grow, b) in &halo {
                    rt.DrawGeometry(&geo, b, (width + 2.0 * grow) * s, &self.round);
                }
                rt.DrawGeometry(&geo, &lit, width * s, &self.round);
            }
            Ok(())
        };
        let block = |x: f32, y: f32, w: f32, h: f32| unsafe {
            for (grow, b) in &halo {
                let g = grow * s;
                rt.FillRoundedRectangle(&rounded(rect(x - w / 2.0 - g, y - h / 2.0 - g, x + w / 2.0 + g, y + h / 2.0 + g), 1.5 * s + g), b);
            }
            rt.FillRoundedRectangle(&rounded(rect(x - w / 2.0, y - h / 2.0, x + w / 2.0, y + h / 2.0), 1.5 * s), &lit);
        };
        // The eyes sit on the pill's centre line; the face moves along the gaze.
        let (fx, ey) = (cx + e.gaze.0 * 3.0 * s, cy + e.gaze.1 * 2.4 * s);
        // An open eye `h` logical px tall, squeezed by a blink (never below a thin bar).
        let lid = |h: f32| (h * e.open).max(1.6) * s;
        let u = s;
        for side in [-1.0f32, 1.0] {
            let x = fx + side * 14.0 * s;
            match e.face {
                Face::Normal => block(x, ey, 7.0 * s, lid(EYE_H)),
                Face::Curious => {
                    let (left, right) = curious_eyes(e.curious);
                    let h = if side < 0.0 { left } else { right };
                    // Bottoms line up (half the mean height below the centre), so the big eye grows upward.
                    block(x, ey + ((left + right) / 4.0 - h / 2.0) * s, 7.0 * s, lid(h))
                }
                Face::Greet => stroke(&[p(x - 4.0 * u, ey + 2.5 * u), p(x, ey - 2.5 * u), p(x + 4.0 * u, ey + 2.5 * u)], 2.0)?,
                Face::Happy => {
                    let d = -side; // '>' on the left, '<' on the right
                    stroke(&[p(x - 3.0 * d * u, ey - 4.0 * u), p(x + 3.0 * d * u, ey), p(x - 3.0 * d * u, ey + 4.0 * u)], 2.0)?
                }
                Face::Sad => {
                    let d = -side; // inner corner lower: droopy eyes
                    // Blinking squeezes the eye toward its middle (ey + 1).
                    let v = |dy: f32| ey + 1.0 * u + (dy - 1.0) * u * e.open.max(0.18);
                    let pts = [p(x - 3.5 * u, v(-2.0 - d * 1.5)), p(x + 3.5 * u, v(-2.0 + d * 1.5)), p(x + 3.5 * u, v(4.0)), p(x - 3.5 * u, v(4.0))];
                    let geo = path(&pts, true)?;
                    unsafe {
                        for (grow, b) in &halo {
                            rt.DrawGeometry(&geo, b, 2.0 * grow * s, &self.round);
                        }
                        rt.FillGeometry(&geo, &lit, None);
                    }
                }
                Face::Sleep => stroke(&quad(p(x - 4.0 * u, ey - 1.0 * u), p(x, ey + 3.0 * u), p(x + 4.0 * u, ey - 1.0 * u)), 2.0)?,
            }
        }
        if e.face == Face::Happy {
            // Two white sparkles beside the green eyes, twinkling out of step.
            let white = |alpha: f32| unsafe { rt.CreateSolidColorBrush(&color(LED.0, LED.1, LED.2, alpha), None) };
            let (spark, glow) = (white(a)?, white(a * 0.14)?);
            for (k, (dx, dy, size)) in [(35.0f32, -4.0f32, 3.2f32), (42.0, 4.0, 2.4)].into_iter().enumerate() {
                let r = size * (0.75 + 0.25 * (e.time * 7.0 + k as f32 * 2.0).sin()) * s;
                let (x, y, c) = (cx + dx * s, cy + dy * s, 0.3 * r);
                let star = path(&[p(x, y - r), p(x + c, y - c), p(x + r, y), p(x + c, y + c), p(x, y + r), p(x - c, y + c), p(x - r, y), p(x - c, y - c)], true)?;
                unsafe {
                    rt.DrawGeometry(&star, &glow, 3.0 * s, &self.round);
                    rt.FillGeometry(&star, &spark, None);
                }
            }
        }
        if e.face == Face::Sad {
            // A plug beside the face that fades in and out slowly: plug in the charger.
            let k = 0.55 + 0.45 * (0.5 + 0.5 * (e.time * std::f32::consts::TAU / 1.5).sin());
            let (plug, glow) = (led(a * k)?, led(a * k * 0.16)?);
            let at = |dx: f32, dy: f32| p(cx + dx * s, cy + dy * s);
            let body = rounded(rect(cx + 31.0 * s, cy - 3.0 * s, cx + 40.0 * s, cy + 3.5 * s), 1.8 * s);
            let lines = [path(&[at(33.5, -6.0), at(33.5, -3.0)], false)?, path(&[at(37.5, -6.0), at(37.5, -3.0)], false)?, path(&[at(35.5, 3.5), at(35.5, 6.5)], false)?];
            unsafe {
                for (width, brush) in [(5.5, &glow), (1.5, &plug)] {
                    rt.DrawRoundedRectangle(&body, brush, width * s, &self.round);
                    for line in &lines {
                        rt.DrawGeometry(line, brush, width * s, &self.round);
                    }
                }
            }
        }
        if e.face == Face::Sleep {
            // Pixel "z" on a 4x4 grid: top row, two steps down the diagonal, bottom row.
            for k in 0..2 {
                let t = (e.time * 0.45 + k as f32 * 0.5).fract();
                // Whole device pixels keep the little squares crisp.
                let q = ((1.5 + t * 0.5) * s).round().max(1.0);
                let (x, y) = ((cx + (30.0 + t * 12.0) * s).round(), (cy + (1.0 - t * 15.0) * s).round());
                let zb = led(a * (1.0 - t))?;
                let z = [(0.0, 0.0), (1.0, 0.0), (2.0, 0.0), (3.0, 0.0), (2.0, 1.0), (1.0, 2.0), (0.0, 3.0), (1.0, 3.0), (2.0, 3.0), (3.0, 3.0)];
                for (i, j) in z {
                    unsafe { rt.FillRectangle(&rect(x + i * q, y + j * q, x + (i + 1.0) * q, y + (j + 1.0) * q), &zb) };
                }
            }
        }
        Ok(())
    }

    /// Water drop icon `size` tall centred on (cx, cy): pointed top, round bottom.
    fn water_drop(&self, cx: f32, cy: f32, size: f32) -> Result<ID2D1PathGeometry> {
        let p = |x: f32, y: f32| Vector2 { X: cx + x * size, Y: cy + y * size };
        let r = 0.33;
        let bottom = 0.5 - r; // centre of the round part
        let arc = |to: Vector2| D2D1_ARC_SEGMENT { point: to, size: D2D_SIZE_F { width: r * size, height: r * size }, rotationAngle: 0.0, sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE, arcSize: D2D1_ARC_SIZE_SMALL };
        unsafe {
            let geo = self.factory.CreatePathGeometry()?;
            let sink = geo.Open()?;
            sink.BeginFigure(p(0.0, -0.5), D2D1_FIGURE_BEGIN_FILLED);
            sink.AddBezier(&D2D1_BEZIER_SEGMENT { point1: p(0.06, -0.3), point2: p(r, -0.08), point3: p(r, bottom) });
            sink.AddArc(&arc(p(0.0, 0.5)));
            sink.AddArc(&arc(p(-r, bottom)));
            sink.AddBezier(&D2D1_BEZIER_SEGMENT { point1: p(-r, -0.08), point2: p(-0.06, -0.3), point3: p(0.0, -0.5) });
            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            sink.Close()?;
            Ok(geo)
        }
    }


    /// Draw one frame and push it to `hwnd` placed at screen (x, y), window size (w, h) device px.
    /// Returns true while an icon morph is still playing (the caller keeps frames coming).
    /// Width (design px) a status banner needs to hold its title and body beside the icon, never
    /// narrower than `BANNER_MIN_W` (the level bar of volume and brightness needs room).
    pub fn status_banner_width(&mut self, title: &str, body: &str, scale: f32) -> f32 {
        let Ok(fonts) = self.fonts(scale) else { return notch::alert_size(0)[0] };
        let line = |text: &str, font: usize| -> f32 {
            let u: Vec<u16> = text.encode_utf16().collect();
            let mut m = DWRITE_TEXT_METRICS::default();
            unsafe {
                match self.dw.CreateTextLayout(&u, &fonts[font], 10_000.0, 40.0) {
                    Ok(layout) if layout.GetMetrics(&mut m).is_ok() => m.widthIncludingTrailingWhitespace / scale,
                    _ => 0.0,
                }
            }
        };
        // Left margin, icon and its gap, the text, the right margin.
        (18.0 + 36.0 + 12.0 + line(title, F_TITLE).max(line(body, F_BODY)).ceil() + 22.0).clamp(BANNER_MIN_W, 420.0)
    }

    /// Lines `text` wraps to in a notification banner (its body font and width, at `scale`).
    pub fn banner_lines(&mut self, text: &str, scale: f32, with_image: bool, choice: bool) -> u32 {
        let Ok(fonts) = self.fonts(scale) else { return 1 };
        let u: Vec<u16> = text.encode_utf16().collect();
        let mut m = DWRITE_TEXT_METRICS::default();
        // A question with buttons is wider than a notification; its text has that room.
        let width = BANNER_TEXT_W + if choice { notch::alert_size_choice(1)[0] - notch::alert_size(1)[0] } else { 0.0 } - if with_image { BANNER_IMAGE_W + BANNER_IMAGE_GAP } else { 0.0 };
        unsafe {
            match self.dw.CreateTextLayout(&u, &fonts[F_BODY_WRAP], width * scale, 10_000.0) {
                Ok(layout) if layout.GetMetrics(&mut m).is_ok() => m.lineCount.max(1),
                _ => 1,
            }
        }
    }

    /// The big cover's shadow at `scale`: a dark rounded square COVER_SHADOW_PAD larger all round,
    /// drawn COVER_SHADOW_SHRINK times smaller and stretched back in two linear steps, which blurs
    /// it into one smooth falloff. Made once; drawn at twice its size.
    fn cover_shadow(&mut self, scale: f32) -> Option<ID2D1Bitmap> {
        if let Some((s, bmp)) = &self.cover_shadow {
            if *s == scale {
                return Some(bmp.clone());
            }
        }
        let (k, side, pad) = (COVER_SHADOW_SHRINK, COVER * scale, COVER_SHADOW_PAD * scale);
        let n = side + 2.0 * pad;
        let clear = color(0.0, 0.0, 0.0, 0.0);
        let mode = D2D1_COMPATIBLE_RENDER_TARGET_OPTIONS_NONE;
        let bmp = unsafe {
            let tiny = self.rt.CreateCompatibleRenderTarget(Some(&D2D_SIZE_F { width: n / k, height: n / k }), None, None, mode).ok()?;
            tiny.BeginDraw();
            tiny.Clear(Some(&clear));
            let body = rect(pad / k, pad / k, (pad + side) / k, (pad + side) / k);
            tiny.FillRoundedRectangle(&rounded(body, 14.0 * scale / k), &tiny.CreateSolidColorBrush(&color(0.0, 0.0, 0.0, 1.0), None).ok()?);
            tiny.EndDraw(None, None).ok()?;
            let mid = self.rt.CreateCompatibleRenderTarget(Some(&D2D_SIZE_F { width: n / 2.0, height: n / 2.0 }), None, None, mode).ok()?;
            mid.BeginDraw();
            mid.Clear(Some(&clear));
            mid.DrawBitmap(&tiny.GetBitmap().ok()?, Some(&rect(0.0, 0.0, n / 2.0, n / 2.0)), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None);
            mid.EndDraw(None, None).ok()?;
            mid.GetBitmap().ok()?
        };
        self.cover_shadow = Some((scale, bmp.clone()));
        Some(bmp)
    }

    /// One frame. A failure other than the handled device loss (the GPU switching with the power
    /// plan or battery saver, a driver reset) used to stop all drawing silently: it is logged, and
    /// the device is made again so the next frame draws.
    pub fn present(&mut self, hwnd: HWND, x: i32, y: i32, w: i32, h: i32, scale: f32, v: &View) -> Result<bool> {
        match self.present_frame(hwnd, x, y, w, h, scale, v) {
            Err(e) => {
                // Once a second at most: a GPU that stays gone must not make a device every frame.
                static LAST_MS: AtomicU64 = AtomicU64::new(0);
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
                if now.saturating_sub(LAST_MS.load(Ordering::Relaxed)) < 1000 {
                    return Err(e);
                }
                LAST_MS.store(now, Ordering::Relaxed);
                crate::system::log(&format!("draw failed: {e} ({:?}); making the device again", e.code()));
                self.lose_device()?;
                Ok(true)
            }
            ok => ok,
        }
    }

    fn present_frame(&mut self, hwnd: HWND, x: i32, y: i32, w: i32, h: i32, scale: f32, v: &View) -> Result<bool> {
        let page = visible_page(v.mode, false, v.page);
        self.set_page_open(page == Some(Page::Notify));
        self.icon_loader.hwnd.store(hwnd.0 as isize, Ordering::Relaxed);
        self.ensure_surface(hwnd, w, h)?;
        self.take_icons();
        let fonts = self.fonts(scale)?;
        let cover = if page == Some(Page::Music) || (v.live && matches!(v.mode, Mode::Collapsed | Mode::Peek)) {
            v.music.and_then(|t| self.cover(t, (COVER * scale).round() as u32, (20.0 * scale).round() as u32))
        } else { v.music.and_then(|t| self.cover.as_ref().filter(|c| c.track_key == t.art_key).and_then(|c| c.bitmaps.clone())) };
        let cover_shadow = cover.as_ref().and_then(|_| self.cover_shadow(scale));
        let target = if v.live && page == Some(Page::Music) { self.cover.as_ref().and_then(|c| c.glow) } else { None };
        let now = Instant::now();
        let elapsed = now.duration_since(self.glow.2).as_secs_f32();
        let dt = elapsed.min(0.1);
        let (tint, level, glow_busy) = glow_step(self.glow.0, self.glow.1, target, dt);
        self.glow = (tint, level, now);
        // A new cover fades in over the old one for COVER_FADE_S, then the fade is dropped (and the old bitmaps with it).
        // It only plays while the Music page is open; a change seen elsewhere is not replayed later.
        if page != Some(Page::Music) || v.mode != Mode::Expanded {
            self.cover_fade = None;
        }
        if let Some(f) = &mut self.cover_fade {
            f.t += dt / anim::COVER_FADE_S;
        }
        if self.cover_fade.as_ref().is_some_and(|f| f.t >= 1.0) {
            self.cover_fade = None;
        }
        let cover_fading = self.cover_fade.is_some();
        let cover_fade = self.cover_fade.as_ref().map(|f| (f.from.clone(), anim::cover_fade(f.t)));
        // The equaliser rises when playing starts and settles flat when it stops, not in one frame.
        let bars_to = if v.music.is_some_and(|t| t.playing) { 1.0 } else { 0.0 };
        self.bars = anim::follow(self.bars, bars_to, dt, anim::BARS_RATE);
        let bars_busy = (bars_to - self.bars).abs() > 0.01;
        if !bars_busy {
            self.bars = bars_to;
        }
        let bars_level = self.bars;
        // The banner's line icon (status / device) with its paths as geometry.
        let banner_line = (|| {
            let icon = if v.mode == Mode::Alert { v.banner?.icon.unwrap_or(ICON_BELL) } else { return None };
            let ic = icons::lookup(icon)?;
            let geoms = ic.parts.iter().map(|part| self.line_geometry(part.d)).collect::<Option<Vec<_>>>()?;
            Some((ic, geoms))
        })();
        // The weather's line icon with its paths as geometry (pill and Weather page).
        let weather_line = (|| {
            let n = v.weather.now?;
            let ic = icons::weather(n.sky, n.day);
            let geoms = ic.parts.iter().map(|part| self.line_geometry(part.d)).collect::<Option<Vec<_>>>()?;
            Some((ic, geoms))
        })();
        // Every third hour of the day ahead with its icon, while the Weather page shows.
        let hour_lines: Vec<_> = if page == Some(Page::Weather) {
            v.weather.hours.iter().step_by(3).filter_map(|h| {
                let ic = icons::weather(h.sky, h.day);
                let geoms = ic.parts.iter().map(|part| self.line_geometry(part.d)).collect::<Option<Vec<_>>>()?;
                Some((*h, ic, geoms))
            }).collect()
        } else { Vec::new() };
        // The pill's battery icon (Lucide) with its paths as geometry.
        // The Weather page's own icon, a sun behind a cloud (a bare sun reads as brightness or as the sun's path), for the page
        // while there is no city or conditions yet (muted, like Tasks' ring) and for its place in the row of page icons.
        let weather_empty = (|| {
            let ic = icons::weather(weather::Sky::PartlyCloudy, true);
            let geoms = ic.parts.iter().map(|part| self.line_geometry(part.d)).collect::<Option<Vec<_>>>()?;
            Some((ic, geoms))
        })();
        // The Music page's player icons (shuffle, previous, next, repeat all, repeat one) with their paths as geometry.
        let player_icons: Vec<(&'static icons::Icon, Vec<ID2D1PathGeometry>)> = if page == Some(Page::Music) {
            [&icons::PLAYER_SHUFFLE, &icons::PLAYER_PREV, &icons::PLAYER_NEXT, &icons::PLAYER_REPEAT, &icons::PLAYER_REPEAT_ONE].into_iter()
                .filter_map(|ic| Some((ic, ic.parts.iter().map(|part| self.line_geometry(part.d)).collect::<Option<Vec<_>>>()?))).collect()
        } else { Vec::new() };
        // The sun view's memory of the last frame is only kept while the view is open, so opening it later does not play a sunrise.
        if !(page == Some(Page::Weather) && v.sun_view) {
            self.sun_seen.set(None);
        }
        // The moon of the Weather page's sun path, drawn at night.
        let moon_line = if page == Some(Page::Weather) && v.sun_view {
            let ic = icons::weather(weather::Sky::Clear, false);
            ic.parts.iter().map(|part| self.line_geometry(part.d)).collect::<Option<Vec<_>>>().map(|geoms| (ic, geoms))
        } else { None };
        // The Focus page's restart and next icons.
        let focus_icons: Vec<(&'static icons::Icon, Vec<ID2D1PathGeometry>)> = if page == Some(Page::Focus) || v.leaving.is_some_and(|(p, _, _)| p == Page::Focus) {
            [&icons::FOCUS_RESET, &icons::PLAYER_NEXT].into_iter()
                .filter_map(|ic| Some((ic, ic.parts.iter().map(|part| self.line_geometry(part.d)).collect::<Option<Vec<_>>>()?))).collect()
        } else { Vec::new() };
        // Height of each wrapped lyric line, while the panel is wider than the player.
        let lyric_heights: Vec<f32> = match v.music {
            Some(t) if page == Some(Page::Music) && v.size[0] > notch::target(Mode::Expanded, false)[0] + 1.0 && !t.lyrics.is_empty() =>
                self.lyric_heights(&t.lyrics, LYRIC_W * scale, 20.0 * scale, &fonts),
            _ => Vec::new(),
        };
        let lyric_tops = lyrics::tops(&lyric_heights, 8.0 * scale);
        let lyric_moving = match v.music {
            Some(t) if !lyric_heights.is_empty() => {
                let position = t.position_ms();
                let phase = lyrics::phase(&t.lyrics, position);
                let focus = lyrics::focus(&lyric_tops, &lyric_heights, phase);
                self.lyric_motion.step(focus, phase, position, elapsed, if t.playing { t.rate } else { 0.0 })
            }
            _ => {
                // Keep the measured layout cached, but do no lyric animation behind a closed page.
                self.lyric_motion = lyrics::Motion::default();
                false
            }
        };
        // Also while the panel shows another page: the icons are then ready (and stay while the page slides out).
        let note_icons: Vec<Option<ID2D1Bitmap>> = if page.is_some() { v.notes.iter().map(|n| self.app_icon(&n.id)).collect() } else { Vec::new() };
        // The picture a banner carries (a screenshot), loaded like an icon: the banner shows without it first.
        let banner_image: Option<ID2D1Bitmap> = v.banner.filter(|_| v.mode == Mode::Alert).and_then(|b| b.image.as_deref()).and_then(|p| self.app_icon(&format!("img:{p}")));
        let banner_app: Option<ID2D1Bitmap> = v.banner.filter(|b| v.mode == Mode::Alert && b.kind() == BannerKind::Notification)
            .and_then(|b| b.app_id.clone()).and_then(|id| self.banner_icon(&id));
        match v.mode {
            Mode::Expanded => self.owner = Owner::Panel,
            Mode::Alert => self.owner = Owner::Banner,
            // Back to a resting pill (the size at which the panel's content has faded out).
            _ if v.size[1] <= 39.0 => self.owner = Owner::Pill,
            _ => {}
        }
        let panel_ok = self.owner == Owner::Panel;
        let rt = &self.rt;
        let s = scale;
        let [nw, nh, nr] = v.size.map(|d| d * s);
        let cx = w as f32 / 2.0;
        let (l, r) = (cx - nw / 2.0, cx + nw / 2.0);
        unsafe {
            let text = |t: &str, f: usize, b: D2D_RECT_F, align: DWRITE_TEXT_ALIGNMENT, c: D2D1_COLOR_F| -> Result<()> {
                let t: Vec<u16> = t.encode_utf16().collect();
                fonts[f].SetTextAlignment(align)?;
                rt.DrawText(&t, &fonts[f], &b, &self.brush(c)?, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
                Ok(())
            };
            // Cover art clipped to a rounded square, or a note glyph when there is none.
            let draw_cover = |box_: D2D_RECT_F, radius: f32, a: f32| -> Result<()> {
                // The big cover gets a soft shadow all round, a little lower, so it stands off the glow.
                if let (true, Some(shadow)) = (box_.right - box_.left > 40.0 * s, &cover_shadow) {
                    let (pad, drop) = (COVER_SHADOW_PAD * s, COVER_SHADOW_DROP * s);
                    let area = rect(box_.left - pad, box_.top + drop - pad, box_.right + pad, box_.bottom + drop + pad);
                    rt.DrawBitmap(shadow, Some(&area), COVER_SHADOW_ALPHA * a, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None);
                }
                let big_box = box_.right - box_.left > 40.0 * s;
                // The picture at `k` times its size about the box's centre, `a` opaque. At full size it is placed on whole
                // pixels, 1:1 (made at exactly the drawn size), so it stays sharp.
                let paint = |bmp: &ID2D1Bitmap, a: f32, k: f32| -> Result<()> {
                    let size = bmp.GetSize();
                    let (w, h) = (size.width * k, size.height * k);
                    let (x, y) = if k == 1.0 { (box_.left.round(), box_.top.round()) } else { ((box_.left + box_.right - w) / 2.0, (box_.top + box_.bottom - h) / 2.0) };
                    let b = rt.CreateBitmapBrush(bmp, None, None)?;
                    b.SetTransform(&Matrix3x2 { M11: k, M12: 0.0, M21: 0.0, M22: k, M31: x, M32: y });
                    b.SetOpacity(a);
                    rt.FillRoundedRectangle(&rounded(rect(x, y, x + w, y + h), radius * k), &b);
                    Ok(())
                };
                let placeholder = |a: f32| -> Result<()> {
                    rt.FillRoundedRectangle(&rounded(box_, radius), &self.brush(grey(0.2, a))?);
                    text(ICON_NOTE, F_ICON, box_, DWRITE_TEXT_ALIGNMENT_CENTER, grey(1.0, a * 0.8))
                };
                match &cover {
                    Some((big, _)) if big_box => match &cover_fade {
                        // A new track: the old cover (or the note) stays at full strength, the new one fades and grows in on top.
                        Some((from, (fade, k))) => {
                            match from { Some((old, _)) => paint(old, a, 1.0)?, None => placeholder(a)? }
                            paint(big, a * fade, *k)?;
                        }
                        None => paint(big, a, 1.0)?,
                    },
                    Some((_, small)) => paint(small, a, 1.0)?,
                    None => placeholder(a)?,
                }
                Ok(())
            };
            // A weather line icon at (x, y), `size` px square, `age` seconds into its motion. `tint` =
            // (colour of the first path, colour of the rest); None draws it white.
            let draw_wx_icon = |ic: &icons::Icon, geoms: &[ID2D1PathGeometry], x: f32, y: f32, size: f32, tint: Option<((f32, f32, f32), (f32, f32, f32))>, a: f32, age: f32| -> Result<()> {
                let base = { let mut m = Matrix3x2::default(); rt.GetTransform(&mut m); m };
                let view = scaling(size / ic.vb) * translate(x, y);
                let groups = icons::motion(ic.anim, age);
                for (i, (part, geo)) in ic.parts.iter().zip(geoms).enumerate() {
                    let g = groups.get(part.g as usize).copied().unwrap_or(icons::Motion::STILL);
                    let (ox, oy) = ic.origins.get(part.g as usize).copied().unwrap_or((0.0, 0.0));
                    let local = translate(-ox, -oy) * rotation(g.rot) * scaling(g.scale) * translate(ox + g.dx, oy + g.dy);
                    let alpha = a * g.alpha * groups.get(part.a as usize).map_or(1.0, |m| m.alpha);
                    let c = match tint { Some((first, rest)) => if i == 0 { first } else { rest }, None => (1.0, 1.0, 1.0) };
                    rt.SetTransform(&(local * view * base));
                    rt.DrawGeometry(geo, &self.brush(color(c.0, c.1, c.2, alpha))?, ic.sw, &self.round);
                }
                rt.SetTransform(&base);
                Ok(())
            };
            let center = DWRITE_TEXT_ALIGNMENT_CENTER;
            // Width of a run in font `f` (caret placement, strike-through).
            let dw = &self.dw;
            let width = |t: &str, f: usize| -> f32 {
                let u: Vec<u16> = t.encode_utf16().collect();
                let mut m = DWRITE_TEXT_METRICS::default();
                match dw.CreateTextLayout(&u, &fonts[f], 10_000.0, 100.0) {
                    Ok(layout) if layout.GetMetrics(&mut m).is_ok() => m.widthIncludingTrailingWhitespace,
                    _ => 0.0,
                }
            };

            // Icon in a named slot; when the glyph changes the old one shrinks away and the new one
            // pops in past full size and settles (anim::ICON_MORPH_S).
            let morphing = std::cell::Cell::new(false);
            let micon = |slot: &'static str, icon: &'static str, f: usize, b: D2D_RECT_F, c: D2D1_COLOR_F| -> Result<()> {
                let (prev, t) = {
                    let mut m = self.morphs.borrow_mut();
                    let e = m.entry(slot).or_insert((icon, icon, Instant::now() - std::time::Duration::from_secs(9)));
                    if e.0 != icon {
                        *e = (icon, e.0, Instant::now());
                    }
                    (e.1, e.2.elapsed().as_secs_f32() / anim::ICON_MORPH_S)
                };
                if t >= 1.0 || prev == icon {
                    return text(icon, f, b, center, c);
                }
                morphing.set(true);
                let base = { let mut m = Matrix3x2::default(); rt.GetTransform(&mut m); m };
                let (mx, my) = ((b.left + b.right) / 2.0, (b.top + b.bottom) / 2.0);
                let scaled = |k: f32| Matrix3x2 { M11: k, M12: 0.0, M21: 0.0, M22: k, M31: mx - k * mx, M32: my - k * my } * base;
                let fade = |a: f32| D2D1_COLOR_F { a: c.a * a, ..c };
                rt.SetTransform(&scaled((1.0 - t).max(0.01)));
                text(prev, f, b, center, fade((1.0 - t * 1.6).max(0.0)))?;
                rt.SetTransform(&scaled(ease_out_back(t).max(0.01)));
                text(icon, f, b, center, fade(t.min(1.0)))?;
                rt.SetTransform(&base);
                Ok(())
            };

            rt.BeginDraw();
            rt.Clear(Some(&color(0.0, 0.0, 0.0, 0.0)));

            // A floating pill is drawn lower in the window, on whole pixels so covers stay sharp.
            let gy = (self.style.gap_at(v.size[1]) * s).round();
            rt.SetTransform(&Matrix3x2::translation(0.0, gy));
            let ear = if self.style == notch::Style::Pill { 0.0 } else { notch::ear(v.size[1]) * s };
            let shape = self.notch_shape(l, r, nh, nr, ear)?;
            let prep = Prep { weather_line: &weather_line, weather_empty: &weather_empty, hour_lines: &hour_lines, focus_icons: &focus_icons, player_icons: &player_icons,
                note_icons: &note_icons, lyric_heights: &lyric_heights, lyric_tops: &lyric_tops, moon_line: &moon_line, tint, level, fonts: &fonts };
            let ctx = Ctx { v, s, l, r, cx, nh, gy, shape: &shape, text: &text, width: &width, micon: &micon, wx_icon: &draw_wx_icon, cover: &draw_cover };
            // How far the pill has grown into the panel (0: pill, 1: open).
            let open = ((nh - 39.0 * s) / (20.0 * s)).clamp(0.0, 1.0);
            // A thin soft shadow under the open panel and the banners (the resting pill has none):
            // faint black strokes of the outline, widest first, drawn a little lower; the fill
            // covers the inside.
            if open > 0.0 {
                let base = { let mut m = Matrix3x2::default(); rt.GetTransform(&mut m); m };
                rt.SetTransform(&(Matrix3x2::translation(0.0, SHADOW_DY * s) * base));
                for k in (1..=SHADOW_STEPS).rev() {
                    rt.DrawGeometry(&shape, &self.brush(color(0.0, 0.0, 0.0, shadow_layer_alpha(k) * open))?, 2.0 * SHADOW_STEP * k as f32 * s, &self.round);
                }
                rt.SetTransform(&base);
            }
            rt.FillGeometry(&shape, &self.brush(grey(0.0, 1.0))?, None);
            // Music page: the cover's colour rises from the bottom, black at the top. The pill stays
            // black; the glow grows in with the panel.
            // During a page change the glow follows the Music page's own fade, so it never shows
            // under the Health or Tasks page it is replacing (or lingers under one arriving).
            let gate = match v.leaving {
                Some((_, t, _)) if v.page == Page::Music => page_fades(t).1,
                Some((Page::Music, t, _)) => page_fades(t).0,
                _ => 1.0,
            };
            if level > 0.0 && open > 0.0 && panel_ok {
                let (gr, gg, gb) = dim_to_lum(tint, GLOW_MAX_LUM);
                let stop = |position: f32, a: f32| D2D1_GRADIENT_STOP { position, color: color(gr, gg, gb, a * open * level * gate) };
                let stops = rt.CreateGradientStopCollection(&[stop(0.58, 0.0), stop(0.68, 0.08), stop(0.82, 0.42), stop(1.0, 1.0)], D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP)?;
                let line = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES { startPoint: Vector2 { X: 0.0, Y: 0.0 }, endPoint: Vector2 { X: 0.0, Y: nh } };
                rt.FillGeometry(&shape, &rt.CreateLinearGradientBrush(&line, None, &stops)?, None);
            }
            // Aurora over the glow: a thin glowing line in the cover's two colours, swaying slowly.
            let aurora_a = open * level * gate;
            if aurora_a > 0.01 && panel_ok {
                self.draw_aurora(&shape, (l, r, nh), w, s, v.phase, aurora_a, panel_size(v.page)[1] * s);
            }

            // Each layout fades in as the pill reaches its size.
            let fade = |from: f32, span: f32| ((nh - from * s) / (span * s)).clamp(0.0, 1.0);
            let small_a = 1.0 - fade(39.0, 20.0);
            if nh >= 20.0 * s && small_a > 0.0 && v.mode != Mode::Alert {
                self.draw_pill(&ctx, &weather_line, bars_level, small_a)?;
            }

            let mut icon_busy = false;
            // A banner is one of three things, each drawn on its own: a status (an icon and its line), a
            // notification (an app's message), an approval (a question with buttons and a countdown).
            if let (Mode::Alert, Some(t)) = (v.mode, v.banner) {
                icon_busy = self.draw_banner(&ctx, t, &banner_line, &banner_image, &banner_app)?;
            }

            let a = fade(110.0, 50.0);
            if a > 0.0 && panel_ok {
                self.draw_header(&ctx, a)?;
                self.draw_pages(&ctx, &prep, a)?;
                let ph = panel_size(v.page)[1];
                self.draw_nav(&ctx, &weather_empty, a, ph)?;
            }
            if let Err(e) = rt.EndDraw(None, None) {
                if e.code() != D2DERR_RECREATE_TARGET {
                    return Err(e);
                }
                self.lose_device()?;
                return Ok(true);
            }
            #[cfg(test)]
            if self.grab {
                self.grab_frame()?;
            }
            // No wait for the refresh: the frame pacing (vsync thread) already asks once per refresh.
            let shown = self.surface.as_ref().unwrap().swap.Present(0, DXGI_PRESENT(0));
            if shown == DXGI_ERROR_DEVICE_REMOVED || shown == DXGI_ERROR_DEVICE_RESET {
                self.lose_device()?;
                return Ok(true);
            }
            shown.ok()?;
            // The mouse belongs to the drawn shape: the pill or panel, its ears, and the shadow under it.
            let reach = if open > 0.0 { (SHADOW_STEPS as f32 * SHADOW_STEP + SHADOW_DY) * s } else { 0.0 };
            // While the row of page icons is open its cells take clicks at once, though the panel is still
            // growing down to them: the region reaches their bottom from the first frame.
            let nh_hit = if nav_open() && v.mode == Mode::Expanded { nh.max((panel_size(v.page)[1] + NAV_EXTRA) * s) } else { nh };
            let region = (((l - ear - reach).floor() as i32).max(0), 0, ((r + ear + reach).ceil() as i32).min(w), ((gy + nh_hit + reach).ceil() as i32).min(h));
            self.place(hwnd, (x, y, w, h), region);
            // Poll the media clock between lines; the spring requests vsync until it settles,
            // including after pausing in the middle of a transition.
            let lyrics_run = !lyric_heights.is_empty() && v.music.is_some_and(|t| t.playing);
            self.slow_frames = lyrics_run;
            // Icon completion posts WM_ICONS; waiting for I/O never keeps animation alive.
            // The Weather page's icon moves for a moment when the panel or the page opens.
            // The Weather page's icon moves for a moment when the panel or the page opens; the sun path keeps the frames
            // coming while it is open (the halo of the sun, or of the moon at night, breathes and blinks).
            let sun_moving = v.sun_view && v.weather.sun.is_some();
            let wx_busy = v.page == Page::Weather && v.mode == Mode::Expanded && (v.page_age < anim::ICON_MOTION_S || sun_moving);
            Ok(morphing.get() || glow_busy || bars_busy || icon_busy || wx_busy || lyric_moving || cover_fading)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lyric_layout_cache_resets_motion_for_tracks_and_dpi() {
        let mut renderer = Renderer::new().unwrap();
        let fonts = renderer.fonts(1.0).unwrap();
        let first = Arc::new(vec![(0, "first".into()), (5000, "second".into())]);
        renderer.lyric_heights(&first, LYRIC_W, 20.0, &fonts);
        renderer.lyric_motion.step(10.0, 0.0, 4900, 0.016, 1.0);
        renderer.lyric_motion.step(50.0, 1.0, 4920, 0.02, 1.0);
        let before = renderer.lyric_motion.focus();
        renderer.lyric_heights(&first, LYRIC_W, 20.0, &fonts);
        assert_eq!(renderer.lyric_motion.focus(), before, "cached layout preserves motion");
        let other = Arc::new(vec![(0, "another song".into())]);
        renderer.lyric_heights(&other, LYRIC_W, 20.0, &fonts);
        assert!(!renderer.lyric_motion.step(90.0, 0.0, 4920, 0.016, 1.0));
        assert_eq!(renderer.lyric_motion.focus(), 90.0, "new lyrics land on their own current line");
        renderer.lyric_heights(&other, LYRIC_W * 1.5, 30.0, &fonts);
        assert!(!renderer.lyric_motion.step(135.0, 0.0, 4936, 0.016, 1.0));
        assert_eq!(renderer.lyric_motion.focus(), 135.0, "DPI change resets pixel coordinates");
    }

    #[test]
    fn the_focus_page_buttons_do_not_overlap_and_the_whole_strip_is_hittable() {
        let (cx, scale) = (300.0, 1.0);
        let list = page_buttons(cx, scale, Page::Focus, 0, 0, None, false);
        // 3 chips, 3 controls, 8 segments.
        assert_eq!(list.iter().filter(|(b, _)| matches!(b, Button::FocusMode(_) | Button::FocusPlay | Button::FocusReset | Button::FocusSkip | Button::FocusSeg(_))).count(), 3 + 3 + 8);
        let overlap = |a: &D2D_RECT_F, b: &D2D_RECT_F| a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom;
        let ones: Vec<_> = list.iter().filter(|(b, _)| matches!(b, Button::FocusMode(_) | Button::FocusPlay | Button::FocusReset | Button::FocusSkip)).collect();
        for (i, (a, ra)) in ones.iter().enumerate() {
            for (b, rb) in &ones[i + 1..] {
                assert!(!overlap(ra, rb), "{a:?} and {b:?} overlap");
            }
        }
        let mid = |b: Button| list.iter().find(|(x, _)| *x == b).map(|(_, r)| ((r.left + r.right) / 2.0, (r.top + r.bottom) / 2.0)).unwrap();
        for b in [Button::FocusPlay, Button::FocusReset, Button::FocusSkip, Button::FocusMode(focus::Kind::Long), Button::FocusSeg(0), Button::FocusSeg(7)] {
            let (x, y) = mid(b);
            assert_eq!(hit_button(cx, scale, x, y, None, Page::Focus, 0, 0, None, false), Some(b));
        }
        // The strip's segments are as long as their minutes: the long break (15) is 3x a short one (5).
        let segs = focus_segments(392.0);
        assert!(((segs[7].1 - segs[7].0) / (segs[1].1 - segs[1].0) - 3.0).abs() < 0.01);
        assert!((segs[7].1 - 392.0).abs() < 0.01, "the strip fills the width");
    }

    #[test]
    fn a_tall_banner_shrinking_back_shows_no_page() {
        unsafe {
            use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let (w, h) = (900i32, 300i32);
            let hwnd = CreateWindowExW(WS_EX_NOREDIRECTIONBITMAP, w!("STATIC"), w!("Shrink"), WS_POPUP, 0, 0, w, h, None, None, None, None).unwrap();
            let mut renderer = Renderer::new().unwrap();
            renderer.grab = true;
            let toast = Toast { app: "Claude".into(), title: "x".into(), body: "y".into(), level: None, icon: None, image: None, open: None, badge: None, choices: Vec::new(), app_id: None };
            let (wx, health, idle_focus) = (weather::State::default(), Health::default(), Focus::default());
            macro_rules! view { ($mode:expr, $banner:expr) => { View { mode: $mode, size: [520.0, 139.0, 24.0], time: "20:49", date: "T2, 26/10",
                battery: Some(Battery { percent: 76, charging: false, saver: false }), banner: $banner, music: None, live: false, robot: None, phase: 0.0,
                banner_age: 2.0, lyrics: false, hovered: None, page: Page::Health, leaving: None, todos: &[], notes: &[], weather: &wx, page_age: 9.0, unit: weather::Unit::C, sun_view: false,
                list: ListView { counts: (0, 0), remind_before: 15, now: 0, draft: "", editing: false, edit_item: None, caret: false, scroll: 0, note_pos: 0.0 },
                focus: &idle_focus, health: HealthView { h: &health, now: 0, minute_of_day: 0, settings: None } } }; }
            // Bright pixels below the pill's own 40 rows: the page's cards and text would put some there.
            let lit = |r: &Renderer| r.grabbed.chunks(4).enumerate().filter(|(i, p)| (i / w as usize) >= 40 && p[3] > 0 && (p[0] > 40 || p[1] > 40 || p[2] > 40)).count();
            for _ in 0..3 {
                renderer.present(hwnd, 0, 0, w, h, 1.0, &view!(Mode::Alert, Some(&toast))).unwrap();
            }
            // The banner ended: the pill is still 139 tall on its way down, as the approval banner's shrink is.
            for _ in 0..3 {
                renderer.present(hwnd, 0, 0, w, h, 1.0, &view!(Mode::Collapsed, None)).unwrap();
            }
            assert_eq!(lit(&renderer), 0, "the page showed while the banner shrank");
            // A panel that opens afterwards does show its page.
            for _ in 0..3 {
                renderer.present(hwnd, 0, 0, w, h, 1.0, &view!(Mode::Expanded, None)).unwrap();
            }
            assert!(lit(&renderer) > 0);
            DestroyWindow(hwnd).unwrap();
        }
    }

    #[test]
    fn the_sun_chip_sits_beside_the_unit_capsule_and_the_x_without_touching_them() {
        let list = page_buttons(300.0, 1.0, Page::Weather, 1, 0, None, false);
        let rect_of = |b: Button| list.iter().find(|(x, _)| *x == b).map(|(_, r)| *r).unwrap();
        let (sun, c, f, off) = (rect_of(Button::SunToggle), rect_of(Button::Unit(weather::Unit::C)), rect_of(Button::Unit(weather::Unit::F)), rect_of(Button::CityOff));
        assert!(sun.right < c.left && f.right < off.left, "left of the capsule, which is left of the x");
        assert!((sun.top - c.top).abs() < 0.01 && (sun.bottom - c.bottom).abs() < 0.01, "level with the unit chips");
        let (x, y) = ((sun.left + sun.right) / 2.0, (sun.top + sun.bottom) / 2.0);
        assert_eq!(hit_button(300.0, 1.0, x, y, None, Page::Weather, 1, 0, None, false), Some(Button::SunToggle));
        assert!(page_buttons(300.0, 1.0, Page::Weather, 0, 0, None, false).iter().all(|(b, _)| *b != Button::SunToggle), "no chip before there is a city");
    }

    #[test]
    fn the_square_in_the_row_of_page_icons_glides_between_cells_while_the_page_changes() {
        unsafe {
            use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let (w, h) = (900i32, 300i32);
            let hwnd = CreateWindowExW(WS_EX_NOREDIRECTIONBITMAP, w!("STATIC"), w!("Nav glide"), WS_POPUP, 0, 0, w, h, None, None, None, None).unwrap();
            let (wx, health, idle_focus) = (weather::State::default(), Health::default(), Focus::default());
            let ph = panel_size(Page::Todo)[1];
            set_nav_open(true);
            macro_rules! view { ($leaving:expr) => { View { mode: Mode::Expanded, size: [480.0, ph + NAV_EXTRA, 28.0], time: "20:49", date: "T2, 26/10",
                battery: None, banner: None, music: None, live: false, robot: None, phase: 0.0, banner_age: 0.0, lyrics: false, hovered: None, page: Page::Todo,
                leaving: $leaving, todos: &[], notes: &[], weather: &wx, page_age: 9.0, unit: weather::Unit::C, sun_view: false,
                list: ListView { counts: (0, 0), remind_before: 15, now: 0, draft: "", editing: false, edit_item: None, caret: false, scroll: 0, note_pos: 0.0 },
                focus: &idle_focus, health: HealthView { h: &health, now: 0, minute_of_day: 0, settings: None } } }; }
            let cells = nav_cells(450.0, 1.0, ph);
            let (from, to) = (cells.iter().find(|(p, _)| *p == Page::Music).unwrap().1, cells.iter().find(|(p, _)| *p == Page::Todo).unwrap().1);
            // The gap between the two cells: dark when the square rests on either, lit when it is passing through.
            let (gx, gy) = (((from.right + to.left) / 2.0) as usize, ((from.top + from.bottom) / 2.0) as usize);
            let lit_in_gap = |t: Option<f32>| {
                let mut renderer = Renderer::new().unwrap();
                renderer.grab = true;
                for _ in 0..4 {
                    renderer.present(hwnd, 0, 0, w, h, 1.0, &view!(t.map(|t| (Page::Music, t, 1)))).unwrap();
                }
                let px = &renderer.grabbed[(gy * w as usize + gx) * 4..][..4];
                px[3] > 0 && px[0].max(px[1]).max(px[2]) > 20
            };
            assert!(!lit_in_gap(None), "resting: the square is on its own cell");
            assert!(!lit_in_gap(Some(0.0)), "just starting: still on the old cell");
            assert!(lit_in_gap(Some(0.5)), "half way: the square is passing through the gap");
            assert!(!lit_in_gap(Some(1.0)), "arrived: on the new cell");
            set_nav_open(false);
            DestroyWindow(hwnd).unwrap();
        }
    }

    #[test]
    fn the_weather_credit_gives_way_to_the_row_of_page_icons() {
        unsafe {
            use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let (w, h) = (900i32, 300i32);
            let hwnd = CreateWindowExW(WS_EX_NOREDIRECTIONBITMAP, w!("STATIC"), w!("Credit"), WS_POPUP, 0, 0, w, h, None, None, None, None).unwrap();
            let (health, idle_focus) = (Health::default(), Focus::default());
            let ph = panel_size(Page::Weather)[1];
            let state = |updated: Option<u64>| weather::State {
                place: Some(weather::Place { name: "Hà Nội".into(), lat: 21.0, lon: 105.8 }),
                now: Some(weather::Now { temp: 29.0, feels: 31.0, humidity: 77.0, wind: 6.0, sky: weather::Sky::Clear, day: true }),
                updated, ..weather::State::default() };
            macro_rules! view { ($wx:expr, $h:expr) => { View { mode: Mode::Expanded, size: [480.0, $h, 28.0], time: "08:14", date: "T5, 05/11",
                battery: None, banner: None, music: None, live: false, robot: None, phase: 0.0, banner_age: 0.0, lyrics: false, hovered: None, page: Page::Weather,
                leaving: None, todos: &[], notes: &[], weather: $wx, page_age: 9.0, unit: weather::Unit::C, sun_view: false,
                list: ListView { counts: (0, 0), remind_before: 15, now: 0, draft: "", editing: false, edit_item: None, caret: false, scroll: 0, note_pos: 0.0 },
                focus: &idle_focus, health: HealthView { h: &health, now: 0, minute_of_day: 0, settings: None } } }; }
            let frame = |open: bool, wx: &weather::State| {
                set_nav_open(open);
                let mut renderer = Renderer::new().unwrap();
                renderer.grab = true;
                for _ in 0..4 {
                    renderer.present(hwnd, 0, 0, w, h, 1.0, &view!(wx, if open { ph + NAV_EXTRA } else { ph })).unwrap();
                }
                renderer.grabbed.clone()
            };
            let (with, without) = (state(Some(202611050813)), state(None));
            // Row open: the credit is not drawn, so the frame is the same with or without a fetch time to credit.
            assert!(frame(true, &with) == frame(true, &without), "the credit text lies under the page icons");
            // Row closed: it is drawn (the frames differ).
            assert!(frame(false, &with) != frame(false, &without), "the credit shows when the row is closed");
            set_nav_open(false);
            DestroyWindow(hwnd).unwrap();
        }
    }

    #[test]
    fn at_sunset_the_blue_path_is_still_there_when_the_sun_has_just_set_and_gone_for_a_view_opened_at_night() {
        unsafe {
            use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let (w, h) = (900i32, 300i32);
            let hwnd = CreateWindowExW(WS_EX_NOREDIRECTIONBITMAP, w!("STATIC"), w!("Sunset"), WS_POPUP, 0, 0, w, h, None, None, None, None).unwrap();
            let (health, idle_focus) = (Health::default(), Focus::default());
            let ph = panel_size(Page::Weather)[1];
            let wx = weather::State {
                place: Some(weather::Place { name: "Hà Nội".into(), lat: 21.0, lon: 105.8 }),
                now: Some(weather::Now { temp: 29.0, feels: 31.0, humidity: 77.0, wind: 6.0, sky: weather::Sky::Clear, day: true }),
                sun: Some(weather::Sun { rise: 342, set: 1068 }), ..weather::State::default() };
            macro_rules! view { ($minute:expr) => { View { mode: Mode::Expanded, size: [480.0, ph, 28.0], time: "17:48", date: "T5, 05/11",
                battery: None, banner: None, music: None, live: false, robot: None, phase: 0.0, banner_age: 0.0, lyrics: false, hovered: None, page: Page::Weather,
                leaving: None, todos: &[], notes: &[], weather: &wx, page_age: 9.0, unit: weather::Unit::C, sun_view: true,
                list: ListView { counts: (0, 0), remind_before: 15, now: 0, draft: "", editing: false, edit_item: None, caret: false, scroll: 0, note_pos: 0.0 },
                focus: &idle_focus, health: HealthView { h: &health, now: 0, minute_of_day: $minute, settings: None } } }; }
            // How many pixels in the arc's area are blue (the lit path).
            let blue = |r: &Renderer| r.grabbed.chunks(4).enumerate().filter(|(i, p)| {
                let (x, y) = (i % w as usize, i / w as usize);
                (88..140).contains(&y) && (330..620).contains(&x) && p[3] > 0 && p[0] > p[2] / 2 + 60 && p[0] > 120 && p[1] < p[0]
            }).count();
            let present = |r: &mut Renderer, minute: u32| { for _ in 0..3 { r.present(hwnd, 0, 0, w, h, 1.0, &view!(minute)).unwrap(); } };
            // Watched through the sunset: the blue path is still drawn (on its way to grey) in the first frames after it.
            let mut watched = Renderer::new().unwrap();
            watched.grab = true;
            present(&mut watched, 1000);
            let by_day = blue(&watched);
            present(&mut watched, 1100);
            let just_set = blue(&watched);
            drop(watched);
            // Opened after dark: the path is grey from the start.
            let mut late = Renderer::new().unwrap();
            late.grab = true;
            present(&mut late, 1100);
            let at_night = blue(&late);
            assert!(by_day > 0, "by day the path is lit");
            assert!(just_set > at_night, "the lit path lingers just after the sunset that was watched: {just_set} vs {at_night}");
            DestroyWindow(hwnd).unwrap();
        }
    }

    #[test]
    fn closed_page_does_not_queue_icons() {
        let mut renderer = Renderer::new().unwrap();
        renderer.prefetch_icon("PuwNotch.ClosedPageTest");
        assert!(renderer.icon_loader.pending.is_empty(), "a closed page must not start loading icons");
    }

    #[test]
    fn page_work_stops_in_pill_mini_banner_and_fullscreen() {
        for mode in [Mode::Collapsed, Mode::Mini, Mode::Peek, Mode::PeekMini, Mode::Alert, Mode::Hidden] {
            assert_eq!(visible_page(mode, false, Page::Notify), None);
        }
        assert_eq!(visible_page(Mode::Expanded, true, Page::Music), None);
        assert_eq!(visible_page(Mode::Expanded, false, Page::Music), Some(Page::Music));
        assert_eq!(visible_page(Mode::Expanded, false, Page::Notify), Some(Page::Notify));
    }

    #[test]
    fn icons_load_on_open_and_banner_images_still_load_when_closed() {
        let mut renderer = Renderer::new().unwrap();
        renderer.set_page_open(true);
        renderer.prefetch_icon("PuwNotch.OpenPageTest");
        assert!(renderer.icon_loader.pending.contains("PuwNotch.OpenPageTest"));
        renderer.set_page_open(false);
        renderer.prefetch_icon("PuwNotch.ClosedPageTest");
        assert!(!renderer.icon_loader.pending.contains("PuwNotch.ClosedPageTest"));
        renderer.prefetch_icon("img:PuwNotch.MissingBannerImage");
        assert!(renderer.icon_loader.pending.contains("img:PuwNotch.MissingBannerImage"));
    }

    #[test]
    fn a_banner_asks_for_its_apps_icon_even_with_the_notifications_page_closed() {
        let mut renderer = Renderer::new().unwrap();
        renderer.set_page_open(false);
        assert!(renderer.banner_icon("PuwNotch.BannerIconTest").is_none(), "nothing until the loader answers");
        assert!(renderer.icon_loader.pending.contains("now:PuwNotch.BannerIconTest"));
        // The loader treats it as asked for now: not deferred like a page's icon.
        let loader = IconLoader::start();
        loader.ask.send("now:PuwNotch.BannerIconTest".into()).unwrap();
        let result = loader.got.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        assert!(!result.deferred);
        assert_eq!(result.id, "now:PuwNotch.BannerIconTest");
    }

    #[test]
    fn icon_worker_defers_queued_page_work_after_close() {
        let loader = IconLoader::start();
        loader.ask.send("PuwNotch.CancelledIcon".into()).unwrap();
        let result = loader.got.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        assert!(result.deferred && result.pixels.is_none());
        assert_eq!(result.id, "PuwNotch.CancelledIcon");
    }

    #[test]
    fn pending_icons_do_not_keep_frames_alive_and_closed_pages_do_not_prepare_music() {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let hwnd = CreateWindowExW(WS_EX_NOREDIRECTIONBITMAP, w!("STATIC"), w!("Page lifecycle test"), WS_POPUP,
                0, 0, 530, 210, None, None, None, None).unwrap();
            let mut renderer = Renderer::new().unwrap();
            renderer.icon_loader.pending.insert("PuwNotch.HungIcon".into());
            let track = Track { art_key: 7, ..Track::default() };
            let note = Note { app: "Test".into(), id: "PuwNotch.NewIcon".into(), title: "Test".into(), body: String::new(), at: 0, open: None };
            let mut view = View { mode: Mode::Collapsed, size: target(Mode::Collapsed, false), time: "12:00", date: "30/10",
                battery: None, banner: None, music: Some(&track), live: false, robot: None, phase: 0.0, banner_age: 9.0,
                lyrics: false, hovered: None, page: Page::Notify, leaving: None, todos: &[], notes: std::slice::from_ref(&note),
                weather: &weather::State::default(), page_age: 9.0, unit: weather::Unit::C, sun_view: false,
                list: ListView { counts: (0, 0), remind_before: 15, now: 0, draft: "", editing: false, edit_item: None, caret: false, scroll: 0, note_pos: 0.0 },
                focus: &Focus::default(), health: HealthView { h: &Health::default(), now: 0, minute_of_day: 0, settings: None } };
            for mode in [Mode::Collapsed, Mode::Mini] {
                view.mode = mode;
                view.size = target(mode, false);
                assert!(!renderer.present(hwnd, 0, 0, 530, 210, 1.0, &view).unwrap(), "closed page must settle even with an unfinished icon");
                assert!(!renderer.icon_loader.pending.contains(&note.id));
                assert!(renderer.cover.is_none(), "closed page must not decode a cover");
            }
            view.mode = Mode::Expanded;
            view.size = target(Mode::Expanded, false);
            assert!(!renderer.present(hwnd, 0, 0, 530, 210, 1.0, &view).unwrap(), "open page also waits for icons by message, not VSync");
            assert!(renderer.icon_loader.pending.contains(&note.id));
            view.page = Page::Music;
            renderer.present(hwnd, 0, 0, 530, 210, 1.0, &view).unwrap();
            assert!(renderer.cover.is_some(), "opening Music prepares its cover cache");
            let next = Track { art_key: 8, playing: true, ..Track::default() };
            view.music = Some(&next);
            view.live = true;
            view.mode = Mode::Collapsed;
            view.size = target(Mode::Collapsed, true);
            renderer.grab = true;
            renderer.present(hwnd, 0, 0, 530, 210, 1.0, &view).unwrap();
            assert_eq!(renderer.cover.as_ref().unwrap().track_key, 8, "the visible pill must discard the previous track's cover");
            let first = renderer.grabbed.clone();
            std::thread::sleep(std::time::Duration::from_millis(30));
            view.phase = 0.4;
            renderer.present(hwnd, 0, 0, 530, 210, 1.0, &view).unwrap();
            assert_ne!(first, renderer.grabbed, "the playing pill's equaliser must move between frames");
            let mut notch = crate::notch::Notch::new(false);
            notch.set_live(true);
            for _ in 0..240 { notch.step(1.0 / 120.0); }
            notch.set_hidden(true);
            view.mode = Mode::Hidden;
            for _ in 0..240 {
                notch.step(1.0 / 120.0);
                if notch.is_hidden() { break; }
                view.size = notch.size();
                renderer.present(hwnd, 0, 0, 530, 210, 1.0, &view).unwrap();
            }
            assert!(notch.is_hidden(), "fullscreen shrink must reach a size where the OS window can hide");
            assert!(!renderer.icon_loader.page_open.load(Ordering::Relaxed));
            DestroyWindow(hwnd).unwrap();
        }
    }

    #[test]
    fn glow_tint_picks_the_strongest_hue_and_greys_out_dull_covers() {
        let px = |b: u8, g: u8, r: u8, a: u8, n: usize| [b, g, r, a].repeat(n);
        assert_eq!(glow_tint(&px(0, 0, 100, 255, 4)), (1.0, 0.0, 0.0), "dark red -> full red");
        assert_eq!(glow_tint(&px(0, 50, 100, 255, 4)), (1.0, 0.5, 0.0), "orange keeps its mix");
        assert_eq!(glow_tint(&px(0, 0, 50, 100, 4)), (1.0, 0.0, 0.0), "premultiplied alpha is undone");
        // Mostly grey with a little vivid blue: the blue wins, not a grey-blue average.
        let mixed = [px(120, 120, 120, 255, 200), px(255, 40, 0, 255, 5)].concat();
        let (r, _, b) = glow_tint(&mixed);
        assert!(b == 1.0 && r < 0.3, "blue, got r={r} b={b}");
        // Red beats the blue when there is more of it.
        let two = [px(255, 0, 0, 255, 2), px(0, 0, 255, 255, 6)].concat();
        assert_eq!(glow_tint(&two), (1.0, 0.0, 0.0));
        for dull in [px(10, 10, 10, 255, 256), px(128, 128, 128, 255, 256), px(255, 255, 255, 255, 256), px(0, 0, 0, 0, 256), Vec::new()] {
            assert_eq!(glow_tint(&dull), GLOW_NEUTRAL);
        }
    }

    #[test]
    fn app_initial_skips_symbols_and_uppercases() {
        assert_eq!(app_initial("zalo"), "Z");
        assert_eq!(app_initial("  (Telegram)"), "T");
        assert_eq!(app_initial("ép"), "É");
        assert_eq!(app_initial("..."), "?");
        assert_eq!(app_initial(""), "?");
    }

    #[test]
    fn app_tint_is_stable_and_white_text_readable() {
        assert_eq!(app_tint("Zalo"), app_tint("zalo"));
        for hue in 0..360 {
            let (r, g, b) = hue_tint(hue as f32);
            let ratio = 1.05 / (luminance(r, g, b) + 0.05);
            assert!(ratio >= 4.5, "hue {hue}: white on tile is only {ratio:.2}:1");
        }
    }

    #[test]
    fn glow_keeps_white_controls_readable() {
        for tint in [(1.0, 1.0, 0.0), (1.0, 1.0, 1.0), (0.0, 1.0, 1.0), (0.0, 1.0, 0.0), (1.0, 0.2, 0.2), (0.2, 0.4, 1.0)] {
            let (r, g, b) = dim_to_lum(tint, GLOW_MAX_LUM);
            let ratio = 1.05 / (luminance(r, g, b) + 0.05);
            assert!(ratio >= 4.5, "{tint:?}: white on the glow is only {ratio:.2}:1");
            assert!(r.max(g).max(b) > 0.2, "{tint:?}: glow went black");
        }
    }

    #[test]
    fn cover_shadow_ends_above_the_page_clip() {
        // The shadow reaches PAD past the dropped cover; pages are cut at dots_y - 3.
        let bottom = COVER_TOP + COVER + COVER_SHADOW_DROP + COVER_SHADOW_PAD;
        assert!(bottom <= dots_y(186.0) - 3.0, "shadow ends at {bottom}, clipped at {}", dots_y(186.0) - 3.0);
    }

    #[test]
    fn lyrics_fit_inside_the_page_clip() {
        // Right edge of the lyrics box, from the panel's left: past the controls, the divider gaps
        // and the column. A last letter's ink may run a few px further and must not be clipped.
        let right = target(Mode::Expanded, false)[0] - SIDE + 2.0 * LYRIC_GAP + LYRIC_W;
        assert!(right + 4.0 <= panel_w(true) - LYRIC_CLIP, "{right} vs {}", panel_w(true) - LYRIC_CLIP);
    }

    #[test]
    fn soft_text_moves_from_grey_to_tinted_white_with_the_level() {
        let red = (1.0, 0.0, 0.0);
        assert_eq!(soft_text(red, 0.0, 0.6, 1.0).r, 0.6, "no glow: plain grey");
        let c = soft_text(red, 1.0, 0.6, 1.0);
        assert!(c.r > c.g && c.g >= 0.8, "full glow: light, leaning towards red");
        let half = soft_text(red, 0.5, 0.6, 1.0);
        assert!(half.g > 0.6 && half.g < c.g, "in between while fading");
    }

    #[test]
    fn shadow_fades_out_smoothly() {
        let layers: Vec<f32> = (1..=SHADOW_STEPS).map(shadow_layer_alpha).collect();
        let total: f32 = layers.iter().sum();
        assert!((total - SHADOW_ALPHA).abs() < 1e-5, "edge darkness is SHADOW_ALPHA, got {total}");
        assert!(layers.windows(2).all(|w| w[0] > w[1] && w[1] > 0.0), "each wider stroke adds less: {layers:?}");
        assert!(layers[0] < SHADOW_ALPHA * 0.2, "no single stroke makes a visible line: {}", layers[0]);
    }

    #[test]
    fn square_crop_is_the_centred_largest_square() {
        assert_eq!(square_crop(300, 300), (0, 0, 300));
        assert_eq!(square_crop(640, 360), (140, 0, 360));
        assert_eq!(square_crop(300, 301), (0, 0, 300));
        assert_eq!(square_crop(100, 400), (0, 150, 100));
    }

    #[test]
    fn glow_step_fades_in_slides_between_covers_and_settles() {
        let (red, blue) = ((1.0, 0.0, 0.0), (0.0, 0.0, 1.0));
        // From nothing: takes the colour at once, fades the strength in.
        let (c, l, busy) = glow_step(GLOW_NEUTRAL, 0.0, Some(red), 0.016);
        assert_eq!(c, red);
        assert!(l > 0.0 && l < 0.3 && busy);
        // Reaches the end and stops asking for frames.
        let (mut c, mut l, mut busy) = (red, l, true);
        for _ in 0..120 { (c, l, busy) = glow_step(c, l, Some(red), 0.016); }
        assert_eq!((c, l, busy), (red, 1.0, false));
        // A new cover while lit: the colour slides, it does not jump.
        let (c2, l2, busy2) = glow_step(red, 1.0, Some(blue), 0.016);
        assert!(busy2 && l2 == 1.0 && c2.0 < 1.0 && c2.0 > 0.8 && c2.2 > 0.0 && c2.2 < 0.2, "{c2:?}");
        // No cover: fades out, keeps the colour, then stops.
        let (c3, l3, busy3) = glow_step(red, 1.0, None, 0.016);
        assert!(c3 == red && l3 < 1.0 && l3 > 0.8 && busy3);
        let (mut l, mut busy) = (1.0, true);
        for _ in 0..200 { (_, l, busy) = glow_step(red, l, None, 0.016); }
        assert_eq!((l, busy), (0.0, false));
    }

    #[test]
    #[ignore = "uses native Direct2D/WIC; run explicitly for delayed cover loading"]
    fn cover_reloads_when_art_arrives_after_track() {
        unsafe {
            use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let mut renderer = Renderer::new().unwrap();
            let mut track = Track { art_key: 1, ..Default::default() };
            assert!(renderer.cover(&track, 120, 20).is_none());
            let mut bmp = Vec::from(&b"BM"[..]);
            bmp.extend_from_slice(&58u32.to_le_bytes()); bmp.extend_from_slice(&[0; 4]);
            bmp.extend_from_slice(&54u32.to_le_bytes()); bmp.extend_from_slice(&40u32.to_le_bytes());
            bmp.extend_from_slice(&1i32.to_le_bytes()); bmp.extend_from_slice(&1i32.to_le_bytes());
            bmp.extend_from_slice(&1u16.to_le_bytes()); bmp.extend_from_slice(&32u16.to_le_bytes());
            bmp.extend_from_slice(&[0; 24]); bmp.extend_from_slice(&[0, 0, 255, 255]);
            assert_eq!(bmp.len(), 58);
            track.art = Some(std::sync::Arc::new(bmp));
            assert!(renderer.cover(&track, 120, 20).is_some(), "late cover data must invalidate the cached empty image");
            assert!(renderer.cover_fade.as_ref().is_some_and(|f| f.from.is_none()), "late art fades in over the note");
            // The same picture made again at another size is not a new cover: no fade.
            assert!(renderer.cover(&track, 140, 20).is_some());
            assert!(renderer.cover_fade.is_none());
            assert!(renderer.cover(&track, 120, 20).is_some());
        }
    }

    #[test]
    fn seek_and_icon_hit_areas_follow_dpi_and_capabilities() {
        let track = Track { duration_ms: 120_000, can_seek: true, can_toggle: true, can_previous: true, can_next: true,
            lyrics: Arc::new(vec![(0, "la la".into())]), ..Default::default() };
        for scale in [1.0, 1.5, 2.0] {
            let cx = 265.0 * scale;
            let [(_, shuffle), (_, prev), (_, play), (_, next), (_, repeat), (_, seek), (_, lyr)] = buttons(cx, scale, false);
            let mid = |r: D2D_RECT_F| ((r.left + r.right) / 2.0, (r.top + r.bottom) / 2.0);
            assert_eq!(seek_at(cx, scale, seek.left - 100.0, track.duration_ms, false), 0);
            assert_eq!(seek_at(cx, scale, seek.right + 100.0, track.duration_ms, false), 120_000);
            assert_eq!(seek_at(cx, scale, (seek.left + seek.right) / 2.0, track.duration_ms, false), 60_000);
            let (px, py) = mid(play);
            assert_eq!(hit_button(cx, scale, px, py, Some(&track), Page::Music, 0, 0, None, false), Some(Button::PlayPause));
            assert_eq!(hit_button(cx, scale, mid(prev).0, mid(prev).1, Some(&track), Page::Music, 0, 0, None, false), Some(Button::Previous));
            assert_eq!(hit_button(cx, scale, mid(next).0, mid(next).1, Some(&track), Page::Music, 0, 0, None, false), Some(Button::Next));
            assert_eq!(hit_button(cx, scale, mid(seek).0, mid(seek).1, Some(&track), Page::Music, 0, 0, None, false), Some(Button::Seek));
            assert_eq!(hit_button(cx, scale, mid(lyr).0, mid(lyr).1, Some(&track), Page::Music, 0, 0, None, false), Some(Button::Lyrics));
            assert_eq!(hit_button(cx, scale, px, py, None, Page::Music, 0, 0, None, false), None);
            assert!(buttons(cx, scale, false).iter().all(|(_, r)| r.bottom <= dots_y(186.0) * scale));
            // Controls live in the right column: right of the cover, left of the side arrow, no overlaps.
            let left = cx - 240.0 * scale;
            let cover_right = left + (SIDE + COVER) * scale;
            for r in [shuffle, prev, play, next, repeat, seek, lyr] {
                assert!(r.left > cover_right && r.right <= left + (480.0 - SIDE) * scale, "{r:?}");
            }
            assert!(shuffle.right <= prev.left && prev.right <= play.left && play.right <= next.left && next.right <= repeat.left && repeat.right <= lyr.left, "buttons must not overlap");
            // Shuffle and repeat answer only when the player can do them.
            assert_eq!(hit_button(cx, scale, mid(shuffle).0, mid(shuffle).1, Some(&track), Page::Music, 0, 0, None, false), None);
            let both = Track { can_shuffle: true, can_repeat: true, ..track.clone() };
            assert_eq!(hit_button(cx, scale, mid(shuffle).0, mid(shuffle).1, Some(&both), Page::Music, 0, 0, None, false), Some(Button::Shuffle));
            assert_eq!(hit_button(cx, scale, mid(repeat).0, mid(repeat).1, Some(&both), Page::Music, 0, 0, None, false), Some(Button::Repeat));
            assert!(seek.bottom <= prev.top.min(play.top).min(lyr.top), "bar above the buttons");
            // The lyrics button works only when the song has lyrics.
            let bare = Track { lyrics: Arc::new(Vec::new()), ..track.clone() };
            assert_eq!(hit_button(cx, scale, mid(lyr).0, mid(lyr).1, Some(&bare), Page::Music, 0, 0, None, false), None);
        }
    }

    #[test]
    fn clear_all_sits_in_the_left_column_and_the_rows_fit_the_panel() {
        let ph = panel_size(Page::Notify)[1];
        assert!(CLEAR_W <= NOTE_X && CLEAR_Y + CLEAR_H <= dots_y(ph) - 3.0, "Clear all stays in the left column, above the dots");
        assert!(NOTE_Y + NOTE_H * NOTE_ROWS as f32 <= dots_y(ph) - 3.0, "the rows end inside the pages' clip");
        for scale in [1.0, 1.5] {
            let cx = 265.0 * scale;
            // The left column starts one SIDE in from the panel's left edge (it is 480 wide).
            let (x, y) = (cx + (-240.0 + SIDE + CLEAR_W / 2.0) * scale, (CLEAR_Y + CLEAR_H / 2.0) * scale);
            assert_eq!(hit_button(cx, scale, x, y, None, Page::Notify, 2, 0, None, false), Some(Button::NotesClear));
            assert_eq!(hit_button(cx, scale, x, y, None, Page::Notify, 0, 0, None, false), None, "nothing to clear: no button");
        }
    }

    #[test]
    fn with_the_lyrics_open_the_player_keeps_its_place_in_the_left_part() {
        let track = Track { duration_ms: 120_000, can_seek: true, can_toggle: true, can_previous: true, can_next: true,
            lyrics: Arc::new(vec![(0, "la la".into())]), ..Default::default() };
        for scale in [1.0, 1.5] {
            let cx = 400.0 * scale; // the panel stays centred on the same point
            let (narrow, wide) = (buttons(cx, scale, false), buttons(cx, scale, true));
            let shift = LYRICS_EXTRA_W / 2.0 * scale;
            for ((b, n), (_, w)) in narrow.iter().zip(wide.iter()) {
                assert!((n.left - shift - w.left).abs() < 0.01 && (n.right - shift - w.right).abs() < 0.01 && n.top == w.top, "{b:?}: moves left by half the extra width");
            }
            // Clicks follow it.
            let (_, play) = wide[2];
            let (px, py) = ((play.left + play.right) / 2.0, (play.top + play.bottom) / 2.0);
            assert_eq!(hit_button(cx, scale, px, py, Some(&track), Page::Music, 0, 0, None, true), Some(Button::PlayPause));
            assert_ne!(hit_button(cx, scale, px, py, Some(&track), Page::Music, 0, 0, None, false), Some(Button::PlayPause), "the old place is no longer the button");
            // The page arrows are gone while the lyrics are open, and back when they close.
            let has_arrows = |wide| page_buttons(cx, scale, Page::Music, 0, 0, None, wide).iter().any(|(b, _)| matches!(b, Button::PagePrev | Button::PageNext));
            assert!(!has_arrows(true));
            assert!(has_arrows(false));
        }
    }

    #[test]
    #[ignore = "writes a Direct2D preview of the lyrics view for visual review"]
    fn lyrics_preview() {
        unsafe {
            use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let (w, h) = (820, 210);
            let hwnd = CreateWindowExW(WS_EX_NOREDIRECTIONBITMAP, w!("STATIC"), w!("Lyrics preview"), WS_POPUP, 0, 0, w, h, None, None, None, None).unwrap();
            let lines = [
                // Made-up lines about the feature itself (no song's lyrics in the README pictures).
                (0, "Lời mẫu cho ảnh xem trước"), (6_000, "Lời lấy từ LRCLIB và chạy theo đúng thời gian của bài đang phát trên máy"),
                (12_000, "Dòng đang hát sáng lên ở giữa"), (18_000, "Dòng dài tự xuống dòng cho vừa khung lời bên phải, không bị cắt mất chữ"),
                (24_000, "Bấm nút lời để mở hoặc đóng khung này"), (30_000, "Tua bài thì lời nhảy theo"),
                (36_000, "Hết lời thì khung tự trống"), (42_000, "Dòng mẫu cuối cùng"),
            ];
            let track = Track { title: "Bài hát mẫu".into(), artist: "Nghệ sĩ mẫu".into(), duration_ms: 210_000, position_ms: 14_000, playing: true,
                can_toggle: true, can_next: true, can_previous: true, can_seek: true,
                lyrics: Arc::new(lines.iter().map(|(t, s)| (*t as u64, s.to_string())).collect()),
                // A cover image for the glow and the cover slot: PUW_PREVIEW_COVER=path.
                art: std::env::var("PUW_PREVIEW_COVER").ok().and_then(|p| std::fs::read(p).ok()).map(Arc::new), art_key: 1, ..Default::default() };
            let mut size = target(Mode::Expanded, false);
            size[0] += LYRICS_EXTRA_W;
            let view = View { mode: Mode::Expanded, size, time: "23:16", date: "T7, 24/10", battery: Some(Battery { percent: 70, charging: true, saver: false }),
                banner: None, music: Some(&track), live: true, robot: None, phase: 0.0, banner_age: 0.0, lyrics: true, hovered: None, page: Page::Music,
                leaving: None, todos: &[], notes: &[], weather: &weather::State::default(), page_age: 0.0, unit: weather::Unit::C, sun_view: false,
                list: ListView { counts: (0, 0), remind_before: 15, now: 0, draft: "", editing: false, edit_item: None, caret: false, scroll: 0, note_pos: 0.0 },
                focus: &Focus::default(), health: HealthView { h: &Health::default(), now: 0, minute_of_day: 0, settings: None } };
            let mut renderer = Renderer::new().unwrap();
            renderer.grab = true;
            // The cover loads and the glow fades in over a second or two.
            let frames = std::env::var("PUW_PREVIEW_FRAMES").ok().and_then(|f| f.parse().ok()).unwrap_or(120);
            for _ in 0..(if track.art.is_some() { frames } else { 1 }) {
                renderer.present(hwnd, 0, 0, w, h, 1.0, &view).unwrap();
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            let pixels = &renderer.grabbed[..];
            let mut bytes = Vec::new(); bytes.extend_from_slice(b"BM");
            bytes.extend_from_slice(&(54u32 + pixels.len() as u32).to_le_bytes()); bytes.extend_from_slice(&[0; 4]);
            bytes.extend_from_slice(&54u32.to_le_bytes()); bytes.extend_from_slice(&40u32.to_le_bytes());
            bytes.extend_from_slice(&w.to_le_bytes()); bytes.extend_from_slice(&(-h).to_le_bytes());
            bytes.extend_from_slice(&1u16.to_le_bytes()); bytes.extend_from_slice(&32u16.to_le_bytes());
            bytes.extend_from_slice(&[0; 24]); bytes.extend_from_slice(pixels);
            std::fs::write(std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/lyrics-preview.bmp"), bytes).unwrap();
            DestroyWindow(hwnd).unwrap();
        }
    }

    #[test]
    #[ignore = "benchmark: prints the time one Music-page frame takes"]
    fn perf_music_page() {
        unsafe {
            use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let (w, h) = (820, 210);
            let hwnd = CreateWindowExW(WS_EX_NOREDIRECTIONBITMAP, w!("STATIC"), w!("perf"), WS_POPUP, 0, 0, w, h, None, None, None, None).unwrap();
            let track = Track { title: "Bản nhạc đang phát".into(), artist: "YouTube / foobar2000".into(),
                duration_ms: 210_000, position_ms: 14_000, playing: true, can_toggle: true, can_next: true,
                can_previous: true, can_seek: true,
                art: std::env::var("PUW_PREVIEW_COVER").ok().and_then(|p| std::fs::read(p).ok()).map(Arc::new), art_key: 1,
                lyrics: Arc::new(vec![(0, "la la".into())]), ..Default::default() };
            let mut size = target(Mode::Expanded, false);
            size[0] += LYRICS_EXTRA_W;
            let view = View { mode: Mode::Expanded, size, time: "23:16", date: "T7, 24/10",
                battery: Some(Battery { percent: 70, charging: true, saver: false }), banner: None, music: Some(&track),
                live: true, robot: None, phase: 0.0, banner_age: 0.0, lyrics: true, hovered: None, page: Page::Music,
                leaving: None, todos: &[], notes: &[], weather: &weather::State::default(), page_age: 0.0, unit: weather::Unit::C, sun_view: false,
                list: ListView { counts: (0, 0), remind_before: 15, now: 0, draft: "", editing: false, edit_item: None, caret: false, scroll: 0, note_pos: 0.0 },
                focus: &Focus::default(), health: HealthView { h: &Health::default(), now: 0, minute_of_day: 0, settings: None } };
            let mut renderer = Renderer::new().unwrap();
            // Let the cover decode and the glow reach its steady strength.
            for _ in 0..400 {
                renderer.present(hwnd, 0, 0, w, h, 1.0, &view).unwrap();
            }
            let n = 400;
            let mut ms: Vec<f64> = Vec::with_capacity(n);
            for _ in 0..n {
                let t = std::time::Instant::now();
                renderer.present(hwnd, 0, 0, w, h, 1.0, &view).unwrap();
                ms.push(t.elapsed().as_secs_f64() * 1000.0);
            }
            ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
            println!("present: median {:.3} ms | p90 {:.3} ms | mean {:.3} ms | min {:.3} ms",
                ms[n / 2], ms[n * 9 / 10], ms.iter().sum::<f64>() / n as f64, ms[0]);
            DestroyWindow(hwnd).unwrap();
        }
    }

    #[test]
    #[ignore = "writes target/todo-preview.bmp: the Tasks page empty, with tasks of every kind, and while typing"]
    fn todo_preview() {
        use crate::todo::{stamp, Item};
        unsafe {
            use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let (w, h) = (530i32, 210i32);
            let hwnd = CreateWindowExW(WS_EX_NOREDIRECTIONBITMAP, w!("STATIC"), w!("Todo preview"), WS_POPUP, 0, 0, w, h, None, None, None, None).unwrap();
            let now = stamp(20261029, 2, 5);
            let item = |text: &str, done: bool, at: Option<u64>| Item { text: text.into(), done: done.then_some(20261029), at, told_soon: false, told_now: false };
            let tasks = vec![
                item("Nộp báo cáo tuần", false, Some(stamp(20261028, 9, 0))),
                item("Uống thuốc", false, Some(stamp(20261029, 2, 15))),
                item("Họp nhóm dự án", false, Some(stamp(20261029, 21, 0))),
                item("Mua sữa và trứng", false, None),
                item("Khám răng", false, Some(stamp(20261030, 9, 0))),
                item("Gọi cho khách hàng", true, None),
            ];
            // (tasks, counts, typed text)
            let frames: Vec<(Vec<Item>, (usize, usize), &str)> = vec![(vec![], (0, 0), ""), (tasks.clone(), (3, 1), ""), (tasks, (3, 1), "Họp 20:00 mai")];
            let mut renderer = Renderer::new().unwrap();
            renderer.grab = true;
            let mut sheet = Vec::new();
            for (items, counts, typed) in &frames {
                let view = View { mode: Mode::Expanded, size: target(Mode::Expanded, false), time: "02:05", date: "T5, 29/10", battery: Some(Battery { percent: 76, charging: false, saver: false }),
                    banner: None, music: None, live: false, robot: None, phase: 0.0, banner_age: 0.0, lyrics: false, hovered: None, page: Page::Todo,
                    leaving: None, todos: items, notes: &[], weather: &weather::State::default(), page_age: 9.0, unit: weather::Unit::C, sun_view: false,
                    list: ListView { counts: *counts, remind_before: 15, now, draft: typed, editing: !typed.is_empty(), edit_item: None, caret: true, scroll: 0, note_pos: 0.0 },
                    focus: &Focus::default(), health: HealthView { h: &Health::default(), now: 0, minute_of_day: 0, settings: None } };
                for _ in 0..3 {
                    renderer.present(hwnd, 0, 0, w, h, 1.0, &view).unwrap();
                }
                sheet.extend_from_slice(&renderer.grabbed[..]);
            }
            let sh = h * frames.len() as i32;
            let mut bytes = Vec::new(); bytes.extend_from_slice(b"BM");
            bytes.extend_from_slice(&(54u32 + sheet.len() as u32).to_le_bytes()); bytes.extend_from_slice(&[0; 4]);
            bytes.extend_from_slice(&54u32.to_le_bytes()); bytes.extend_from_slice(&40u32.to_le_bytes());
            bytes.extend_from_slice(&w.to_le_bytes()); bytes.extend_from_slice(&(-sh).to_le_bytes());
            bytes.extend_from_slice(&1u16.to_le_bytes()); bytes.extend_from_slice(&32u16.to_le_bytes());
            bytes.extend_from_slice(&[0; 24]); bytes.extend_from_slice(&sheet);
            std::fs::write(std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/todo-preview.bmp"), bytes).unwrap();
            DestroyWindow(hwnd).unwrap();
        }
    }

    #[test]
    #[ignore = "writes target/weather-preview.bmp: the pill (plain and with music) and the Weather page in each sky"]
    fn weather_preview() {
        use weather::{Now, Place, Sky, State};
        unsafe {
            use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let (w, h) = (530i32, 210i32);
            let hwnd = CreateWindowExW(WS_EX_NOREDIRECTIONBITMAP, w!("STATIC"), w!("Weather preview"), WS_POPUP, 0, 0, w, h, None, None, None, None).unwrap();
            let track = Track { title: "Bài hát mẫu".into(), artist: "Nghệ sĩ mẫu".into(), duration_ms: 210_000, position_ms: 64_000, playing: true,
                can_toggle: true, can_next: true, can_previous: true, can_seek: true, ..Default::default() };
            let place = Some(Place { name: "Đà Nẵng".into(), lat: 16.0, lon: 108.0 });
            let state = |sky, day, temp: f32| State { place: place.clone(), now: Some(Now { temp, feels: temp + 3.0, humidity: 82.0, wind: 11.0, sky, day }), updated: Some(202610290140), ..State::default() };
            let mut renderer = Renderer::new().unwrap();
            renderer.grab = true;
            let mut sheet = Vec::new();
            // (mode, music?, page, state, typed text)
            let frames: Vec<(Mode, bool, Page, State, &str, weather::Unit)> = vec![
                (Mode::Collapsed, false, Page::Music, state(Sky::Clear, true, 31.0), "", weather::Unit::F),
                (Mode::Expanded, false, Page::Weather, state(Sky::Cloudy, true, 28.0), "", weather::Unit::F),
                // Conditions fetched over 90 minutes ago: dimmed and marked "old".
                (Mode::Expanded, false, Page::Weather, State { updated: Some(202610290020), ..state(Sky::Rain, true, 26.0) }, "", weather::Unit::C),
                (Mode::Collapsed, false, Page::Music, state(Sky::PartlyCloudy, true, 29.0), "", weather::Unit::C),
                (Mode::Collapsed, true, Page::Music, state(Sky::Rain, true, 26.0), "", weather::Unit::C),
                (Mode::Expanded, false, Page::Weather, state(Sky::Storm, true, 27.0), "", weather::Unit::C),
                (Mode::Expanded, false, Page::Weather, state(Sky::Clear, false, 24.0), "", weather::Unit::C),
                (Mode::Expanded, false, Page::Weather, State::default(), "Đà Nẵ", weather::Unit::C),
                (Mode::Expanded, false, Page::Weather, State { not_found: Some("Xyz".into()), ..State::default() }, "", weather::Unit::C),
                // The other skies, for the icons.
                (Mode::Expanded, false, Page::Weather, state(Sky::PartlyCloudy, true, 30.0), "", weather::Unit::C),
                (Mode::Expanded, false, Page::Weather, state(Sky::PartlyCloudy, false, 25.0), "", weather::Unit::C),
                (Mode::Expanded, false, Page::Weather, state(Sky::Cloudy, true, 28.0), "", weather::Unit::C),
                (Mode::Expanded, false, Page::Weather, state(Sky::Rain, true, 26.0), "", weather::Unit::C),
                (Mode::Expanded, false, Page::Weather, state(Sky::Fog, true, 22.0), "", weather::Unit::C),
                (Mode::Expanded, false, Page::Weather, state(Sky::Snow, true, 1.0), "", weather::Unit::C),
                (Mode::Expanded, false, Page::Weather, state(Sky::Clear, true, 33.0), "", weather::Unit::C),
                (Mode::Expanded, false, Page::Weather, state(Sky::Snow, true, -12.0), "", weather::Unit::C),
                (Mode::Expanded, false, Page::Weather, state(Sky::Clear, false, -89.0), "", weather::Unit::F),
                (Mode::Collapsed, false, Page::Music, state(Sky::Snow, true, -12.0), "", weather::Unit::C),
                (Mode::Collapsed, false, Page::Music, state(Sky::Clear, true, 9.0), "", weather::Unit::C),
                (Mode::Collapsed, false, Page::Music, state(Sky::Snow, true, -26.0), "", weather::Unit::F),
                (Mode::Collapsed, false, Page::Music, state(Sky::Snow, true, -89.0), "", weather::Unit::F),
                (Mode::Collapsed, false, Page::Music, state(Sky::Clear, true, 44.0), "", weather::Unit::F),
                (Mode::Collapsed, true, Page::Music, state(Sky::Snow, true, -26.0), "", weather::Unit::F),
            ];
            let n = frames.len() as i32;
            for (mode, music, page, st, typed, unit) in &frames {
                let view = View { mode: *mode, size: target(*mode, *music), time: "02:05", date: "T5, 29/10", battery: Some(Battery { percent: 76, charging: false, saver: false }),
                    banner: None, music: music.then_some(&track), live: *music, robot: None, phase: 0.0, banner_age: 0.0, lyrics: false, hovered: None, page: *page,
                    leaving: None, todos: &[], notes: &[], weather: st, page_age: 9.0, unit: *unit, sun_view: false,
                    list: ListView { counts: (0, 0), remind_before: 15, now: 202610290205, draft: typed, editing: !typed.is_empty(), edit_item: None, caret: true, scroll: 0, note_pos: 0.0 },
                    focus: &Focus::default(), health: HealthView { h: &Health::default(), now: 0, minute_of_day: 0, settings: None } };
                for _ in 0..3 {
                    renderer.present(hwnd, 0, 0, w, h, 1.0, &view).unwrap();
                }
                sheet.extend_from_slice(&renderer.grabbed[..]);
            }
            let sh = h * n;
            let mut bytes = Vec::new(); bytes.extend_from_slice(b"BM");
            bytes.extend_from_slice(&(54u32 + sheet.len() as u32).to_le_bytes()); bytes.extend_from_slice(&[0; 4]);
            bytes.extend_from_slice(&54u32.to_le_bytes()); bytes.extend_from_slice(&40u32.to_le_bytes());
            bytes.extend_from_slice(&w.to_le_bytes()); bytes.extend_from_slice(&(-sh).to_le_bytes());
            bytes.extend_from_slice(&1u16.to_le_bytes()); bytes.extend_from_slice(&32u16.to_le_bytes());
            bytes.extend_from_slice(&[0; 24]); bytes.extend_from_slice(&sheet);
            std::fs::write(std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/weather-preview.bmp"), bytes).unwrap();
            DestroyWindow(hwnd).unwrap();
        }
    }

    #[test]
    fn only_the_apps_found_get_a_tab() {
        let tabs = |mask| {
            set_usage_apps(mask);
            let found: Vec<Button> = page_buttons(410.0, 1.0, Page::Usage, 0, 0, None, false).into_iter().map(|(b, _)| b).filter(|b| matches!(b, Button::UsageTab(_))).collect();
            set_usage_apps(0b111);
            found
        };
        assert_eq!(tabs(0b101), vec![Button::UsageTab(0), Button::UsageTab(2)], "Claude and Antigravity, no Codex");
        assert_eq!(tabs(0b010), vec![Button::UsageTab(1)]);
        assert!(tabs(0).is_empty(), "none of them: no tab");
    }

    #[test]
    fn banner_close_sits_in_the_top_right_corner() {
        let (cx, w) = (410.0, notch::alert_size(1)[0]);
        for (scale, pill) in [(1.0, false), (2.0, false), (1.0, true), (2.0, true)] {
            let b = banner_close(cx * scale, scale, w, pill);
            assert!(b.right <= (cx + w / 2.0) * scale && b.left > cx * scale, "inside the banner's right half");
            assert!(b.top >= 0.0 && b.bottom <= 28.0 * scale, "in the top row, above the title");
            assert!(b.right - b.left >= 20.0 * scale - 0.01, "easy to hit");
        }
    }

    #[test]
    #[ignore = "writes target/banner-preview.bmp: a notification banner (with its x), a Bluetooth battery banner and a charging banner and an approval banner"]
    fn banner_preview() {
        unsafe {
            use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let (w, h) = (500i32, 110i32);
            let hwnd = CreateWindowExW(WS_EX_NOREDIRECTIONBITMAP, w!("STATIC"), w!("Banner preview"), WS_POPUP, 0, 0, w, h, None, None, None, None).unwrap();
            let note = Toast { app: "powershell".into(), title: "Anh Minh".into(), body: "Tối nay 8h họp nhóm nhé, nhớ mang laptop.".into(), level: None, icon: None, image: None, open: Some("powershell".into()), badge: None, choices: Vec::new(), app_id: None };
            let device = Toast { app: "Bluetooth".into(), title: "UGREEN HiTune Max5c".into(), body: "Đã kết nối".into(), level: None,
                icon: Some(device_icon(crate::connectivity::DeviceKind::Headphones)), image: None, open: None, badge: Some("80%".into()), choices: Vec::new(), app_id: None };
            let charging = Toast { app: "Pin".into(), title: "Đang sạc".into(), body: "Đã cắm sạc".into(), level: None,
                icon: Some(ICON_PLUG), image: None, open: None, badge: Some("70%".into()), choices: Vec::new(), app_id: None };
            let mut renderer = Renderer::new().unwrap();
            renderer.grab = true;
            let mut sheet = Vec::new();
            let approve = Toast { app: "Claude".into(), title: "Bash".into(), body: "Cho phép chạy list_projects của codebase-memory-mcp để liệt kê các project đã được index trong đồ thị mã nguồn (chỉ đọc, không thay đổi gì)?".into(), level: None, icon: Some(ICON_WARN), image: None, open: None, badge: None, choices: vec!["Đồng ý".into(), "Không đồng ý".into(), "Trong app".into()] , app_id: None };
            for (toast, lines) in [(&note, 1), (&device, 0), (&charging, 0), (&approve, 3)] {
            let view = View { mode: Mode::Alert, size: if toast.choices.is_empty() { notch::alert_size(lines) } else { notch::alert_size_choice(lines) }, time: "20:49", date: "T2, 26/10", battery: Some(Battery { percent: 76, charging: true, saver: false }),
                banner: Some(toast), music: None, live: false, robot: None, phase: 0.0, banner_age: 3.0, lyrics: false, hovered: None, page: Page::Music,
                leaving: None, todos: &[], notes: &[], weather: &weather::State::default(), page_age: 0.0, unit: weather::Unit::C, sun_view: false,
                list: ListView { counts: (0, 0), remind_before: 15, now: 0, draft: "", editing: false, edit_item: None, caret: false, scroll: 0, note_pos: 0.0 },
                focus: &Focus::default(), health: HealthView { h: &Health::default(), now: 0, minute_of_day: 0, settings: None } };
            renderer.present(hwnd, 0, 0, w, h, 1.0, &view).unwrap();
            sheet.extend_from_slice(&renderer.grabbed[..]);
            }
            let (pixels, h) = (&sheet[..], h * 4);
            let mut bytes = Vec::new(); bytes.extend_from_slice(b"BM");
            bytes.extend_from_slice(&(54u32 + pixels.len() as u32).to_le_bytes()); bytes.extend_from_slice(&[0; 4]);
            bytes.extend_from_slice(&54u32.to_le_bytes()); bytes.extend_from_slice(&40u32.to_le_bytes());
            bytes.extend_from_slice(&w.to_le_bytes()); bytes.extend_from_slice(&(-h).to_le_bytes());
            bytes.extend_from_slice(&1u16.to_le_bytes()); bytes.extend_from_slice(&32u16.to_le_bytes());
            bytes.extend_from_slice(&[0; 24]); bytes.extend_from_slice(pixels);
            std::fs::write(std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/banner-preview.bmp"), bytes).unwrap();
            DestroyWindow(hwnd).unwrap();
        }
    }

    #[test]
    #[ignore = "writes target/aurora-preview.bmp: four frames of the Music page, 1.5 s apart"]
    fn aurora_preview() {
        unsafe {
            use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let (w, h) = (530i32, 210i32);
            let hwnd = CreateWindowExW(WS_EX_NOREDIRECTIONBITMAP, w!("STATIC"), w!("Aurora preview"), WS_POPUP, 0, 0, w, h, None, None, None, None).unwrap();
            let track = Track { title: "Bài hát mẫu".into(), artist: "Nghệ sĩ mẫu".into(), duration_ms: 210_000, position_ms: 64_000, playing: true,
                can_toggle: true, can_next: true, can_previous: true, can_seek: true,
                art: std::env::var("PUW_PREVIEW_COVER").ok().and_then(|p| std::fs::read(p).ok()).map(Arc::new), art_key: 1, ..Default::default() };
            let mut renderer = Renderer::new().unwrap();
            renderer.grab = true;
            let mut sheet = Vec::new();
            for (i, t) in [0.0f32, 1.5, 3.0, 4.5].into_iter().enumerate() {
                let view = View { mode: Mode::Expanded, size: target(Mode::Expanded, false), time: "23:16", date: "T5, 29/10",
                    battery: Some(Battery { percent: 70, charging: true, saver: false }), banner: None, music: Some(&track), live: true, robot: None,
                    phase: t, banner_age: 0.0, lyrics: false, hovered: None, page: Page::Music, leaving: None, todos: &[], notes: &[], weather: &weather::State::default(), page_age: 0.0, unit: weather::Unit::C, sun_view: false,
                    list: ListView { counts: (0, 0), remind_before: 15, now: 0, draft: "", editing: false, edit_item: None, caret: false, scroll: 0, note_pos: 0.0 },
                    focus: &Focus::default(), health: HealthView { h: &Health::default(), now: 0, minute_of_day: 0, settings: None } };
                // The first frame waits for the cover and the fade-in.
                for _ in 0..(if i == 0 { 80 } else { 1 }) {
                    renderer.present(hwnd, 0, 0, w, h, 1.0, &view).unwrap();
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                sheet.extend_from_slice(&renderer.grabbed[..]);
            }
            let sh = h * 4;
            let mut bytes = Vec::new(); bytes.extend_from_slice(b"BM");
            bytes.extend_from_slice(&(54u32 + sheet.len() as u32).to_le_bytes()); bytes.extend_from_slice(&[0; 4]);
            bytes.extend_from_slice(&54u32.to_le_bytes()); bytes.extend_from_slice(&40u32.to_le_bytes());
            bytes.extend_from_slice(&w.to_le_bytes()); bytes.extend_from_slice(&(-sh).to_le_bytes());
            bytes.extend_from_slice(&1u16.to_le_bytes()); bytes.extend_from_slice(&32u16.to_le_bytes());
            bytes.extend_from_slice(&[0; 24]); bytes.extend_from_slice(&sheet);
            std::fs::write(std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/aurora-preview.bmp"), bytes).unwrap();
            DestroyWindow(hwnd).unwrap();
        }
    }

    #[test]
    #[ignore = "timing: prints ms per frame (average and worst) with music playing: pill, mini line, lyrics view and each page at 200 % scale"]
    fn frame_time() {
        unsafe {
            use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let scale = 2.0;
            let (w, h) = ((crate::notch::MAX_W * scale) as i32, (crate::notch::MAX_H * scale) as i32);
            let hwnd = CreateWindowExW(WS_EX_NOREDIRECTIONBITMAP, w!("STATIC"), w!("Frame time"), WS_POPUP, 0, 0, w, h, None, None, None, None).unwrap();
            // Playing, with a lyric line every 0.4 s so the lyrics keep moving while timed.
            let track = Track { title: "Bài hát mẫu".into(), artist: "Nghệ sĩ mẫu".into(), duration_ms: 210_000, position_ms: 64_000, playing: true,
                can_toggle: true, can_next: true, can_previous: true, can_seek: true, art_key: 1, at: Some(Instant::now()),
                lyrics: Arc::new((0..600).map(|k| (k * 400, format!("Dòng lời mẫu số {k} chạy theo bài đang phát"))).collect()),
                art: Some(Arc::new(std::fs::read(std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/screenshots/page-music.png")).unwrap())), ..Default::default() };
            let (wx, health, idle_focus) = (weather::State::default(), Health::default(), Focus::default());
            let mut renderer = Renderer::new().unwrap();
            let cases = [("pill", Mode::Collapsed, Page::Music, false), ("mini", Mode::Mini, Page::Music, false), ("lyrics", Mode::Expanded, Page::Music, true)].into_iter()
                .chain(PAGES.map(|p| ("page", Mode::Expanded, p, false)));
            for (name, mode, page, lyrics) in cases {
                let mut size = target(mode, true);
                if lyrics { size[0] += LYRICS_EXTRA_W; }
                let view = |phase: f32| View { mode, size, time: "23:16", date: "T5, 29/10",
                    battery: Some(Battery { percent: 70, charging: true, saver: false }), banner: None, music: Some(&track), live: true, robot: None,
                    phase, banner_age: 0.0, lyrics, hovered: None, page, leaving: None, todos: &[], notes: &[], weather: &wx, page_age: 5.0, unit: weather::Unit::C, sun_view: false,
                    list: ListView { counts: (0, 0), remind_before: 15, now: 0, draft: "", editing: false, edit_item: None, caret: false, scroll: 0, note_pos: 0.0 },
                    focus: &idle_focus, health: HealthView { h: &health, now: 1_000_000, minute_of_day: 840, settings: None } };
                // Settle first: the cover loads and the Music page's glow fades in.
                for i in 0..120 {
                    renderer.present(hwnd, 0, 0, w, h, scale, &view(i as f32 / 60.0)).unwrap();
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                let (n, mut sum, mut worst, mut over) = (600, 0.0f64, 0.0f64, 0);
                for i in 0..n {
                    let t0 = Instant::now();
                    renderer.present(hwnd, 0, 0, w, h, scale, &view(2.0 + i as f32 / 60.0)).unwrap();
                    let ms = t0.elapsed().as_secs_f64() * 1000.0;
                    sum += ms;
                    worst = worst.max(ms);
                    over += usize::from(ms > 1000.0 / 60.0);
                }
                println!("{name:6} {page:?}: {:.2} ms/frame, worst {worst:.2} ms, {over}/{n} over 16.7 ms", sum / n as f64);
            }
            DestroyWindow(hwnd).unwrap();
        }
    }

    #[test]
    fn the_page_icons_close_up_when_there_are_many_so_the_row_always_fits() {
        assert_eq!(nav_pitch(7), NAV_PITCH, "as many pages as today: the usual spacing");
        for pages in [1, 7, 11, 14, 20, 30] {
            assert!(nav_pitch(pages) * pages as f32 <= NAV_ROW_W + 0.01, "{pages} pages: the row is {} wide", nav_pitch(pages) * pages as f32);
        }
        assert!(nav_pitch(14) < nav_pitch(7), "more pages, closer");
        let cells = nav_cells(240.0, 1.0, 186.0);
        assert!(cells.windows(2).all(|c| c[1].1.left - c[0].1.right >= NAV_GAP - 0.01), "neighbouring cells never touch");
    }

    #[test]
    fn the_page_icons_sit_where_the_dots_are_with_a_thin_margin_under_them() {
        let ph = 186.0;
        let cells = nav_cells(240.0, 1.0, ph);
        let first = cells[0].1;
        assert_eq!(first.top, dots_y(ph), "they open where the dots are");
        assert_eq!(first.bottom - first.top, 22.0);
        // The dots rest 7 px above the panel's bottom (ph - (dots_y + 5)); the opened panel leaves 5 px under the cells
        // (7 was too thick to the eye: the glyph is 16 px in a 22 px cell).
        assert_eq!(ph - (dots_y(ph) + 5.0), 7.0);
        assert_eq!(ph + NAV_EXTRA - first.bottom, 5.0);
    }

    #[test]
    #[ignore = "writes target/readme/<name>.bmp (900 x 230, the notch centred at the top): the README pictures of the pill and the pages"]
    fn readme_shots() {
        use weather::{Now, Place, Sky, State};
        unsafe {
            use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let (w, h) = (900, 340);
            // PUW_README_LANG=en draws the English set (into target/readme-en) with English sample content.
            let english = std::env::var("PUW_README_LANG").is_ok_and(|v| v == "en");
            crate::i18n::set_english(english);
            let tl = |vi: &'static str, en: &'static str| if english { en } else { vi };
            let en_date = |d: &'static str| if english { match d { "T7, 24/10" => "Sat, 24/10", "T2, 26/10" => "Mon, 26/10", "T5, 29/10" => "Thu, 29/10", d => d } } else { d };
            let hwnd = CreateWindowExW(WS_EX_NOREDIRECTIONBITMAP, w!("STATIC"), w!("README shots"), WS_POPUP, 0, 0, w, h, None, None, None, None).unwrap();
            let lines = [
                // Made-up lines about the feature itself (no song's lyrics in the README pictures).
                (0, tl("Lời mẫu cho ảnh xem trước", "Sample lyrics for the preview")),
                (6_000, tl("Lời lấy từ LRCLIB và chạy theo đúng thời gian của bài đang phát trên máy", "Lyrics come from LRCLIB and follow the song playing on this PC")),
                (12_000, tl("Dòng đang hát sáng lên ở giữa", "The line being sung lights up in the middle")),
                (18_000, tl("Dòng dài tự xuống dòng cho vừa khung lời bên phải, không bị cắt mất chữ", "A long line wraps to fit the lyrics column on the right, with no words cut off")),
                (24_000, tl("Bấm nút lời để mở hoặc đóng khung này", "Press the lyrics button to open or close this panel")),
                (30_000, tl("Tua bài thì lời nhảy theo", "Seek the song and the lyrics jump along")),
            ];
            // A cover image for the glow and the cover slot: PUW_PREVIEW_COVER=path.
            let art = std::env::var("PUW_PREVIEW_COVER").ok().and_then(|p| std::fs::read(p).ok()).map(Arc::new);
            let song = |position_ms| Track { title: tl("Bài hát mẫu", "Sample song").into(), artist: tl("Nghệ sĩ mẫu", "Sample artist").into(), duration_ms: 210_000, position_ms, playing: true,
                can_toggle: true, can_next: true, can_previous: true, can_seek: true, can_shuffle: true, can_repeat: true,
                repeat: crate::music::Repeat::All, art: art.clone(), art_key: 1,
                lyrics: Arc::new(lines.iter().map(|(t, s)| (*t as u64, s.to_string())).collect()), ..Default::default() };
            let (playing, singing) = (song(64_000), song(14_000));
            let place = Some(Place { name: tl("Đà Nẵng", "Da Nang").into(), lat: 16.0, lon: 108.0 });
            let sky = |sky, temp: f32| State { place: place.clone(), now: Some(Now { temp, feels: temp + 3.0, humidity: 82.0, wind: 11.0, sky, day: true }), updated: Some(202610290140), ..State::default() };
            let (none, partly, rain) = (State::default(), sky(Sky::PartlyCloudy, 29.0), sky(Sky::Rain, 26.0));
            // With the next 24 hours: a shower late in the afternoon, so the outlook line has rain.
            let hours: Vec<weather::Hour> = (0..24).map(|i| {
                let hour = ((2 + i) % 24) as u8;
                let rain = match hour { 16 => 40, 17 => 70, _ => 0 };
                weather::Hour { hour, temp: if (6..18).contains(&hour) { 26.0 + (hour.min(13) - 6) as f32 } else { 25.0 }, rain, wind: 12.0, sky: if rain >= 50 { Sky::Rain } else { Sky::PartlyCloudy }, day: (6..18).contains(&hour) }
            }).collect();
            let partly_30 = State { hours, ..sky(Sky::PartlyCloudy, 30.0) };
            // The sun's path on a day with sunrise at 05:42 and sunset at 17:48 (the clock of the pictures reads 14:00).
            let sunny = State { sun: Some(weather::Sun { rise: 342, set: 1068 }), ..partly_30.clone() };
            // The same day when the sun has set (the pictures' clock reads 14:00): sunrise 04:30, sunset 13:15.
            let night = State { sun: Some(weather::Sun { rise: 270, set: 795 }), ..partly_30.clone() };
            // The third focus round, 7 min 18 s in.
            let running_focus = Focus::demo(4, std::time::Duration::from_secs(7 * 60 + 18));
            let mut health = Health::default();
            // A mid-afternoon day: 750 ml of 2000 so far, 42 minutes at the desk.
            (health.water_on, health.sit_on, health.taken_ml, health.water_mark, health.sitting_since) = (true, true, 750, Some(999_970), Some(999_958));
            let charging = |percent| Some(Battery { percent, charging: true, saver: false });
            let on_battery = Some(Battery { percent: 76, charging: false, saver: false });
            // (file, mode, music, page, lyrics open, weather, battery, time, date)
            let shots: [(&str, Mode, Option<&Track>, Page, bool, &State, Option<Battery>, &str, &str); 16] = [
                ("page-nav", Mode::Expanded, Some(&playing), Page::Music, false, &none, charging(70), "23:16", "T7, 24/10"),
                ("pill-idle", Mode::Collapsed, None, Page::Music, false, &none, charging(76), "20:50", ""),
                ("robot", Mode::Collapsed, None, Page::Music, false, &none, on_battery, "20:50", ""),
                ("robot-happy", Mode::Collapsed, None, Page::Music, false, &none, charging(100), "20:50", ""),
                ("pill-music", Mode::Collapsed, Some(&playing), Page::Music, false, &none, charging(76), "20:49", ""),
                ("weather-pill", Mode::Collapsed, None, Page::Music, false, &partly, on_battery, "02:05", ""),
                ("weather-pill-music", Mode::Collapsed, Some(&playing), Page::Music, false, &rain, on_battery, "02:05", ""),
                ("page-music", Mode::Expanded, Some(&playing), Page::Music, false, &none, charging(70), "23:16", "T7, 24/10"),
                ("page-music-lyrics", Mode::Expanded, Some(&singing), Page::Music, true, &none, charging(70), "23:16", "T7, 24/10"),
                ("page-music-empty", Mode::Expanded, None, Page::Music, false, &none, charging(76), "20:55", "T2, 26/10"),
                ("page-health", Mode::Expanded, None, Page::Health, false, &none, charging(76), "20:49", "T2, 26/10"),
                ("page-focus", Mode::Expanded, None, Page::Focus, false, &none, charging(76), "20:49", "T2, 26/10"),
                ("pill-focus", Mode::Collapsed, None, Page::Music, false, &none, charging(76), "20:49", ""),
                ("page-weather", Mode::Expanded, None, Page::Weather, false, &partly_30, on_battery, "02:05", "T5, 29/10"),
                ("page-weather-sun", Mode::Expanded, None, Page::Weather, false, &sunny, on_battery, "02:05", "T5, 29/10"),
                ("page-weather-night", Mode::Expanded, None, Page::Weather, false, &night, on_battery, "02:05", "T5, 29/10"),
            ];
            let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(if english { "target/readme-en" } else { "target/readme" });
            std::fs::create_dir_all(&dir).unwrap();
            for (name, mode, music, page, lyrics, wx, battery, time, date) in shots {
                let date = en_date(date);
                let live = music.is_some();
                let mut size = target(mode, live);
                if mode == Mode::Expanded {
                    let [w, h] = panel_size(page);
                    (size[0], size[1]) = (w, h);
                }
                if lyrics {
                    size[0] += LYRICS_EXTRA_W;
                }
                // The mouse on the page dots: they open into a row of page icons (the panel grows by NAV_EXTRA), one under the mouse.
                let nav = name == "page-nav";
                set_nav_open(nav);
                if nav {
                    size[1] += NAV_EXTRA;
                }
                // The idle robot: plain eyes looking around below, and the cheer for a full battery.
                let eyes = |face, gaze: (f32, f32)| Some(Eyes { face, alpha: 1.0, gaze, curious: 0.0, open: 1.0, time: 0.4 });
                let robot = match name { "robot" => eyes(Face::Normal, (0.5, 0.9)), "robot-happy" => eyes(Face::Happy, (0.0, 0.0)), _ => None };
                let view = View { mode, size, time, date, battery, banner: None, music, live, robot, phase: 1.3, banner_age: 0.0, lyrics, hovered: nav.then_some(Button::GoPage(Page::Todo)), page,
                    leaving: None, todos: &[], notes: &[], weather: wx, page_age: 9.0, unit: weather::Unit::C, sun_view: matches!(name, "page-weather-sun" | "page-weather-night"),
                    list: ListView { counts: (0, 0), remind_before: 15, now: 202610290205, draft: "", editing: false, edit_item: None, caret: false, scroll: 0, note_pos: 0.0 },
                    focus: if name.ends_with("-focus") { &running_focus } else { &Focus::default() }, health: HealthView { h: &health, now: 1_000_000, minute_of_day: 840, settings: None } };
                // A fresh renderer per picture, run for a second and a half: the cover decodes, the
                // glow and the equaliser reach their resting strength.
                let mut renderer = Renderer::new().unwrap();
                renderer.grab = true;
                for _ in 0..90 {
                    renderer.present(hwnd, 0, 0, w, h, 1.0, &view).unwrap();
                    std::thread::sleep(std::time::Duration::from_millis(16));
                }
                let pixels = &renderer.grabbed[..];
                let mut bytes = Vec::new(); bytes.extend_from_slice(b"BM");
                bytes.extend_from_slice(&(54u32 + pixels.len() as u32).to_le_bytes()); bytes.extend_from_slice(&[0; 4]);
                bytes.extend_from_slice(&54u32.to_le_bytes()); bytes.extend_from_slice(&40u32.to_le_bytes());
                bytes.extend_from_slice(&w.to_le_bytes()); bytes.extend_from_slice(&(-h).to_le_bytes());
                bytes.extend_from_slice(&1u16.to_le_bytes()); bytes.extend_from_slice(&32u16.to_le_bytes());
                bytes.extend_from_slice(&[0; 24]); bytes.extend_from_slice(pixels);
                std::fs::write(dir.join(format!("{name}.bmp")), bytes).unwrap();
            }
            // The newer pictures: the floating pill, the Limits page and the approval banners.
            {
                use crate::usage::{Bar, Snapshot};
                let now = crate::usage::now_secs();
                let bar = |label: &str, weekly, used, hours: i64| Bar { label: label.into(), weekly, used, resets_at: now + hours * 3600 };
                let mut snap = Snapshot::default();
                snap.apps[0] = Some(vec![bar(tl("5 giờ", "5 h"), false, 37.0, 3), bar(tl("Tuần", "Week"), true, 8.0, 150)]);
                snap.apps[1] = Some(vec![bar(tl("30 ngày", "30 days"), true, 14.0, 718)]);
                snap.plans[1] = Some("free".into());
                snap.updated[1] = Some(now);
                snap.apps[2] = Some(vec![bar(tl("Gemini · Tuần", "Gemini · Week"), true, 0.4, 150), bar(tl("Gemini · 5 giờ", "Gemini · 5 h"), false, 0.0, 4), bar(tl("Claude · GPT · Tuần", "Claude · GPT · Week"), true, 36.0, 160),
                    bar(tl("Claude · GPT · 5 giờ", "Claude · GPT · 5 h"), false, 30.0, 4), bar(tl("Argon · Tuần", "Argon · Week"), true, 89.0, 150)]);
                let toast = |app: &str, title: &str, body: &str, choices: &[&str]| Toast { app: app.into(), title: title.into(), body: body.into(), level: None, icon: Some(ICON_WARN),
                    image: None, open: None, badge: None, choices: choices.iter().map(|c| c.to_string()).collect(), app_id: None };
                let claude_ask = toast("Claude", tl("Duyệt lệnh", "Approve command"), tl("Cho phép chạy công cụ get_architecture của codebase-memory-mcp cho project này (chỉ đọc)?", "Allow the get_architecture tool of codebase-memory-mcp to run for this project (read only)?"),
                    if english { &["Allow", "Deny", "In the app"] } else { &["Đồng ý", "Không đồng ý", "Trong app"] });
                // powershell's own icon (the shell's AppsFolder knows it by this id) stands in the banner.
                let notification = Toast { app: "powershell".into(), title: tl("Anh Minh", "Alex").into(), body: tl("Tối nay 8h họp nhóm nhé, nhớ mang laptop.", "Team call at 8 tonight, bring your laptop.").into(),
                    level: None, icon: None, image: None, open: None, badge: None, choices: Vec::new(),
                    app_id: Some(r"{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\WindowsPowerShell\v1.0\powershell.exe".into()) };
                // The reminders of the Health page: a status banner each (icon, title, a bar of today's water or a second line).
                let status = |app: &str, title: &str, body: &str, level: Option<f32>, icon: &'static str| Toast { app: app.into(), title: title.into(), body: body.into(), level,
                    icon: Some(icon), image: None, open: None, badge: None, choices: Vec::new(), app_id: None };
                let water = status(tl("Uống nước", "Water"), tl("Đến giờ uống 250 ml nước", "Time to drink 250 ml"), "", Some(0.375), ICON_DROP);
                let water_goal = status(tl("Uống nước", "Water"), tl("Đạt mục tiêu 2.0 L hôm nay", "Goal of 2.0 L reached today"), "", Some(1.0), ICON_DROP);
                let stand = status(tl("Đứng dậy đi lại", "Stand up and move"), tl("Bạn đã ngồi 45 phút liền", "You have been sitting for 45 min"),
                    tl("Vận động vài phút nhé", "Walk around for a few minutes"), None, ICON_WALK);
                let antigravity_ask = toast("Antigravity", tl("Cho phép mcp", "Allow mcp"), "codebase-memory-mcp/get_architecture", if english { &["Allow", "Deny"] } else { &["Đồng ý", "Không"] });
                // The Tasks and Notifications pages with made-up entries.
                let item = |text: &str, done: bool, at: Option<u64>| crate::todo::Item { text: text.into(), done: done.then_some(20261029), at, told_soon: false, told_now: false };
                let stamp = crate::todo::stamp;
                let tasks = vec![
                    item(tl("Nộp báo cáo tuần", "Send the weekly report"), false, Some(stamp(20261028, 9, 0))), item(tl("Uống thuốc", "Take vitamins"), false, Some(stamp(20261029, 2, 15))),
                    item(tl("Họp nhóm dự án", "Project sync"), false, Some(stamp(20261029, 21, 0))), item(tl("Mua sữa và trứng", "Buy milk and eggs"), false, None),
                    item(tl("Khám răng", "Dentist"), false, Some(stamp(20261030, 9, 0))), item(tl("Gọi cho khách hàng", "Call the client"), true, None),
                ];
                let note = |app: &str, title: &str, body: &str, at: u64| Note { id: format!("none.{app}"), app: app.into(), title: title.into(), body: body.into(), at, open: None };
                let notes = vec![note("ScreenSketch", "Screenshot copied to clipboard", "Automatically saved to screenshots folder.", 202610291557),
                    note("Zalo", tl("Anh Minh", "Alex"), tl("Tối nay 8h họp nhóm nhé", "Team call at 8 tonight"), 202610291402), note("DolbySettings", "Headphones (UGREEN HiTune Max5c)", "", 202610290207),
                    note("Outlook", tl("Lịch họp tuần", "Weekly planning"), tl("Phòng 3, 9:00 thứ Hai", "Room 3, Monday 9:00"), 202610281730)];
                // (file, style, mode, music, page, Limits tab, banner)
                let extra: [(&str, notch::Style, Mode, Option<&Track>, Page, usize, Option<&Toast>); 14] = [
                    ("banner", notch::Style::Ears, Mode::Alert, None, Page::Music, 0, Some(&notification)),
                    ("banner-water", notch::Style::Ears, Mode::Alert, None, Page::Music, 0, Some(&water)),
                    ("banner-water-goal", notch::Style::Ears, Mode::Alert, None, Page::Music, 0, Some(&water_goal)),
                    ("banner-standup", notch::Style::Ears, Mode::Alert, None, Page::Music, 0, Some(&stand)),
                    ("page-tasks", notch::Style::Ears, Mode::Expanded, None, Page::Todo, 0, None),
                    ("page-notifications", notch::Style::Ears, Mode::Expanded, None, Page::Notify, 0, None),
                    ("style-pill-idle", notch::Style::Pill, Mode::Collapsed, None, Page::Music, 0, None),
                    ("style-pill-music", notch::Style::Pill, Mode::Collapsed, Some(&playing), Page::Music, 0, None),
                    ("style-pill-page", notch::Style::Pill, Mode::Expanded, Some(&playing), Page::Music, 0, None),
                    ("page-usage-claude", notch::Style::Ears, Mode::Expanded, None, Page::Usage, 0, None),
                    ("page-usage-codex", notch::Style::Ears, Mode::Expanded, None, Page::Usage, 1, None),
                    ("page-usage-antigravity", notch::Style::Ears, Mode::Expanded, None, Page::Usage, 2, None),
                    ("banner-approve-claude", notch::Style::Pill, Mode::Alert, None, Page::Music, 0, Some(&claude_ask)),
                    ("banner-approve-antigravity", notch::Style::Pill, Mode::Alert, None, Page::Music, 0, Some(&antigravity_ask)),
                ];
                for (name, style, mode, music, page, tab, banner) in extra {
                    let live = music.is_some();
                    let size = match banner { Some(t) if t.choices.is_empty() => notch::alert_size(if t.icon.is_some() { 0 } else { 1 }), Some(t) => notch::alert_size_choice(if t.body.chars().count() > 60 { 2 } else { 1 }), None if mode == Mode::Expanded => { let [w, h] = panel_size(page); [w, h, target(mode, live)[2]] } None => target(mode, live) };
                    let view = View { mode, size, time: "20:50", date: en_date("T2, 26/10"), battery: charging(76), banner, music, live, robot: None, phase: 1.3, banner_age: 9.0, lyrics: false, hovered: None, page,
                        leaving: None, todos: if page == Page::Todo { &tasks } else { &[] }, notes: if page == Page::Notify { &notes } else { &[] }, weather: &none, page_age: 9.0, unit: weather::Unit::C, sun_view: false,
                        list: ListView { counts: if page == Page::Todo { (3, 1) } else { (0, 0) }, remind_before: 15, now: crate::todo::stamp(20261029, 2, 5), draft: "", editing: false, edit_item: None, caret: false, scroll: 0, note_pos: 0.0 },
                        focus: &Focus::default(), health: HealthView { h: &health, now: 1_000_000, minute_of_day: 840, settings: None } };
                    let mut renderer = Renderer::new().unwrap();
                    renderer.grab = true;
                    (renderer.style, renderer.usage, renderer.usage_tab) = (style, snap.clone(), tab);
                    for _ in 0..90 {
                        renderer.present(hwnd, 0, 0, w, h, 1.0, &view).unwrap();
                        std::thread::sleep(std::time::Duration::from_millis(16));
                    }
                    let pixels = &renderer.grabbed[..];
                    let mut bytes = Vec::new(); bytes.extend_from_slice(b"BM");
                    bytes.extend_from_slice(&(54u32 + pixels.len() as u32).to_le_bytes()); bytes.extend_from_slice(&[0; 4]);
                    bytes.extend_from_slice(&54u32.to_le_bytes()); bytes.extend_from_slice(&40u32.to_le_bytes());
                    bytes.extend_from_slice(&w.to_le_bytes()); bytes.extend_from_slice(&(-h).to_le_bytes());
                    bytes.extend_from_slice(&1u16.to_le_bytes()); bytes.extend_from_slice(&32u16.to_le_bytes());
                    bytes.extend_from_slice(&[0; 24]); bytes.extend_from_slice(pixels);
                    std::fs::write(dir.join(format!("{name}.bmp")), bytes).unwrap();
                }
            }
            DestroyWindow(hwnd).unwrap();
        }
    }

    #[test]
    #[ignore = "writes target/battery-preview.bmp: the resting pill at each battery state, stacked"]
    fn battery_preview() {
        unsafe {
            use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let (w, h) = (240, 40);
            let hwnd = CreateWindowExW(WS_EX_NOREDIRECTIONBITMAP, w!("STATIC"), w!("Battery preview"), WS_POPUP, 0, 0, w, h, None, None, None, None).unwrap();
            let states = [(79, true, false), (90, false, false), (50, false, false), (25, false, false), (15, false, false), (8, false, false), (40, false, true)];
            let mut renderer = Renderer::new().unwrap();
            renderer.grab = true;
            let mut sheet = Vec::new();
            for (percent, charging, saver) in states {
                let view = View { mode: Mode::Collapsed, size: target(Mode::Collapsed, false), time: "16:00", date: "", battery: Some(Battery { percent, charging, saver }),
                    banner: None, music: None, live: false, robot: None, phase: 0.0, banner_age: 0.0, lyrics: false, hovered: None, page: Page::Music,
                    leaving: None, todos: &[], notes: &[], weather: &weather::State::default(), page_age: 0.0, unit: weather::Unit::C, sun_view: false,
                    list: ListView { counts: (0, 0), remind_before: 15, now: 0, draft: "", editing: false, edit_item: None, caret: false, scroll: 0, note_pos: 0.0 },
                    focus: &Focus::default(), health: HealthView { h: &Health::default(), now: 0, minute_of_day: 0, settings: None } };
                renderer.present(hwnd, 0, 0, w, h, 1.0, &view).unwrap();
                sheet.extend_from_slice(&renderer.grabbed);
            }
            let sh = h * states.len() as i32;
            let mut bytes = Vec::new(); bytes.extend_from_slice(b"BM");
            bytes.extend_from_slice(&(54u32 + sheet.len() as u32).to_le_bytes()); bytes.extend_from_slice(&[0; 4]);
            bytes.extend_from_slice(&54u32.to_le_bytes()); bytes.extend_from_slice(&40u32.to_le_bytes());
            bytes.extend_from_slice(&w.to_le_bytes()); bytes.extend_from_slice(&(-sh).to_le_bytes());
            bytes.extend_from_slice(&1u16.to_le_bytes()); bytes.extend_from_slice(&32u16.to_le_bytes());
            bytes.extend_from_slice(&[0; 24]); bytes.extend_from_slice(&sheet);
            std::fs::write(std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/battery-preview.bmp"), bytes).unwrap();
            DestroyWindow(hwnd).unwrap();
        }
    }

    #[test]
    #[ignore = "writes target/notify-preview.bmp: the Notifications page with a full list"]
    fn notify_preview() {
        unsafe {
            use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let (w, h) = (530, 210);
            let hwnd = CreateWindowExW(WS_EX_NOREDIRECTIONBITMAP, w!("STATIC"), w!("Notify preview"), WS_POPUP, 0, 0, w, h, None, None, None, None).unwrap();
            let note = |app: &str, title: &str, body: &str, at: u64| Note { id: String::new(), app: app.into(), title: title.into(), body: body.into(), at, open: None };
            let notes = [note("ScreenSketch", "Screenshot copied to clipboard", "Automatically saved to screenshots folder.", 202610291557),
                note("Zalo", "Anh Minh", "Tối nay 8h họp nhóm nhé", 202610291402),
                note("DolbySettings", "Headphones (UGREEN HiTune Max5c)", "", 202610290207),
                note("Outlook", "Lịch họp tuần", "Phòng 3, 9:00 thứ Hai", 202610281730)];
            let view = View { mode: Mode::Expanded, size: target(Mode::Expanded, false), time: "16:00", date: "T5, 29/10",
                battery: Some(Battery { percent: 79, charging: true, saver: false }), banner: None, music: None, live: false, robot: None,
                phase: 0.0, banner_age: 0.0, lyrics: false, hovered: None, page: Page::Notify, leaving: None, todos: &[], notes: &notes,
                weather: &weather::State::default(), page_age: 0.0, unit: weather::Unit::C, sun_view: false,
                list: ListView { counts: (0, 0), remind_before: 15, now: 202610291600, draft: "", editing: false, edit_item: None, caret: false, scroll: 0, note_pos: 0.0 },
                focus: &Focus::default(), health: HealthView { h: &Health::default(), now: 0, minute_of_day: 0, settings: None } };
            let mut renderer = Renderer::new().unwrap();
            renderer.grab = true;
            renderer.present(hwnd, 0, 0, w, h, 1.0, &view).unwrap();
            let pixels = &renderer.grabbed[..];
            let mut bytes = Vec::new(); bytes.extend_from_slice(b"BM");
            bytes.extend_from_slice(&(54u32 + pixels.len() as u32).to_le_bytes()); bytes.extend_from_slice(&[0; 4]);
            bytes.extend_from_slice(&54u32.to_le_bytes()); bytes.extend_from_slice(&40u32.to_le_bytes());
            bytes.extend_from_slice(&w.to_le_bytes()); bytes.extend_from_slice(&(-h).to_le_bytes());
            bytes.extend_from_slice(&1u16.to_le_bytes()); bytes.extend_from_slice(&32u16.to_le_bytes());
            bytes.extend_from_slice(&[0; 24]); bytes.extend_from_slice(pixels);
            std::fs::write(std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/notify-preview.bmp"), bytes).unwrap();
            DestroyWindow(hwnd).unwrap();
        }
    }

    #[test]
    #[ignore = "writes a Direct2D preview for visual review"]
    fn media_preview() {
        unsafe {
            use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let hwnd = CreateWindowExW(WS_EX_NOREDIRECTIONBITMAP, w!("STATIC"), w!("Media preview"), WS_POPUP,
                0, 0, 530, 210, None, None, None, None).unwrap();
            let track = Track { title: "Bản nhạc đang phát".into(), artist: "YouTube / foobar2000".into(),
                duration_ms: 210_000, playing: true, can_toggle: true, can_next: true,
                can_previous: true, can_seek: true, ..Default::default() };
            let view = View { mode: Mode::Expanded, size: target(Mode::Expanded, false), time: "23:16",
                date: "T7, 24/10", battery: Some(Battery { percent: 70, charging: true, saver: false }),
                banner: None, music: Some(&track), live: true, robot: None, phase: 0.0, banner_age: 0.0, lyrics: false, hovered: Some(Button::PlayPause), page: Page::Music,
                leaving: None, todos: &[], notes: &[], weather: &weather::State::default(), page_age: 0.0, unit: weather::Unit::C, sun_view: false,
                list: ListView { counts: (0, 0), remind_before: 15, now: 0, draft: "", editing: false, edit_item: None, caret: false, scroll: 0, note_pos: 0.0 },
                focus: &Focus::default(), health: HealthView { h: &Health::default(), now: 0, minute_of_day: 0, settings: None } };
            let mut renderer = Renderer::new().unwrap();
            renderer.grab = true;
            renderer.present(hwnd, 0, 0, 530, 210, 1.0, &view).unwrap();
            let pixels = &renderer.grabbed[..];
            let mut bytes = Vec::new(); bytes.extend_from_slice(b"BM");
            bytes.extend_from_slice(&(54u32 + pixels.len() as u32).to_le_bytes()); bytes.extend_from_slice(&[0; 4]);
            bytes.extend_from_slice(&54u32.to_le_bytes()); bytes.extend_from_slice(&40u32.to_le_bytes());
            bytes.extend_from_slice(&530i32.to_le_bytes()); bytes.extend_from_slice(&(-210i32).to_le_bytes());
            bytes.extend_from_slice(&1u16.to_le_bytes()); bytes.extend_from_slice(&32u16.to_le_bytes());
            bytes.extend_from_slice(&[0; 24]); bytes.extend_from_slice(pixels);
            std::fs::write(std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/media-preview.bmp"), bytes).unwrap();
            DestroyWindow(hwnd).unwrap();
        }
    }

    #[test]
    #[ignore = "writes target/promo/notch-vi.mov and notch-en.mov through PUW_FFMPEG (the 30 s promo's notch layer); tools/promo.py composes the video"]
    fn promo_frames() {
        for english in [false, true] {
            promo_layer(english);
        }
        crate::i18n::set_english(false);
    }

    /// The promo's notch, 30 s at 30 fps and 2x, driven by the app's own state machine and painter
    /// in real time (the painter's own motion runs on the clock): the pill grows, music starts, the
    /// mouse opens the panel and clicks through the five pages, a notification banner drops in, then
    /// the idle robot. Premultiplied BGRA, 960 x 420, piped into ffmpeg as PNG frames in a .mov.
    fn promo_layer(english: bool) {
        use crate::notch::Notch;
        use crate::robot::{Mood, Robot};
        use crate::todo::stamp;
        use std::io::Write;
        use weather::{Now, Place, Sky, State};
        crate::i18n::set_english(english);
        let t = |vi: &str, en: &str| if english { en.to_string() } else { vi.to_string() };
        let (w, h, scale, fps, frames) = (960, 420, 2.0, 30.0, 900);
        let out = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/promo");
        std::fs::create_dir_all(&out).unwrap();
        let ffmpeg = std::env::var("PUW_FFMPEG").unwrap_or_else(|_| "ffmpeg".into());
        let mut enc = std::process::Command::new(ffmpeg)
            .args(["-y", "-loglevel", "error", "-f", "rawvideo", "-pix_fmt", "bgra", "-s", &format!("{w}x{h}"), "-r", "30", "-i", "-", "-c:v", "png"])
            .arg(out.join(if english { "notch-en.mov" } else { "notch-vi.mov" }))
            .stdin(std::process::Stdio::piped()).spawn().expect("ffmpeg (set PUW_FFMPEG)");
        let pipe = enc.stdin.as_mut().unwrap();

        let art = std::env::var("PUW_PREVIEW_COVER").ok().and_then(|p| std::fs::read(p).ok()).map(Arc::new);
        let lines: Vec<(u64, String)> = vec![(0, t("Lời chạy theo bài đang phát", "Lyrics follow the song")), (90_000, t("Ngay trên notch của bạn", "Right in your notch"))];
        let song = |ms| Track { title: t("Bài hát mẫu", "Sample song"), artist: t("Nghệ sĩ mẫu", "Sample artist"), duration_ms: 210_000, position_ms: ms, playing: true,
            can_toggle: true, can_next: true, can_previous: true, can_seek: true, art: art.clone(), art_key: 1, lyrics: Arc::new(lines.clone()), ..Default::default() };
        let item = |text: String, at: Option<u64>| Item { text, done: None, at, told_soon: false, told_now: false };
        let todos = vec![item(t("Gửi báo cáo tuần", "Send the weekly report"), Some(stamp(20261029, 9, 0))), item(t("Uống thuốc", "Take vitamins"), Some(stamp(20261029, 10, 15))),
            item(t("Họp nhóm dự án", "Project sync"), Some(stamp(20261029, 20, 0))), item(t("Khám răng", "Dentist"), Some(stamp(20261030, 9, 0)))];
        // Ids Windows has no icon for: each row gets its app's coloured initial, the same on every PC.
        let note = |app: &str, title: String, body: String, at: u64| Note { id: format!("promo.{app}"), app: app.into(), title, body, at, open: None };
        let notes = [note("Zalo", t("Anh Minh", "Alex"), t("Tối nay 8h họp nhóm nhé", "Team call at 8 tonight"), 202610290938),
            note("Outlook", t("Lịch họp tuần", "Weekly planning"), t("Phòng 3, 9:00 thứ Hai", "Room 3, Monday 9:00"), 202610290910),
            note("Spotify", t("Danh sách mới cho bạn", "New playlist for you"), t("Nhạc tập trung buổi sáng", "Morning focus"), 202610290805)];
        let toast = Toast { app: "Zalo".into(), title: t("Anh Minh", "Alex"), body: t("Tối nay 8h họp nhóm nhé, nhớ mang laptop.", "Team call at 8 tonight, bring your laptop."),
            level: None, icon: None, image: None, open: None, badge: None, choices: Vec::new(), app_id: None };
        let wx = State { place: Some(Place { name: t("Đà Nẵng", "Da Nang"), lat: 16.0, lon: 108.0 }),
            now: Some(Now { temp: 29.0, feels: 32.0, humidity: 78.0, wind: 9.0, sky: Sky::PartlyCloudy, day: true }), updated: Some(202610290930), ..State::default() };
        let health = Health::default();
        let date = t("T5, 29/10", "Thu, 29/10");

        unsafe {
            use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let hwnd = CreateWindowExW(WS_EX_NOREDIRECTIONBITMAP, w!("STATIC"), w!("Promo"), WS_POPUP, 0, 0, w, h, None, None, None, None).unwrap();
            let mut renderer = Renderer::new().unwrap();
            renderer.grab = true;
            let mut notch = Notch::new(false);
            let mut robot = Robot::default();
            let (mut page, mut page_since, mut page_anim) = (Page::Music, 0.0f32, None::<(Page, f32)>);
            let (mut live, mut charging) = (false, false);
            let clicks = [10.5, 12.5, 14.5, 16.5];
            let start = Instant::now();
            for i in 0..frames {
                let (now, before) = (i as f32 / fps, (i as f32 - 1.0) / fps);
                let at = |s: f32| before < s && now >= s;
                std::thread::sleep((start + std::time::Duration::from_secs_f32(now)).saturating_duration_since(Instant::now()));
                if at(4.0) {
                    live = true;
                    notch.set_live(true);
                }
                if at(7.0) {
                    notch.on_enter();
                }
                if at(7.4) {
                    notch.on_dwell(true);
                    page_since = now;
                }
                if let Some(k) = clicks.iter().position(|&c| at(c)) {
                    page_anim = Some((page, now));
                    page = PAGES[k + 1];
                    page_since = now;
                }
                if at(18.6) {
                    notch.on_leave();
                }
                if at(19.6) {
                    let lines = renderer.banner_lines(&toast.body, scale, false, false).min(notch::BANNER_MAX_LINES) as u8;
                    notch.on_alert(lines);
                }
                if at(23.2) {
                    notch.on_alert_end();
                }
                if at(23.8) {
                    (live, charging) = (false, true);
                    notch.set_live(false);
                    robot.update(40, true);
                }
                notch.step(1.0 / fps);
                let leaving = page_anim.map(|(from, t0)| (from, ((now - t0) / 0.32).min(1.0), 1));
                if leaving.is_some_and(|(_, k, _)| k >= 1.0) {
                    page_anim = None;
                }
                let eyes = robot.visible().then(|| robot.step(1.0 / fps, None, Mood::Normal));
                let track = song(64_000 + ((now - 4.0).max(0.0) * 1000.0) as u64);
                let mode = notch.mode;
                let view = View { mode, size: notch.size(), time: "09:41", date: &date, battery: Some(Battery { percent: 82, charging, saver: false }),
                    banner: (mode == Mode::Alert).then_some(&toast), music: live.then_some(&track), live: notch.live, robot: eyes, phase: now,
                    banner_age: (now - 19.6).max(0.0), lyrics: false,
                    hovered: (mode == Mode::Expanded && (10.0..18.2).contains(&now)).then_some(Button::PageNext), page, leaving,
                    list: ListView { counts: (3, 1), remind_before: 15, now: 202610290941, draft: "", editing: false, edit_item: None, caret: false, scroll: 0, note_pos: 0.0 },
                    todos: &todos, notes: &notes, focus: &Focus::default(), health: HealthView { h: &health, now: crate::todo::minutes(202610290941), minute_of_day: 9 * 60 + 41, settings: None },
                    weather: &wx, page_age: if mode == Mode::Expanded { now - page_since } else { 0.0 }, unit: weather::Unit::C, sun_view: false };
                renderer.present(hwnd, 0, 0, w, h, scale, &view).unwrap();
                pipe.write_all(&renderer.grabbed).unwrap();
            }
            DestroyWindow(hwnd).unwrap();
        }
        drop(enc.stdin.take());
        assert!(enc.wait().unwrap().success());
    }
}

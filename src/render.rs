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
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::time::Instant;

use crate::connectivity::DeviceKind;
use crate::icons::{self, Seg};
use crate::lyrics;
use crate::music::Track;
use crate::notch::{self, target, Mode, LYRICS_EXTRA_W};
use crate::notify::{Note, Toast};
use crate::system::Battery;

pub const WM_ICONS: u32 = WM_APP + 10;

pub fn visible_page(mode: Mode, hidden: bool, page: Page) -> Option<Page> {
    (mode == Mode::Expanded && !hidden).then_some(page)
}
use crate::health::{self, Health, Setting};
use crate::i18n::tr;
use crate::robot::{curious_eyes, Eyes, Face};
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
    /// The weather's city and current conditions (shown on the pill and the Weather page).
    pub weather: &'a weather::State,
    /// Seconds since the open panel or its page appeared: the Weather page's icon moves for a moment.
    pub page_age: f32,
    /// °C or °F, for every temperature shown.
    pub unit: weather::Unit,
}

/// Pages of the open panel, switched with the side arrows.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Page {
    Music,
    /// Tasks and appointments in one list.
    Todo,
    /// Windows notifications, to read back.
    Notify,
    Health,
    /// The city (typed here) and its current weather.
    Weather,
}

pub const PAGES: [Page; 5] = [Page::Music, Page::Todo, Page::Notify, Page::Health, Page::Weather];

impl Page {
    /// Header of the page: icon, name, accent colour.
    fn title(self) -> (&'static str, &'static str, (f32, f32, f32)) {
        match self {
            Page::Music => ("\u{E8D6}", tr("Nhạc", "Music"), (0.72, 0.52, 1.0)),
            Page::Todo => ("\u{E9D5}", tr("Việc", "Tasks"), GREEN),
            Page::Notify => (ICON_BELL, tr("Thông báo", "Notifications"), BLUE),
            Page::Health => ("\u{E95E}", tr("Sức khỏe", "Health"), (1.0, 0.42, 0.5)),
            Page::Weather => ("", tr("Thời tiết", "Weather"), (1.0, 0.78, 0.25)),
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
    Seek,
    /// Show / hide the lyrics beside the player.
    Lyrics,
    PagePrev,
    PageNext,
    ListInput,
    /// Tick / untick task at this index (the whole row, text included). To-do only.
    ListToggle(usize),
    ListEdit(usize),
    ListDelete(usize),
    /// Open the notification at this index (its app, link or picture).
    NoteOpen(usize),
    /// Show the temperature in this unit (Weather page).
    Unit(weather::Unit),
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
}

impl Button {
    fn label(self) -> &'static str {
        match self {
            Self::Previous => tr("Bài trước", "Previous"), Self::PlayPause => tr("Phát / Tạm dừng", "Play / Pause"),
            Self::Next => tr("Bài tiếp theo", "Next"), Self::Seek => tr("Bấm để tua", "Click to seek"),
            Self::Lyrics => tr("Lời bài hát", "Lyrics"),
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
            _ => true,
        }
    }
}

const PAD: f32 = 24.0;
/// Icon glyph change animation length.
const ICON_MORPH_S: f32 = 0.25;
/// Music page: cover on the left, a column of text, bar and controls on its right.
const COVER: f32 = 120.0;
const COVER_TOP: f32 = 40.0;
const COVER_GAP: f32 = 16.0;
const PLAY_BTN: f32 = 38.0;
const SKIP_BTN: f32 = 32.0;
const LYRICS_BTN: f32 = 28.0;
const CONTROLS_Y: f32 = 148.0;
const BAR_Y: f32 = 108.0;
/// Page content sits between the side arrows (logical px from the panel edge).
const SIDE: f32 = 44.0;
const ARROW: f32 = 24.0;
/// Pages are clipped this far in from the panel's sides (just inside the arrows).
const PAGE_CLIP: f32 = 34.0;
const ARROW_Y: f32 = 80.0;
const DOTS_Y: f32 = 174.0;
/// Task list page.
const INPUT_Y: f32 = 36.0;
const INPUT_H: f32 = 26.0;
const COUNT_Y: f32 = 66.0;
const ROW_Y: f32 = 84.0;
/// A task row is also its tick, edit and delete buttons: 26 px high, above macOS' 20 px minimum.
const ROW_H: f32 = 26.0;
pub const LIST_ROWS: usize = 3;
/// Weather page: the two unit chips (°C, °F) after the temperature: left edge from the content's
/// left, top, width of each and height.
const UNIT_X: f32 = 134.0;
const UNIT_Y: f32 = 97.0;
const UNIT_W: f32 = 32.0;
const UNIT_H: f32 = 22.0;
/// Width of the text at the right of the city line ("Open-Meteo · 14:30"), and so where the
/// button that switches the weather off sits: just before it.
const CREDIT_W: f32 = 126.0;
/// Notifications page: rows of two lines.
const NOTE_Y: f32 = 36.0;
const NOTE_H: f32 = 38.0;
pub const NOTE_ROWS: usize = 3;
/// "Clear all" under the rows, right-aligned with their times: a pill hugging its label (the click
/// area is this wide), inside the pages' clip, which ends at DOTS_Y - 3.
const CLEAR_W: f32 = 64.0;
const CLEAR_H: f32 = 18.0;
const CLEAR_Y: f32 = NOTE_Y + NOTE_H * NOTE_ROWS as f32 + 1.0;
/// Room between the label and the pill's edge on each side.
const CLEAR_PAD: f32 = 9.0;
/// Health page cards.
const CARD_TOP: f32 = 34.0;
const CARD_BOTTOM: f32 = 158.0;
const CARD_W: f32 = 172.0;
const CARD_GAP: f32 = 8.0;
const SET_ROW: f32 = 18.0;
const SET_Y: f32 = 62.0;

/// Width of the settled open panel; `wide` = the lyrics show beside the player.
pub fn panel_w(wide: bool) -> f32 {
    target(Mode::Expanded, false)[0] + if wide { LYRICS_EXTRA_W } else { 0.0 }
}

/// Media controls of the music page, in window px for the settled panel centred at `cx`. The
/// player is the left 440 px of the panel; with the lyrics open (`wide`) the panel is wider and
/// the lyrics take the part on the right.
pub fn buttons(cx: f32, scale: f32, wide: bool) -> [(Button, D2D_RECT_F); 5] {
    buttons_from(cx / scale - panel_w(wide) / 2.0, scale)
}

/// The same controls for a panel whose left edge is at `left` (logical px): follows the panel
/// while it grows or shrinks.
fn buttons_from(left: f32, scale: f32) -> [(Button, D2D_RECT_F); 5] {
    let (x0, x1) = (SIDE + COVER + COVER_GAP, target(Mode::Expanded, false)[0] - SIDE);
    let mid = (x0 + x1) / 2.0;
    let round = |x: f32, size: f32| rect((left + x - size / 2.0) * scale, (CONTROLS_Y - size / 2.0) * scale, (left + x + size / 2.0) * scale, (CONTROLS_Y + size / 2.0) * scale);
    let seek = rect((left + x0) * scale, (BAR_Y - 8.0) * scale, (left + x1) * scale, (BAR_Y + 8.0) * scale);
    [
        (Button::Previous, round(mid - 58.0, SKIP_BTN)),
        (Button::PlayPause, round(mid, PLAY_BTN)),
        (Button::Next, round(mid + 58.0, SKIP_BTN)),
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
    // Only the Music page can be wide.
    let wide = wide && page == Page::Music;
    let w = panel_w(wide);
    let left = cx / scale - w / 2.0;
    let r = |x: f32, y: f32, bw: f32, bh: f32| rect((left + x) * scale, y * scale, (left + x + bw) * scale, (y + bh) * scale);
    let mut out = Vec::new();
    // The arrows are hidden while the lyrics are open.
    if !wide {
        out.push((Button::PagePrev, r(10.0, ARROW_Y, ARROW, ARROW)));
        out.push((Button::PageNext, r(w - 10.0 - ARROW, ARROW_Y, ARROW, ARROW)));
    }
    let (cl, cw) = (SIDE, w - 2.0 * SIDE);
    match page {
        Page::Music => out.extend(buttons(cx, scale, wide)),
        Page::Todo => {
            out.push((Button::ListInput, r(cl, INPUT_Y, cw, INPUT_H)));
            out.push((Button::ListClear, r(cl + cw - 26.0, INPUT_Y, 26.0, INPUT_H)));
            out.push((Button::Gear(Card::Tasks), r(cl + cw - 20.0, COUNT_Y - 2.0, 20.0, 18.0)));
            if settings == Some(Card::Tasks) {
                out.push((Button::RemindStep(-1), r(cl + cw - 112.0, ROW_Y, 22.0, 22.0)));
                out.push((Button::RemindStep(1), r(cl + cw - 24.0, ROW_Y, 22.0, 22.0)));
            } else {
                for (row, i) in (scroll..rows.min(scroll + LIST_ROWS)).enumerate() {
                    let y = ROW_Y + row as f32 * ROW_H;
                    out.push((Button::ListToggle(i), r(cl, y, cw - 56.0, ROW_H)));
                    out.push((Button::ListEdit(i), r(cl + cw - 54.0, y, 26.0, ROW_H)));
                    out.push((Button::ListDelete(i), r(cl + cw - 26.0, y, 26.0, ROW_H)));
                }
            }
        }
        Page::Weather => {
            out.push((Button::ListInput, r(cl, INPUT_Y, cw, INPUT_H)));
            out.push((Button::ListClear, r(cl + cw - 26.0, INPUT_Y, 26.0, INPUT_H)));
            // The x that switches the weather off, and the °C | °F choice beside the temperature,
            // once there is one (`rows` = 1).
            if rows > 0 {
                out.push((Button::CityOff, r(cl + cw - CREDIT_W - 28.0, 66.0, 24.0, 24.0)));
                for (k, unit) in [weather::Unit::C, weather::Unit::F].into_iter().enumerate() {
                    out.push((Button::Unit(unit), r(cl + UNIT_X + k as f32 * UNIT_W, UNIT_Y, UNIT_W, UNIT_H)));
                }
            }
        }
        Page::Notify => {
            for (row, i) in (scroll..rows.min(scroll + NOTE_ROWS)).enumerate() {
                out.push((Button::NoteOpen(i), r(cl, NOTE_Y + row as f32 * NOTE_H, cw, NOTE_H - 4.0)));
            }
            if rows > 0 {
                out.push((Button::NotesClear, r(cl + cw - CLEAR_W, CLEAR_Y, CLEAR_W, CLEAR_H)));
            }
        }
        Page::Health => {
            for (card, x) in [(Card::Water, cl), (Card::Sit, cl + CARD_W + CARD_GAP)] {
                out.push((Button::Gear(card), r(x + CARD_W - 28.0, CARD_TOP + 4.0, 22.0, 22.0)));
                if settings == Some(card) {
                    for (k, &(set, switch)) in setting_rows(card).iter().enumerate() {
                        let y = SET_Y + k as f32 * SET_ROW;
                        if switch {
                            out.push((Button::Toggle(set), r(x + CARD_W - 46.0, y + 1.0, 34.0, 16.0)));
                        } else {
                            out.push((Button::Step(set, -1), r(x + CARD_W - 90.0, y, 18.0, 18.0)));
                            out.push((Button::Step(set, 1), r(x + CARD_W - 30.0, y, 18.0, 18.0)));
                        }
                    }
                } else if card == Card::Water {
                    out.push((Button::Drink, r(x + 12.0, CARD_BOTTOM - 32.0, CARD_W - 84.0, 24.0)));
                    out.push((Button::Snooze, r(x + CARD_W - 68.0, CARD_BOTTOM - 32.0, 56.0, 24.0)));
                } else {
                    out.push((Button::Stood, r(x + 12.0, CARD_BOTTOM - 32.0, CARD_W - 24.0, 24.0)));
                }
            }
        }
    }
    out
}

pub fn seek_at(cx: f32, scale: f32, x: f32, duration: u64, wide: bool) -> u64 {
    let (_, r) = buttons(cx, scale, wide)[3];
    (((x - r.left) / (r.right - r.left)).clamp(0.0, 1.0) as f64 * duration as f64) as u64
}

pub fn hit_button(cx: f32, scale: f32, x: f32, y: f32, track: Option<&Track>, page: Page, rows: usize, scroll: usize, settings: Option<Card>, wide: bool) -> Option<Button> {
    page_buttons(cx, scale, page, rows, scroll, settings, wide).into_iter().find(|(b, r)| b.enabled(track)
        && x >= r.left && x <= r.right && y >= r.top && y <= r.bottom).map(|(b, _)| b)
}

/// Opacity of the page leaving and the page arriving at progress `t` (0..1) of a page change.
/// The old page is gone before the new one shows, so their text never overlaps (ghosting).
fn page_fades(t: f32) -> (f32, f32) {
    ((1.0 - t * 3.5).clamp(0.0, 1.0), ((t - 0.2) * 2.5).clamp(0.0, 1.0))
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

/// Overshoot easing for the page morph: arrives a touch past and settles back.
fn ease_out_back(t: f32) -> f32 {
    let (c1, t) = (1.3, t.clamp(0.0, 1.0) - 1.0);
    1.0 + (c1 + 1.0) * t * t * t + c1 * t * t
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
/// Lyrics area beside the player, logical px: its top and bottom in the panel, the space each side
/// of the thin divider after the player's controls, its right margin and its width. The margin
/// stays wider than the pages' clip (PAGE_CLIP), with room for a last letter's ink past the box.
const LYRIC_TOP: f32 = 30.0;
const LYRIC_BOTTOM: f32 = 168.0;
const LYRIC_GAP: f32 = 14.0;
const LYRIC_RIGHT: f32 = PAGE_CLIP + 6.0;
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
const ICON_PREV: &str = "\u{E892}";
const ICON_PLAY: &str = "\u{E768}";
const ICON_PAUSE: &str = "\u{E769}";
const ICON_NEXT: &str = "\u{E893}";
const ICON_NOTE: &str = "\u{E8D6}";
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
/// Idle robot's LED eyes: near-white, like a small LED face.
const LED: (f32, f32, f32) = (0.93, 0.97, 1.0);

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

/// Brightest the aurora's colours may get (before the line's own transparency): it runs
/// under the controls at the bottom of the panel.
const AURORA_MAX_LUM: f32 = 0.5;

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

/// Seconds for the glow to fade in or out and to move to a new cover's colour.
const GLOW_FADE_S: f32 = 0.3;

/// One animation step of the Music-page glow towards `target` (None: fade out, keeping the last
/// colour). Returns the new (colour, level) and whether it still has somewhere to go.
fn glow_step(color: (f32, f32, f32), level: f32, target: Option<(f32, f32, f32)>, dt: f32) -> ((f32, f32, f32), f32, bool) {
    let k = 1.0 - (-3.0 * dt / GLOW_FADE_S).exp();
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

/// Points along the aurora band.
const AURORA_STEPS: usize = 48;
/// Aurora layers, widest first: (thickness scale, opacity): a soft halo and a brighter core.
const AURORA_LAYERS: [(f32, f32); 8] = [(2.4, 0.12), (2.0, 0.14), (1.65, 0.16), (1.3, 0.19), (1.0, 0.22), (0.72, 0.26), (0.48, 0.3), (0.26, 0.36)];
/// The aurora is drawn this many times smaller, then stretched (the stretch blurs it).
const AURORA_SHRINK: f32 = 8.0;
/// Where the aurora runs, as fractions of the panel height: centre line, sway, halo thickness.
const AURORA_Y: f32 = 0.8;
const AURORA_AMP: f32 = 0.07;
const AURORA_HALO: f32 = 0.09;
/// The sharp core over the blurred halo, drawn at full size: (stroke width in px, opacity),
/// widest first, in the colours turned towards white by AURORA_CORE_WHITE.
const AURORA_CORE: [(f32, f32); 4] = [(9.0, 0.10), (5.0, 0.16), (2.6, 0.30), (1.2, 0.55)];
const AURORA_CORE_WHITE: f32 = 0.35;

/// The aurora at time `t` (s) and `u` across the panel (0..1): centre line offset (-1..1) and
/// thickness (0.1..1). Slow sums of sines, so it sways and breathes without repeating soon.
fn aurora_at(t: f32, u: f32) -> (f32, f32) {
    use std::f32::consts::TAU;
    let c = 0.6 * (TAU * 0.7 * u + t * 0.45).sin() + 0.3 * (TAU * 1.3 * u - t * 0.3 + 1.1).sin() + 0.1 * (TAU * 2.1 * u + t * 0.6).sin();
    let w = 0.55 + 0.45 * (TAU * 1.1 * u - t * 0.25 + 0.5).sin();
    (c, w)
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

/// The aurora's two colours: the glow's hue, and the cover's next strongest hue at least 60
/// degrees away. A cover with one colour gets that colour turned 45 degrees for the second.
fn aurora_tints(pixels: &[u8]) -> [(f32, f32, f32); 2] {
    let first = glow_tint(pixels);
    if first == GLOW_NEUTRAL {
        return [GLOW_NEUTRAL, (0.55, 0.6, 0.75)];
    }
    let bins = hue_bins(pixels);
    let top = (0..12).max_by(|&a, &b| bins[a].0.total_cmp(&bins[b].0)).unwrap_or(0);
    let far = |i: usize| (i as i32 - top as i32).rem_euclid(12).min((top as i32 - i as i32).rem_euclid(12)) >= 2;
    let second = (0..12).filter(|&i| far(i)).max_by(|&a, &b| bins[a].0.total_cmp(&bins[b].0)).filter(|&i| bins[i].0 > bins[top].0 * 0.08);
    match second {
        Some(i) => [first, full_bright(bins[i].1.map(|c| c / bins[i].0))],
        None => [first, rotate_hue(first, 45.0)],
    }
}

/// `c` with its hue turned by `deg` degrees (same brightness and saturation).
fn rotate_hue((r, g, b): (f32, f32, f32), deg: f32) -> (f32, f32, f32) {
    let (cos, sin) = (deg.to_radians().cos(), deg.to_radians().sin());
    let k = (1.0 - cos) / 3.0;
    let s3 = (1.0f32 / 3.0).sqrt() * sin;
    let m = |x: f32, y: f32, z: f32| (x * r + y * g + z * b).clamp(0.0, 1.0);
    (m(cos + k, k - s3, k + s3), m(k + s3, cos + k, k - s3), m(k - s3, k + s3, cos + k))
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
        std::thread::spawn(move || unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let wic: Option<IWICImagingFactory> = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok();
            for id in asked {
                let deferred = !id.starts_with("img:") && !open.load(Ordering::Relaxed);
                let pixels = wic.as_ref().filter(|_| !deferred).and_then(|wic| match id.strip_prefix("img:") {
                    Some(path) => image_pixels(wic, path),
                    None => app_icon_pixels(wic, &id),
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

pub struct Renderer {
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
    /// Copy each frame back to `grabbed` (preview tests only).
    #[cfg(test)]
    grab: bool,
    #[cfg(test)]
    grabbed: Vec<u8>,
    fonts: Option<(f32, Vec<IDWriteTextFormat>)>,
    /// Decoded cover of the current track (keyed by Track::art_key).
    cover: Option<CoverCache>,
    /// The cover's blurred shadow, made once per scale: (scale, bitmap).
    cover_shadow: Option<(f32, ID2D1Bitmap)>,
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
    /// Round caps and joins for the robot face's strokes.
    round: ID2D1StrokeStyle,
}

/// Colours of the battery icon, (bolt or first path, the rest): a green bolt on a white case while
/// charging, all yellow in battery saver (like iPhone's low power mode), all red at 20 % or less.
fn battery_tint(b: &Battery) -> ((f32, f32, f32), (f32, f32, f32)) {
    let white = (1.0, 1.0, 1.0);
    if b.charging {
        ((0.2, 0.84, 0.29), white)
    } else if b.saver {
        ((1.0, 0.8, 0.0), (1.0, 0.8, 0.0))
    } else if b.percent <= 20 {
        ((1.0, 0.27, 0.23), (1.0, 0.27, 0.23))
    } else {
        (white, white)
    }
}

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
            Ok(Renderer { factory, d3d, ctx, rt, dw, wic, surface: None, w: 0, h: 0, placed: (0, 0, 0, 0), region: (0, 0, 0, 0),
                #[cfg(test)] grab: false, #[cfg(test)] grabbed: Vec::new(),
                fonts: None, cover: None, cover_shadow: None, glow: (GLOW_NEUTRAL, 0.0, Instant::now()), bars: 0.0, slow_frames: false, line_geoms: HashMap::new(), lyric_heights: None, lyric_motion: lyrics::Motion::default(), app_icons: HashMap::new(), icon_loader: IconLoader::start(), morphs: RefCell::new(HashMap::new()), round })
        }
    }

    /// The GPU was reset or removed (driver update, sleep): start over on a new device. Every bitmap
    /// belonged to the old one, so they are made again as they are needed.
    fn lose_device(&mut self) -> Result<()> {
        (self.d3d, self.ctx, self.rt) = gpu_device(&self.factory)?;
        self.surface = None;
        self.cover = None;
        self.cover_shadow = None;
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
                Some(((fit(big)?, fit(small)?), glow_tint(&px), aurora_tints(&px)))
            });
            let (bitmaps, glow, aurora) = match decoded { Some((b, g, au)) => (Some(b), Some(g), au), None => (None, None, [GLOW_NEUTRAL; 2]) };
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

    /// Idle robot's LED face centred on (cx, cy): eyes 14 px either side, the mouth below, the
    /// whole face shifted along `e.gaze`. Strokes have round ends like the glowing lines of an LED
    /// face; asleep a pixel "z" drifts up beside it.
    fn draw_face(&self, e: &Eyes, cx: f32, cy: f32, s: f32, a: f32) -> Result<()> {
        let rt = &self.rt;
        let led = |alpha: f32| unsafe { rt.CreateSolidColorBrush(&color(LED.0, LED.1, LED.2, alpha), None) };
        let lit = led(a)?;
        let glow = led(a * 0.12)?;
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
            unsafe { rt.DrawGeometry(&path(pts, false)?, &lit, width * s, &self.round) };
            Ok(())
        };
        let block = |x: f32, y: f32, w: f32, h: f32| unsafe {
            // A glowing square eye: a faint larger block behind the bright one.
            let g = 2.0 * s;
            rt.FillRoundedRectangle(&rounded(rect(x - w / 2.0 - g, y - h / 2.0 - g, x + w / 2.0 + g, y + h / 2.0 + g), 3.0 * s), &glow);
            rt.FillRoundedRectangle(&rounded(rect(x - w / 2.0, y - h / 2.0, x + w / 2.0, y + h / 2.0), 1.5 * s), &lit);
        };
        let (fx, fy) = (cx + e.gaze.0 * 3.0 * s, cy + e.gaze.1 * 2.4 * s);
        let (ey, my) = (fy - 3.0 * s, fy + 6.5 * s);
        // An open eye `h` logical px tall, squeezed by a blink (never below a thin bar).
        let lid = |h: f32| (h * e.open).max(1.6) * s;
        let u = s;
        for side in [-1.0f32, 1.0] {
            let x = fx + side * 14.0 * s;
            match e.face {
                Face::Normal | Face::Greet => block(x, ey, 7.0 * s, lid(9.0)),
                Face::Curious => {
                    let (left, right) = curious_eyes(e.gaze.0);
                    let h = if side < 0.0 { left } else { right };
                    // Bottoms line up, so the big eye grows upward.
                    block(x, ey + (4.5 - h / 2.0) * s, 7.0 * s, lid(h))
                }
                Face::Wink if side < 0.0 => block(x, ey, 7.0 * s, lid(9.0)),
                Face::Happy | Face::Wink => stroke(&[p(x - 4.0 * u, ey + 3.0 * u), p(x, ey - 2.0 * u), p(x + 4.0 * u, ey + 3.0 * u)], 2.0)?,
                Face::Excited => {
                    let d = -side; // '>' on the left, '<' on the right
                    stroke(&[p(x - 3.0 * d * u, ey - 3.5 * u), p(x + 3.0 * d * u, ey + 0.5 * u), p(x - 3.0 * d * u, ey + 4.5 * u)], 2.0)?
                }
                Face::Sad => {
                    let d = -side; // inner corner lower: droopy eyes
                    // Blinking squeezes the eye toward its middle (ey + 1).
                    let v = |dy: f32| ey + 1.0 * u + (dy - 1.0) * u * e.open.max(0.18);
                    let pts = [p(x - 3.5 * u, v(-2.0 - d * 1.5)), p(x + 3.5 * u, v(-2.0 + d * 1.5)), p(x + 3.5 * u, v(4.0)), p(x - 3.5 * u, v(4.0))];
                    unsafe { rt.FillGeometry(&path(&pts, true)?, &lit, None) };
                }
                Face::Sleep => stroke(&quad(p(x - 4.0 * u, ey + 1.0 * u), p(x, ey + 5.0 * u), p(x + 4.0 * u, ey + 1.0 * u)), 2.0)?,
            }
        }
        let m = |dx: f32, dy: f32| p(fx + dx * u, my + dy * u);
        match e.face {
            Face::Normal | Face::Wink => stroke(&quad(m(-3.0, -1.0), m(0.0, 2.0), m(3.0, -1.0)), 1.6)?,
            Face::Happy => stroke(&quad(m(-4.0, -2.0), m(0.0, 3.0), m(4.0, -2.0)), 1.6)?,
            Face::Excited => {
                let mut pts = quad(m(-4.0, -1.0), m(-2.0, 2.0), m(0.0, -1.0));
                pts.extend(quad(m(0.0, -1.0), m(2.0, 2.0), m(4.0, -1.0)).into_iter().skip(1));
                stroke(&pts, 1.6)?
            }
            Face::Greet => {
                // Big open grin: a flat top and a deep curve under it, filled ("D" on its side).
                let mut pts = vec![m(-5.5, -2.0)];
                pts.extend(quad(m(5.5, -2.0), m(0.0, 9.0), m(-5.5, -2.0)));
                unsafe { rt.FillGeometry(&path(&pts, true)?, &lit, None) };
            }
            Face::Curious | Face::Sleep => stroke(&[m(-2.5, 0.0), m(2.5, 0.0)], 1.6)?,
            Face::Sad => stroke(&quad(m(-3.0, 1.0), m(0.0, -2.0), m(3.0, 1.0)), 1.6)?,
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

    /// One layer of the aurora: the band between `left` and `right`, its centre line and
    /// thickness from `aurora_at`, with the thickness scaled by `k` (wide soft layers, thin core).
    fn aurora_band(&self, t: f32, left: f32, right: f32, yc: f32, amp: f32, thick: f32, k: f32) -> Result<ID2D1PathGeometry> {
        let pts: Vec<(f32, f32, f32)> = (0..=AURORA_STEPS).map(|i| {
            let u = i as f32 / AURORA_STEPS as f32;
            let (c, w) = aurora_at(t, u);
            (left + (right - left) * u, yc + c * amp, w * thick * k / 2.0)
        }).collect();
        unsafe {
            let geo = self.factory.CreatePathGeometry()?;
            let sink = geo.Open()?;
            let v = |x: f32, y: f32| Vector2 { X: x, Y: y };
            sink.BeginFigure(v(pts[0].0, pts[0].1 - pts[0].2), D2D1_FIGURE_BEGIN_FILLED);
            for &(x, y, h) in &pts[1..] {
                sink.AddLine(v(x, y - h));
            }
            for &(x, y, h) in pts.iter().rev() {
                sink.AddLine(v(x, y + h));
            }
            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            sink.Close()?;
            Ok(geo)
        }
    }

    /// The aurora's centre line (an open polyline, for the stroked core).
    fn aurora_line(&self, t: f32, left: f32, right: f32, yc: f32, amp: f32) -> Result<ID2D1PathGeometry> {
        unsafe {
            let geo = self.factory.CreatePathGeometry()?;
            let sink = geo.Open()?;
            let at = |i: usize| {
                let u = i as f32 / AURORA_STEPS as f32;
                Vector2 { X: left + (right - left) * u, Y: yc + aurora_at(t, u).0 * amp }
            };
            sink.BeginFigure(at(0), D2D1_FIGURE_BEGIN_HOLLOW);
            for i in 1..=AURORA_STEPS {
                sink.AddLine(at(i));
            }
            sink.EndFigure(D2D1_FIGURE_END_OPEN);
            sink.Close()?;
            Ok(geo)
        }
    }

    /// Draw one frame and push it to `hwnd` placed at screen (x, y), window size (w, h) device px.
    /// Returns true while an icon morph is still playing (the caller keeps frames coming).
    /// Lines `text` wraps to in a notification banner (its body font and width, at `scale`).
    pub fn banner_lines(&mut self, text: &str, scale: f32, with_image: bool) -> u32 {
        let Ok(fonts) = self.fonts(scale) else { return 1 };
        let u: Vec<u16> = text.encode_utf16().collect();
        let mut m = DWRITE_TEXT_METRICS::default();
        let width = BANNER_TEXT_W - if with_image { BANNER_IMAGE_W + BANNER_IMAGE_GAP } else { 0.0 };
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

    pub fn present(&mut self, hwnd: HWND, x: i32, y: i32, w: i32, h: i32, scale: f32, v: &View) -> Result<bool> {
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
        // The equaliser rises when playing starts and settles flat when it stops, not in one frame.
        let bars_to = if v.music.is_some_and(|t| t.playing) { 1.0 } else { 0.0 };
        self.bars += (bars_to - self.bars) * (1.0 - (-dt * 9.0).exp());
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
        // The pill's battery icon (Lucide) with its paths as geometry.
        let battery_line = (|| {
            let b = v.battery?;
            let ic = icons::battery(b.percent, b.charging);
            let geoms = ic.parts.iter().map(|part| self.line_geometry(part.d)).collect::<Option<Vec<_>>>()?;
            Some((b, ic, geoms))
        })();
        // A sun for the Weather page while there is no city or conditions yet (muted, like Tasks' ring).
        let weather_empty = (|| {
            let ic = icons::weather(weather::Sky::Clear, true);
            let geoms = ic.parts.iter().map(|part| self.line_geometry(part.d)).collect::<Option<Vec<_>>>()?;
            Some((ic, geoms))
        })();
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
        let note_icons: Vec<Option<ID2D1Bitmap>> = if page == Some(Page::Notify) { v.notes.iter().map(|n| self.app_icon(&n.id)).collect() } else { Vec::new() };
        // The picture a banner carries (a screenshot), loaded like an icon: the banner shows without it first.
        let banner_image: Option<ID2D1Bitmap> = v.banner.filter(|_| v.mode == Mode::Alert).and_then(|b| b.image.as_deref()).and_then(|p| self.app_icon(&format!("img:{p}")));
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
                match &cover {
                    Some((big, small)) => {
                        // Made at exactly the drawn size: placed on whole pixels, 1:1, so it stays sharp.
                        let bmp = if box_.right - box_.left > 40.0 * s { big } else { small };
                        let size = bmp.GetSize();
                        let (x, y) = (box_.left.round(), box_.top.round());
                        let b = rt.CreateBitmapBrush(bmp, None, None)?;
                        b.SetTransform(&Matrix3x2 { M11: 1.0, M12: 0.0, M21: 0.0, M22: 1.0, M31: x, M32: y });
                        b.SetOpacity(a);
                        rt.FillRoundedRectangle(&rounded(rect(x, y, x + size.width, y + size.height), radius), &b);
                    }
                    None => {
                        rt.FillRoundedRectangle(&rounded(box_, radius), &self.brush(grey(0.2, a))?);
                        text(ICON_NOTE, F_ICON, box_, DWRITE_TEXT_ALIGNMENT_CENTER, grey(1.0, a * 0.8))?;
                    }
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
            let lead = DWRITE_TEXT_ALIGNMENT_LEADING;
            let center = DWRITE_TEXT_ALIGNMENT_CENTER;
            let trail = DWRITE_TEXT_ALIGNMENT_TRAILING;
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
            // pops in past full size and settles (ICON_MORPH_S).
            let morphing = std::cell::Cell::new(false);
            let micon = |slot: &'static str, icon: &'static str, f: usize, b: D2D_RECT_F, c: D2D1_COLOR_F| -> Result<()> {
                let (prev, t) = {
                    let mut m = self.morphs.borrow_mut();
                    let e = m.entry(slot).or_insert((icon, icon, Instant::now() - std::time::Duration::from_secs(9)));
                    if e.0 != icon {
                        *e = (icon, e.0, Instant::now());
                    }
                    (e.1, e.2.elapsed().as_secs_f32() / ICON_MORPH_S)
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

            let ear = notch::ear(v.size[1]) * s;
            let shape = self.notch_shape(l, r, nh, nr, ear)?;
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
            if level > 0.0 && open > 0.0 && v.mode != Mode::Alert {
                let (gr, gg, gb) = dim_to_lum(tint, GLOW_MAX_LUM);
                let stop = |position: f32, a: f32| D2D1_GRADIENT_STOP { position, color: color(gr, gg, gb, a * open * level * gate) };
                let stops = rt.CreateGradientStopCollection(&[stop(0.58, 0.0), stop(0.68, 0.08), stop(0.82, 0.42), stop(1.0, 1.0)], D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP)?;
                let line = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES { startPoint: Vector2 { X: 0.0, Y: 0.0 }, endPoint: Vector2 { X: 0.0, Y: nh } };
                rt.FillGeometry(&shape, &rt.CreateLinearGradientBrush(&line, None, &stops)?, None);
            }
            // Aurora over the glow: a thin glowing line in the cover's two colours, swaying slowly.
            let aurora_a = open * level * gate;
            if aurora_a > 0.01 && v.mode != Mode::Alert {
                let [c1, c2] = self.cover.as_ref().map_or([GLOW_NEUTRAL; 2], |c| c.aurora);
                let t = v.phase;
                let (c1, c2) = (dim_to_lum(c1, AURORA_MAX_LUM), dim_to_lum(c2, AURORA_MAX_LUM));
                // First colour on the left into the second on the right; each stop's strength
                // drifts on its own, so brighter and darker stretches travel along the band.
                let stops: Vec<D2D1_GRADIENT_STOP> = (0..=5).map(|i| {
                    let f = i as f32 / 5.0;
                    let c = (c1.0 + (c2.0 - c1.0) * f, c1.1 + (c2.1 - c1.1) * f, c1.2 + (c2.2 - c1.2) * f);
                    let pulse = 0.45 + 0.55 * (0.5 + 0.5 * (t * 0.4 + i as f32 * 1.7).sin());
                    D2D1_GRADIENT_STOP { position: f, color: color(c.0, c.1, c.2, pulse) }
                }).collect();
                let line = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES { startPoint: Vector2 { X: l, Y: 0.0 }, endPoint: Vector2 { X: r, Y: 0.0 } };
                let (yc, amp) = (nh * AURORA_Y, nh * AURORA_AMP);
                // Everything that can fail is made first: a failure skips the aurora for this
                // frame instead of leaving the panel's layer pushed and the frame unfinished.
                let made = (|| -> Result<(ID2D1Bitmap, D2D_SIZE_F, ID2D1LinearGradientBrush, ID2D1PathGeometry)> {
                // Drawn at 1/AURORA_SHRINK size, then stretched with linear filtering: the stretch
                // is the blur, for a few small fills instead of a blur effect.
                let small = rt.CreateCompatibleRenderTarget(Some(&D2D_SIZE_F { width: (w as f32 / AURORA_SHRINK).ceil(), height: (nh / AURORA_SHRINK).ceil() + 1.0 }), None, None, D2D1_COMPATIBLE_RENDER_TARGET_OPTIONS_NONE)?;
                small.BeginDraw();
                small.Clear(Some(&color(0.0, 0.0, 0.0, 0.0)));
                small.SetTransform(&Matrix3x2 { M11: 1.0 / AURORA_SHRINK, M12: 0.0, M21: 0.0, M22: 1.0 / AURORA_SHRINK, M31: 0.0, M32: 0.0 });
                let brush = small.CreateLinearGradientBrush(&line, None, &small.CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP)?)?;
                for (k, alpha) in AURORA_LAYERS {
                    let band = self.aurora_band(t, l, r, yc, amp, nh * AURORA_HALO, k)?;
                    brush.SetOpacity(alpha);
                    small.FillGeometry(&band, &brush, None);
                }
                small.EndDraw(None, None)?;
                // Stretched in two linear steps (8x would show its blocks): to half size, then full.
                let sz = small.GetBitmap()?.GetSize();
                let half = AURORA_SHRINK / 2.0;
                let mid = rt.CreateCompatibleRenderTarget(Some(&D2D_SIZE_F { width: sz.width * half, height: sz.height * half }), None, None, D2D1_COMPATIBLE_RENDER_TARGET_OPTIONS_NONE)?;
                mid.BeginDraw();
                mid.Clear(Some(&color(0.0, 0.0, 0.0, 0.0)));
                mid.DrawBitmap(&small.GetBitmap()?, Some(&rect(0.0, 0.0, sz.width * half, sz.height * half)), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None);
                mid.EndDraw(None, None)?;
                // The thin bright core, drawn sharp and anti-aliased at full size.
                let whiten = |c: D2D1_COLOR_F| color(c.r + (1.0 - c.r) * AURORA_CORE_WHITE, c.g + (1.0 - c.g) * AURORA_CORE_WHITE, c.b + (1.0 - c.b) * AURORA_CORE_WHITE, c.a);
                let core_stops: Vec<D2D1_GRADIENT_STOP> = stops.iter().map(|st| D2D1_GRADIENT_STOP { position: st.position, color: whiten(st.color) }).collect();
                let core = rt.CreateLinearGradientBrush(&line, None, &rt.CreateGradientStopCollection(&core_stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP)?)?;
                Ok((mid.GetBitmap()?, sz, core, self.aurora_line(t, l, r, yc, amp)?))
                })();
                if let Ok((halo, sz, core, path)) = made {
                    // Only inside the panel's outline.
                    let params = D2D1_LAYER_PARAMETERS { contentBounds: rect(l, 0.0, r, nh), geometricMask: std::mem::ManuallyDrop::new(Some(shape.clone().into())),
                        maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, maskTransform: Matrix3x2::identity(), opacity: 1.0,
                        opacityBrush: std::mem::ManuallyDrop::new(None), layerOptions: D2D1_LAYER_OPTIONS_NONE };
                    rt.PushLayer(&params, None);
                    rt.DrawBitmap(&halo, Some(&rect(0.0, 0.0, sz.width * AURORA_SHRINK, sz.height * AURORA_SHRINK)), aurora_a, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None);
                    for (width, alpha) in AURORA_CORE {
                        core.SetOpacity(alpha * aurora_a);
                        rt.DrawGeometry(&path, &core, width * s, &self.round);
                    }
                    rt.PopLayer();
                    drop(std::mem::ManuallyDrop::into_inner(params.geometricMask));
                }
            }

            // Each layout fades in as the pill reaches its size.
            let fade = |from: f32, span: f32| ((nh - from * s) / (span * s)).clamp(0.0, 1.0);
            let small_a = 1.0 - fade(39.0, 20.0);
            if nh >= 20.0 * s && small_a > 0.0 && v.mode != Mode::Alert {
                let pill_music = v.music.filter(|_| v.live);
                // The idle robot fades the clock and battery out as its eyes fade in.
                let robot_a = v.robot.map_or(0.0, |e| e.alpha);
                let pill_a = small_a;
                let small_a = small_a * (1.0 - robot_a);
                text(v.time, F_SMALL, rect(l, 0.0, r, nh), center, grey(1.0, small_a))?;
                if let Some((b, ic, geoms)) = &battery_line {
                    // A battery icon on the 24 px grid, 20 px square: left of the time; with music the
                    // cover takes that spot, so it sits before the bars.
                    let d = 20.0 * s;
                    let bx = if pill_music.is_some() { r - 61.0 * s } else { l + 13.0 * s };
                    let by = (nh - d) / 2.0;
                    draw_wx_icon(ic, geoms, bx, by, d, Some(battery_tint(b)), small_a, 9.0)?;
                }
                // Weather: icon and temperature in the room the clock leaves, to its right on the plain
                // pill (balancing the battery), after the cover on the music pill. Left out
                // when it would touch the clock.
                if let (Some((ic, geoms)), Some(n)) = (&weather_line, v.weather.now) {
                    let (icon, gap) = (14.0 * s, 2.0 * s);
                    let clock_half = width(v.time, F_SMALL) / 2.0 + 3.0 * s;
                    // Where the icon and `label` shrunk by `k` go, and the room left between them and the clock.
                    let place = |label: &str, k: f32| {
                        let total = icon + gap + width(label, F_LABEL) * k;
                        if pill_music.is_some() {
                            let left = l + 38.0 * s;
                            (left, total, cx - clock_half - (left + total))
                        } else {
                            let left = r - 11.0 * s - total;
                            (left, total, left - (cx + clock_half))
                        }
                    };
                    // "−12°" is wider than "29°": it keeps its degree sign by shrinking a little (down
                    // to 80%); only a number that still does not fit loses the sign, not the weather.
                    let number = v.unit.number(n.temp);
                    let with_sign = format!("{number}°");
                    let tries = [(&with_sign, 1.0), (&with_sign, 0.9), (&with_sign, 0.8), (&number, 1.0)];
                    let fitted = tries.into_iter().map(|(label, k)| (place(label, k), label, k)).find(|((_, _, room), _, _)| *room >= 0.0);
                    if let Some(((left, _, _), label, k)) = fitted {
                        draw_wx_icon(ic, geoms, left, (nh - icon) / 2.0, icon, None, small_a * 0.9, 9.0)?;
                        let (tx, ty) = (left + icon + gap, nh / 2.0);
                        let base = { let mut m = Matrix3x2::default(); rt.GetTransform(&mut m); m };
                        rt.SetTransform(&(translate(-tx, -ty) * scaling(k) * translate(tx, ty) * base));
                        text(label, F_LABEL, rect(tx, 0.0, tx + width(label, F_LABEL) + 6.0 * s, nh), lead, grey(0.9, small_a))?;
                        rt.SetTransform(&base);
                    }
                }
                if pill_music.is_some() {
                    // Live pill: cover on the left, equaliser on the right.
                    let c = 20.0 * s;
                    let top = (nh - c) / 2.0;
                    draw_cover(rect(l + 10.0 * s, top, l + 10.0 * s + c, top + c), 5.0 * s, small_a)?;
                    let bw = 2.0 * s;
                    for i in 0..4 {
                        let k = i as f32;
                        let amp = bars_level * (0.5 + 0.5 * (v.phase * (6.0 + k * 1.7) + k * 1.3).sin());
                        let bh = (4.0 + 10.0 * amp) * s;
                        let bx = r - 14.0 * s - (4.0 - k) * (bw + 2.0 * s);
                        let my = nh / 2.0;
                        rt.FillRoundedRectangle(&rounded(rect(bx, my - bh / 2.0, bx + bw, my + bh / 2.0), bw / 2.0), &self.brush(grey(1.0, small_a * 0.9))?);
                    }
                }
                if let Some(e) = v.robot.filter(|e| e.alpha > 0.0) {
                    self.draw_face(&e, cx, nh / 2.0, s, pill_a * e.alpha)?;
                }
            }

            let mut icon_busy = false;
            if v.mode == Mode::Alert {
                if let Some(t) = v.banner {
                    let a = fade(44.0, 18.0);
                    // Icon on the left, text beside it. Status and device icons, and a notification's
                    // bell, are line icons whose parts move for a moment after the banner appears
                    // (icons.rs); the water drop hops.
                    let d = 36.0 * s;
                    let (ix, iy) = (l + 18.0 * s, nh / 2.0 - d / 2.0 + 2.0 * s);
                    let icon = t.icon.unwrap_or(ICON_BELL);
                    let (mx, my) = (ix + d / 2.0, iy + d / 2.0);
                    let base = { let mut b = Matrix3x2::default(); rt.GetTransform(&mut b); b };
                    let age = v.banner_age;
                    let (pop_scale, pop_alpha) = icons::pop(age);
                    let pop = translate(-mx, -my) * scaling(pop_scale) * translate(mx, my);
                    if let Some((ic, geoms)) = &banner_line {
                        icon_busy = age < icons::DUR;
                        let size = LINE_ICON * s;
                        let view = scaling(size / ic.vb) * translate(mx - size / 2.0, my - size / 2.0);
                        let groups = icons::motion(ic.anim, age);
                        for (part, geo) in ic.parts.iter().zip(geoms) {
                            let g = groups.get(part.g as usize).copied().unwrap_or(icons::Motion::STILL);
                            let (ox, oy) = ic.origins.get(part.g as usize).copied().unwrap_or((0.0, 0.0));
                            let local = translate(-ox, -oy) * rotation(g.rot) * scaling(g.scale) * translate(ox + g.dx, oy + g.dy);
                            let alpha = a * pop_alpha * g.alpha * groups.get(part.a as usize).map_or(1.0, |m| m.alpha);
                            rt.SetTransform(&(local * view * pop * base));
                            rt.DrawGeometry(geo, &self.brush(grey(1.0, alpha))?, ic.sw, &self.round);
                        }
                        rt.SetTransform(&base);
                    } else if icon == ICON_DROP {
                        icon_busy = age < icons::DUR;
                        let hop = icons::drop_motion(age).dy * (LINE_ICON * s / 24.0);
                        rt.SetTransform(&(pop * translate(0.0, hop) * base));
                        let g = self.water_drop(mx, my, 24.0 * s)?;
                        rt.FillGeometry(&g, &self.brush(color(BLUE.0, BLUE.1, BLUE.2, a * pop_alpha))?, None);
                        rt.SetTransform(&base);
                    } else {
                        micon("banner", icon, F_BANNER_ICON, rect(ix, iy, ix + d, iy + d), grey(1.0, a))?;
                    }
                    let (tl, mut tr) = (ix + d + 12.0 * s, r - 22.0 * s);
                    if t.image.is_some() {
                        // The picture sits at the right edge, centred on the banner; text ends before it.
                        let (bw, bh) = (BANNER_IMAGE_W * s, BANNER_IMAGE_H * s);
                        let (bx, by) = (r - 18.0 * s - bw, nh / 2.0 - bh / 2.0 + 2.0 * s);
                        tr = bx - BANNER_IMAGE_GAP * s;
                        match &banner_image {
                            Some(img) => {
                                let size = img.GetSize();
                                let k = (bw / size.width).min(bh / size.height);
                                let (w, h) = (size.width * k, size.height * k);
                                let (x, y) = (bx + (bw - w) / 2.0, by + (bh - h) / 2.0);
                                let brush = rt.CreateBitmapBrush(img, None, None)?;
                                brush.SetTransform(&(scaling(k) * translate(x, y)));
                                brush.SetOpacity(a);
                                rt.FillRoundedRectangle(&rounded(rect(x, y, x + w, y + h), 5.0 * s), &brush);
                            }
                            // Still loading (or gone): a quiet placeholder of the same size.
                            None => rt.FillRoundedRectangle(&rounded(rect(bx, by, bx + bw, by + bh), 5.0 * s), &self.brush(grey(0.2, a))?),
                        }
                    }
                    text(&t.app, F_LABEL, rect(tl, 8.0 * s, tr, 24.0 * s), lead, grey(0.6, a))?;
                    text(&t.title, F_TITLE, rect(tl, 24.0 * s, tr, 42.0 * s), lead, grey(1.0, a))?;
                    match t.level {
                        Some(level) => {
                            let th = 5.0 * s;
                            let my = 50.0 * s;
                            let bar = |right: f32| rounded(rect(tl, my - th / 2.0, right, my + th / 2.0), th / 2.0);
                            rt.FillRoundedRectangle(&bar(tr), &self.brush(grey(0.25, a))?);
                            rt.FillRoundedRectangle(&bar(tl + (tr - tl) * level.clamp(0.0, 1.0)), &self.brush(grey(1.0, a))?);
                        }
                        // Status banner (icon): one line under the title.
                        None if t.icon.is_some() => text(&t.body, F_BODY, rect(tl, 42.0 * s, tr, 58.0 * s), lead, grey(0.75, a))?,
                        // Notification: wraps over the banner's height (sized to its lines, at most
                        // `BANNER_MAX_LINES`); what still does not fit ends with "…".
                        None => {
                            let top = notch::BANNER_TEXT_TOP * s;
                            let bottom = (nh - 7.0 * s).max(top + notch::BANNER_LINE_H * s);
                            text(&t.body, F_BODY_WRAP, rect(tl, top, tr, bottom), lead, grey(0.75, a))?
                        }
                    }
                }
            }

            let a = fade(110.0, 50.0);
            if a > 0.0 && v.mode != Mode::Alert {
                let (hl, hr) = (l + PAD * s, r - PAD * s);
                // Header: the page's icon and name on the left (they morph with the page),
                // short date · time in the centre, battery on the right.
                let (ht, hb) = (6.0 * s, 28.0 * s);
                let draw_title = |page: Page, dx: f32, ta: f32| -> Result<()> {
                    if ta <= 0.01 {
                        return Ok(());
                    }
                    let (icon, name, c) = page.title();
                    // Weather's icon is the same Lucide line sun as the page's, not a font glyph.
                    match (&weather_empty, page) {
                        (Some((ic, geoms)), Page::Weather) => draw_wx_icon(ic, geoms, hl + dx + 1.0 * s, (ht + hb) / 2.0 - 8.0 * s, 16.0 * s, Some((c, c)), a * ta, 9.0)?,
                        _ => text(icon, F_SMALL_ICON, rect(hl + dx, ht, hl + dx + 18.0 * s, hb), center, color(c.0, c.1, c.2, a * ta))?,
                    }
                    text(name, F_SMALL, rect(hl + dx + 24.0 * s, ht, hl + dx + 170.0 * s, hb), lead, grey(1.0, a * ta))
                };
                match v.leaving {
                    Some((from, t, dir)) => {
                        let d = dir as f32 * 24.0 * s;
                        let (out_a, in_a) = page_fades(t);
                        draw_title(from, -d * t.min(1.0), out_a)?;
                        draw_title(v.page, d * (1.0 - ease_out_back(t)), in_a)?;
                    }
                    None => draw_title(v.page, 0.0, 1.0)?,
                }
                // The same battery icon as the pill's, then its percentage.
                if let Some((b, ic, geoms)) = &battery_line {
                    let pct = format!("{}%", b.percent);
                    let tx = hr - width(&pct, F_LABEL);
                    text(&pct, F_LABEL, rect(tx, ht, hr, hb), trail, grey(0.85, a))?;
                    let d = 18.0 * s;
                    draw_wx_icon(ic, geoms, tx - 3.0 * s - d, (ht + hb - d) / 2.0, d, Some(battery_tint(b)), a, 9.0)?;
                }
                // Date · time in the middle of the header.
                text(&format!("{} · {}", v.date, v.time), F_LABEL, rect(cx - 80.0 * s, ht, cx + 80.0 * s, hb), center, grey(0.85, a))?;
                // Page content sits between the side arrows.
                let (cl, cr) = (l + SIDE * s, r - SIDE * s);
                let lv = &v.list;
                let rows_of = |page: Page| match page {
                    Page::Todo => v.todos.len(),
                    Page::Notify => v.notes.len(),
                    Page::Weather => usize::from(v.weather.now.is_some()),
                    _ => 0,
                };
                let minutes = todo::minutes;
                let solid = |c: (f32, f32, f32), a: f32| color(c.0, c.1, c.2, a);

                // One page, shifted by `dx` and faded by `pa` (the page switch morph).
                let draw_page = |page: Page, dx: f32, pa: f32| -> Result<()> {
                    let a = a * pa;
                    if a <= 0.01 {
                        return Ok(());
                    }
                    let current = page == v.page;
                    let scroll = if current { lv.scroll } else { 0 };
                    let settings = if current { v.health.settings } else { None };
                    let hovered = if current { v.hovered } else { None };
                    let btns = page_buttons(cx, s, page, rows_of(page), scroll, settings, v.lyrics);
                    let rect_of = |b: Button| btns.iter().find(|(x, _)| *x == b).map(|(_, r)| *r);
                    // A soft disc behind an icon-only button under the mouse: the colour change alone
                    // does not say "this can be clicked".
                    let hover_disc = |b: Button| -> Result<()> {
                        if let (true, Some(r)) = (hovered == Some(b), rect_of(b)) {
                            let point = Vector2 { X: (r.left + r.right) / 2.0, Y: (r.top + r.bottom) / 2.0 };
                            rt.FillEllipse(&D2D1_ELLIPSE { point, radiusX: 11.0 * s, radiusY: 11.0 * s }, &self.brush(grey(0.18, a))?);
                        }
                        Ok(())
                    };
                    rt.SetTransform(&Matrix3x2 { M11: 1.0, M12: 0.0, M21: 0.0, M22: 1.0, M31: dx, M32: 0.0 });

                    // Add / edit box shared by the list pages.
                    let draw_input = |placeholder: &str| -> Result<()> {
                        let Some(ib) = rect_of(Button::ListInput) else { return Ok(()) };
                        let editing = current && lv.editing;
                        let edit_row = if current { lv.edit_item } else { None };
                        let focus = editing || hovered == Some(Button::ListInput);
                        rt.FillRoundedRectangle(&rounded(ib, 13.0 * s), &self.brush(grey(if focus { 0.16 } else { 0.12 }, a))?);
                        if editing || edit_row.is_some() {
                            let inner = rect(ib.left + 0.5, ib.top + 0.5, ib.right - 0.5, ib.bottom - 0.5);
                            let c = if edit_row.is_some() { solid(YELLOW, a * 0.8) } else { grey(1.0, a * 0.35) };
                            rt.DrawRoundedRectangle(&rounded(inner, 13.0 * s), &self.brush(c)?, 1.0 * s, None);
                        }
                        let tag_w = if edit_row.is_some() { 58.0 * s } else { 0.0 };
                        if edit_row.is_some() {
                            text(tr("Đang sửa", "Editing"), F_LABEL, rect(ib.right - tag_w - 8.0 * s, ib.top, ib.right - 10.0 * s, ib.bottom), trail, solid(YELLOW, a))?;
                        }
                        let icon_box = rect(ib.left + 8.0 * s, ib.top, ib.left + 26.0 * s, ib.bottom);
                        text(ICON_ADD, F_SMALL_ICON, icon_box, center, grey(0.6, a))?;
                        let draft = if current { lv.draft } else { "" };
                        // The x that wipes the text, while there is some (the "Editing" tag has the room otherwise).
                        let clear = !draft.is_empty() && edit_row.is_none();
                        if clear {
                            if let Some(cb) = rect_of(Button::ListClear) {
                                hover_disc(Button::ListClear)?;
                                text(ICON_CLOSE, F_SMALL_ICON, cb, center, if hovered == Some(Button::ListClear) { grey(1.0, a) } else { grey(0.55, a) })?;
                            }
                        }
                        let tb = rect(icon_box.right + 4.0 * s, ib.top, ib.right - if clear { 26.0 } else { 12.0 } * s - tag_w, ib.bottom);
                        if draft.is_empty() && !editing {
                            text(placeholder, F_BODY, tb, lead, grey(0.55, a))?;
                        } else {
                            // Long drafts keep their end in view.
                            let tw = width(draft, F_BODY);
                            let room = tb.right - tb.left - 4.0 * s;
                            text(draft, F_BODY, tb, if tw > room { trail } else { lead }, grey(1.0, a))?;
                            if editing && lv.caret {
                                let x = if tw > room { tb.right - 2.0 * s } else { tb.left + tw + 1.0 * s };
                                let my = (ib.top + ib.bottom) / 2.0;
                                rt.FillRectangle(&rect(x, my - 7.0 * s, x + 1.5 * s, my + 7.0 * s), &self.brush(grey(1.0, a))?);
                            }
                        }
                        Ok(())
                    };
                    // Edit / delete on every list row, so they are there without a pointer to hover
                    // with (touch, pen); lit while the row is hovered or being edited.
                    let row_tools = |i: usize, lit: bool| -> Result<()> {
                        let rest = if lit { 0.65 } else { 0.4 };
                        let edit_hot = hovered == Some(Button::ListEdit(i)) || (current && lv.edit_item == Some(i));
                        if let Some(eb) = btns.iter().filter(|(b, _)| *b == Button::ListEdit(i)).map(|(_, r)| *r).last() {
                            hover_disc(Button::ListEdit(i))?;
                            text(ICON_EDIT, F_SMALL_ICON, eb, center, if edit_hot { solid(YELLOW, a) } else { grey(rest, a) })?;
                        }
                        if let Some(db) = rect_of(Button::ListDelete(i)) {
                            let c = if hovered == Some(Button::ListDelete(i)) { color(1.0, 0.45, 0.4, a) } else { grey(rest, a) };
                            hover_disc(Button::ListDelete(i))?;
                            text(ICON_CLOSE, F_SMALL_ICON, db, center, c)?;
                        }
                        Ok(())
                    };
                    let row_hovered = |i: usize| {
                        matches!(hovered, Some(Button::ListToggle(j)) | Some(Button::ListEdit(j)) | Some(Button::ListDelete(j)) if j == i)
                            || (current && lv.edit_item == Some(i))
                    };
                    // Scroll position of `n` rows shown `rows` at a time from `y0`, `row_h` apart.
                    let scrollbar = |n: usize, rows: usize, y0: f32, row_h: f32| -> Result<()> {
                        if n > rows {
                            let (top, bottom) = (y0 * s, (y0 + row_h * rows as f32) * s);
                            let x = cr + 6.0 * s;
                            rt.FillRoundedRectangle(&rounded(rect(x, top, x + 3.0 * s, bottom), 1.5 * s), &self.brush(grey(0.2, a))?);
                            let len = (bottom - top) * rows as f32 / n as f32;
                            let start = top + (bottom - top - len) * scroll as f32 / (n - rows) as f32;
                            rt.FillRoundedRectangle(&rounded(rect(x, start, x + 3.0 * s, start + len), 1.5 * s), &self.brush(grey(0.6, a))?);
                        }
                        Ok(())
                    };
                    // Time label coloured by how close it is: orange within the early reminder, red when past.
                    let time_color = |due: u64, quiet: bool| {
                        let left = minutes(due) - minutes(lv.now);
                        if quiet { grey(0.4, a) } else if left <= 0 { color(1.0, 0.35, 0.3, a) }
                        else if left <= lv.remind_before.max(15) as i64 { color(1.0, 0.62, 0.2, a) } else { grey(0.6, a) }
                    };

                    match page {
                        Page::Music => {
                            let top = COVER_TOP * s;
                            // The player is the left 440 px of the panel, wider or not.
                            let cr = l + (notch::target(Mode::Expanded, false)[0] - SIDE) * s;
                            draw_cover(rect(cl, top, cl + COVER * s, top + COVER * s), 14.0 * s, a)?;
                            let tl = cl + (COVER + COVER_GAP) * s;
                            match v.music {
                                Some(t) => {
                                    let title = if t.title.is_empty() { tr("Đang phát", "Playing") } else { &t.title };
                                    text(&t.artist, F_BODY, rect(tl, top, cr, top + 18.0 * s), lead, soft_text(tint, level, 0.65, a))?;
                                    text(title, F_TITLE, rect(tl, top + 18.0 * s, cr, top + 40.0 * s), lead, grey(1.0, a))?;
                                    let lyric = t.lyric_now().unwrap_or_else(|| hovered.map_or(
                                        if t.duration_ms == 0 { tr("Trình phát chưa cung cấp thời gian", "The player gives no timeline") }
                                        else if t.lyrics.is_empty() { "" } else { "♪" }, Button::label));
                                    text(t.command_error.unwrap_or(lyric), F_BODY, rect(tl, top + 42.0 * s, cr, top + 60.0 * s), lead,
                                        if t.command_error.is_some() { color(1.0, 0.5, 0.4, a) } else { grey(0.95, a) })?;
                                }
                                None => text(tr("Không có bài", "Nothing playing"), F_TITLE, rect(tl, top, cr, top + 40.0 * s), lead, grey(1.0, a))?,
                            }
                            let pos = v.music.map_or(0, |t| t.position_ms());
                            let dur = v.music.map_or(0, |t| t.duration_ms);
                            let by = BAR_Y * s;
                            let (bl, brr) = (tl, cr);
                            let times = soft_text(tint, level, 0.6, a);
                            text(&if dur > 0 { clock(pos) } else { "--:--".into() }, F_LABEL, rect(bl, by + 4.0 * s, bl + 60.0 * s, by + 20.0 * s), lead, times)?;
                            text(&if dur > 0 { format!("-{}", clock(dur.saturating_sub(pos))) } else { "--:--".into() }, F_LABEL, rect(brr - 60.0 * s, by + 4.0 * s, brr, by + 20.0 * s), trail, times)?;
                            let th = 4.0 * s;
                            let bar = |right: f32| rounded(rect(bl, by - th / 2.0, right, by + th / 2.0), th / 2.0);
                            rt.FillRoundedRectangle(&bar(brr), &self.brush({ let g = 0.25 + 0.75 * level; color(g, g, g, a * (1.0 - 0.7 * level)) })?);
                            if dur > 0 {
                                let p = (pos as f32 / dur as f32).clamp(0.0, 1.0);
                                rt.FillRoundedRectangle(&bar(bl + (brr - bl) * p), &self.brush(grey(1.0, a))?);
                                if Button::Seek.enabled(v.music) {
                                    let radius = if hovered == Some(Button::Seek) { 5.0 } else { 3.0 } * s;
                                    rt.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: bl + (brr - bl) * p, Y: by }, radiusX: radius, radiusY: radius }, &self.brush(grey(1.0, a))?);
                                }
                            }
                            let playing = v.music.is_some_and(|t| t.playing);
                            for (b, br) in buttons_from(l / s, s) {
                                let on = b.enabled(v.music);
                                if b == Button::Lyrics {
                                    // A speech bubble with two lines; filled while the lyrics are open.
                                    let (bx, by) = ((br.left + br.right) / 2.0, (br.top + br.bottom) / 2.0);
                                    let bubble = rect(bx - 9.0 * s, by - 7.0 * s, bx + 9.0 * s, by + 7.0 * s);
                                    let ink = if !on { grey(0.3, a) } else if v.lyrics { grey(0.05, a) } else if hovered == Some(b) { color(0.25, 0.85, 0.5, a) } else { grey(1.0, a) };
                                    if on && v.lyrics {
                                        rt.FillRoundedRectangle(&rounded(bubble, 4.0 * s), &self.brush(grey(1.0, a))?);
                                    } else {
                                        rt.DrawRoundedRectangle(&rounded(bubble, 4.0 * s), &self.brush(ink)?, 1.6 * s, None);
                                    }
                                    for (k, len) in [10.0, 6.0].into_iter().enumerate() {
                                        let y = by - 2.2 * s + k as f32 * 4.4 * s;
                                        rt.DrawLine(Vector2 { X: bx - 5.0 * s, Y: y }, Vector2 { X: bx - 5.0 * s + len * s, Y: y }, &self.brush(ink)?, 1.5 * s, &self.round);
                                    }
                                    continue;
                                }
                                let icon = match b {
                                    Button::Previous => ICON_PREV,
                                    Button::PlayPause => if playing { ICON_PAUSE } else { ICON_PLAY },
                                    Button::Next => ICON_NEXT,
                                    _ => continue,
                                };
                                if b == Button::PlayPause {
                                    // The main action: a white disc with a dark glyph.
                                    let fill = if !on { grey(0.25, a) } else if hovered == Some(b) { grey(0.85, a) } else { grey(1.0, a) };
                                    let disc = D2D1_ELLIPSE { point: Vector2 { X: (br.left + br.right) / 2.0, Y: (br.top + br.bottom) / 2.0 },
                                        radiusX: (br.right - br.left) / 2.0, radiusY: (br.bottom - br.top) / 2.0 };
                                    rt.FillEllipse(&disc, &self.brush(fill)?);
                                    micon("play", icon, F_ICON, br, if on { grey(0.05, a) } else { grey(0.5, a) })?;
                                    continue;
                                }
                                let c = if !on { grey(0.3, a) }
                                    else if hovered == Some(b) { color(0.25, 0.85, 0.5, a) } else { grey(1.0, a) };
                                micon(if b == Button::Previous { "prev" } else { "next" }, icon, F_ICON, br, c)?;
                            }
                            // Lyrics beside the player, fading in as the panel widens: the line being
                            // sung is bright and centred, the others dim; the whole column glides up
                            // as the song moves on. Long lines wrap onto more lines.
                            let player_w = notch::target(Mode::Expanded, false)[0];
                            let grow = ((v.size[0] - player_w) / LYRICS_EXTRA_W).clamp(0.0, 1.0);
                            if grow > 0.02 {
                                let appear = grow * grow * (3.0 - 2.0 * grow);
                                // Right after the controls, past a thin divider.
                                let divider = l + (player_w - SIDE + LYRIC_GAP) * s;
                                let (x0, y0, y1) = (divider + LYRIC_GAP * s, LYRIC_TOP * s, LYRIC_BOTTOM * s);
                                let x1 = x0 + LYRIC_W * s;
                                // Fading out at both ends.
                                let peak = 0.18 * a * appear;
                                let stops = rt.CreateGradientStopCollection(&[
                                    D2D1_GRADIENT_STOP { position: 0.0, color: grey(1.0, 0.0) },
                                    D2D1_GRADIENT_STOP { position: 0.5, color: grey(1.0, peak) },
                                    D2D1_GRADIENT_STOP { position: 1.0, color: grey(1.0, 0.0) },
                                ], D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP)?;
                                let down = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES { startPoint: Vector2 { X: divider, Y: y0 }, endPoint: Vector2 { X: divider, Y: y1 } };
                                rt.FillRectangle(&rect(divider, y0, divider + 1.0 * s, y1), &rt.CreateLinearGradientBrush(&down, None, &stops)?);
                                match v.music {
                                    Some(t) if !lyric_heights.is_empty() => {
                                        let phase = self.lyric_motion.phase();
                                        let focus = self.lyric_motion.focus();
                                        let middle = (y0 + y1) / 2.0;
                                        for (j, (_, line)) in t.lyrics.iter().enumerate() {
                                            let (top, h) = (middle + lyric_tops[j] - focus, lyric_heights[j]);
                                            if top + h < y0 || top > y1 {
                                                continue;
                                            }
                                            let alpha = a * appear * lyrics::edge_fade(top + h / 2.0, y0, y1, 28.0 * s) * lyrics::line_alpha(j, phase);
                                            if alpha > 0.01 {
                                                // Not clipped to the box: a line wraps to fit by the letters' advance,
                                                // and its last letter's ink ("y", a tone mark) can stick out past it.
                                                let units: Vec<u16> = line.encode_utf16().collect();
                                                fonts[F_LYRIC].SetTextAlignment(lead)?;
                                                rt.DrawText(&units, &fonts[F_LYRIC], &rect(x0, top, x1, top + h), &self.brush(soft_text(tint, level, 1.0, alpha))?, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
                                            }
                                        }
                                    }
                                    _ => text(tr("Không có lời cho bài này", "No lyrics for this song"), F_BODY, rect(x0, y0, x1, y1), center, grey(0.6, a * appear))?,
                                }
                            }
                        }
                        Page::Todo => {
                            draw_input(tr("Thêm việc… (vd: Họp 20:00, Khám răng 28/10 9h)", "Add a task… (e.g. Call 20:00, Dentist 28/10 9h)"))?;
                            let today = (lv.now / 10_000) as u32;
                            let (open, later) = if current { lv.counts } else { (0, 0) };
                            let cy = COUNT_Y * s;
                            text(&format!("{} · {open}", tr("Hôm nay", "Today")), F_LABEL, rect(cl, cy - 2.0 * s, cr, cy + 16.0 * s), lead, grey(0.6, a))?;
                            text(&format!("{} · {later}", tr("Sắp tới", "Upcoming")), F_LABEL, rect(cl, cy - 2.0 * s, cr - 26.0 * s, cy + 16.0 * s), trail,
                                if later > 0 { solid(BLUE, a) } else { grey(0.6, a) })?;
                            let open_set = settings == Some(Card::Tasks);
                            if let Some(gb) = rect_of(Button::Gear(Card::Tasks)) {
                                let hot = hovered == Some(Button::Gear(Card::Tasks));
                                let (icon, c) = if open_set { (ICON_CHECK, solid(GREEN, a)) } else { (ICON_GEAR, grey(if hot { 1.0 } else { 0.6 }, a)) };
                                hover_disc(Button::Gear(Card::Tasks))?;
                                micon("tasks-gear", icon, F_SMALL_ICON, gb, c)?;
                            }
                            if open_set {
                                // Settings: the early reminder, stepped with − / +.
                                let y = ROW_Y * s;
                                text(tr("Nhắc trước", "Remind before"), F_BODY, rect(cl, y, cr - 112.0 * s, y + 22.0 * s), lead, grey(0.9, a))?;
                                let value = if lv.remind_before == 0 { tr("Tắt", "Off").to_string() } else { format!("{} {}", lv.remind_before, tr("phút", "min")) };
                                text(&value, F_BODY, rect(cr - 88.0 * s, y, cr - 26.0 * s, y + 22.0 * s), center, grey(1.0, a))?;
                                for (b, icon) in [(Button::RemindStep(-1), ICON_MINUS), (Button::RemindStep(1), ICON_PLUS)] {
                                    if let Some(sb) = rect_of(b) {
                                        rt.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: (sb.left + sb.right) / 2.0, Y: (sb.top + sb.bottom) / 2.0 }, radiusX: 11.0 * s, radiusY: 11.0 * s },
                                            &self.brush(grey(if hovered == Some(b) { 0.3 } else { 0.2 }, a))?);
                                        text(icon, F_TINY_ICON, sb, center, grey(1.0, a))?;
                                    }
                                }
                                text(tr("Việc có giờ còn được nhắc đúng giờ", "Timed tasks also ring on time"), F_LABEL,
                                    rect(cl, y + 26.0 * s, cr, y + 44.0 * s), lead, grey(0.5, a))?;
                            } else {
                                if v.todos.is_empty() {
                                    let mid = (cl + cr) / 2.0;
                                    let ring = D2D1_ELLIPSE { point: Vector2 { X: mid, Y: (ROW_Y + 12.0) * s }, radiusX: 11.0 * s, radiusY: 11.0 * s };
                                    rt.DrawEllipse(&ring, &self.brush(grey(0.3, a))?, 1.5 * s, None);
                                    text(ICON_CHECK, F_TINY_ICON, rect(mid - 11.0 * s, (ROW_Y + 1.0) * s, mid + 11.0 * s, (ROW_Y + 23.0) * s), center, grey(0.3, a))?;
                                    text(tr("Chưa có việc nào", "No tasks yet"), F_BODY, rect(cl, (ROW_Y + 28.0) * s, cr, (ROW_Y + 46.0) * s), center, grey(0.7, a))?;
                                    text(tr("Gõ vào ô trên rồi nhấn Enter", "Type above and press Enter"), F_LABEL, rect(cl, (ROW_Y + 46.0) * s, cr, (ROW_Y + 62.0) * s), center, grey(0.5, a))?;
                                }
                                for (b, br) in btns.iter() {
                                    let Button::ListToggle(i) = *b else { continue };
                                    let Some(item) = v.todos.get(i) else { continue };
                                    let hot = row_hovered(i);
                                    if hot {
                                        rt.FillRoundedRectangle(&rounded(rect(br.left - 6.0 * s, br.top + 1.0 * s, cr + 2.0 * s, br.bottom - 1.0 * s), 8.0 * s), &self.brush(grey(0.1, a))?);
                                    }
                                    // Tick circle.
                                    let d = 14.0 * s;
                                    let (ccx, ccy) = (br.left + d / 2.0, (br.top + br.bottom) / 2.0);
                                    let circle = D2D1_ELLIPSE { point: Vector2 { X: ccx, Y: ccy }, radiusX: d / 2.0 - 0.75 * s, radiusY: d / 2.0 - 0.75 * s };
                                    let done = item.done.is_some();
                                    if done {
                                        rt.FillEllipse(&circle, &self.brush(solid(GREEN, a))?);
                                        text(ICON_CHECK, F_TINY_ICON, rect(ccx - d / 2.0, ccy - d / 2.0, ccx + d / 2.0, ccy + d / 2.0), center, grey(0.0, a))?;
                                    } else {
                                        rt.DrawEllipse(&circle, &self.brush(grey(if hot { 0.85 } else { 0.55 }, a))?, 1.5 * s, None);
                                    }
                                    // When, on the right: "HH:MM" today, "Mai HH:MM", "dd/mm HH:MM" (red once past).
                                    let mut text_right = br.right;
                                    if let Some(at) = item.at {
                                        let (day, h, m) = ((at / 10_000) as u32, at / 100 % 100, at % 100);
                                        let when = if day == today { format!("{h:02}:{m:02}") }
                                            else if day == todo::next_day(today) { format!("{} {h:02}:{m:02}", tr("Mai", "Tmrw")) }
                                            else { format!("{:02}/{:02} {h:02}:{m:02}", day % 100, day / 100 % 100) };
                                        // Urgency in words as well as colour: past due, or inside the early reminder.
                                        let left = minutes(at) - minutes(lv.now);
                                        let flag = if done { "" } else if left <= 0 { tr("Trễ", "Late") } else if left <= lv.remind_before.max(15) as i64 { tr("Sắp", "Soon") } else { "" };
                                        let label = if flag.is_empty() { when } else { format!("{flag} {when}") };
                                        let lw = width(&label, F_LABEL) + 4.0 * s;
                                        text(&label, F_LABEL, rect(br.right - lw, br.top, br.right - 2.0 * s, br.bottom), trail, time_color(at, done))?;
                                        text_right = br.right - lw - 6.0 * s;
                                    }
                                    // Text, struck through when done.
                                    let tb = rect(br.left + d + 10.0 * s, br.top, text_right, br.bottom);
                                    text(&item.text, F_BODY, tb, lead, grey(if done { 0.5 } else { 1.0 }, a))?;
                                    if done {
                                        let tw = width(&item.text, F_BODY).min(tb.right - tb.left);
                                        let my = (br.top + br.bottom) / 2.0 + 0.5 * s;
                                        rt.FillRectangle(&rect(tb.left, my, tb.left + tw, my + 1.0 * s), &self.brush(grey(0.5, a))?);
                                    }
                                    row_tools(i, hot)?;
                                }
                                scrollbar(v.todos.len(), LIST_ROWS, ROW_Y, ROW_H)?;
                            }
                        }
                        Page::Notify => {
                            if v.notes.is_empty() {
                                let area = rect(cl, NOTE_Y * s, cr, (NOTE_Y + NOTE_H * NOTE_ROWS as f32) * s);
                                text(tr("Chưa có thông báo", "No notifications"), F_BODY, area, center, grey(0.55, a))?;
                            }
                            let today = (lv.now / 10_000) as u32;
                            // Rows glide to their place after a wheel step: drawn from the fractional
                            // position, inside the list's own clip.
                            let pos = if current { lv.note_pos } else { 0.0 };
                            let first = pos.floor().max(0.0) as usize;
                            rt.PushAxisAlignedClip(&rect(cl - 8.0 * s, NOTE_Y * s, cr + 10.0 * s, (NOTE_Y + NOTE_H * NOTE_ROWS as f32 - 3.0) * s), D2D1_ANTIALIAS_MODE_ALIASED);
                            for (row, n) in v.notes.iter().enumerate().skip(first).take(NOTE_ROWS + 1) {
                                let y = (NOTE_Y + (row as f32 - pos) * NOTE_H) * s;
                                // The row under the mouse lights up: clicking it opens the notification.
                                if hovered == Some(Button::NoteOpen(row)) {
                                    let hb = rect(cl - 6.0 * s, y - 1.0 * s, cr + 2.0 * s, y + (NOTE_H - 3.0) * s);
                                    rt.FillRoundedRectangle(&rounded(hb, 8.0 * s), &self.brush(grey(0.1, a))?);
                                }
                                // Line 1: title, time on the right. Line 2: app · text.
                                let day = (n.at / 10_000) as u32;
                                let when = if day == today { format!("{:02}:{:02}", n.at / 100 % 100, n.at % 100) }
                                    else if todo::next_day(day) == today { tr("Hôm qua", "Yesterday").to_string() }
                                    else { format!("{:02}/{:02}", day % 100, day / 100 % 100) };
                                // App tile: its initial on a colour taken from its name.
                                let tile = rect(cl, y + 3.0 * s, cl + 30.0 * s, y + 33.0 * s);
                                match note_icons.get(row).and_then(|i| i.as_ref()) {
                                    // The app's own icon on a light-grey tile, so dark logos show on black.
                                    Some(icon) => {
                                        rt.FillRoundedRectangle(&rounded(tile, 8.0 * s), &self.brush(grey(0.16, a))?);
                                        rt.DrawBitmap(icon, Some(&rect(tile.left + 3.0 * s, tile.top + 3.0 * s, tile.right - 3.0 * s, tile.bottom - 3.0 * s)), a, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None);
                                    }
                                    None => {
                                        let (tr_, tg, tb) = app_tint(&n.app);
                                        rt.FillRoundedRectangle(&rounded(tile, 8.0 * s), &self.brush(color(tr_, tg, tb, a))?);
                                        text(&app_initial(&n.app), F_TITLE, tile, center, grey(1.0, a))?;
                                    }
                                }
                                let tx = cl + 38.0 * s;
                                text(&when, F_LABEL, rect(cr - 64.0 * s, y, cr, y + 20.0 * s), trail, grey(0.55, a))?;
                                text(&n.title, F_TITLE, rect(tx, y, cr - 68.0 * s, y + 20.0 * s), lead, grey(1.0, a))?;
                                let line2 = if n.body.is_empty() { n.app.clone() } else { format!("{} · {}", n.app, n.body) };
                                text(&line2, F_BODY, rect(tx, y + 19.0 * s, cr, y + 37.0 * s), lead, grey(0.65, a))?;
                            }
                            rt.PopAxisAlignedClip();
                            scrollbar(v.notes.len(), NOTE_ROWS, NOTE_Y, NOTE_H)?;
                            if let Some(cb) = rect_of(Button::NotesClear) {
                                let hot = hovered == Some(Button::NotesClear);
                                let label = tr("Xóa hết", "Clear all");
                                let pw = (width(label, F_LABEL) + 2.0 * CLEAR_PAD * s).min(cb.right - cb.left);
                                let pb = rect((cb.right - pw).round(), cb.top.round(), cb.right.round(), cb.bottom.round());
                                let radius = (pb.bottom - pb.top) / 2.0;
                                rt.FillRoundedRectangle(&rounded(pb, radius), &self.brush(grey(if hot { 0.28 } else { 0.16 }, a))?);
                                let edge = rect(pb.left + 0.5, pb.top + 0.5, pb.right - 0.5, pb.bottom - 0.5);
                                rt.DrawRoundedRectangle(&rounded(edge, radius - 0.5), &self.brush(grey(1.0, a * if hot { 0.35 } else { 0.22 }))?, 1.0, None);
                                text(label, F_LABEL, pb, center, grey(if hot { 1.0 } else { 0.85 }, a))?;
                            }
                        }
                        Page::Weather => {
                            draw_input(tr("Nhập thành phố (vd: Hà Nội, Đà Nẵng)", "Type a city (e.g. Hanoi, Da Nang)"))?;
                            let wx = v.weather;
                            match (&wx.place, wx.now, &weather_line) {
                                (Some(place), Some(n), Some((ic, geoms))) => {
                                    // Conditions older than STALE_MIN are dimmed and say so in words.
                                    let stale = wx.is_stale(lv.now);
                                    let fade = if stale { 0.55 } else { 1.0 };
                                    // City on the left; on the right where the data comes from (Open-Meteo asks
                                    // for the credit, CC BY 4.0) and when it was fetched, "old" when stale; the
                                    // x before it switches the weather off.
                                    text(&place.name, F_TITLE, rect(cl, 68.0 * s, cr - (CREDIT_W + 32.0) * s, 88.0 * s), lead, grey(1.0, a))?;
                                    let fetched = wx.updated.map_or(String::new(), |u| format!(" · {}{:02}:{:02}", if stale { format!("{} ", tr("cũ", "old")) } else { String::new() }, u / 100 % 100, u % 100));
                                    let credit = if stale { solid(ORANGE, a) } else { grey(0.5, a) };
                                    text(&format!("Open-Meteo{fetched}"), F_LABEL, rect(cr - CREDIT_W * s, 68.0 * s, cr, 88.0 * s), trail, credit)?;
                                    if let Some(ob) = rect_of(Button::CityOff) {
                                        let hot = hovered == Some(Button::CityOff);
                                        hover_disc(Button::CityOff)?;
                                        text(ICON_CLOSE, F_SMALL_ICON, ob, center, if hot { color(1.0, 0.45, 0.4, a) } else { grey(0.55, a) })?;
                                    }
                                    // The icon, then the temperature and the sky in words.
                                    draw_wx_icon(ic, geoms, cl + 4.0 * s, 92.0 * s, 56.0 * s, Some(weather::tints(n.sky, n.day)), a * fade, if current { v.page_age } else { 9.0 })?;
                                    // The big temperature, shrunk to fit before the unit chips when it is long
                                    // ("−128°"), turning about its left end.
                                    let temp = format!("{}°", v.unit.number(n.temp));
                                    let (tx, ty) = (cl + 72.0 * s, 108.0 * s);
                                    let k = (UNIT_X - 72.0 - 4.0) * s / width(&temp, F_BIG);
                                    let base = { let mut m = Matrix3x2::default(); rt.GetTransform(&mut m); m };
                                    rt.SetTransform(&(translate(-tx, -ty) * scaling(k.min(1.0)) * translate(tx, ty) * base));
                                    text(&temp, F_BIG, rect(tx, 92.0 * s, tx + width(&temp, F_BIG) + 6.0 * s, 124.0 * s), lead, grey(1.0, a * fade))?;
                                    rt.SetTransform(&base);
                                    // The unit to show it in: one capsule holding both, the chosen one lit, the
                                    // other lighting on hover.
                                    if let (Some(first), Some(last)) = (rect_of(Button::Unit(weather::Unit::C)), rect_of(Button::Unit(weather::Unit::F))) {
                                        rt.FillRoundedRectangle(&rounded(rect(first.left, first.top, last.right, last.bottom), UNIT_H / 2.0 * s), &self.brush(grey(0.1, a))?);
                                    }
                                    for unit in [weather::Unit::C, weather::Unit::F] {
                                        let Some(chip) = rect_of(Button::Unit(unit)) else { continue };
                                        let on = v.unit == unit;
                                        if on || hovered == Some(Button::Unit(unit)) {
                                            rt.FillRoundedRectangle(&rounded(chip, UNIT_H / 2.0 * s), &self.brush(grey(1.0, a * if on { 0.2 } else { 0.08 }))?);
                                        }
                                        text(unit.symbol(), F_LABEL, chip, center, grey(if on { 1.0 } else { 0.55 }, a))?;
                                    }
                                    text(weather::describe(n.sky), F_BODY, rect(cl + 72.0 * s, 124.0 * s, cl + 210.0 * s, 144.0 * s), lead, grey(0.7, a))?;
                                    // Feels like, humidity, wind.
                                    let rows = [
                                        (tr("Cảm giác", "Feels like"), format!("{}{}", v.unit.number(n.feels), v.unit.symbol())),
                                        (tr("Độ ẩm", "Humidity"), format!("{:.0}%", n.humidity)),
                                        (tr("Gió", "Wind"), format!("{:.0} km/h", n.wind)),
                                    ];
                                    for (k, (label, value)) in rows.iter().enumerate() {
                                        let y = (96.0 + k as f32 * 20.0) * s;
                                        text(label, F_LABEL, rect(cl + 222.0 * s, y, cl + 292.0 * s, y + 18.0 * s), lead, grey(0.55, a))?;
                                        text(value, F_LABEL, rect(cl + 288.0 * s, y, cr, y + 18.0 * s), trail, grey(0.95, a))?;
                                    }
                                }
                                _ => {
                                    let (first, second) = if wx.looking {
                                        (tr("Đang tìm thành phố…", "Looking for the city…").to_string(), "")
                                    } else if let Some(city) = &wx.not_found {
                                        (format!("{} “{city}”", tr("Không tìm thấy", "Not found:")), tr("Thử viết khác, hoặc tên tỉnh lân cận", "Try another spelling, or a nearby city"))
                                    } else if wx.place.is_some() {
                                        (tr("Đang lấy thời tiết…", "Getting the weather…").to_string(), "")
                                    } else {
                                        (tr("Nhập tên thành phố rồi nhấn Enter", "Type a city and press Enter").to_string(), tr("Có dấu hay không dấu đều được", "With or without accents"))
                                    };
                                    // A muted sun above the words, as the Tasks page has its ring.
                                    if let Some((ic, geoms)) = &weather_empty {
                                        let mid = (cl + cr) / 2.0;
                                        draw_wx_icon(ic, geoms, mid - 16.0 * s, 72.0 * s, 32.0 * s, Some(((0.4, 0.4, 0.4), (0.4, 0.4, 0.4))), a, 9.0)?;
                                    }
                                    text(&first, F_BODY, rect(cl, 110.0 * s, cr, 130.0 * s), center, grey(0.75, a))?;
                                    text(second, F_LABEL, rect(cl, 130.0 * s, cr, 150.0 * s), center, grey(0.5, a))?;
                                }
                            }
                        }
                        Page::Health => {
                            let hv = &v.health;
                            let h = hv.h;
                            for card in [Card::Water, Card::Sit] {
                                let x = if card == Card::Water { cl } else { cl + (CARD_W + CARD_GAP) * s };
                                let cb = rect(x, CARD_TOP * s, x + CARD_W * s, CARD_BOTTOM * s);
                                rt.FillRoundedRectangle(&rounded(cb, 14.0 * s), &self.brush(grey(0.11, a))?);
                                let accent = if card == Card::Water { BLUE } else { ORANGE };
                                // Title row: icon, name, gear.
                                let ty = (CARD_TOP + 4.0) * s;
                                let ib = rect(x + 10.0 * s, ty, x + 32.0 * s, ty + 22.0 * s);
                                if card == Card::Water {
                                    let g = self.water_drop((ib.left + ib.right) / 2.0, (ib.top + ib.bottom) / 2.0, 16.0 * s)?;
                                    rt.FillGeometry(&g, &self.brush(solid(accent, a))?, None);
                                } else {
                                    text(ICON_WALK, F_ICON, ib, center, solid(accent, a))?;
                                }
                                let title = if card == Card::Water { tr("Uống nước", "Water") } else { tr("Đứng dậy", "Stand up") };
                                text(title, F_TITLE, rect(ib.right + 4.0 * s, ty, x + CARD_W * s - 30.0 * s, ty + 22.0 * s), lead, grey(1.0, a))?;
                                let open = settings == Some(card);
                                if let Some(gb) = rect_of(Button::Gear(card)) {
                                    let hot = hovered == Some(Button::Gear(card));
                                    let (icon, c) = if open { (ICON_CHECK, solid(GREEN, a)) } else { (ICON_GEAR, grey(if hot { 1.0 } else { 0.6 }, a)) };
                                    hover_disc(Button::Gear(card))?;
                                    micon(if card == Card::Water { "gear_water" } else { "gear_sit" }, icon, F_SMALL_ICON, gb, c)?;
                                }
                                if open {
                                    for (k, &(set, switch)) in setting_rows(card).iter().enumerate() {
                                        let y = (SET_Y + k as f32 * SET_ROW) * s;
                                        let row = rect(x + 12.0 * s, y, x + CARD_W * s - 12.0 * s, y + SET_ROW * s);
                                        let label = match set {
                                            Setting::WaterOn | Setting::SitOn => tr("Nhắc", "Remind"),
                                            Setting::Cup => tr("Mỗi lần", "Per drink"),
                                            Setting::Goal => tr("Mục tiêu", "Goal"),
                                            Setting::From => tr("Từ", "From"),
                                            Setting::To => tr("Đến", "To"),
                                            Setting::SitEvery => tr("Mỗi", "Every"),
                                        };
                                        text(label, F_LABEL, row, lead, grey(0.65, a))?;
                                        if switch {
                                            if let Some(sb) = rect_of(Button::Toggle(set)) {
                                                let on = if set == Setting::WaterOn { h.water_on } else { h.sit_on };
                                                let rad = (sb.bottom - sb.top) / 2.0;
                                                rt.FillRoundedRectangle(&rounded(sb, rad), &self.brush(if on { solid(GREEN, a) } else { grey(0.3, a) })?);
                                                let kx = if on { sb.right - rad } else { sb.left + rad };
                                                let knob = D2D1_ELLIPSE { point: Vector2 { X: kx, Y: (sb.top + sb.bottom) / 2.0 }, radiusX: rad - 2.0 * s, radiusY: rad - 2.0 * s };
                                                rt.FillEllipse(&knob, &self.brush(grey(1.0, a))?)
                                            }
                                        } else {
                                            let value = match set {
                                                Setting::Cup => format!("{} ml", h.cup_ml),
                                                Setting::Goal => format!("{:.1} L", h.goal_ml as f32 / 1000.0),
                                                Setting::From => health::hhmm(h.from_min),
                                                Setting::To => health::hhmm(h.to_min),
                                                _ => format!("{} {}", h.sit_every_min, tr("phút", "min")),
                                            };
                                            if let (Some(mb), Some(pb)) = (rect_of(Button::Step(set, -1)), rect_of(Button::Step(set, 1))) {
                                                for (bb, icon, dir) in [(mb, ICON_MINUS, -1), (pb, ICON_PLUS, 1)] {
                                                    let hot = hovered == Some(Button::Step(set, dir));
                                                    let c = D2D1_ELLIPSE { point: Vector2 { X: (bb.left + bb.right) / 2.0, Y: (bb.top + bb.bottom) / 2.0 }, radiusX: 8.0 * s, radiusY: 8.0 * s };
                                                    rt.FillEllipse(&c, &self.brush(grey(if hot { 0.32 } else { 0.2 }, a))?);
                                                    text(icon, F_TINY_ICON, bb, center, grey(1.0, a))?;
                                                }
                                                text(&value, F_LABEL, rect(mb.right, mb.top, pb.left, pb.bottom), center, grey(1.0, a))?;
                                            }
                                        }
                                    }
                                    if card == Card::Sit {
                                        let y = (SET_Y + 2.0 * SET_ROW) * s;
                                        text(tr("Rời máy ≥ 5 phút: tính lại", "Away ≥ 5 min: starts over"), F_LABEL, rect(x + 12.0 * s, y, x + CARD_W * s - 8.0 * s, y + SET_ROW * s), lead, grey(0.5, a))?;
                                    }
                                    continue;
                                }
                                // Big number + unit, progress bar, status line, action button.
                                let (big, unit, frac, status, action) = if card == Card::Water {
                                    let status = if !h.water_on { tr("Đang tắt", "Off").to_string() }
                                        else if h.taken_ml >= h.goal_ml { tr("Đạt mục tiêu hôm nay", "Goal reached today").to_string() }
                                        else if hv.minute_of_day < h.from_min { format!("{} {}", tr("Bắt đầu nhắc lúc", "Reminders start at"), health::hhmm(h.from_min)) }
                                        else {
                                            match h.next_drink(hv.now, hv.minute_of_day) {
                                                Some(at) => format!("{} {}", tr("Nhắc lúc", "Next at"), health::hhmm((at - (hv.now - hv.minute_of_day as i64)).max(0) as u32)),
                                                None => tr("Ngoài giờ nhắc", "Outside reminder hours").to_string(),
                                            }
                                        };
                                    (h.taken_ml.to_string(), format!(" / {} ml", h.goal_ml), h.taken_ml as f32 / h.goal_ml.max(1) as f32, status, format!("+ {} ml", h.cup_ml))
                                } else {
                                    let sat = h.sitting_minutes(hv.now);
                                    let status = if !h.sit_on { tr("Đang tắt", "Off").to_string() }
                                        else if h.sitting_since.is_none() { tr("Đang rời máy", "Away from the PC").to_string() }
                                        else { format!("{} {} {}", tr("Nhắc sau", "Reminder in"), (h.sit_every_min as i64 - sat).max(0), tr("phút", "min")) };
                                    (sat.to_string(), format!(" {}", tr("phút ngồi", "min sitting")), sat as f32 / h.sit_every_min.max(1) as f32, status, tr("Đã đứng dậy", "I stood up").to_string())
                                };
                                let by = 60.0 * s;
                                let bw = width(&big, F_BIG);
                                text(&big, F_BIG, rect(x + 12.0 * s, by, x + CARD_W * s - 8.0 * s, by + 28.0 * s), lead, grey(1.0, a))?;
                                text(&unit, F_LABEL, rect(x + 12.0 * s + bw + 2.0 * s, by + 6.0 * s, x + CARD_W * s - 8.0 * s, by + 26.0 * s), lead, grey(0.6, a))?;
                                let (bl, br_) = (x + 12.0 * s, x + CARD_W * s - 12.0 * s);
                                let bar = |right: f32| rounded(rect(bl, 94.0 * s, right, 98.0 * s), 2.0 * s);
                                rt.FillRoundedRectangle(&bar(br_), &self.brush(grey(0.25, a))?);
                                rt.FillRoundedRectangle(&bar(bl + (br_ - bl) * frac.clamp(0.0, 1.0)), &self.brush(solid(accent, a))?);
                                text(&status, F_LABEL, rect(bl, 101.0 * s, br_, 119.0 * s), lead, grey(0.65, a))?;
                                let ab = if card == Card::Water { Button::Drink } else { Button::Stood };
                                if let Some(pb) = rect_of(ab) {
                                    let hot = hovered == Some(ab);
                                    let k = if hot { 1.0 } else { 0.82 };
                                    rt.FillRoundedRectangle(&rounded(pb, 12.0 * s), &self.brush(color(accent.0 * k, accent.1 * k, accent.2 * k, a))?);
                                    // Dark label on the orange button: white on it is only 3.1:1.
                                    text(&action, F_SMALL, pb, center, if card == Card::Sit { grey(0.06, a) } else { grey(1.0, a) })?;
                                }
                                if let Some(sb) = rect_of(Button::Snooze) {
                                    if card == Card::Water {
                                        let hot = hovered == Some(Button::Snooze);
                                        rt.FillRoundedRectangle(&rounded(sb, 12.0 * s), &self.brush(grey(if hot { 0.3 } else { 0.2 }, a))?);
                                        text(tr("Để sau", "Later"), F_LABEL, sb, center, grey(1.0, a))?;
                                    }
                                }
                            }
                        }
                    }
                    rt.SetTransform(&Matrix3x2 { M11: 1.0, M12: 0.0, M21: 0.0, M22: 1.0, M31: 0.0, M32: 0.0 });
                    Ok(())
                };

                // Pages slide inside the space between the arrows.
                rt.PushAxisAlignedClip(&rect(l + PAGE_CLIP * s, 30.0 * s, r - PAGE_CLIP * s, (DOTS_Y - 3.0) * s), D2D1_ANTIALIAS_MODE_ALIASED);
                match v.leaving {
                    Some((from, t, dir)) => {
                        let span = (cr - cl) * 0.55;
                        let d = dir as f32;
                        let (out_a, in_a) = page_fades(t);
                        draw_page(from, -d * span * t.min(1.0), out_a)?;
                        draw_page(v.page, d * span * (1.0 - ease_out_back(t)), in_a)?;
                    }
                    None => draw_page(v.page, 0.0, 1.0)?,
                }
                rt.PopAxisAlignedClip();

                // Side arrows and page dots.
                // They sit on the panel's own edges, so they follow it while it widens or narrows.
                let nav = [
                    (Button::PagePrev, rect(l + 10.0 * s, ARROW_Y * s, l + (10.0 + ARROW) * s, (ARROW_Y + ARROW) * s)),
                    (Button::PageNext, rect(r - (10.0 + ARROW) * s, ARROW_Y * s, r - 10.0 * s, (ARROW_Y + ARROW) * s)),
                ];
                // While the lyrics are open the arrows are hidden (they fade as the panel widens), so the
                // way back is the lyrics button: shrinking the panel never leaves the mouse outside it.
                let arrow_a = a * (1.0 - ((v.size[0] - 440.0) / notch::LYRICS_EXTRA_W).clamp(0.0, 1.0));
                for (b, icon) in [(Button::PagePrev, ICON_LEFT), (Button::PageNext, ICON_RIGHT)] {
                    if arrow_a < 0.01 {
                        break;
                    }
                    let a = arrow_a;
                    if let Some((_, br)) = nav.iter().find(|(x, _)| *x == b) {
                        let hot = v.hovered == Some(b);
                        let circle = D2D1_ELLIPSE {
                            point: Vector2 { X: (br.left + br.right) / 2.0, Y: (br.top + br.bottom) / 2.0 },
                            radiusX: (br.right - br.left) / 2.0,
                            radiusY: (br.bottom - br.top) / 2.0,
                        };
                        rt.FillEllipse(&circle, &self.brush(grey(if hot { 0.26 } else { 0.15 }, a))?);
                        text(icon, F_SMALL_ICON, *br, center, grey(if hot { 1.0 } else { 0.75 }, a))?;
                    }
                }
                let (dot, wide, gap) = (5.0 * s, 14.0 * s, 5.0 * s);
                let total = wide + (PAGES.len() as f32 - 1.0) * (dot + gap);
                let mut dx = cx - total / 2.0;
                for p in PAGES {
                    let on = p == v.page;
                    let dw = if on { wide } else { dot };
                    rt.FillRoundedRectangle(&rounded(rect(dx, DOTS_Y * s, dx + dw, DOTS_Y * s + dot), dot / 2.0), &self.brush(if on { grey(1.0, a) } else { grey(0.45, a) })?);
                    dx += dw + gap;
                }
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
            let region = (((l - ear - reach).floor() as i32).max(0), 0, ((r + ear + reach).ceil() as i32).min(w), ((nh + reach).ceil() as i32).min(h));
            self.place(hwnd, (x, y, w, h), region);
            // Poll the media clock between lines; the spring requests vsync until it settles,
            // including after pausing in the middle of a transition.
            let lyrics_run = !lyric_heights.is_empty() && v.music.is_some_and(|t| t.playing);
            self.slow_frames = lyrics_run;
            // Icon completion posts WM_ICONS; waiting for I/O never keeps animation alive.
            // The Weather page's icon moves for a moment when the panel or the page opens.
            let wx_busy = v.page == Page::Weather && v.mode == Mode::Expanded && v.page_age < icons::DUR;
            Ok(morphing.get() || glow_busy || bars_busy || icon_busy || wx_busy || lyric_moving)
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
                weather: &weather::State::default(), page_age: 9.0, unit: weather::Unit::C,
                list: ListView { counts: (0, 0), remind_before: 15, now: 0, draft: "", editing: false, edit_item: None, caret: false, scroll: 0, note_pos: 0.0 },
                health: HealthView { h: &Health::default(), now: 0, minute_of_day: 0, settings: None } };
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
        // The shadow reaches PAD past the dropped cover; pages are cut at DOTS_Y - 3.
        let bottom = COVER_TOP + COVER + COVER_SHADOW_DROP + COVER_SHADOW_PAD;
        assert!(bottom <= DOTS_Y - 3.0, "shadow ends at {bottom}, clipped at {}", DOTS_Y - 3.0);
    }

    #[test]
    fn lyrics_fit_inside_the_page_clip() {
        // Right edge of the lyrics box, from the panel's left: past the controls, the divider gaps
        // and the column. A last letter's ink may run a few px further and must not be clipped.
        let right = target(Mode::Expanded, false)[0] - SIDE + 2.0 * LYRIC_GAP + LYRIC_W;
        assert!(right + 4.0 <= panel_w(true) - PAGE_CLIP, "{right} vs {}", panel_w(true) - PAGE_CLIP);
    }

    #[test]
    fn aurora_colours_are_capped_but_not_black() {
        for tint in [(1.0, 1.0, 0.0), (1.0, 1.0, 1.0), (0.0, 1.0, 1.0), (0.0, 1.0, 0.0), (1.0, 0.2, 0.2), (0.2, 0.4, 1.0)] {
            let (r, g, b) = dim_to_lum(tint, AURORA_MAX_LUM);
            assert!(luminance(r, g, b) <= AURORA_MAX_LUM, "{tint:?}: too bright");
            assert!(r.max(g).max(b) > 0.3, "{tint:?}: went black");
        }
    }

    #[test]
    fn aurora_takes_the_covers_second_colour() {
        // Half blue, half pink (premultiplied BGRA, opaque).
        let mut px = Vec::new();
        for i in 0..64 {
            px.extend_from_slice(if i < 32 { &[255, 140, 90, 255] } else { &[170, 70, 255, 255] });
        }
        let [a, b] = aurora_tints(&px);
        assert_ne!(a, b);
        let blue = |c: (f32, f32, f32)| c.2 > c.0;
        assert!(blue(a) != blue(b), "one blue, one pink: {a:?} {b:?}");
        // One colour only: the second is the first turned, not a copy.
        let [c, d] = aurora_tints(&[0, 0, 255, 255].repeat(64));
        assert_eq!(c, (1.0, 0.0, 0.0));
        assert!(d != c && d.0 > 0.5, "{d:?}");
        assert_eq!(aurora_tints(&[128, 128, 128, 255].repeat(64))[0], GLOW_NEUTRAL, "grey cover");
    }

    #[test]
    fn aurora_band_stays_in_range_and_moves() {
        for i in 0..=40 {
            let (t, u) = (i as f32 * 0.7, i as f32 / 40.0);
            let (c, w) = aurora_at(t, u);
            assert!((-1.0..=1.0).contains(&c) && (0.1 - 1e-4..=1.0 + 1e-4).contains(&w), "t {t} u {u}: {c} {w}");
        }
        assert_ne!(aurora_at(0.0, 0.3), aurora_at(1.0, 0.3), "it moves with time");
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
    fn page_change_never_shows_both_pages_strongly() {
        assert_eq!(page_fades(0.0), (1.0, 0.0));
        assert_eq!(page_fades(1.0), (0.0, 1.0));
        for i in 0..=100 {
            let (out_a, in_a) = page_fades(i as f32 / 100.0);
            assert!(out_a * in_a <= 0.05, "t={}: old {out_a:.2} over new {in_a:.2}", i as f32 / 100.0);
        }
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
            assert!(renderer.cover(&track, 120, 20).is_some());
        }
    }

    #[test]
    fn seek_and_icon_hit_areas_follow_dpi_and_capabilities() {
        let track = Track { duration_ms: 120_000, can_seek: true, can_toggle: true, can_previous: true, can_next: true,
            lyrics: Arc::new(vec![(0, "la la".into())]), ..Default::default() };
        for scale in [1.0, 1.5, 2.0] {
            let cx = 265.0 * scale;
            let [(_, prev), (_, play), (_, next), (_, seek), (_, lyr)] = buttons(cx, scale, false);
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
            assert!(buttons(cx, scale, false).iter().all(|(_, r)| r.bottom <= DOTS_Y * scale));
            // Controls live in the right column: right of the cover, left of the side arrow, no overlaps.
            let left = cx - 220.0 * scale;
            let cover_right = left + (SIDE + COVER) * scale;
            for r in [prev, play, next, seek, lyr] {
                assert!(r.left > cover_right && r.right <= left + (440.0 - SIDE) * scale, "{r:?}");
            }
            assert!(prev.right <= play.left && play.right <= next.left && next.right <= lyr.left, "buttons must not overlap");
            assert!(seek.bottom <= prev.top.min(play.top).min(lyr.top), "bar above the buttons");
            // The lyrics button works only when the song has lyrics.
            let bare = Track { lyrics: Arc::new(Vec::new()), ..track.clone() };
            assert_eq!(hit_button(cx, scale, mid(lyr).0, mid(lyr).1, Some(&bare), Page::Music, 0, 0, None, false), None);
        }
    }

    #[test]
    fn clear_all_sits_under_the_rows_clear_of_the_bottom_edge() {
        let [_, panel_h, _] = target(Mode::Expanded, false);
        assert!(CLEAR_Y >= NOTE_Y + NOTE_H * NOTE_ROWS as f32, "below the last row");
        assert!(panel_h - (CLEAR_Y + CLEAR_H) >= 12.0, "well inside the panel's bottom edge");
        assert!(CLEAR_Y + CLEAR_H + 1.0 <= DOTS_Y - 3.0, "whole, border included, inside the pages' clip");
        for scale in [1.0, 1.5] {
            let cx = 265.0 * scale;
            let (x, y) = (cx + (220.0 - SIDE - 10.0) * scale, (CLEAR_Y + CLEAR_H / 2.0) * scale);
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
            let (_, play) = wide[1];
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
                leaving: None, todos: &[], notes: &[], weather: &weather::State::default(), page_age: 0.0, unit: weather::Unit::C,
                list: ListView { counts: (0, 0), remind_before: 15, now: 0, draft: "", editing: false, edit_item: None, caret: false, scroll: 0, note_pos: 0.0 },
                health: HealthView { h: &Health::default(), now: 0, minute_of_day: 0, settings: None } };
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
                leaving: None, todos: &[], notes: &[], weather: &weather::State::default(), page_age: 0.0, unit: weather::Unit::C,
                list: ListView { counts: (0, 0), remind_before: 15, now: 0, draft: "", editing: false, edit_item: None, caret: false, scroll: 0, note_pos: 0.0 },
                health: HealthView { h: &Health::default(), now: 0, minute_of_day: 0, settings: None } };
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
                    leaving: None, todos: items, notes: &[], weather: &weather::State::default(), page_age: 9.0, unit: weather::Unit::C,
                    list: ListView { counts: *counts, remind_before: 15, now, draft: typed, editing: !typed.is_empty(), edit_item: None, caret: true, scroll: 0, note_pos: 0.0 },
                    health: HealthView { h: &Health::default(), now: 0, minute_of_day: 0, settings: None } };
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
                    leaving: None, todos: &[], notes: &[], weather: st, page_age: 9.0, unit: *unit,
                    list: ListView { counts: (0, 0), remind_before: 15, now: 202610290205, draft: typed, editing: !typed.is_empty(), edit_item: None, caret: true, scroll: 0, note_pos: 0.0 },
                    health: HealthView { h: &Health::default(), now: 0, minute_of_day: 0, settings: None } };
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
    #[ignore = "writes target/banner-preview.bmp: a notification banner, for the README picture"]
    fn banner_preview() {
        unsafe {
            use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let (w, h) = (500i32, 110i32);
            let hwnd = CreateWindowExW(WS_EX_NOREDIRECTIONBITMAP, w!("STATIC"), w!("Banner preview"), WS_POPUP, 0, 0, w, h, None, None, None, None).unwrap();
            let toast = Toast { app: "powershell".into(), title: "Anh Minh".into(), body: "Tối nay 8h họp nhóm nhé, nhớ mang laptop.".into(), level: None, icon: None, image: None, open: None };
            let view = View { mode: Mode::Alert, size: notch::alert_size(1), time: "20:49", date: "T2, 26/10", battery: Some(Battery { percent: 76, charging: true, saver: false }),
                banner: Some(&toast), music: None, live: false, robot: None, phase: 0.0, banner_age: 3.0, lyrics: false, hovered: None, page: Page::Music,
                leaving: None, todos: &[], notes: &[], weather: &weather::State::default(), page_age: 0.0, unit: weather::Unit::C,
                list: ListView { counts: (0, 0), remind_before: 15, now: 0, draft: "", editing: false, edit_item: None, caret: false, scroll: 0, note_pos: 0.0 },
                health: HealthView { h: &Health::default(), now: 0, minute_of_day: 0, settings: None } };
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
                    phase: t, banner_age: 0.0, lyrics: false, hovered: None, page: Page::Music, leaving: None, todos: &[], notes: &[], weather: &weather::State::default(), page_age: 0.0, unit: weather::Unit::C,
                    list: ListView { counts: (0, 0), remind_before: 15, now: 0, draft: "", editing: false, edit_item: None, caret: false, scroll: 0, note_pos: 0.0 },
                    health: HealthView { h: &Health::default(), now: 0, minute_of_day: 0, settings: None } };
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
    #[ignore = "writes target/readme/<name>.bmp (900 x 230, the notch centred at the top): the README pictures of the pill and the pages"]
    fn readme_shots() {
        use weather::{Now, Place, Sky, State};
        unsafe {
            use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let (w, h) = (900, 230);
            let hwnd = CreateWindowExW(WS_EX_NOREDIRECTIONBITMAP, w!("STATIC"), w!("README shots"), WS_POPUP, 0, 0, w, h, None, None, None, None).unwrap();
            let lines = [
                // Made-up lines about the feature itself (no song's lyrics in the README pictures).
                (0, "Lời mẫu cho ảnh xem trước"), (6_000, "Lời lấy từ LRCLIB và chạy theo đúng thời gian của bài đang phát trên máy"),
                (12_000, "Dòng đang hát sáng lên ở giữa"), (18_000, "Dòng dài tự xuống dòng cho vừa khung lời bên phải, không bị cắt mất chữ"),
                (24_000, "Bấm nút lời để mở hoặc đóng khung này"), (30_000, "Tua bài thì lời nhảy theo"),
            ];
            // A cover image for the glow and the cover slot: PUW_PREVIEW_COVER=path.
            let art = std::env::var("PUW_PREVIEW_COVER").ok().and_then(|p| std::fs::read(p).ok()).map(Arc::new);
            let song = |position_ms| Track { title: "Bài hát mẫu".into(), artist: "Nghệ sĩ mẫu".into(), duration_ms: 210_000, position_ms, playing: true,
                can_toggle: true, can_next: true, can_previous: true, can_seek: true, art: art.clone(), art_key: 1,
                lyrics: Arc::new(lines.iter().map(|(t, s)| (*t as u64, s.to_string())).collect()), ..Default::default() };
            let (playing, singing) = (song(64_000), song(14_000));
            let place = Some(Place { name: "Đà Nẵng".into(), lat: 16.0, lon: 108.0 });
            let sky = |sky, temp: f32| State { place: place.clone(), now: Some(Now { temp, feels: temp + 3.0, humidity: 82.0, wind: 11.0, sky, day: true }), updated: Some(202610290140), ..State::default() };
            let (none, partly, rain) = (State::default(), sky(Sky::PartlyCloudy, 29.0), sky(Sky::Rain, 26.0));
            let partly_30 = sky(Sky::PartlyCloudy, 30.0);
            let mut health = Health::default();
            (health.water_on, health.sit_on) = (false, false);
            let charging = |percent| Some(Battery { percent, charging: true, saver: false });
            let on_battery = Some(Battery { percent: 76, charging: false, saver: false });
            // (file, mode, music, page, lyrics open, weather, battery, time, date)
            let shots: [(&str, Mode, Option<&Track>, Page, bool, &State, Option<Battery>, &str, &str); 9] = [
                ("pill-idle", Mode::Collapsed, None, Page::Music, false, &none, charging(76), "20:50", ""),
                ("pill-music", Mode::Collapsed, Some(&playing), Page::Music, false, &none, charging(76), "20:49", ""),
                ("weather-pill", Mode::Collapsed, None, Page::Music, false, &partly, on_battery, "02:05", ""),
                ("weather-pill-music", Mode::Collapsed, Some(&playing), Page::Music, false, &rain, on_battery, "02:05", ""),
                ("page-music", Mode::Expanded, Some(&playing), Page::Music, false, &none, charging(70), "23:16", "T7, 24/10"),
                ("page-music-lyrics", Mode::Expanded, Some(&singing), Page::Music, true, &none, charging(70), "23:16", "T7, 24/10"),
                ("page-music-empty", Mode::Expanded, None, Page::Music, false, &none, charging(76), "20:55", "T2, 26/10"),
                ("page-health", Mode::Expanded, None, Page::Health, false, &none, charging(76), "20:49", "T2, 26/10"),
                ("page-weather", Mode::Expanded, None, Page::Weather, false, &partly_30, on_battery, "02:05", "T5, 29/10"),
            ];
            let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/readme");
            std::fs::create_dir_all(&dir).unwrap();
            for (name, mode, music, page, lyrics, wx, battery, time, date) in shots {
                let live = music.is_some();
                let mut size = target(mode, live);
                if lyrics {
                    size[0] += LYRICS_EXTRA_W;
                }
                let view = View { mode, size, time, date, battery, banner: None, music, live, robot: None, phase: 1.3, banner_age: 0.0, lyrics, hovered: None, page,
                    leaving: None, todos: &[], notes: &[], weather: wx, page_age: 9.0, unit: weather::Unit::C,
                    list: ListView { counts: (0, 0), remind_before: 15, now: 202610290205, draft: "", editing: false, edit_item: None, caret: false, scroll: 0, note_pos: 0.0 },
                    health: HealthView { h: &health, now: 0, minute_of_day: 0, settings: None } };
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
                    leaving: None, todos: &[], notes: &[], weather: &weather::State::default(), page_age: 0.0, unit: weather::Unit::C,
                    list: ListView { counts: (0, 0), remind_before: 15, now: 0, draft: "", editing: false, edit_item: None, caret: false, scroll: 0, note_pos: 0.0 },
                    health: HealthView { h: &Health::default(), now: 0, minute_of_day: 0, settings: None } };
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
                weather: &weather::State::default(), page_age: 0.0, unit: weather::Unit::C,
                list: ListView { counts: (0, 0), remind_before: 15, now: 202610291600, draft: "", editing: false, edit_item: None, caret: false, scroll: 0, note_pos: 0.0 },
                health: HealthView { h: &Health::default(), now: 0, minute_of_day: 0, settings: None } };
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
                leaving: None, todos: &[], notes: &[], weather: &weather::State::default(), page_age: 0.0, unit: weather::Unit::C,
                list: ListView { counts: (0, 0), remind_before: 15, now: 0, draft: "", editing: false, edit_item: None, caret: false, scroll: 0, note_pos: 0.0 },
                health: HealthView { h: &Health::default(), now: 0, minute_of_day: 0, settings: None } };
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
            level: None, icon: None, image: None, open: None };
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
                    let lines = renderer.banner_lines(&toast.body, scale, false).min(notch::BANNER_MAX_LINES) as u8;
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
                let eyes = robot.visible().then(|| robot.step(1.0 / fps, None, Mood::Charging));
                let track = song(64_000 + ((now - 4.0).max(0.0) * 1000.0) as u64);
                let mode = notch.mode;
                let view = View { mode, size: notch.size(), time: "09:41", date: &date, battery: Some(Battery { percent: 82, charging, saver: false }),
                    banner: (mode == Mode::Alert).then_some(&toast), music: live.then_some(&track), live: notch.live, robot: eyes, phase: now,
                    banner_age: (now - 19.6).max(0.0), lyrics: false,
                    hovered: (mode == Mode::Expanded && (10.0..18.2).contains(&now)).then_some(Button::PageNext), page, leaving,
                    list: ListView { counts: (3, 1), remind_before: 15, now: 202610290941, draft: "", editing: false, edit_item: None, caret: false, scroll: 0, note_pos: 0.0 },
                    todos: &todos, notes: &notes, health: HealthView { h: &health, now: crate::todo::minutes(202610290941), minute_of_day: 9 * 60 + 41, settings: None },
                    weather: &wx, page_age: if mode == Mode::Expanded { now - page_since } else { 0.0 }, unit: weather::Unit::C };
                renderer.present(hwnd, 0, 0, w, h, scale, &view).unwrap();
                pipe.write_all(&renderer.grabbed).unwrap();
            }
            DestroyWindow(hwnd).unwrap();
        }
        drop(enc.stdin.take());
        assert!(enc.wait().unwrap().success());
    }
}

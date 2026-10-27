//! Direct2D drawing into a 32-bit DIB, shown with UpdateLayeredWindow (per-pixel alpha).
//! Pixels with alpha 0 let the mouse through, so the drawn shape is the hit area.

use windows::core::{w, Result, HSTRING, PCWSTR};
use windows::Win32::UI::Shell::{IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_BIGGERSIZEOK, SIIGBF_ICONONLY};
use windows::Win32::Foundation::{COLORREF, HWND, POINT, RECT, SIZE};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows_numerics::{Matrix3x2, Vector2};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use crate::connectivity::DeviceKind;
use crate::icons::{self, Seg};
use crate::lyrics;
use crate::music::Track;
use crate::notch::{self, target, Mode, LYRICS_EXTRA_W};
use crate::notify::{Note, Toast};
use crate::system::Battery;
use crate::health::{self, Health, Setting};
use crate::i18n::tr;
use crate::robot::{curious_eyes, Eyes, Face};
use crate::todo::{self, Item};

/// Everything the painter needs for one frame (logical px).
pub struct View<'a> {
    pub mode: Mode,
    pub size: [f32; 3],
    pub time: &'a str,
    /// Short date for the panel header ("CN, 25/10").
    pub date: &'a str,
    /// None on a desktop PC: no ring, no label.
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
}

pub const PAGES: [Page; 4] = [Page::Music, Page::Todo, Page::Notify, Page::Health];

impl Page {
    /// Header of the page: icon, name, accent colour.
    fn title(self) -> (&'static str, &'static str, (f32, f32, f32)) {
        match self {
            Page::Music => ("\u{E8D6}", tr("Nhạc", "Music"), (0.72, 0.52, 1.0)),
            Page::Todo => ("\u{E9D5}", tr("Việc", "Tasks"), GREEN),
            Page::Notify => (ICON_BELL, tr("Thông báo", "Notifications"), BLUE),
            Page::Health => ("\u{E95E}", tr("Sức khỏe", "Health"), (1.0, 0.42, 0.5)),
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
const COVER_GAP: f32 = 16.0;
const PLAY_BTN: f32 = 38.0;
const SKIP_BTN: f32 = 32.0;
const LYRICS_BTN: f32 = 28.0;
const CONTROLS_Y: f32 = 148.0;
const BAR_Y: f32 = 108.0;
/// Page content sits between the side arrows (logical px from the panel edge).
const SIDE: f32 = 44.0;
const ARROW: f32 = 24.0;
const ARROW_Y: f32 = 80.0;
const DOTS_Y: f32 = 174.0;
/// Task list page.
const INPUT_Y: f32 = 36.0;
const INPUT_H: f32 = 26.0;
const COUNT_Y: f32 = 66.0;
const ROW_Y: f32 = 84.0;
const ROW_H: f32 = 22.0;
pub const LIST_ROWS: usize = 3;
/// Notifications page: rows of two lines.
const NOTE_Y: f32 = 36.0;
const NOTE_H: f32 = 42.0;
pub const NOTE_ROWS: usize = 3;
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
            out.push((Button::Gear(Card::Tasks), r(cl + cw - 20.0, COUNT_Y - 2.0, 20.0, 18.0)));
            if settings == Some(Card::Tasks) {
                out.push((Button::RemindStep(-1), r(cl + cw - 108.0, ROW_Y + 2.0, 18.0, 18.0)));
                out.push((Button::RemindStep(1), r(cl + cw - 22.0, ROW_Y + 2.0, 18.0, 18.0)));
            } else {
                for (row, i) in (scroll..rows.min(scroll + LIST_ROWS)).enumerate() {
                    let y = ROW_Y + row as f32 * ROW_H;
                    out.push((Button::ListToggle(i), r(cl, y, cw - 48.0, ROW_H)));
                    out.push((Button::ListEdit(i), r(cl + cw - 46.0, y, 22.0, ROW_H)));
                    out.push((Button::ListDelete(i), r(cl + cw - 22.0, y, 22.0, ROW_H)));
                }
            }
        }
        // Read only: the arrows and the wheel.
        Page::Notify => {}
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
/// A line of lyrics: wraps onto more lines instead of being cut short.
const F_LYRIC: usize = 10;
/// Lyrics area beside the player, logical px: its top and bottom in the panel, and its width.
const LYRIC_TOP: f32 = 30.0;
const LYRIC_BOTTOM: f32 = 168.0;
const LYRIC_GAP: f32 = 6.0;
const LYRIC_W: f32 = LYRICS_EXTRA_W - LYRIC_GAP - SIDE;

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
const ICON_BOLT: &str = "\u{E945}";
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

/// The cover's shadow: strokes of growing width centred on its edge (the inner half hides under the
/// cover), with the darkness of each from the narrowest out, and the step in px.
const COVER_SHADOW: [f32; 3] = [0.14, 0.09, 0.05];
const COVER_SHADOW_STEP: f32 = 0.8;

/// Brightest the glow may get at the bottom of the panel: white text on it stays above 4.5:1.
const GLOW_MAX_LUM: f32 = 0.17;

/// Glow colour at the panel's bottom: the cover's tint, dimmed until white controls read on it.
fn glow_bottom((r, g, b): (f32, f32, f32)) -> (f32, f32, f32) {
    let mut k = 0.75;
    while k > 0.05 && luminance(r * k, g * k, b * k) > GLOW_MAX_LUM {
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

/// Tint for covers with no colour to speak of (black, white, grey): a light grey.
const GLOW_NEUTRAL: (f32, f32, f32) = (0.8, 0.8, 0.8);

/// A small copy of the cover (premultiplied BGRA pixels) -> glow colour: its most prominent hue
/// at full brightness. Pixels vote for their 30-degree hue bin by how vivid and bright they are;
/// the winning bin's colours are averaged, so one strong colour beats a muddy overall average.
/// Covers that are nearly all black or grey get GLOW_NEUTRAL.
fn glow_tint(pixels: &[u8]) -> (f32, f32, f32) {
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
    let (weight, sum) = bins.into_iter().fold((0.0, [0.0; 3]), |best, bin| if bin.0 > best.0 { bin } else { best });
    // Under ~1% of the pixels at a pale tint: not enough colour to bother with.
    if weight <= pixels.len() as f32 / 4.0 * 0.0005 {
        return GLOW_NEUTRAL;
    }
    let [r, g, b] = sum.map(|c| c / weight);
    let max = r.max(g).max(b);
    (r / max, g / max, b / max)
}

pub struct Renderer {
    factory: ID2D1Factory,
    rt: ID2D1DCRenderTarget,
    dw: IDWriteFactory,
    wic: Option<IWICImagingFactory>,
    mem: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    w: i32,
    h: i32,
    fonts: Option<(f32, Vec<IDWriteTextFormat>)>,
    /// Decoded cover of the current track (keyed by Track::art_key).
    cover: Option<CoverCache>,
    /// The Music page's glow: (colour, strength 0..1, time of the last frame), see glow_step.
    glow: ((f32, f32, f32), f32, Instant),
    /// The last frame showed the lyrics scrolling with the song: frames at a relaxed pace will do.
    pub slow_frames: bool,
    /// Geometry of each line-icon path, by its path data.
    line_geoms: HashMap<&'static str, ID2D1PathGeometry>,
    /// Height of each lyric line wrapped to the lyrics area, for one song (the lyrics' address)
    /// and width: measured once, not every frame.
    lyric_heights: Option<(usize, u32, Vec<f32>)>,
    /// Start-menu icon per app id (None: Windows has none for it); asked once per app.
    app_icons: HashMap<String, Option<ID2D1Bitmap>>,
    /// Icon slots that just changed glyph: (current, previous, when) for the morph.
    morphs: RefCell<HashMap<&'static str, (&'static str, &'static str, Instant)>>,
    /// Round caps and joins for the robot face's strokes.
    round: ID2D1StrokeStyle,
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

impl Renderer {
    pub fn new() -> Result<Self> {
        unsafe {
            let factory: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                // Software: a 530x210 DC target draws faster on the CPU (1.0 vs 2.0 ms a frame) and skips
                // loading the GPU driver (private RAM 9.4 vs 27.9 MB), measured 2026-10-25.
                r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                dpiX: 96.0,
                dpiY: 96.0,
                usage: D2D1_RENDER_TARGET_USAGE_NONE,
                minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
            };
            let rt = factory.CreateDCRenderTarget(&props)?;
            rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let wic = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok();
            let mem = CreateCompatibleDC(None);
            let round = factory.CreateStrokeStyle(&D2D1_STROKE_STYLE_PROPERTIES {
                startCap: D2D1_CAP_STYLE_ROUND, endCap: D2D1_CAP_STYLE_ROUND, dashCap: D2D1_CAP_STYLE_ROUND,
                lineJoin: D2D1_LINE_JOIN_ROUND, miterLimit: 1.0, dashStyle: D2D1_DASH_STYLE_SOLID, dashOffset: 0.0,
            }, None)?;
            Ok(Renderer { factory, rt, dw, wic, mem, bmp: HBITMAP::default(), old: HGDIOBJ::default(), w: 0, h: 0, fonts: None, cover: None, glow: (GLOW_NEUTRAL, 0.0, Instant::now()), slow_frames: false, line_geoms: HashMap::new(), lyric_heights: None, app_icons: HashMap::new(), morphs: RefCell::new(HashMap::new()), round })
        }
    }

    fn ensure_surface(&mut self, w: i32, h: i32) -> Result<()> {
        if (w, h) == (self.w, self.h) {
            return Ok(());
        }
        unsafe {
            if !self.bmp.is_invalid() {
                SelectObject(self.mem, self.old);
                let _ = DeleteObject(self.bmp.into());
            }
            let mut bmi = BITMAPINFO::default();
            bmi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
            bmi.bmiHeader.biWidth = w;
            bmi.bmiHeader.biHeight = -h;
            bmi.bmiHeader.biPlanes = 1;
            bmi.bmiHeader.biBitCount = 32;
            let mut bits = std::ptr::null_mut();
            self.bmp = CreateDIBSection(Some(self.mem), &bmi, DIB_RGB_COLORS, &mut bits, None, 0)?;
            self.old = SelectObject(self.mem, self.bmp.into());
            self.rt.BindDC(self.mem, &RECT { left: 0, top: 0, right: w, bottom: h })?;
        }
        self.w = w;
        self.h = h;
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
                Some(((fit(big)?, fit(small)?), glow_tint(&px)))
            });
            let (bitmaps, glow) = match decoded { Some((b, g)) => (Some(b), Some(g)), None => (None, None) };
            self.cover = Some(CoverCache { track_key: t.art_key, bytes: t.art.clone(), sizes: (big, small), bitmaps, glow });
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
        self.lyric_heights = Some((key, w, heights.clone()));
        heights
    }

    /// The icon Windows shows for this app id (the shell's AppsFolder knows packaged and
    /// Start-menu apps alike).
    fn app_icon(&mut self, id: &str) -> Option<ID2D1Bitmap> {
        if !self.app_icons.contains_key(id) {
            let bitmap = (|| unsafe {
                let item: IShellItemImageFactory = SHCreateItemFromParsingName(&HSTRING::from(format!("shell:AppsFolder\\{id}")), None).ok()?;
                let hbmp = item.GetImage(SIZE { cx: 64, cy: 64 }, SIIGBF_BIGGERSIZEOK | SIIGBF_ICONONLY).ok()?;
                let wic = self.wic.as_ref()?;
                let src = wic.CreateBitmapFromHBITMAP(hbmp, HPALETTE::default(), WICBitmapUseAlpha);
                let _ = DeleteObject(hbmp.into());
                let conv = wic.CreateFormatConverter().ok()?;
                conv.Initialize(&src.ok()?, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeMedianCut).ok()?;
                self.rt.CreateBitmapFromWicBitmap(&conv, None).ok()
            })();
            self.app_icons.insert(id.to_string(), bitmap);
        }
        self.app_icons[id].clone()
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

    /// Open arc from 12 o'clock clockwise covering `frac` of the circle (battery ring).
    fn ring(&self, cx: f32, cy: f32, r: f32, frac: f32) -> Result<ID2D1PathGeometry> {
        let frac = frac.clamp(0.0, 0.9999);
        let angle = frac * std::f32::consts::TAU;
        unsafe {
            let geo = self.factory.CreatePathGeometry()?;
            let sink = geo.Open()?;
            sink.BeginFigure(Vector2 { X: cx, Y: cy - r }, D2D1_FIGURE_BEGIN_HOLLOW);
            sink.AddArc(&D2D1_ARC_SEGMENT {
                point: Vector2 { X: cx + r * angle.sin(), Y: cy - r * angle.cos() },
                size: D2D_SIZE_F { width: r, height: r },
                rotationAngle: 0.0,
                sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
                arcSize: if frac > 0.5 { D2D1_ARC_SIZE_LARGE } else { D2D1_ARC_SIZE_SMALL },
            });
            sink.EndFigure(D2D1_FIGURE_END_OPEN);
            sink.Close()?;
            Ok(geo)
        }
    }

    /// Draw one frame and push it to `hwnd` placed at screen (x, y), window size (w, h) device px.
    /// Returns true while an icon morph is still playing (the caller keeps frames coming).
    /// Lines `text` wraps to in a notification banner (its body font and width, at `scale`).
    pub fn banner_lines(&mut self, text: &str, scale: f32) -> u32 {
        let Ok(fonts) = self.fonts(scale) else { return 1 };
        let u: Vec<u16> = text.encode_utf16().collect();
        let mut m = DWRITE_TEXT_METRICS::default();
        unsafe {
            match self.dw.CreateTextLayout(&u, &fonts[F_BODY_WRAP], BANNER_TEXT_W * scale, 10_000.0) {
                Ok(layout) if layout.GetMetrics(&mut m).is_ok() => m.lineCount.max(1),
                _ => 1,
            }
        }
    }

    pub fn present(&mut self, hwnd: HWND, x: i32, y: i32, w: i32, h: i32, scale: f32, v: &View) -> Result<bool> {
        self.ensure_surface(w, h)?;
        let fonts = self.fonts(scale)?;
        let cover = v.music.and_then(|t| self.cover(t, (COVER * scale).round() as u32, (20.0 * scale).round() as u32));
        let target = if v.live && v.page == Page::Music { self.cover.as_ref().and_then(|c| c.glow) } else { None };
        let now = Instant::now();
        let dt = now.duration_since(self.glow.2).as_secs_f32().min(0.1);
        let (tint, level, glow_busy) = glow_step(self.glow.0, self.glow.1, target, dt);
        self.glow = (tint, level, now);
        // The banner's line icon (status / device) with its paths as geometry.
        let banner_line = (|| {
            let icon = if v.mode == Mode::Alert { v.banner?.icon.unwrap_or(ICON_BELL) } else { return None };
            let ic = icons::lookup(icon)?;
            let geoms = ic.parts.iter().map(|part| self.line_geometry(part.d)).collect::<Option<Vec<_>>>()?;
            Some((ic, geoms))
        })();
        // Height of each wrapped lyric line, while the panel is wider than the player.
        let lyric_heights: Vec<f32> = match v.music {
            Some(t) if v.page == Page::Music && v.size[0] > notch::target(Mode::Expanded, false)[0] + 1.0 && !t.lyrics.is_empty() =>
                self.lyric_heights(&t.lyrics, LYRIC_W * scale, 20.0 * scale, &fonts),
            _ => Vec::new(),
        };
        let note_icons: Vec<Option<ID2D1Bitmap>> = if v.page == Page::Notify { v.notes.iter().map(|n| self.app_icon(&n.id)).collect() } else { Vec::new() };
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
                // The big cover gets a very thin shadow all round, so it stands off the glow behind it.
                if box_.right - box_.left > 40.0 * s {
                    let (x, y) = (box_.left.round(), box_.top.round());
                    let edge = rect(x, y, x + (box_.right - box_.left), y + (box_.bottom - box_.top));
                    for (k, alpha) in COVER_SHADOW.iter().enumerate().rev() {
                        rt.DrawRoundedRectangle(&rounded(edge, radius), &self.brush(color(0.0, 0.0, 0.0, alpha * a))?, 2.0 * COVER_SHADOW_STEP * (k as f32 + 1.0) * s, None);
                    }
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
                let (gr, gg, gb) = glow_bottom(tint);
                let stop = |position: f32, a: f32| D2D1_GRADIENT_STOP { position, color: color(gr, gg, gb, a * open * level * gate) };
                let stops = rt.CreateGradientStopCollection(&[stop(0.58, 0.0), stop(0.68, 0.08), stop(0.82, 0.42), stop(1.0, 1.0)], D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP)?;
                let line = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES { startPoint: Vector2 { X: 0.0, Y: 0.0 }, endPoint: Vector2 { X: 0.0, Y: nh } };
                rt.FillGeometry(&shape, &rt.CreateLinearGradientBrush(&line, None, &stops)?, None);
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
                if let Some(b) = v.battery {
                    let d = 18.0 * s;
                    // Left of the time; with music the cover takes that spot, so sit before the bars.
                    let bx = if pill_music.is_some() { r - 60.0 * s } else { l + 14.0 * s };
                    let by = (nh - d) / 2.0;
                    let ring_c = if b.charging {
                        color(0.2, 0.84, 0.29, small_a)
                    } else if b.saver {
                        // battery saver: yellow, like iPhone low power mode
                        color(1.0, 0.8, 0.0, small_a)
                    } else if b.percent <= 20 {
                        color(1.0, 0.27, 0.23, small_a)
                    } else {
                        grey(1.0, small_a)
                    };
                    let stroke = 2.2 * s;
                    let (rcx, rcy, rr) = (bx + d / 2.0, by + d / 2.0, d / 2.0 - stroke / 2.0);
                    let track = D2D1_ELLIPSE { point: Vector2 { X: rcx, Y: rcy }, radiusX: rr, radiusY: rr };
                    rt.DrawEllipse(&track, &self.brush(grey(0.3, small_a))?, stroke, None);
                    rt.DrawGeometry(&self.ring(rcx, rcy, rr, b.percent as f32 / 100.0)?, &self.brush(ring_c)?, stroke, None);
                    if b.charging {
                        text(ICON_BOLT, F_TINY_ICON, rect(bx, by, bx + d, by + d), center, ring_c)?;
                    }
                }
                if let Some(t) = pill_music {
                    // Live pill: cover on the left, equaliser on the right.
                    let c = 20.0 * s;
                    let top = (nh - c) / 2.0;
                    draw_cover(rect(l + 10.0 * s, top, l + 10.0 * s + c, top + c), 5.0 * s, small_a)?;
                    let bw = 3.0 * s;
                    for i in 0..4 {
                        let k = i as f32;
                        let amp = if t.playing { 0.5 + 0.5 * (v.phase * (6.0 + k * 1.7) + k * 1.3).sin() } else { 0.0 };
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
                    let (tl, tr) = (ix + d + 12.0 * s, r - 22.0 * s);
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
                    text(icon, F_SMALL_ICON, rect(hl + dx, ht, hl + dx + 18.0 * s, hb), center, color(c.0, c.1, c.2, a * ta))?;
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
                if let Some(b) = v.battery {
                    text(&b.label(), F_LABEL, rect(hr - 92.0 * s, ht, hr, hb), trail, grey(0.6, a))?;
                }
                // Date · time in the middle of the header.
                text(&format!("{} · {}", v.date, v.time), F_LABEL, rect(cx - 80.0 * s, ht, cx + 80.0 * s, hb), center, grey(0.85, a))?;
                // Page content sits between the side arrows.
                let (cl, cr) = (l + SIDE * s, r - SIDE * s);
                let lv = &v.list;
                let rows_of = |page: Page| match page {
                    Page::Todo => v.todos.len(),
                    Page::Notify => v.notes.len(),
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
                        let tb = rect(icon_box.right + 4.0 * s, ib.top, ib.right - 12.0 * s - tag_w, ib.bottom);
                        let draft = if current { lv.draft } else { "" };
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
                    // Edit / delete on a hovered list row (or the row being edited).
                    let row_tools = |i: usize, show: bool| -> Result<()> {
                        if !show {
                            return Ok(());
                        }
                        let edit_hot = hovered == Some(Button::ListEdit(i)) || (current && lv.edit_item == Some(i));
                        if let Some(eb) = btns.iter().filter(|(b, _)| *b == Button::ListEdit(i)).map(|(_, r)| *r).last() {
                            text(ICON_EDIT, F_SMALL_ICON, eb, center, if edit_hot { solid(YELLOW, a) } else { grey(0.6, a) })?;
                        }
                        if let Some(db) = rect_of(Button::ListDelete(i)) {
                            let c = if hovered == Some(Button::ListDelete(i)) { color(1.0, 0.45, 0.4, a) } else { grey(0.6, a) };
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
                            let top = 40.0 * s;
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
                                let (x0, y0, y1) = (l + (player_w + LYRIC_GAP) * s, LYRIC_TOP * s, LYRIC_BOTTOM * s);
                                let x1 = x0 + LYRIC_W * s;
                                match v.music {
                                    Some(t) if !lyric_heights.is_empty() => {
                                        let tops = lyrics::tops(&lyric_heights, 8.0 * s);
                                        let phase = lyrics::phase(&t.lyrics, t.position_ms());
                                        let focus = lyrics::focus(&tops, &lyric_heights, phase);
                                        let middle = (y0 + y1) / 2.0;
                                        for (j, (_, line)) in t.lyrics.iter().enumerate() {
                                            let (top, h) = (middle + tops[j] - focus, lyric_heights[j]);
                                            if top + h < y0 || top > y1 {
                                                continue;
                                            }
                                            let alpha = a * appear * lyrics::edge_fade(top + h / 2.0, y0, y1, 28.0 * s) * lyrics::line_alpha(j, phase);
                                            if alpha > 0.01 {
                                                text(line, F_LYRIC, rect(x0, top, x1, top + h), lead, soft_text(tint, level, 1.0, alpha))?;
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
                                        rt.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: (sb.left + sb.right) / 2.0, Y: (sb.top + sb.bottom) / 2.0 }, radiusX: 9.0 * s, radiusY: 9.0 * s },
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
                                        let (label, lw) = if day == today { (format!("{h:02}:{m:02}"), 40.0) }
                                            else if day == todo::next_day(today) { (format!("{} {h:02}:{m:02}", tr("Mai", "Tmrw")), 64.0) }
                                            else { (format!("{:02}/{:02} {h:02}:{m:02}", day % 100, day / 100 % 100), 74.0) };
                                        text(&label, F_LABEL, rect(br.right - lw * s, br.top, br.right - 2.0 * s, br.bottom), trail, time_color(at, done))?;
                                        text_right = br.right - (lw + 6.0) * s;
                                    }
                                    // Text, struck through when done.
                                    let tb = rect(br.left + d + 10.0 * s, br.top, text_right, br.bottom);
                                    text(&item.text, F_BODY, tb, lead, grey(if done { 0.45 } else { 1.0 }, a))?;
                                    if done {
                                        let tw = width(&item.text, F_BODY).min(tb.right - tb.left);
                                        let my = (br.top + br.bottom) / 2.0 + 0.5 * s;
                                        rt.FillRectangle(&rect(tb.left, my, tb.left + tw, my + 1.0 * s), &self.brush(grey(0.45, a))?);
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
                            for (row, n) in v.notes.iter().skip(scroll).take(NOTE_ROWS).enumerate() {
                                let y = (NOTE_Y + row as f32 * NOTE_H) * s;
                                // Line 1: title, time on the right. Line 2: app · text.
                                let day = (n.at / 10_000) as u32;
                                let when = if day == today { format!("{:02}:{:02}", n.at / 100 % 100, n.at % 100) }
                                    else if todo::next_day(day) == today { tr("Hôm qua", "Yesterday").to_string() }
                                    else { format!("{:02}/{:02}", day % 100, day / 100 % 100) };
                                // App tile: its initial on a colour taken from its name.
                                let tile = rect(cl, y + 3.0 * s, cl + 30.0 * s, y + 33.0 * s);
                                match note_icons.get(scroll + row).and_then(|i| i.as_ref()) {
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
                            scrollbar(v.notes.len(), NOTE_ROWS, NOTE_Y, NOTE_H)?;
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
                rt.PushAxisAlignedClip(&rect(l + 34.0 * s, 30.0 * s, r - 34.0 * s, (DOTS_Y - 3.0) * s), D2D1_ANTIALIAS_MODE_ALIASED);
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
            rt.EndDraw(None, None)?;

            let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
            UpdateLayeredWindow(
                hwnd,
                None,
                Some(&POINT { x, y }),
                Some(&SIZE { cx: w, cy: h }),
                Some(self.mem),
                Some(&POINT { x: 0, y: 0 }),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            )?;
            // The lyrics need frames while they show and the song plays, but a slow trickle is enough
            // between lines; only a lyric gliding to the next line asks for full-speed frames.
            let lyrics_run = !lyric_heights.is_empty() && v.music.is_some_and(|t| t.playing);
            let lyric_glide = lyrics_run && v.music.is_some_and(|t| lyrics::phase(&t.lyrics, t.position_ms()).fract() != 0.0);
            self.slow_frames = lyrics_run;
            Ok(morphing.get() || glow_busy || icon_busy || lyric_glide)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn glow_bottom_keeps_white_controls_readable() {
        for tint in [(1.0, 1.0, 0.0), (1.0, 1.0, 1.0), (0.0, 1.0, 1.0), (0.0, 1.0, 0.0), (1.0, 0.2, 0.2), (0.2, 0.4, 1.0)] {
            let (r, g, b) = glow_bottom(tint);
            let ratio = 1.05 / (luminance(r, g, b) + 0.05);
            assert!(ratio >= 4.5, "{tint:?}: white on the glow is only {ratio:.2}:1");
            assert!(r.max(g).max(b) > 0.2, "{tint:?}: glow went black");
        }
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
            renderer.ensure_surface(2, 2).unwrap();
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
            let hwnd = CreateWindowExW(WS_EX_LAYERED, w!("STATIC"), w!("Lyrics preview"), WS_POPUP, 0, 0, w, h, None, None, None, None).unwrap();
            let lines = [
                (0, "Đêm nay phố vắng người qua"), (6_000, "Ngọn đèn vàng đổ bóng nghiêng nghiêng trên con đường quen thuộc nơi ta từng đi"),
                (12_000, "Em đi giữa những con đường quen"), (18_000, "Mà nghe lòng nhẹ như gió thoảng, như một lời hứa chưa kịp nói thành tên"),
                (24_000, "Có những điều chưa kịp nói ra"), (30_000, "Cứ theo tháng ngày trôi mất"),
                (36_000, "Nếu mai này ta còn gặp lại"), (42_000, "Xin cho nhau một nụ cười hiền"),
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
                leaving: None, todos: &[], notes: &[],
                list: ListView { counts: (0, 0), remind_before: 15, now: 0, draft: "", editing: false, edit_item: None, caret: false, scroll: 0 },
                health: HealthView { h: &Health::default(), now: 0, minute_of_day: 0, settings: None } };
            let mut renderer = Renderer::new().unwrap();
            // The cover loads and the glow fades in over a second or two.
            let frames = std::env::var("PUW_PREVIEW_FRAMES").ok().and_then(|f| f.parse().ok()).unwrap_or(120);
            for _ in 0..(if track.art.is_some() { frames } else { 1 }) {
                renderer.present(hwnd, 0, 0, w, h, 1.0, &view).unwrap();
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            let mut dib = DIBSECTION::default();
            assert_ne!(GetObjectW(renderer.bmp.into(), size_of::<DIBSECTION>() as i32, Some(&mut dib as *mut _ as _)), 0);
            let pixels = std::slice::from_raw_parts(dib.dsBm.bmBits as *const u8, (w * h * 4) as usize);
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
            let hwnd = CreateWindowExW(WS_EX_LAYERED, w!("STATIC"), w!("perf"), WS_POPUP, 0, 0, w, h, None, None, None, None).unwrap();
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
                leaving: None, todos: &[], notes: &[],
                list: ListView { counts: (0, 0), remind_before: 15, now: 0, draft: "", editing: false, edit_item: None, caret: false, scroll: 0 },
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
    #[ignore = "writes a Direct2D preview for visual review"]
    fn media_preview() {
        unsafe {
            use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let hwnd = CreateWindowExW(WS_EX_LAYERED, w!("STATIC"), w!("Media preview"), WS_POPUP,
                0, 0, 530, 210, None, None, None, None).unwrap();
            let track = Track { title: "Bản nhạc đang phát".into(), artist: "YouTube / foobar2000".into(),
                duration_ms: 210_000, playing: true, can_toggle: true, can_next: true,
                can_previous: true, can_seek: true, ..Default::default() };
            let view = View { mode: Mode::Expanded, size: target(Mode::Expanded, false), time: "23:16",
                date: "T7, 24/10", battery: Some(Battery { percent: 70, charging: true, saver: false }),
                banner: None, music: Some(&track), live: true, robot: None, phase: 0.0, banner_age: 0.0, lyrics: false, hovered: Some(Button::PlayPause), page: Page::Music,
                leaving: None, todos: &[], notes: &[],
                list: ListView { counts: (0, 0), remind_before: 15, now: 0, draft: "", editing: false, edit_item: None, caret: false, scroll: 0 },
                health: HealthView { h: &Health::default(), now: 0, minute_of_day: 0, settings: None } };
            let mut renderer = Renderer::new().unwrap();
            renderer.present(hwnd, 0, 0, 530, 210, 1.0, &view).unwrap();
            let mut dib = DIBSECTION::default();
            assert_ne!(GetObjectW(renderer.bmp.into(), size_of::<DIBSECTION>() as i32, Some(&mut dib as *mut _ as _)), 0);
            let pixels = std::slice::from_raw_parts(dib.dsBm.bmBits as *const u8, 530 * 210 * 4);
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
}

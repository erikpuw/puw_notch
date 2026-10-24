//! Direct2D drawing into a 32-bit DIB, shown with UpdateLayeredWindow (per-pixel alpha).
//! Pixels with alpha 0 let the mouse through, so the drawn shape is the hit area.

use windows::core::{w, Result, PCWSTR};
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
use crate::music::Track;
use crate::notch::{target, Mode, HOVER_MARGIN};
use crate::notify::Toast;
use crate::system::Battery;
use crate::calendar::Appointment;
use crate::health::{self, Health, Setting};
use crate::i18n::tr;
use crate::todo::Item;

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
    /// Now playing; Some = "live" layout (cover in the pill).
    pub music: Option<&'a Track>,
    /// Seconds, drives the equaliser bars.
    pub phase: f32,
    pub hovered: Option<Button>,
    pub page: Page,
    /// Page switch in progress: (page sliding out, progress 0..1, direction +1 next / -1 previous).
    pub leaving: Option<(Page, f32, i32)>,
    /// Box, scroll and counts of the list page on screen (to-do or calendar).
    pub list: ListView<'a>,
    pub todos: &'a [Item],
    pub appts: &'a [Appointment],
    pub health: HealthView<'a>,
}

/// Pages of the open panel, switched with the side arrows.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Page {
    Music,
    Todo,
    Calendar,
    Health,
}

pub const PAGES: [Page; 4] = [Page::Music, Page::Todo, Page::Calendar, Page::Health];

impl Page {
    /// Header of the page: icon, name, accent colour.
    fn title(self) -> (&'static str, &'static str, (f32, f32, f32)) {
        match self {
            Page::Music => ("\u{E8D6}", tr("Nhạc", "Music"), (0.72, 0.52, 1.0)),
            Page::Todo => ("\u{E9D5}", tr("Việc cần làm", "To-do"), GREEN),
            Page::Calendar => (ICON_CALENDAR, tr("Lịch hẹn", "Appointments"), BLUE),
            Page::Health => ("\u{E95E}", tr("Sức khỏe", "Health"), (1.0, 0.42, 0.5)),
        }
    }
}

/// The add / edit box and the visible rows of a list page.
pub struct ListView<'a> {
    /// Counts shown above the rows: to-do = (open, done today); calendar = (upcoming, today).
    pub counts: (usize, usize),
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
    /// Total minutes (calendar::minutes) and minute of the day, now.
    pub now: i64,
    pub minute_of_day: u32,
    /// Card whose settings are open.
    pub settings: Option<Card>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Card {
    Water,
    Sit,
}

/// Clickable things of the open panel.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Button {
    Previous,
    PlayPause,
    Next,
    Seek,
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
    Toggle(Setting),
}

impl Button {
    fn label(self) -> &'static str {
        match self {
            Self::Previous => tr("Bài trước", "Previous"), Self::PlayPause => tr("Phát / Tạm dừng", "Play / Pause"),
            Self::Next => tr("Bài tiếp theo", "Next"), Self::Seek => tr("Bấm để tua", "Click to seek"),
            _ => "",
        }
    }
    pub fn enabled(self, track: Option<&Track>) -> bool {
        match self {
            Self::Previous => track.is_some_and(|t| t.can_previous),
            Self::PlayPause => track.is_some_and(|t| t.can_toggle),
            Self::Next => track.is_some_and(|t| t.can_next),
            Self::Seek => track.is_some_and(|t| t.can_seek && t.duration_ms > 0),
            _ => true,
        }
    }
}

const PAD: f32 = 24.0;
/// Icon glyph change animation length.
const ICON_MORPH_S: f32 = 0.25;
/// Concave flares joining the notch sides to the screen edge: grow with the notch, capped.
const EAR_RATIO: f32 = 0.35;
const EAR_MAX: f32 = 14.0;
const MEDIA_BTN: f32 = 40.0;
const COVER: f32 = 64.0;
/// Page content sits between the side arrows (logical px from the panel edge).
const SIDE: f32 = 44.0;
const ARROW: f32 = 24.0;
const ARROW_Y: f32 = 80.0;
const DOTS_Y: f32 = 174.0;
/// List pages (to-do, calendar).
const INPUT_Y: f32 = 36.0;
const INPUT_H: f32 = 26.0;
const COUNT_Y: f32 = 66.0;
const ROW_Y: f32 = 84.0;
const ROW_H: f32 = 22.0;
pub const LIST_ROWS: usize = 3;
/// Health page cards.
const CARD_TOP: f32 = 34.0;
const CARD_BOTTOM: f32 = 158.0;
const CARD_W: f32 = 172.0;
const CARD_GAP: f32 = 8.0;
const SET_ROW: f32 = 18.0;
const SET_Y: f32 = 62.0;

/// Media controls of the music page, in window px for the settled panel centred at `cx`.
pub fn buttons(cx: f32, scale: f32) -> [(Button, D2D_RECT_F); 4] {
    let w = target(Mode::Expanded, false)[0];
    let left = cx / scale - w / 2.0;
    let (y, c) = (124.0, w / 2.0 - MEDIA_BTN / 2.0);
    let r = |x: f32| rect((left + x) * scale, y * scale, (left + x + MEDIA_BTN) * scale, (y + MEDIA_BTN) * scale);
    let seek = rect((left + SIDE + 40.0) * scale, 102.0 * scale, (left + w - SIDE - 40.0) * scale, 122.0 * scale);
    [(Button::Previous, r(c - 48.0)), (Button::PlayPause, r(c)), (Button::Next, r(c + 48.0)), (Button::Seek, seek)]
}

/// Settings rows of a health card: (setting, is a switch).
fn setting_rows(card: Card) -> &'static [(Setting, bool)] {
    match card {
        Card::Water => &[(Setting::WaterOn, true), (Setting::Cup, false), (Setting::Goal, false), (Setting::From, false), (Setting::To, false)],
        Card::Sit => &[(Setting::SitOn, true), (Setting::SitEvery, false)],
    }
}

/// Every clickable rect of the open panel on `page`. `rows` / `scroll` place list rows,
/// `settings` = health card showing its settings.
pub fn page_buttons(cx: f32, scale: f32, page: Page, rows: usize, scroll: usize, settings: Option<Card>) -> Vec<(Button, D2D_RECT_F)> {
    let w = target(Mode::Expanded, false)[0];
    let left = cx / scale - w / 2.0;
    let r = |x: f32, y: f32, bw: f32, bh: f32| rect((left + x) * scale, y * scale, (left + x + bw) * scale, (y + bh) * scale);
    let mut out = vec![(Button::PagePrev, r(10.0, ARROW_Y, ARROW, ARROW)), (Button::PageNext, r(w - 10.0 - ARROW, ARROW_Y, ARROW, ARROW))];
    let (cl, cw) = (SIDE, w - 2.0 * SIDE);
    match page {
        Page::Music => out.extend(buttons(cx, scale)),
        Page::Todo | Page::Calendar => {
            out.push((Button::ListInput, r(cl, INPUT_Y, cw, INPUT_H)));
            for (row, i) in (scroll..rows.min(scroll + LIST_ROWS)).enumerate() {
                let y = ROW_Y + row as f32 * ROW_H;
                // To-do: the row ticks. Calendar: the row opens the appointment for editing.
                let main = if page == Page::Todo { Button::ListToggle(i) } else { Button::ListEdit(i) };
                out.push((main, r(cl, y, cw - 48.0, ROW_H)));
                out.push((Button::ListEdit(i), r(cl + cw - 46.0, y, 22.0, ROW_H)));
                out.push((Button::ListDelete(i), r(cl + cw - 22.0, y, 22.0, ROW_H)));
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

pub fn seek_at(cx: f32, scale: f32, x: f32, duration: u64) -> u64 {
    let (_, r) = buttons(cx, scale)[3];
    (((x - r.left) / (r.right - r.left)).clamp(0.0, 1.0) as f64 * duration as f64) as u64
}

pub fn hit_button(cx: f32, scale: f32, x: f32, y: f32, track: Option<&Track>, page: Page, rows: usize, scroll: usize, settings: Option<Card>) -> Option<Button> {
    page_buttons(cx, scale, page, rows, scroll, settings).into_iter().find(|(b, r)| b.enabled(track)
        && x >= r.left && x <= r.right && y >= r.top && y <= r.bottom).map(|(b, _)| b)
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
const F_SMALL_ICON: usize = 7;
const F_BIG: usize = 8;

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
pub const ICON_CALENDAR: &str = "\u{E787}";
pub const ICON_WALK: &str = "\u{E805}";
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
const ICON_BELL: &str = "\u{EA8F}";
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

/// Banner icon for a Bluetooth device.
pub fn device_icon(kind: DeviceKind) -> &'static str {
    use DeviceKind::*;
    match kind {
        Headphones => "\u{E7F6}",
        Speaker => "\u{E7F5}",
        Phone => "\u{E8EA}",
        Mouse => "\u{E962}",
        Keyboard => ICON_KEYBOARD,
        Gamepad => "\u{E7FC}",
        Other => ICON_BT,
    }
}

struct CoverCache {
    track_key: u64,
    bytes: Option<Arc<Vec<u8>>>,
    bitmap: Option<ID2D1Bitmap>,
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
    /// Icon slots that just changed glyph: (current, previous, when) for the morph.
    morphs: RefCell<HashMap<&'static str, (&'static str, &'static str, Instant)>>,
}

fn color(r: f32, g: f32, b: f32, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r, g, b, a }
}

fn grey(v: f32, a: f32) -> D2D1_COLOR_F {
    color(v, v, v, a)
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
                r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
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
            Ok(Renderer { factory, rt, dw, wic, mem, bmp: HBITMAP::default(), old: HGDIOBJ::default(), w: 0, h: 0, fonts: None, cover: None, morphs: RefCell::new(HashMap::new()) })
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
            // Same body text, but wrapping onto more lines (top-aligned; the box clips it).
            let wrap = |f: IDWriteTextFormat| -> Result<IDWriteTextFormat> {
                unsafe {
                    f.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP)?;
                    f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR)?;
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
            ];
            self.fonts = Some((scale, set));
        }
        Ok(self.fonts.as_ref().unwrap().1.clone())
    }

    /// Decode again when the track's asynchronous artwork arrives.
    fn cover(&mut self, t: &Track) -> Option<ID2D1Bitmap> {
        let cached = self.cover.as_ref().is_some_and(|cache| cache.track_key == t.art_key
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
                self.rt.CreateBitmapFromWicBitmap(&conv, None).ok()
            });
            self.cover = Some(CoverCache { track_key: t.art_key, bytes: t.art.clone(), bitmap: decoded });
        }
        self.cover.as_ref().and_then(|c| c.bitmap.clone())
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
    pub fn present(&mut self, hwnd: HWND, x: i32, y: i32, w: i32, h: i32, scale: f32, v: &View) -> Result<bool> {
        self.ensure_surface(w, h)?;
        let fonts = self.fonts(scale)?;
        let cover = v.music.and_then(|t| self.cover(t));
        let rt = &self.rt;
        let s = scale;
        let [nw, nh, nr] = v.size.map(|d| d * s);
        let cx = w as f32 / 2.0;
        let (l, r) = (cx - nw / 2.0, cx + nw / 2.0);
        unsafe {
            let brush = |c: D2D1_COLOR_F| rt.CreateSolidColorBrush(&c, None);
            let text = |t: &str, f: usize, b: D2D_RECT_F, align: DWRITE_TEXT_ALIGNMENT, c: D2D1_COLOR_F| -> Result<()> {
                let t: Vec<u16> = t.encode_utf16().collect();
                fonts[f].SetTextAlignment(align)?;
                rt.DrawText(&t, &fonts[f], &b, &brush(c)?, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
                Ok(())
            };
            // Cover art clipped to a rounded square, or a note glyph when there is none.
            let draw_cover = |box_: D2D_RECT_F, radius: f32, a: f32| -> Result<()> {
                match &cover {
                    Some(bmp) => {
                        let size = bmp.GetSize();
                        let b = rt.CreateBitmapBrush(bmp, None, None)?;
                        let (sx, sy) = ((box_.right - box_.left) / size.width, (box_.bottom - box_.top) / size.height);
                        b.SetTransform(&Matrix3x2 { M11: sx, M12: 0.0, M21: 0.0, M22: sy, M31: box_.left, M32: box_.top });
                        b.SetOpacity(a);
                        rt.FillRoundedRectangle(&rounded(box_, radius), &b);
                    }
                    None => {
                        rt.FillRoundedRectangle(&rounded(box_, radius), &brush(grey(0.2, a))?);
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

            let ear = (nh * EAR_RATIO).min(EAR_MAX * s);
            if !matches!(v.mode, Mode::Mini | Mode::PeekMini) {
                // Alpha 1/255: invisible but still catches the mouse.
                let m = HOVER_MARGIN * s;
                rt.FillRectangle(&rect(l - ear - m, 0.0, r + ear + m, nh + m), &brush(color(0.0, 0.0, 0.0, 1.0 / 255.0))?);
            }
            rt.FillGeometry(&self.notch_shape(l, r, nh, nr, ear)?, &brush(grey(0.0, 1.0))?, None);

            // Each layout fades in as the pill reaches its size.
            let fade = |from: f32, span: f32| ((nh - from * s) / (span * s)).clamp(0.0, 1.0);
            let small_a = 1.0 - fade(39.0, 20.0);
            if nh >= 20.0 * s && small_a > 0.0 && v.mode != Mode::Alert {
                text(v.time, F_SMALL, rect(l, 0.0, r, nh), center, grey(1.0, small_a))?;
                if let Some(b) = v.battery {
                    let d = 18.0 * s;
                    // Left of the time; with music the cover takes that spot, so sit before the bars.
                    let bx = if v.music.is_some() { r - 60.0 * s } else { l + 14.0 * s };
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
                    rt.DrawEllipse(&track, &brush(grey(0.3, small_a))?, stroke, None);
                    rt.DrawGeometry(&self.ring(rcx, rcy, rr, b.percent as f32 / 100.0)?, &brush(ring_c)?, stroke, None);
                    if b.charging {
                        text(ICON_BOLT, F_TINY_ICON, rect(bx, by, bx + d, by + d), center, ring_c)?;
                    }
                }
                if let Some(t) = v.music {
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
                        rt.FillRoundedRectangle(&rounded(rect(bx, my - bh / 2.0, bx + bw, my + bh / 2.0), bw / 2.0), &brush(grey(1.0, small_a * 0.9))?);
                    }
                }
            }

            if v.mode == Mode::Alert {
                if let Some(t) = v.banner {
                    let a = fade(44.0, 18.0);
                    // Icon in a circle on the left, text beside it.
                    let d = 36.0 * s;
                    let (ix, iy) = (l + 18.0 * s, nh / 2.0 - d / 2.0 + 2.0 * s);
                    let circle = D2D1_ELLIPSE { point: Vector2 { X: ix + d / 2.0, Y: iy + d / 2.0 }, radiusX: d / 2.0, radiusY: d / 2.0 };
                    rt.FillEllipse(&circle, &brush(grey(0.2, a))?);
                    if t.icon == Some(ICON_DROP) {
                        let g = self.water_drop(ix + d / 2.0, iy + d / 2.0, 18.0 * s)?;
                        rt.FillGeometry(&g, &brush(color(BLUE.0, BLUE.1, BLUE.2, a))?, None);
                    } else {
                        micon("banner", t.icon.unwrap_or(ICON_BELL), F_ICON, rect(ix, iy, ix + d, iy + d), grey(1.0, a))?;
                    }
                    let (tl, tr) = (ix + d + 12.0 * s, r - 22.0 * s);
                    text(&t.app, F_LABEL, rect(tl, 8.0 * s, tr, 24.0 * s), lead, grey(0.6, a))?;
                    text(&t.title, F_TITLE, rect(tl, 24.0 * s, tr, 42.0 * s), lead, grey(1.0, a))?;
                    match t.level {
                        Some(level) => {
                            let th = 5.0 * s;
                            let my = 50.0 * s;
                            let bar = |right: f32| rounded(rect(tl, my - th / 2.0, right, my + th / 2.0), th / 2.0);
                            rt.FillRoundedRectangle(&bar(tr), &brush(grey(0.25, a))?);
                            rt.FillRoundedRectangle(&bar(tl + (tr - tl) * level.clamp(0.0, 1.0)), &brush(grey(1.0, a))?);
                        }
                        // Tall banner: up to two lines of the notification text.
                        None if nh > 74.0 * s => text(&t.body, F_BODY_WRAP, rect(tl, 43.0 * s, tr, 79.0 * s), lead, grey(0.75, a))?,
                        None => text(&t.body, F_BODY, rect(tl, 42.0 * s, tr, 58.0 * s), lead, grey(0.75, a))?,
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
                        draw_title(from, -d * t.min(1.0), (1.0 - t * 1.8).max(0.0))?;
                        draw_title(v.page, d * (1.0 - ease_out_back(t)), (t * 1.6).min(1.0))?;
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
                    Page::Calendar => v.appts.len(),
                    _ => 0,
                };
                let minutes = |t: u64| (t / 10_000) as i64 * 1440 + (t / 100 % 100) as i64 * 60 + (t % 100) as i64;
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
                    let btns = page_buttons(cx, s, page, rows_of(page), scroll, settings);
                    let rect_of = |b: Button| btns.iter().find(|(x, _)| *x == b).map(|(_, r)| *r);
                    rt.SetTransform(&Matrix3x2 { M11: 1.0, M12: 0.0, M21: 0.0, M22: 1.0, M31: dx, M32: 0.0 });

                    // Add / edit box shared by the list pages.
                    let draw_input = |placeholder: &str| -> Result<()> {
                        let Some(ib) = rect_of(Button::ListInput) else { return Ok(()) };
                        let editing = current && lv.editing;
                        let edit_row = if current { lv.edit_item } else { None };
                        let focus = editing || hovered == Some(Button::ListInput);
                        rt.FillRoundedRectangle(&rounded(ib, 13.0 * s), &brush(grey(if focus { 0.16 } else { 0.12 }, a))?);
                        if editing || edit_row.is_some() {
                            let inner = rect(ib.left + 0.5, ib.top + 0.5, ib.right - 0.5, ib.bottom - 0.5);
                            let c = if edit_row.is_some() { solid(YELLOW, a * 0.8) } else { grey(1.0, a * 0.35) };
                            rt.DrawRoundedRectangle(&rounded(inner, 13.0 * s), &brush(c)?, 1.0 * s, None);
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
                                rt.FillRectangle(&rect(x, my - 7.0 * s, x + 1.5 * s, my + 7.0 * s), &brush(grey(1.0, a))?);
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
                    let scrollbar = |n: usize| -> Result<()> {
                        if n > LIST_ROWS {
                            let (top, bottom) = (ROW_Y * s, (ROW_Y + ROW_H * LIST_ROWS as f32) * s);
                            let x = cr + 6.0 * s;
                            rt.FillRoundedRectangle(&rounded(rect(x, top, x + 3.0 * s, bottom), 1.5 * s), &brush(grey(0.2, a))?);
                            let len = (bottom - top) * LIST_ROWS as f32 / n as f32;
                            let start = top + (bottom - top - len) * scroll as f32 / (n - LIST_ROWS) as f32;
                            rt.FillRoundedRectangle(&rounded(rect(x, start, x + 3.0 * s, start + len), 1.5 * s), &brush(grey(0.6, a))?);
                        }
                        Ok(())
                    };
                    // "HH:MM" coloured by how close it is: orange within 15 min, red when past.
                    let time_color = |due: u64, quiet: bool| {
                        let left = minutes(due) - minutes(lv.now);
                        if quiet { grey(0.4, a) } else if left <= 0 { color(1.0, 0.35, 0.3, a) } else if left <= 15 { color(1.0, 0.62, 0.2, a) } else { grey(0.6, a) }
                    };

                    match page {
                        Page::Music => {
                            let top = 38.0 * s;
                            draw_cover(rect(cl, top, cl + COVER * s, top + COVER * s), 10.0 * s, a)?;
                            let tl = cl + (COVER + 12.0) * s;
                            match v.music {
                                Some(t) => {
                                    let title = if t.title.is_empty() { tr("Đang phát", "Playing") } else { &t.title };
                                    text(title, F_TITLE, rect(tl, top + 2.0 * s, cr, top + 20.0 * s), lead, grey(1.0, a))?;
                                    text(&t.artist, F_BODY, rect(tl, top + 20.0 * s, cr, top + 36.0 * s), lead, grey(0.65, a))?;
                                    let lyric = t.lyric_now().unwrap_or_else(|| hovered.map_or(
                                        if t.duration_ms == 0 { tr("Trình phát chưa cung cấp thời gian", "The player gives no timeline") }
                                        else if t.lyrics.is_empty() { "" } else { "♪" }, Button::label));
                                    text(t.command_error.unwrap_or(lyric), F_BODY, rect(tl, top + 40.0 * s, cr, top + 58.0 * s), lead,
                                        if t.command_error.is_some() { color(1.0, 0.5, 0.4, a) } else { grey(0.95, a) })?;
                                }
                                None => text(tr("Không có bài", "Nothing playing"), F_TITLE, rect(tl, top, cr, top + COVER * s), lead, grey(1.0, a))?,
                            }
                            let pos = v.music.map_or(0, |t| t.position_ms());
                            let dur = v.music.map_or(0, |t| t.duration_ms);
                            let by = 112.0 * s;
                            let lw = 40.0 * s;
                            let (bl, brr) = (cl + lw, cr - lw);
                            text(&if dur > 0 { clock(pos) } else { "--:--".into() }, F_LABEL, rect(cl, by - 9.0 * s, bl - 4.0 * s, by + 11.0 * s), lead, grey(0.6, a))?;
                            text(&if dur > 0 { format!("-{}", clock(dur.saturating_sub(pos))) } else { "--:--".into() }, F_LABEL, rect(brr + 4.0 * s, by - 9.0 * s, cr, by + 11.0 * s), trail, grey(0.6, a))?;
                            let th = 4.0 * s;
                            let bar = |right: f32| rounded(rect(bl, by - th / 2.0, right, by + th / 2.0), th / 2.0);
                            rt.FillRoundedRectangle(&bar(brr), &brush(grey(0.25, a))?);
                            if dur > 0 {
                                let p = (pos as f32 / dur as f32).clamp(0.0, 1.0);
                                rt.FillRoundedRectangle(&bar(bl + (brr - bl) * p), &brush(grey(1.0, a))?);
                            }
                            let playing = v.music.is_some_and(|t| t.playing);
                            for (b, br) in buttons(cx, s) {
                                let icon = match b {
                                    Button::Previous => ICON_PREV,
                                    Button::PlayPause => if playing { ICON_PAUSE } else { ICON_PLAY },
                                    Button::Next => ICON_NEXT,
                                    _ => continue,
                                };
                                let c = if !b.enabled(v.music) { grey(0.3, a) }
                                    else if hovered == Some(b) { color(0.25, 0.85, 0.5, a) } else { grey(1.0, a) };
                                let slot = match b { Button::Previous => "prev", Button::PlayPause => "play", _ => "next" };
                                micon(slot, icon, F_ICON, br, c)?;
                            }
                        }
                        Page::Todo => {
                            draw_input(tr("Thêm việc… (vd: Họp nhóm 20:00)", "Add a task… (e.g. Team call 20:00)"))?;
                            let (open, done_today) = if current { lv.counts } else { (v.todos.iter().filter(|i| i.done.is_none()).count(), 0) };
                            let cy = COUNT_Y * s;
                            text(&format!("{} · {open}", tr("Chưa xong", "Open")), F_LABEL, rect(cl, cy - 2.0 * s, cr, cy + 16.0 * s), lead, grey(0.6, a))?;
                            text(&format!("✓ {} {done_today}", tr("Xong hôm nay", "Done today")), F_LABEL, rect(cl, cy - 2.0 * s, cr, cy + 16.0 * s), trail,
                                if done_today > 0 { solid(GREEN, a) } else { grey(0.6, a) })?;
                            if v.todos.is_empty() {
                                let area = rect(cl, ROW_Y * s, cr, (ROW_Y + ROW_H * LIST_ROWS as f32) * s);
                                text(tr("Chưa có việc nào", "No tasks yet"), F_BODY, area, center, grey(0.55, a))?;
                            }
                            for (b, br) in btns.iter() {
                                let Button::ListToggle(i) = *b else { continue };
                                let Some(item) = v.todos.get(i) else { continue };
                                let hot = row_hovered(i);
                                if hot {
                                    rt.FillRoundedRectangle(&rounded(rect(br.left - 6.0 * s, br.top + 1.0 * s, cr + 2.0 * s, br.bottom - 1.0 * s), 8.0 * s), &brush(grey(0.1, a))?);
                                }
                                // Tick circle.
                                let d = 14.0 * s;
                                let (ccx, ccy) = (br.left + d / 2.0, (br.top + br.bottom) / 2.0);
                                let circle = D2D1_ELLIPSE { point: Vector2 { X: ccx, Y: ccy }, radiusX: d / 2.0 - 0.75 * s, radiusY: d / 2.0 - 0.75 * s };
                                let done = item.done.is_some();
                                if done {
                                    rt.FillEllipse(&circle, &brush(solid(GREEN, a))?);
                                    text(ICON_CHECK, F_TINY_ICON, rect(ccx - d / 2.0, ccy - d / 2.0, ccx + d / 2.0, ccy + d / 2.0), center, grey(0.0, a))?;
                                } else {
                                    rt.DrawEllipse(&circle, &brush(grey(if hot { 0.85 } else { 0.55 }, a))?, 1.5 * s, None);
                                }
                                // Reminder time on the right.
                                let mut text_right = br.right;
                                if let Some(due) = item.due {
                                    let (h, m) = (due / 100 % 100, due % 100);
                                    let tomorrow = due / 10_000 > lv.now / 10_000;
                                    let label = if tomorrow { format!("{} {h:02}:{m:02}", tr("Mai", "Tmrw")) } else { format!("{h:02}:{m:02}") };
                                    let lw = if tomorrow { 60.0 } else { 40.0 } * s;
                                    text(&label, F_LABEL, rect(br.right - lw, br.top, br.right - 2.0 * s, br.bottom), trail, time_color(due, done))?;
                                    text_right = br.right - lw - 6.0 * s;
                                }
                                // Text, struck through when done.
                                let tb = rect(br.left + d + 10.0 * s, br.top, text_right, br.bottom);
                                text(&item.text, F_BODY, tb, lead, grey(if done { 0.45 } else { 1.0 }, a))?;
                                if done {
                                    let tw = width(&item.text, F_BODY).min(tb.right - tb.left);
                                    let my = (br.top + br.bottom) / 2.0 + 0.5 * s;
                                    rt.FillRectangle(&rect(tb.left, my, tb.left + tw, my + 1.0 * s), &brush(grey(0.45, a))?);
                                }
                                row_tools(i, hot)?;
                            }
                            scrollbar(v.todos.len())?;
                        }
                        Page::Calendar => {
                            draw_input(tr("Thêm lịch hẹn… (vd: Khám răng 28/10 14:30)", "Add an appointment… (e.g. Dentist 28/10 14:30)"))?;
                            let today = lv.now / 10_000;
                            let (upcoming, today_n) = if current { lv.counts } else { (v.appts.iter().filter(|x| x.at > lv.now).count(), 0) };
                            let cy = COUNT_Y * s;
                            text(&format!("{} · {upcoming}", tr("Sắp tới", "Upcoming")), F_LABEL, rect(cl, cy - 2.0 * s, cr, cy + 16.0 * s), lead, grey(0.6, a))?;
                            text(&format!("{} · {today_n}", tr("Hôm nay", "Today")), F_LABEL, rect(cl, cy - 2.0 * s, cr, cy + 16.0 * s), trail,
                                if today_n > 0 { solid(BLUE, a) } else { grey(0.6, a) })?;
                            if v.appts.is_empty() {
                                let area = rect(cl, ROW_Y * s, cr, (ROW_Y + ROW_H * LIST_ROWS as f32) * s);
                                text(tr("Chưa có lịch hẹn", "No appointments"), F_BODY, area, center, grey(0.55, a))?;
                            }
                            for (b, br) in btns.iter() {
                                // the first ListEdit of a row is its text area
                                let Button::ListEdit(i) = *b else { continue };
                                if br.right - br.left < 30.0 * s {
                                    continue;
                                }
                                let Some(ap) = v.appts.get(i) else { continue };
                                let hot = row_hovered(i);
                                if hot {
                                    rt.FillRoundedRectangle(&rounded(rect(br.left - 6.0 * s, br.top + 1.0 * s, cr + 2.0 * s, br.bottom - 1.0 * s), 8.0 * s), &brush(grey(0.1, a))?);
                                }
                                // Day on the left, time on the right.
                                let day = ap.at / 10_000;
                                let (d, mo) = (day % 100, day / 100 % 100);
                                let day_label = if day == today { tr("Hôm nay", "Today").to_string() }
                                    else if day == crate::todo::next_day(today as u32) as u64 { tr("Mai", "Tmrw").to_string() }
                                    else { format!("{d:02}/{mo:02}") };
                                let past = ap.at <= lv.now;
                                text(&day_label, F_LABEL, rect(br.left, br.top, br.left + 52.0 * s, br.bottom), lead,
                                    if day == today && !past { solid(BLUE, a) } else { grey(0.6, a) })?;
                                text(&format!("{:02}:{:02}", ap.at / 100 % 100, ap.at % 100), F_LABEL, rect(br.right - 40.0 * s, br.top, br.right - 2.0 * s, br.bottom), trail, time_color(ap.at, false))?;
                                text(&ap.text, F_BODY, rect(br.left + 56.0 * s, br.top, br.right - 46.0 * s, br.bottom), lead, grey(if past { 0.5 } else { 1.0 }, a))?;
                                row_tools(i, hot)?;
                            }
                            scrollbar(v.appts.len())?;
                        }
                        Page::Health => {
                            let hv = &v.health;
                            let h = hv.h;
                            for card in [Card::Water, Card::Sit] {
                                let x = if card == Card::Water { cl } else { cl + (CARD_W + CARD_GAP) * s };
                                let cb = rect(x, CARD_TOP * s, x + CARD_W * s, CARD_BOTTOM * s);
                                rt.FillRoundedRectangle(&rounded(cb, 14.0 * s), &brush(grey(0.11, a))?);
                                let accent = if card == Card::Water { BLUE } else { ORANGE };
                                // Title row: icon, name, gear.
                                let ty = (CARD_TOP + 4.0) * s;
                                let ib = rect(x + 10.0 * s, ty, x + 32.0 * s, ty + 22.0 * s);
                                if card == Card::Water {
                                    let g = self.water_drop((ib.left + ib.right) / 2.0, (ib.top + ib.bottom) / 2.0, 16.0 * s)?;
                                    rt.FillGeometry(&g, &brush(solid(accent, a))?, None);
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
                                                rt.FillRoundedRectangle(&rounded(sb, rad), &brush(if on { solid(GREEN, a) } else { grey(0.3, a) })?);
                                                let kx = if on { sb.right - rad } else { sb.left + rad };
                                                let knob = D2D1_ELLIPSE { point: Vector2 { X: kx, Y: (sb.top + sb.bottom) / 2.0 }, radiusX: rad - 2.0 * s, radiusY: rad - 2.0 * s };
                                                rt.FillEllipse(&knob, &brush(grey(1.0, a))?)
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
                                                    rt.FillEllipse(&c, &brush(grey(if hot { 0.32 } else { 0.2 }, a))?);
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
                                rt.FillRoundedRectangle(&bar(br_), &brush(grey(0.25, a))?);
                                rt.FillRoundedRectangle(&bar(bl + (br_ - bl) * frac.clamp(0.0, 1.0)), &brush(solid(accent, a))?);
                                text(&status, F_LABEL, rect(bl, 101.0 * s, br_, 119.0 * s), lead, grey(0.65, a))?;
                                let ab = if card == Card::Water { Button::Drink } else { Button::Stood };
                                if let Some(pb) = rect_of(ab) {
                                    let hot = hovered == Some(ab);
                                    let k = if hot { 1.0 } else { 0.82 };
                                    rt.FillRoundedRectangle(&rounded(pb, 12.0 * s), &brush(color(accent.0 * k, accent.1 * k, accent.2 * k, a))?);
                                    text(&action, F_SMALL, pb, center, grey(1.0, a))?;
                                }
                                if let Some(sb) = rect_of(Button::Snooze) {
                                    if card == Card::Water {
                                        let hot = hovered == Some(Button::Snooze);
                                        rt.FillRoundedRectangle(&rounded(sb, 12.0 * s), &brush(grey(if hot { 0.3 } else { 0.2 }, a))?);
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
                        draw_page(from, -d * span * t.min(1.0), (1.0 - t * 1.8).max(0.0))?;
                        draw_page(v.page, d * span * (1.0 - ease_out_back(t)), (t * 1.6).min(1.0))?;
                    }
                    None => draw_page(v.page, 0.0, 1.0)?,
                }
                rt.PopAxisAlignedClip();

                // Side arrows and page dots.
                let nav = page_buttons(cx, s, v.page, 0, 0, None);
                for (b, icon) in [(Button::PagePrev, ICON_LEFT), (Button::PageNext, ICON_RIGHT)] {
                    if let Some((_, br)) = nav.iter().find(|(x, _)| *x == b) {
                        let hot = v.hovered == Some(b);
                        let circle = D2D1_ELLIPSE {
                            point: Vector2 { X: (br.left + br.right) / 2.0, Y: (br.top + br.bottom) / 2.0 },
                            radiusX: (br.right - br.left) / 2.0,
                            radiusY: (br.bottom - br.top) / 2.0,
                        };
                        rt.FillEllipse(&circle, &brush(grey(if hot { 0.26 } else { 0.15 }, a))?);
                        text(icon, F_SMALL_ICON, *br, center, grey(if hot { 1.0 } else { 0.75 }, a))?;
                    }
                }
                let (dot, wide, gap) = (5.0 * s, 14.0 * s, 5.0 * s);
                let total = wide + (PAGES.len() as f32 - 1.0) * (dot + gap);
                let mut dx = cx - total / 2.0;
                for p in PAGES {
                    let on = p == v.page;
                    let dw = if on { wide } else { dot };
                    rt.FillRoundedRectangle(&rounded(rect(dx, DOTS_Y * s, dx + dw, DOTS_Y * s + dot), dot / 2.0), &brush(if on { grey(1.0, a) } else { grey(0.45, a) })?);
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
            Ok(morphing.get())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "uses native Direct2D/WIC; run explicitly for delayed cover loading"]
    fn cover_reloads_when_art_arrives_after_track() {
        unsafe {
            use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let mut renderer = Renderer::new().unwrap();
            renderer.ensure_surface(2, 2).unwrap();
            let mut track = Track { art_key: 1, ..Default::default() };
            assert!(renderer.cover(&track).is_none());
            let mut bmp = Vec::from(&b"BM"[..]);
            bmp.extend_from_slice(&58u32.to_le_bytes()); bmp.extend_from_slice(&[0; 4]);
            bmp.extend_from_slice(&54u32.to_le_bytes()); bmp.extend_from_slice(&40u32.to_le_bytes());
            bmp.extend_from_slice(&1i32.to_le_bytes()); bmp.extend_from_slice(&1i32.to_le_bytes());
            bmp.extend_from_slice(&1u16.to_le_bytes()); bmp.extend_from_slice(&32u16.to_le_bytes());
            bmp.extend_from_slice(&[0; 24]); bmp.extend_from_slice(&[0, 0, 255, 255]);
            assert_eq!(bmp.len(), 58);
            track.art = Some(std::sync::Arc::new(bmp));
            assert!(renderer.cover(&track).is_some(), "late cover data must invalidate the cached empty image");
            assert!(renderer.cover(&track).is_some());
        }
    }

    #[test]
    fn seek_and_icon_hit_areas_follow_dpi_and_capabilities() {
        let track = Track { duration_ms: 120_000, can_seek: true, can_toggle: true, ..Default::default() };
        for scale in [1.0, 1.5, 2.0] {
            let cx = 265.0 * scale;
            let (_, seek) = buttons(cx, scale)[3];
            assert_eq!(seek_at(cx, scale, seek.left - 100.0, track.duration_ms), 0);
            assert_eq!(seek_at(cx, scale, seek.right + 100.0, track.duration_ms), 120_000);
            assert_eq!(seek_at(cx, scale, (seek.left + seek.right) / 2.0, track.duration_ms), 60_000);
            assert_eq!(hit_button(cx, scale, cx, 140.0 * scale, Some(&track), Page::Music, 0, 0, None), Some(Button::PlayPause));
            assert_eq!(hit_button(cx, scale, cx, 112.0 * scale, Some(&track), Page::Music, 0, 0, None), Some(Button::Seek));
            assert_eq!(hit_button(cx, scale, cx, 140.0 * scale, None, Page::Music, 0, 0, None), None);
            assert!(buttons(cx, scale).iter().all(|(_, r)| r.bottom <= DOTS_Y * scale));
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
                banner: None, music: Some(&track), phase: 0.0, hovered: Some(Button::PlayPause), page: Page::Music,
                leaving: None, todos: &[], appts: &[],
                list: ListView { counts: (0, 0), now: 0, draft: "", editing: false, edit_item: None, caret: false, scroll: 0 },
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

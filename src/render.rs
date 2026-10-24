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

use crate::bluetooth::Bt;
use crate::music::Track;
use crate::notch::{target, Mode, HOVER_MARGIN};
use crate::notify::Toast;

/// Everything the painter needs for one frame (logical px).
pub struct View<'a> {
    pub mode: Mode,
    pub size: [f32; 3],
    pub time: &'a str,
    pub date: &'a str,
    pub battery: &'a str,
    /// Banner content while in Alert (a notification or a status like Caps Lock).
    pub banner: Option<&'a Toast>,
    /// Newest notification, shown in the panel.
    pub toast: Option<&'a Toast>,
    pub volume: Option<(f32, bool)>,
    pub mic_muted: Option<bool>,
    pub bt: Bt,
    /// Now playing; Some = "live" layout (cover in the pill, music card in the panel).
    pub music: Option<&'a Track>,
    /// Seconds, drives the equaliser bars.
    pub phase: f32,
    pub page: Page,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Button {
    SpeakerMute,
    VolDown,
    VolBar,
    VolUp,
    Mic,
    Bluetooth,
    Previous,
    PlayPause,
    Next,
    PagePrev,
    PageNext,
}

/// Pages of the open panel, switched with the side arrows or the mouse wheel.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Page {
    Music,
    Notifications,
    Controls,
}

pub const PAGES: [Page; 3] = [Page::Music, Page::Notifications, Page::Controls];

const PAD: f32 = 24.0;
/// Concave flares joining the notch sides to the screen edge: grow with the notch, capped.
const EAR_RATIO: f32 = 0.35;
const EAR_MAX: f32 = 14.0;
const BTN: f32 = 36.0;
const MEDIA_BTN: f32 = 32.0;
const ARROW: f32 = 24.0;
/// Page content sits between the side arrows (logical px from the panel edge).
const SIDE: f32 = 48.0;
const COVER: f32 = 64.0;
const DOTS_Y: f32 = 166.0;

/// Clickable rects in window px for `page` of the settled panel centred at `cx`.
pub fn buttons(cx: f32, scale: f32, page: Page) -> Vec<(Button, D2D_RECT_F)> {
    let w = target(Mode::Expanded, false)[0];
    let h = target(Mode::Expanded, false)[1];
    let left = cx / scale - w / 2.0;
    // (x from panel left, y, width, height) in logical px
    let r = |x: f32, y: f32, bw: f32, bh: f32| rect((left + x) * scale, y * scale, (left + x + bw) * scale, (y + bh) * scale);
    let mid = h / 2.0 + 4.0 - ARROW / 2.0;
    let mut out = vec![(Button::PagePrev, r(14.0, mid, ARROW, ARROW)), (Button::PageNext, r(w - 14.0 - ARROW, mid, ARROW, ARROW))];
    let (cl, cr) = (SIDE, w - SIDE);
    match page {
        Page::Music => {
            let y = 124.0;
            let c = w / 2.0 - MEDIA_BTN / 2.0;
            out.push((Button::Previous, r(c - 48.0, y, MEDIA_BTN, MEDIA_BTN)));
            out.push((Button::PlayPause, r(c, y, MEDIA_BTN, MEDIA_BTN)));
            out.push((Button::Next, r(c + 48.0, y, MEDIA_BTN, MEDIA_BTN)));
        }
        Page::Notifications => {}
        Page::Controls => {
            let y = 54.0;
            out.push((Button::SpeakerMute, r(cl, y, BTN, BTN)));
            out.push((Button::VolDown, r(cl + BTN + 8.0, y, BTN, BTN)));
            let bar_l = cl + 2.0 * (BTN + 8.0);
            let bar_r = cr - BTN - 8.0;
            out.push((Button::VolBar, r(bar_l, y, bar_r - bar_l, BTN)));
            out.push((Button::VolUp, r(cr - BTN, y, BTN, BTN)));
            let y2 = 106.0;
            let half = (cr - cl - 10.0) / 2.0;
            out.push((Button::Mic, r(cl, y2, half, BTN)));
            out.push((Button::Bluetooth, r(cl + half + 10.0, y2, half, BTN)));
        }
    }
    out
}

/// Text styles, indexed by these constants.
const F_SMALL: usize = 0;
const F_BODY: usize = 1;
const F_LABEL: usize = 2;
const F_ICON: usize = 3;
const F_TITLE: usize = 4;
const F_LABEL_ICON: usize = 5;

pub const ICON_VOLUME: &str = "\u{E767}";
pub const ICON_MUTE: &str = "\u{E74F}";
const ICON_MINUS: &str = "\u{E738}";
const ICON_PLUS: &str = "\u{E710}";
const ICON_MIC: &str = "\u{E720}";
pub const ICON_BT: &str = "\u{E702}";
const ICON_PREV: &str = "\u{E892}";
const ICON_PLAY: &str = "\u{E768}";
const ICON_PAUSE: &str = "\u{E769}";
const ICON_NEXT: &str = "\u{E893}";
const ICON_NOTE: &str = "\u{E8D6}";
const ICON_LEFT: &str = "\u{E76B}";
const ICON_RIGHT: &str = "\u{E76C}";
const ICON_BELL: &str = "\u{EA8F}";
pub const ICON_BRIGHTNESS: &str = "\u{E706}";
pub const ICON_KEYBOARD: &str = "\u{E765}";

/// Banner icon for a Bluetooth device.
pub fn device_icon(kind: crate::bluetooth::DeviceKind) -> &'static str {
    use crate::bluetooth::DeviceKind::*;
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
    cover: Option<(u64, Option<ID2D1Bitmap>)>,
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
            Ok(Renderer { factory, rt, dw, wic, mem, bmp: HBITMAP::default(), old: HGDIOBJ::default(), w: 0, h: 0, fonts: None, cover: None })
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
            let ui = w!("Segoe UI");
            let set = vec![
                make(ui, 15.0, DWRITE_FONT_WEIGHT_BOLD)?,
                make(ui, 13.0, DWRITE_FONT_WEIGHT_NORMAL)?,
                make(ui, 12.0, DWRITE_FONT_WEIGHT_NORMAL)?,
                make(w!("Segoe Fluent Icons"), 16.0, DWRITE_FONT_WEIGHT_NORMAL)?,
                make(ui, 14.0, DWRITE_FONT_WEIGHT_SEMI_BOLD)?,
                make(w!("Segoe Fluent Icons"), 11.0, DWRITE_FONT_WEIGHT_NORMAL)?,
            ];
            self.fonts = Some((scale, set));
        }
        Ok(self.fonts.as_ref().unwrap().1.clone())
    }

    /// Decode the cover once per track (WIC → premultiplied BGRA → D2D bitmap).
    fn cover(&mut self, t: &Track) -> Option<ID2D1Bitmap> {
        if self.cover.as_ref().map(|c| c.0) != Some(t.art_key) {
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
            self.cover = Some((t.art_key, decoded));
        }
        self.cover.as_ref().and_then(|c| c.1.clone())
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

    /// Draw one frame and push it to `hwnd` placed at screen (x, y), window size (w, h) device px.
    pub fn present(&mut self, hwnd: HWND, x: i32, y: i32, w: i32, h: i32, scale: f32, v: &View) -> Result<()> {
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
                    text(t.icon.unwrap_or(ICON_BELL), F_ICON, rect(ix, iy, ix + d, iy + d), center, grey(1.0, a))?;
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
                        None => text(&t.body, F_BODY, rect(tl, 42.0 * s, tr, 58.0 * s), lead, grey(0.75, a))?,
                    }
                }
            }

            let a = fade(110.0, 50.0);
            if a > 0.0 && v.mode != Mode::Alert {
                let (cl, cr) = (l + SIDE * s, r - SIDE * s);
                // Header shared by every page: battery · date · time.
                text(v.battery, F_LABEL, rect(l + PAD * s, 6.0 * s, cl + 60.0 * s, 28.0 * s), lead, grey(0.6, a))?;
                text(&format!("{}   {}", v.date, v.time), F_SMALL, rect(cl, 6.0 * s, cr, 28.0 * s), center, grey(1.0, a))?;

                let btns = buttons(cx, s, v.page);
                match v.page {
                    Page::Music => {
                        let top = 38.0 * s;
                        draw_cover(rect(cl, top, cl + COVER * s, top + COVER * s), 10.0 * s, a)?;
                        let tl = cl + (COVER + 12.0) * s;
                        match v.music {
                            Some(t) => {
                                let title = if t.title.is_empty() { "Đang phát" } else { &t.title };
                                text(title, F_TITLE, rect(tl, top + 2.0 * s, cr, top + 20.0 * s), lead, grey(1.0, a))?;
                                text(&t.artist, F_BODY, rect(tl, top + 20.0 * s, cr, top + 36.0 * s), lead, grey(0.65, a))?;
                                let lyric = t.lyric_now().unwrap_or(if t.lyrics.is_empty() { "" } else { "♪" });
                                text(lyric, F_BODY, rect(tl, top + 40.0 * s, cr, top + 58.0 * s), lead, grey(0.95, a))?;
                            }
                            None => text("Không có bài", F_TITLE, rect(tl, top, cr, top + COVER * s), lead, grey(1.0, a))?,
                        }
                        let pos = v.music.map_or(0, |t| t.position_ms());
                        let dur = v.music.map_or(0, |t| t.duration_ms);
                        let by = 112.0 * s;
                        let lw = 40.0 * s;
                        let (bl, brr) = (cl + lw, cr - lw);
                        text(&clock(pos), F_LABEL, rect(cl, by - 9.0 * s, bl - 4.0 * s, by + 11.0 * s), lead, grey(0.6, a))?;
                        text(&format!("-{}", clock(dur.saturating_sub(pos))), F_LABEL, rect(brr + 4.0 * s, by - 9.0 * s, cr, by + 11.0 * s), trail, grey(0.6, a))?;
                        let th = 4.0 * s;
                        let bar = |right: f32| rounded(rect(bl, by - th / 2.0, right, by + th / 2.0), th / 2.0);
                        rt.FillRoundedRectangle(&bar(brr), &brush(grey(0.25, a))?);
                        if dur > 0 {
                            let p = (pos as f32 / dur as f32).clamp(0.0, 1.0);
                            rt.FillRoundedRectangle(&bar(bl + (brr - bl) * p), &brush(grey(1.0, a))?);
                        }
                    }
                    Page::Notifications => {
                        text("Thông báo", F_LABEL, rect(cl, 34.0 * s, cr, 50.0 * s), lead, grey(0.6, a))?;
                        let card = rounded(rect(cl, 54.0 * s, cr, 146.0 * s), 14.0 * s);
                        rt.FillRoundedRectangle(&card, &brush(grey(0.11, a))?);
                        let (tl, tr) = (cl + 14.0 * s, cr - 14.0 * s);
                        let top = card.rect.top;
                        match v.toast {
                            Some(t) => {
                                text(&t.app, F_LABEL, rect(tl, top + 12.0 * s, tr, top + 28.0 * s), lead, grey(0.6, a))?;
                                text(&t.title, F_TITLE, rect(tl, top + 32.0 * s, tr, top + 52.0 * s), lead, grey(1.0, a))?;
                                text(&t.body, F_BODY, rect(tl, top + 54.0 * s, tr, top + 74.0 * s), lead, grey(0.75, a))?;
                            }
                            None => text("Không có thông báo", F_BODY, card.rect, center, grey(0.55, a))?,
                        }
                    }
                    Page::Controls => {}
                }

                // Page dots: current one is a short white pill.
                let n = PAGES.len() as f32;
                let current = PAGES.iter().position(|p| *p == v.page).unwrap_or(0) as f32;
                let (dot, wide, gap) = (5.0 * s, 14.0 * s, 5.0 * s);
                let total = wide + (n - 1.0) * dot + (n - 1.0) * gap;
                let mut dx = cx - total / 2.0;
                for i in 0..PAGES.len() {
                    let dw = if i as f32 == current { wide } else { dot };
                    let c = if i as f32 == current { grey(1.0, a) } else { grey(0.45, a) };
                    rt.FillRoundedRectangle(&rounded(rect(dx, DOTS_Y * s, dx + dw, DOTS_Y * s + dot), dot / 2.0), &brush(c)?);
                    dx += dw + gap;
                }

                let playing = v.music.is_some_and(|t| t.playing);
                for &(b, br) in &btns {
                    match b {
                        Button::VolBar => {
                            let level = v.volume.map_or(0.0, |(lv, _)| lv);
                            let track_h = 6.0 * s;
                            let my = (br.top + br.bottom) / 2.0;
                            let track = |right: f32| rounded(rect(br.left, my - track_h / 2.0, right, my + track_h / 2.0), track_h / 2.0);
                            rt.FillRoundedRectangle(&track(br.right), &brush(grey(0.25, a))?);
                            rt.FillRoundedRectangle(&track(br.left + (br.right - br.left) * level), &brush(grey(1.0, a))?);
                            if let Some((level, _)) = v.volume {
                                let label = format!("Âm lượng {}%", (level * 100.0).round());
                                text(&label, F_LABEL, rect(br.left, br.top - 16.0 * s, br.right, br.top + 2.0 * s), center, grey(0.6, a))?;
                            }
                        }
                        Button::Mic | Button::Bluetooth => {
                            // Wide pill: icon + label + state.
                            let (icon, label, on, missing) = if b == Button::Mic {
                                (ICON_MIC, "Mic", v.mic_muted == Some(false), v.mic_muted.is_none())
                            } else {
                                (ICON_BT, "Bluetooth", v.bt == Bt::On, v.bt == Bt::Missing)
                            };
                            let warn = b == Button::Mic && v.mic_muted == Some(true);
                            let bg = if warn { color(1.0, 0.27, 0.23, a) } else if on { grey(1.0, a) } else { grey(0.17, a) };
                            let fg = if on && !warn { grey(0.0, a) } else { grey(1.0, if missing { a * 0.35 } else { a }) };
                            rt.FillRoundedRectangle(&rounded(br, (br.bottom - br.top) / 2.0), &brush(bg)?);
                            let ib = rect(br.left + 8.0 * s, br.top, br.left + 36.0 * s, br.bottom);
                            text(icon, F_ICON, ib, center, fg)?;
                            let state = if missing { "Không có" } else if on { "Bật" } else { "Tắt" };
                            text(&format!("{label}: {state}"), F_SMALL, rect(ib.right + 4.0 * s, br.top, br.right - 10.0 * s, br.bottom), lead, fg)?;
                        }
                        _ => {
                            // (icon, lit = white with black icon, warn = red, small = side arrow)
                            let (icon, lit, warn) = match b {
                                Button::SpeakerMute => match v.volume {
                                    Some((_, true)) => (ICON_MUTE, false, true),
                                    _ => (ICON_VOLUME, false, false),
                                },
                                Button::VolDown => (ICON_MINUS, false, false),
                                Button::VolUp => (ICON_PLUS, false, false),
                                Button::Previous => (ICON_PREV, false, false),
                                Button::PlayPause => (if playing { ICON_PAUSE } else { ICON_PLAY }, true, false),
                                Button::Next => (ICON_NEXT, false, false),
                                Button::PagePrev => (ICON_LEFT, false, false),
                                _ => (ICON_RIGHT, false, false),
                            };
                            let bg = if warn { color(1.0, 0.27, 0.23, a) } else if lit { grey(1.0, a) } else { grey(0.17, a) };
                            let fg = if lit { grey(0.0, a) } else { grey(1.0, a) };
                            let circle = D2D1_ELLIPSE {
                                point: Vector2 { X: (br.left + br.right) / 2.0, Y: (br.top + br.bottom) / 2.0 },
                                radiusX: (br.right - br.left) / 2.0,
                                radiusY: (br.bottom - br.top) / 2.0,
                            };
                            rt.FillEllipse(&circle, &brush(bg)?);
                            text(icon, if matches!(b, Button::PagePrev | Button::PageNext) { F_LABEL_ICON } else { F_ICON }, br, center, fg)?;
                        }
                    }
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
        }
        Ok(())
    }
}

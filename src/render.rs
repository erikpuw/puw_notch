//! Direct2D drawing into a 32-bit DIB, shown with UpdateLayeredWindow (per-pixel alpha).
//! Pixels with alpha 0 let the mouse through, so the drawn shape is the hit area.

use windows::core::{w, Result, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, POINT, RECT, SIZE};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::bluetooth::Bt;
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
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Button {
    SpeakerMute,
    VolDown,
    VolBar,
    VolUp,
    Mic,
    Bluetooth,
}

const PAD: f32 = 24.0;
/// Radius of the concave flares joining the notch sides to the screen edge.
const EAR: f32 = 7.0;
const BTN: f32 = 36.0;
const CONTROLS_TOP: f32 = 146.0;

/// Control rects in window px for the settled Expanded panel centred at `cx`.
pub fn buttons(cx: f32, scale: f32) -> [(Button, D2D_RECT_F); 6] {
    let w = target(Mode::Expanded)[0];
    let x0 = cx / scale - w / 2.0 + PAD;
    let inner = w - 2.0 * PAD;
    let r = |a: f32, b: f32| rect((x0 + a) * scale, CONTROLS_TOP * scale, (x0 + b) * scale, (CONTROLS_TOP + BTN) * scale);
    let gap = 8.0;
    let right = |i: f32| inner - BTN * (i + 1.0) - gap * i;
    [
        (Button::SpeakerMute, r(0.0, BTN)),
        (Button::VolDown, r(BTN + gap, 2.0 * BTN + gap)),
        (Button::VolBar, r(2.0 * (BTN + gap), right(2.0) - gap)),
        (Button::VolUp, r(right(2.0), right(2.0) + BTN)),
        (Button::Mic, r(right(1.0), right(1.0) + BTN)),
        (Button::Bluetooth, r(right(0.0), right(0.0) + BTN)),
    ]
}

/// Text styles, indexed by these constants.
const F_SMALL: usize = 0;
const F_BIG: usize = 1;
const F_BODY: usize = 2;
const F_LABEL: usize = 3;
const F_ICON: usize = 4;
const F_TITLE: usize = 5;

const ICON_VOLUME: &str = "\u{E767}";
const ICON_MUTE: &str = "\u{E74F}";
const ICON_MINUS: &str = "\u{E738}";
const ICON_PLUS: &str = "\u{E710}";
const ICON_MIC: &str = "\u{E720}";
const ICON_BT: &str = "\u{E702}";

pub struct Renderer {
    factory: ID2D1Factory,
    rt: ID2D1DCRenderTarget,
    dw: IDWriteFactory,
    mem: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    w: i32,
    h: i32,
    fonts: Option<(f32, Vec<IDWriteTextFormat>)>,
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
            let mem = CreateCompatibleDC(None);
            Ok(Renderer { factory, rt, dw, mem, bmp: HBITMAP::default(), old: HGDIOBJ::default(), w: 0, h: 0, fonts: None })
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
                make(ui, 13.0, DWRITE_FONT_WEIGHT_SEMI_BOLD)?,
                make(ui, 36.0, DWRITE_FONT_WEIGHT_SEMI_BOLD)?,
                make(ui, 13.0, DWRITE_FONT_WEIGHT_NORMAL)?,
                make(ui, 12.0, DWRITE_FONT_WEIGHT_NORMAL)?,
                make(w!("Segoe Fluent Icons"), 16.0, DWRITE_FONT_WEIGHT_NORMAL)?,
                make(ui, 14.0, DWRITE_FONT_WEIGHT_SEMI_BOLD)?,
            ];
            self.fonts = Some((scale, set));
        }
        Ok(self.fonts.as_ref().unwrap().1.clone())
    }

    /// iPhone-style notch: flat top on the screen edge, concave "ears" flaring into it,
    /// straight sides, round bottom corners.
    fn notch_shape(&self, l: f32, r: f32, h: f32, radius: f32, ear: f32) -> Result<ID2D1PathGeometry> {
        let p = |x: f32, y: f32| windows_numerics::Vector2 { X: x, Y: y };
        let rad = radius.min(h - ear).min((r - l) / 2.0).max(0.0);
        let arc = |to: windows_numerics::Vector2, size: f32, dir: D2D1_SWEEP_DIRECTION| D2D1_ARC_SEGMENT {
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
            let lead = DWRITE_TEXT_ALIGNMENT_LEADING;
            let center = DWRITE_TEXT_ALIGNMENT_CENTER;
            let trail = DWRITE_TEXT_ALIGNMENT_TRAILING;

            rt.BeginDraw();
            rt.Clear(Some(&color(0.0, 0.0, 0.0, 0.0)));

            let ear = (EAR * s).min(nh / 2.0);
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
            }

            if v.mode == Mode::Alert {
                if let Some(t) = v.banner {
                    let a = fade(44.0, 18.0);
                    let (tl, tr) = (l + 22.0 * s, r - 22.0 * s);
                    text(&t.app, F_LABEL, rect(tl, 8.0 * s, tr, 24.0 * s), lead, grey(0.6, a))?;
                    text(&t.title, F_TITLE, rect(tl, 24.0 * s, tr, 42.0 * s), lead, grey(1.0, a))?;
                    match t.level {
                        Some(level) => {
                            let th = 5.0 * s;
                            let my = 50.0 * s;
                            let bar = |right: f32| D2D1_ROUNDED_RECT { rect: rect(tl, my - th / 2.0, right, my + th / 2.0), radiusX: th / 2.0, radiusY: th / 2.0 };
                            rt.FillRoundedRectangle(&bar(tr), &brush(grey(0.25, a))?);
                            rt.FillRoundedRectangle(&bar(tl + (tr - tl) * level.clamp(0.0, 1.0)), &brush(grey(1.0, a))?);
                        }
                        None => text(&t.body, F_BODY, rect(tl, 42.0 * s, tr, 58.0 * s), lead, grey(0.75, a))?,
                    }
                }
            }

            let a = fade(130.0, 60.0);
            if a > 0.0 && v.mode != Mode::Alert {
                let (pl, pr) = (l + PAD * s, r - PAD * s);
                text(v.time, F_BIG, rect(pl, 12.0 * s, pr, 60.0 * s), lead, grey(1.0, a))?;
                text(v.date, F_BODY, rect(pl, 16.0 * s, pr, 36.0 * s), trail, grey(0.75, a))?;
                text(v.battery, F_BODY, rect(pl, 36.0 * s, pr, 56.0 * s), trail, grey(0.75, a))?;

                let card = D2D1_ROUNDED_RECT { rect: rect(pl, 68.0 * s, pr, 132.0 * s), radiusX: 14.0 * s, radiusY: 14.0 * s };
                rt.FillRoundedRectangle(&card, &brush(grey(0.11, a))?);
                let (tl, tr) = (pl + 14.0 * s, pr - 14.0 * s);
                match v.toast {
                    Some(t) => {
                        text(&t.app, F_LABEL, rect(tl, 74.0 * s, tr, 90.0 * s), lead, grey(0.6, a))?;
                        text(&t.title, F_TITLE, rect(tl, 90.0 * s, tr, 108.0 * s), lead, grey(1.0, a))?;
                        text(&t.body, F_BODY, rect(tl, 108.0 * s, tr, 126.0 * s), lead, grey(0.75, a))?;
                    }
                    None => text("Không có thông báo", F_BODY, card.rect, center, grey(0.55, a))?,
                }

                for (b, br) in buttons(cx, s) {
                    if b == Button::VolBar {
                        let level = v.volume.map_or(0.0, |(lv, _)| lv);
                        let track_h = 6.0 * s;
                        let my = (br.top + br.bottom) / 2.0;
                        let track = |right: f32| D2D1_ROUNDED_RECT { rect: rect(br.left, my - track_h / 2.0, right, my + track_h / 2.0), radiusX: track_h / 2.0, radiusY: track_h / 2.0 };
                        rt.FillRoundedRectangle(&track(br.right), &brush(grey(0.25, a))?);
                        rt.FillRoundedRectangle(&track(br.left + (br.right - br.left) * level), &brush(grey(1.0, a))?);
                        continue;
                    }
                    // (icon, lit = white button with black icon, warn = red)
                    let (icon, lit, warn) = match b {
                        Button::SpeakerMute => match v.volume {
                            Some((_, true)) => (ICON_MUTE, false, true),
                            _ => (ICON_VOLUME, false, false),
                        },
                        Button::VolDown => (ICON_MINUS, false, false),
                        Button::VolUp => (ICON_PLUS, false, false),
                        Button::Mic => (ICON_MIC, false, v.mic_muted == Some(true)),
                        Button::Bluetooth => (ICON_BT, v.bt == Bt::On, false),
                        Button::VolBar => unreachable!(),
                    };
                    let missing = (b == Button::Mic && v.mic_muted.is_none()) || (b == Button::Bluetooth && v.bt == Bt::Missing);
                    let bg = if warn { color(1.0, 0.27, 0.23, a) } else if lit { grey(1.0, a) } else { grey(0.17, a) };
                    let fg = if lit { grey(0.0, a) } else { grey(1.0, if missing { a * 0.35 } else { a }) };
                    let circle = D2D1_ELLIPSE {
                        point: windows_numerics::Vector2 { X: (br.left + br.right) / 2.0, Y: (br.top + br.bottom) / 2.0 },
                        radiusX: (br.right - br.left) / 2.0,
                        radiusY: (br.bottom - br.top) / 2.0,
                    };
                    rt.FillEllipse(&circle, &brush(bg)?);
                    text(icon, F_ICON, br, center, fg)?;
                }
                if let Some((level, _)) = v.volume {
                    let bar = buttons(cx, s)[2].1;
                    let label = format!("{}%", (level * 100.0).round());
                    text(&label, F_LABEL, rect(bar.left, bar.top - 14.0 * s, bar.right, bar.top + 4.0 * s), center, grey(0.6, a))?;
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

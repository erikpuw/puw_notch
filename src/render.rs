//! Direct2D drawing into a 32-bit DIB, shown with UpdateLayeredWindow (per-pixel alpha).
//! Pixels with alpha 0 let the mouse through, so the drawn shape is the hit area.

use windows::core::{w, Result};
use windows::Win32::Foundation::{COLORREF, HWND, POINT, RECT, SIZE};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::notch::{Mode, HOVER_MARGIN};

/// Everything the painter needs for one frame (logical px).
pub struct View<'a> {
    pub mode: Mode,
    pub size: [f32; 3],
    pub time: &'a str,
    pub date: &'a str,
    pub battery: &'a str,
}

pub struct Renderer {
    rt: ID2D1DCRenderTarget,
    dw: IDWriteFactory,
    mem: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    w: i32,
    h: i32,
    fonts: Option<(f32, [IDWriteTextFormat; 3])>,
}

fn color(r: f32, g: f32, b: f32, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r, g, b, a }
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
            Ok(Renderer { rt, dw, mem, bmp: HBITMAP::default(), old: HGDIOBJ::default(), w: 0, h: 0, fonts: None })
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

    fn fonts(&mut self, scale: f32) -> Result<&[IDWriteTextFormat; 3]> {
        if self.fonts.as_ref().map(|f| f.0) != Some(scale) {
            let make = |size: f32, weight: DWRITE_FONT_WEIGHT| -> Result<IDWriteTextFormat> {
                unsafe {
                    let f = self.dw.CreateTextFormat(w!("Segoe UI"), None, weight, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, size * scale, w!("vi-VN"))?;
                    f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
                    f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
                    Ok(f)
                }
            };
            let set = [make(13.0, DWRITE_FONT_WEIGHT_SEMI_BOLD)?, make(40.0, DWRITE_FONT_WEIGHT_SEMI_BOLD)?, make(14.0, DWRITE_FONT_WEIGHT_NORMAL)?];
            self.fonts = Some((scale, set));
        }
        Ok(&self.fonts.as_ref().unwrap().1)
    }

    /// Draw one frame and push it to `hwnd` placed at screen (x, y), window size (w, h) device px.
    pub fn present(&mut self, hwnd: HWND, x: i32, y: i32, w: i32, h: i32, scale: f32, v: &View) -> Result<()> {
        self.ensure_surface(w, h)?;
        let fonts = self.fonts(scale)?.clone();
        let rt = &self.rt;
        let [nw, nh, nr] = v.size.map(|s| s * scale);
        let cx = w as f32 / 2.0;
        let (l, r) = (cx - nw / 2.0, cx + nw / 2.0);
        unsafe {
            rt.BeginDraw();
            rt.Clear(Some(&color(0.0, 0.0, 0.0, 0.0)));

            if !matches!(v.mode, Mode::Mini | Mode::PeekMini) {
                // Alpha 1/255: invisible but still catches the mouse.
                let m = HOVER_MARGIN * scale;
                let ghost = rt.CreateSolidColorBrush(&color(0.0, 0.0, 0.0, 1.0 / 255.0), None)?;
                rt.FillRectangle(&rect(l - m, 0.0, r + m, nh + m), &ghost);
            }
            let black = rt.CreateSolidColorBrush(&color(0.0, 0.0, 0.0, 1.0), None)?;
            // Rounded rect pushed up by the radius: flat top glued to the screen edge, round bottom.
            let shape = D2D1_ROUNDED_RECT { rect: rect(l, -nr, r, nh), radiusX: nr, radiusY: nr };
            rt.FillRoundedRectangle(&shape, &black);

            let white = |a: f32| rt.CreateSolidColorBrush(&color(1.0, 1.0, 1.0, a), None);
            let draw = |text: &str, f: &IDWriteTextFormat, box_: D2D_RECT_F, align: DWRITE_TEXT_ALIGNMENT, a: f32| -> Result<()> {
                let t: Vec<u16> = text.encode_utf16().collect();
                f.SetTextAlignment(align)?;
                rt.DrawText(&t, f, &box_, &white(a)?, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
                Ok(())
            };

            // Collapsed content fades out as the pill grows, expanded content fades in.
            let base_h = 30.0 * scale;
            let small_a = (1.0 - (nh - base_h * 1.3) / (base_h * 0.8)).clamp(0.0, 1.0);
            if nh >= 20.0 * scale && small_a > 0.0 {
                draw(v.time, &fonts[0], rect(l, 0.0, r, nh), DWRITE_TEXT_ALIGNMENT_CENTER, small_a)?;
            }
            let big_a = ((nh - 90.0 * scale) / (40.0 * scale)).clamp(0.0, 1.0);
            if big_a > 0.0 {
                let pad = 26.0 * scale;
                draw(v.time, &fonts[1], rect(l + pad, 18.0 * scale, r - pad, 74.0 * scale), DWRITE_TEXT_ALIGNMENT_LEADING, big_a)?;
                draw(v.date, &fonts[2], rect(l + pad, 76.0 * scale, r - pad, 100.0 * scale), DWRITE_TEXT_ALIGNMENT_LEADING, big_a * 0.75)?;
                draw(v.battery, &fonts[2], rect(l + pad, 18.0 * scale, r - pad, 74.0 * scale), DWRITE_TEXT_ALIGNMENT_TRAILING, big_a * 0.75)?;
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

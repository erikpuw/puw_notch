use std::mem::ManuallyDrop;

use super::*;
use crate::settings_win::{nav_rects, Action, Item, Placed, Rect, Section, Thumb, CLOSE, CW, CX, DRAG_H, HEIGHT, NAV_W};

const ICON_CLOSE: &str = "\u{E711}";
const ICON_CHECK_MARK: &str = "\u{E73E}";
const WIN_RADIUS: f32 = 16.0;
const GROUP_RADIUS: f32 = 12.0;
const SLIDE: f32 = 8.0;

pub struct SettingsView<'a> {
    pub section: Section,
    pub placed: &'a [Placed],
    pub scroll: f32,
    pub max_scroll: f32,
    pub hovered: Option<Action>,
    pub title: &'a str,
    pub hint: &'a str,
    pub version: &'a str,
    pub pane: f32,
    pub flips: &'a [(Action, f32)],
}

fn scaled(q: Rect, s: f32) -> D2D_RECT_F {
    rect(q.x * s, q.y * s, (q.x + q.w) * s, (q.y + q.h) * s)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

impl Renderer {
    fn window_shape(&self, w: i32, h: i32, s: f32) -> Result<ID2D1PathGeometry> {
        unsafe {
            let round = self.factory.CreateRoundedRectangleGeometry(&rounded(rect(0.0, 0.0, w as f32, h as f32), WIN_RADIUS * s))?;
            let path = self.factory.CreatePathGeometry()?;
            let sink = path.Open()?;
            round.Simplify(D2D1_GEOMETRY_SIMPLIFICATION_OPTION_CUBICS_AND_LINES, None, 0.25, &sink.cast::<ID2D1SimplifiedGeometrySink>()?)?;
            sink.Close()?;
            Ok(path)
        }
    }

    pub fn present_settings(&mut self, hwnd: HWND, w: i32, h: i32, s: f32, v: &SettingsView) -> Result<()> {
        self.ensure_surface(hwnd, w, h)?;
        let shape = self.window_shape(w, h, s)?;
        if let Some(surface) = self.surface.as_mut() {
            surface.sync_glass(&shape, 0.0, 1.0);
        }
        let fonts = self.fonts(s)?;
        let logo = self.logo();
        let rt = self.rt.clone();
        let (center, lead, trail) = (DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_ALIGNMENT_TRAILING);
        unsafe {
            let text = |t: &str, f: usize, b: D2D_RECT_F, align: DWRITE_TEXT_ALIGNMENT, c: D2D1_COLOR_F| -> Result<()> {
                let t: Vec<u16> = t.encode_utf16().collect();
                fonts[f].SetTextAlignment(align)?;
                rt.DrawText(&t, &fonts[f], &b, &self.brush(c)?, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
                Ok(())
            };
            let fill = |b: D2D_RECT_F, radius: f32, c: D2D1_COLOR_F| -> Result<()> {
                rt.FillRoundedRectangle(&rounded(b, radius), &self.brush(c)?);
                Ok(())
            };
            let outline = |b: D2D_RECT_F, radius: f32, width: f32, c: D2D1_COLOR_F| -> Result<()> {
                let half = width / 2.0;
                let inner = rect(b.left + half, b.top + half, b.right - half, b.bottom - half);
                rt.DrawRoundedRectangle(&rounded(inner, radius - half), &self.brush(c)?, width, None);
                Ok(())
            };
            let line = |x0: f32, x1: f32, y: f32, c: D2D1_COLOR_F| -> Result<()> {
                rt.FillRectangle(&rect(x0, y, x1, y + 1.0_f32.max(s.floor())), &self.brush(c)?);
                Ok(())
            };
            let disc = |cx: f32, cy: f32, r: f32, c: D2D1_COLOR_F| -> Result<()> {
                rt.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cy }, radiusX: r, radiusY: r }, &self.brush(c)?);
                Ok(())
            };
            let clipped = |b: D2D_RECT_F, radius: f32, draw: &dyn Fn() -> Result<()>| -> Result<()> {
                let geometry = self.factory.CreateRoundedRectangleGeometry(&rounded(b, radius))?;
                let params = D2D1_LAYER_PARAMETERS {
                    contentBounds: b,
                    geometricMask: ManuallyDrop::new(Some(geometry.into())),
                    maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                    maskTransform: Matrix3x2::identity(),
                    opacity: 1.0,
                    opacityBrush: ManuallyDrop::new(None),
                    layerOptions: D2D1_LAYER_OPTIONS_NONE,
                };
                rt.PushLayer(&params, None);
                let drawn = draw();
                rt.PopLayer();
                drop(ManuallyDrop::into_inner(params.geometricMask));
                drawn
            };
            let hot = |a: Action| v.hovered == Some(a);
            let green = |a: f32| color(GREEN.0, GREEN.1, GREEN.2, a);
            let flip = |a: Action| v.flips.iter().find(|(x, _)| *x == a).map(|(_, p)| *p);

            rt.BeginDraw();
            rt.Clear(Some(&color(0.0, 0.0, 0.0, 0.0)));
            let window = rect(0.0, 0.0, w as f32, h as f32);
            fill(window, WIN_RADIUS * s, color(0.055, 0.055, 0.07, 0.76))?;
            clipped(rect(0.0, 0.0, NAV_W * s, h as f32), 0.0, &|| fill(rect(0.0, 0.0, (NAV_W + 40.0) * s, h as f32), WIN_RADIUS * s, grey(1.0, 0.04)))?;
            outline(window, WIN_RADIUS * s, 1.0, grey(1.0, 0.13))?;
            line((WIN_RADIUS + 2.0) * s, w as f32 - (WIN_RADIUS + 2.0) * s, 1.0, grey(1.0, 0.10))?;
            line(NAV_W * s, NAV_W * s + 1.0, 0.0, grey(0.0, 0.0))?;
            rt.FillRectangle(&rect(NAV_W * s, 1.0, NAV_W * s + 1.0, h as f32 - 1.0), &self.brush(grey(1.0, 0.08))?);

            if let Some(logo) = &logo {
                rt.DrawBitmap(logo, Some(&rect(18.0 * s, 20.0 * s, 48.0 * s, 50.0 * s)), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None);
            }
            text("PUW Notch", F_TITLE, rect(58.0 * s, 20.0 * s, (NAV_W - 10.0) * s, 37.0 * s), lead, grey(1.0, 1.0))?;
            text(&format!("{} {}", crate::i18n::tr("Phiên bản", "Version"), v.version), F_LABEL, rect(58.0 * s, 36.0 * s, (NAV_W - 10.0) * s, 52.0 * s), lead, grey(0.45, 1.0))?;
            for (section, q) in nav_rects() {
                let b = scaled(q, s);
                let active = section == v.section;
                let over = hot(Action::Go(section));
                if active {
                    fill(b, 8.0 * s, grey(1.0, 0.10))?;
                } else if over {
                    fill(b, 8.0 * s, grey(1.0, 0.06))?;
                }
                let ink = if active { grey(1.0, 1.0) } else if over { grey(1.0, 0.92) } else { grey(0.6, 1.0) };
                text(section.icon(), F_ICON, rect(b.left + 8.0 * s, b.top, b.left + 30.0 * s, b.bottom), center, if active { green(1.0) } else { ink })?;
                text(section.title(), F_BODY, rect(b.left + 40.0 * s, b.top, b.right - 6.0 * s, b.bottom), lead, ink)?;
            }
            let close = scaled(CLOSE, s);
            if hot(Action::Close) {
                disc((close.left + close.right) / 2.0, (close.top + close.bottom) / 2.0, 13.0 * s, grey(1.0, 0.12))?;
            }
            text(ICON_CLOSE, F_SMALL_ICON, close, center, grey(if hot(Action::Close) { 1.0 } else { 0.6 }, 1.0))?;

            let viewport = rect(NAV_W * s + 1.0, DRAG_H * s, w as f32 - 1.0, h as f32 - 1.0);
            let layer = D2D1_LAYER_PARAMETERS {
                contentBounds: viewport,
                geometricMask: ManuallyDrop::new(None),
                maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                maskTransform: Matrix3x2::identity(),
                opacity: v.pane,
                opacityBrush: ManuallyDrop::new(None),
                layerOptions: D2D1_LAYER_OPTIONS_NONE,
            };
            rt.PushLayer(&layer, None);
            rt.SetTransform(&Matrix3x2::translation(0.0, (-v.scroll + (1.0 - v.pane) * SLIDE) * s));
            text(v.title, F_BIG, rect(CX * s, DRAG_H * s, (CX + CW) * s, (DRAG_H + 34.0) * s), lead, grey(1.0, 1.0))?;
            text(v.hint, F_LABEL, rect(CX * s, (DRAG_H + 38.0) * s, (CX + CW) * s, (DRAG_H + 58.0) * s), lead, grey(0.6, 1.0))?;

            let mut run: Option<(f32, f32)> = None;
            for p in v.placed {
                if p.item.is_row() {
                    run = Some(run.map_or((p.rect.y, p.rect.y + p.rect.h), |(top, _)| (top, p.rect.y + p.rect.h)));
                } else if let Some((top, bottom)) = run.take() {
                    fill(scaled(Rect { x: CX, y: top, w: CW, h: bottom - top }, s), GROUP_RADIUS * s, grey(1.0, 0.055))?;
                }
            }
            if let Some((top, bottom)) = run {
                fill(scaled(Rect { x: CX, y: top, w: CW, h: bottom - top }, s), GROUP_RADIUS * s, grey(1.0, 0.055))?;
            }

            let mut first_row = true;
            for p in v.placed {
                let q = scaled(p.rect, s);
                let is_row = p.item.is_row();
                if !is_row {
                    first_row = true;
                }
                match &p.item {
                    Item::Heading { text: t, aside } => {
                        text(t, F_LABEL, rect(q.left + 2.0 * s, q.top + 22.0 * s, q.right, q.bottom), lead, grey(0.6, 1.0))?;
                        if let Some(a) = aside {
                            text(a, F_LABEL, rect(q.left, q.top + 22.0 * s, q.right, q.bottom), trail, grey(0.6, 1.0))?;
                        }
                    }
                    Item::Stage { pill, raised, glass } => {
                        fill(q, 14.0 * s, color(0.047, 0.043, 0.078, 1.0))?;
                        clipped(q, 14.0 * s, &|| {
                            let blob = |cx: f32, cy: f32, rx: f32, ry: f32, c: (f32, f32, f32), a: f32| -> Result<()> {
                                let stops = rt.CreateGradientStopCollection(&[D2D1_GRADIENT_STOP { position: 0.0, color: color(c.0, c.1, c.2, a) }, D2D1_GRADIENT_STOP { position: 1.0, color: color(c.0, c.1, c.2, 0.0) }], D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP)?;
                                let props = D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES { center: Vector2 { X: cx, Y: cy }, gradientOriginOffset: Vector2 { X: 0.0, Y: 0.0 }, radiusX: rx, radiusY: ry };
                                let brush = rt.CreateRadialGradientBrush(&props, None, &stops)?;
                                rt.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cy }, radiusX: rx, radiusY: ry }, &brush);
                                Ok(())
                            };
                            blob(q.left + 90.0 * s, q.top + 90.0 * s, 150.0 * s, 110.0 * s, (0.416, 0.275, 1.0), 0.75)?;
                            blob(q.right - 80.0 * s, q.top + 50.0 * s, 140.0 * s, 110.0 * s, (1.0, 0.24, 0.545), 0.7)?;
                            blob((q.left + q.right) / 2.0, q.bottom - 6.0 * s, 150.0 * s, 90.0 * s, (0.09, 0.70, 0.77), 0.55)?;
                            let (nl, nr, nh) = ((q.left + q.right) / 2.0 - 136.0 * s, (q.left + q.right) / 2.0 + 136.0 * s, 44.0 * s);
                            let top = q.top + if *pill { 16.0 * s } else { 0.0 };
                            let shape = if *pill { self.pill_shape(nl, nr, nh, 22.0 * s)? } else { self.notch_shape(nl, nr, nh, 22.0 * s, 16.0 * s, true)? };
                            rt.SetTransform(&(Matrix3x2::translation(0.0, top) * Matrix3x2::translation(0.0, (-v.scroll + (1.0 - v.pane) * SLIDE) * s)));
                            rt.FillGeometry(&shape, &self.brush(grey(0.0, if *glass { 0.62 } else { 1.0 }))?, None);
                            if *raised && *glass {
                                rt.DrawGeometry(&shape, &self.brush(grey(1.0, 0.22))?, 1.0, &self.round);
                            }
                            rt.SetTransform(&Matrix3x2::translation(0.0, (-v.scroll + (1.0 - v.pane) * SLIDE) * s));
                            text("21:24", F_TITLE, rect(nl + 18.0 * s, top, nl + 120.0 * s, top + nh), lead, grey(1.0, 1.0))?;
                            rt.DrawEllipse(&D2D1_ELLIPSE { point: Vector2 { X: nr - 26.0 * s, Y: top + nh / 2.0 }, radiusX: 7.0 * s, radiusY: 7.0 * s }, &self.brush(green(1.0))?, 2.5 * s, None);
                            text(crate::i18n::tr("Xem trước trực tiếp", "Live preview"), F_LABEL, rect(q.left + 14.0 * s, q.bottom - 26.0 * s, q.right, q.bottom - 6.0 * s), lead, grey(1.0, 0.7))
                        })?;
                        outline(q, 14.0 * s, 1.0, grey(1.0, 0.08))?;
                    }
                    Item::Option { title, note, thumb, on, action } => {
                        let over = hot(*action);
                        fill(q, GROUP_RADIUS * s, if *on { green(0.14) } else { grey(1.0, if over { 0.085 } else { 0.055 }) })?;
                        if *on {
                            outline(q, GROUP_RADIUS * s, 1.5 * s, green(1.0))?;
                        }
                        let tb = rect(q.left + 10.0 * s, q.top + 10.0 * s, q.right - 10.0 * s, q.top + 64.0 * s);
                        let stops = rt.CreateGradientStopCollection(&[D2D1_GRADIENT_STOP { position: 0.0, color: color(0.231, 0.184, 0.561, 1.0) }, D2D1_GRADIENT_STOP { position: 1.0, color: color(0.627, 0.165, 0.424, 1.0) }], D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP)?;
                        let backdrop = rt.CreateLinearGradientBrush(&D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES { startPoint: Vector2 { X: tb.left, Y: tb.top }, endPoint: Vector2 { X: tb.right, Y: tb.bottom } }, None, &stops)?;
                        rt.FillRoundedRectangle(&rounded(tb, 9.0 * s), &backdrop);
                        clipped(tb, 9.0 * s, &|| draw_thumb(&rt, self, tb, *thumb, s))?;
                        text(title, F_BODY, rect(q.left, q.top + 70.0 * s, q.right, q.top + 90.0 * s), center, grey(1.0, 1.0))?;
                        text(note, F_LABEL, rect(q.left + 6.0 * s, q.top + 90.0 * s, q.right - 6.0 * s, q.top + 108.0 * s), center, grey(0.6, 1.0))?;
                    }
                    Item::Tile { label, glyph, on, action } => {
                        let over = hot(*action);
                        fill(q, GROUP_RADIUS * s, if *on { green(0.14) } else { grey(1.0, if over { 0.085 } else { 0.055 }) })?;
                        if *on {
                            outline(q, GROUP_RADIUS * s, 1.5 * s, green(0.7))?;
                            disc(q.right - 20.0 * s, q.top + 20.0 * s, 10.0 * s, green(1.0))?;
                            text(ICON_CHECK_MARK, F_TINY_ICON, rect(q.right - 30.0 * s, q.top + 10.0 * s, q.right - 10.0 * s, q.top + 30.0 * s), center, color(0.02, 0.18, 0.05, 1.0))?;
                        }
                        text(glyph, F_BANNER_ICON, rect(q.left + 12.0 * s, q.top + 12.0 * s, q.left + 44.0 * s, q.top + 40.0 * s), lead, if *on { green(1.0) } else { grey(0.42, 1.0) })?;
                        text(label, F_BODY, rect(q.left + 12.0 * s, q.bottom - 30.0 * s, q.right - 8.0 * s, q.bottom - 8.0 * s), lead, if *on { grey(1.0, 1.0) } else { grey(0.62, 1.0) })?;
                    }
                    Item::Hero { title, value, note, go } => {
                        fill(q, GROUP_RADIUS * s, grey(1.0, if hot(Action::Go(*go)) { 0.085 } else { 0.055 }))?;
                        text(title, F_LABEL, rect(q.left + 16.0 * s, q.top + 14.0 * s, q.right, q.top + 32.0 * s), lead, grey(0.6, 1.0))?;
                        text(value, F_SONG, rect(q.left + 16.0 * s, q.top + 32.0 * s, q.right - 8.0 * s, q.top + 62.0 * s), lead, grey(1.0, 1.0))?;
                        text(note, F_LABEL, rect(q.left + 16.0 * s, q.top + 64.0 * s, q.right - 8.0 * s, q.top + 84.0 * s), lead, green(1.0))?;
                    }
                    Item::Stat { title, value, go } => {
                        fill(q, GROUP_RADIUS * s, grey(1.0, if hot(Action::Go(*go)) { 0.085 } else { 0.055 }))?;
                        text(title, F_LABEL, rect(q.left + 14.0 * s, q.top + 7.0 * s, q.right, q.top + 25.0 * s), lead, grey(0.6, 1.0))?;
                        text(value, F_TITLE, rect(q.left + 14.0 * s, q.top + 24.0 * s, q.right - 8.0 * s, q.top + 44.0 * s), lead, grey(1.0, 1.0))?;
                    }
                    Item::Switch { label, note, glyph, on, action } => {
                        if hot(*action) {
                            fill(rect(q.left + 3.0 * s, q.top + 2.0 * s, q.right - 3.0 * s, q.bottom - 2.0 * s), 8.0 * s, grey(1.0, 0.045))?;
                        }
                        if !first_row {
                            line(q.left + 14.0 * s, q.right, q.top, grey(1.0, 0.08))?;
                        }
                        let mut x = q.left + 14.0 * s;
                        if let Some(g) = glyph {
                            fill(rect(x, (q.top + q.bottom) / 2.0 - 15.0 * s, x + 30.0 * s, (q.top + q.bottom) / 2.0 + 15.0 * s), 8.0 * s, grey(1.0, 0.08))?;
                            text(g, F_ICON, rect(x, (q.top + q.bottom) / 2.0 - 15.0 * s, x + 30.0 * s, (q.top + q.bottom) / 2.0 + 15.0 * s), center, grey(0.62, 1.0))?;
                            x += 42.0 * s;
                        }
                        let track = rect(q.right - 14.0 * s - 40.0 * s, (q.top + q.bottom) / 2.0 - 12.0 * s, q.right - 14.0 * s, (q.top + q.bottom) / 2.0 + 12.0 * s);
                        let t = flip(*action).map_or(if *on { 1.0 } else { 0.0 }, |p| if *on { p } else { 1.0 - p });
                        match note {
                            Some(n) => {
                                text(label, F_BODY, rect(x, q.top + 7.0 * s, track.left - 12.0 * s, q.top + 27.0 * s), lead, grey(1.0, 1.0))?;
                                text(n, F_LABEL, rect(x, q.top + 27.0 * s, track.left - 12.0 * s, q.top + 46.0 * s), lead, grey(0.6, 1.0))?;
                            }
                            None => text(label, F_BODY, rect(x, q.top, track.left - 12.0 * s, q.bottom), lead, grey(1.0, 1.0))?,
                        }
                        fill(track, 12.0 * s, color(lerp(1.0, GREEN.0, t), lerp(1.0, GREEN.1, t), lerp(1.0, GREEN.2, t), lerp(0.2, 1.0, t)))?;
                        disc(track.left + (12.0 + 16.0 * t) * s, (track.top + track.bottom) / 2.0, 10.0 * s, grey(1.0, 1.0))?;
                        first_row = false;
                    }
                    Item::Choice { label, note, options } => {
                        if !first_row {
                            line(q.left + 14.0 * s, q.right, q.top, grey(1.0, 0.08))?;
                        }
                        let (first, last) = (scaled(p.targets[0].0, s), scaled(p.targets[p.targets.len() - 1].0, s));
                        let holder = rect(first.left - 2.0 * s, first.top - 2.0 * s, last.right + 2.0 * s, last.bottom + 2.0 * s);
                        match note {
                            Some(n) => {
                                text(label, F_BODY, rect(q.left + 14.0 * s, q.top + 7.0 * s, holder.left - 12.0 * s, q.top + 27.0 * s), lead, grey(1.0, 1.0))?;
                                text(n, F_LABEL, rect(q.left + 14.0 * s, q.top + 27.0 * s, holder.left - 12.0 * s, q.top + 46.0 * s), lead, grey(0.6, 1.0))?;
                            }
                            None => text(label, F_BODY, rect(q.left + 14.0 * s, q.top, holder.left - 12.0 * s, q.bottom), lead, grey(1.0, 1.0))?,
                        }
                        fill(holder, 9.0 * s, grey(1.0, 0.08))?;
                        for (b, (name, active, action)) in p.targets.iter().zip(options) {
                            let seg = scaled(b.0, s);
                            if *active {
                                fill(seg, 7.0 * s, grey(1.0, 0.17))?;
                            }
                            let ink = if *active { grey(1.0, 1.0) } else if hot(*action) { grey(1.0, 0.9) } else { grey(0.6, 1.0) };
                            text(name, F_LABEL, seg, center, ink)?;
                        }
                        first_row = false;
                    }
                    Item::About { version } => {
                        let cx = (q.left + q.right) / 2.0;
                        if let Some(logo) = &logo {
                            rt.DrawBitmap(logo, Some(&rect(cx - 34.0 * s, q.top + 14.0 * s, cx + 34.0 * s, q.top + 82.0 * s)), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None);
                        }
                        text("PUW Notch", F_BIG, rect(q.left, q.top + 92.0 * s, q.right, q.top + 126.0 * s), center, grey(1.0, 1.0))?;
                        text(&format!("{} {version}", crate::i18n::tr("Phiên bản", "Version")), F_BODY, rect(q.left, q.top + 126.0 * s, q.right, q.top + 148.0 * s), center, grey(0.6, 1.0))?;
                        for (i, (b, action)) in p.targets.iter().enumerate() {
                            let r = scaled(*b, s);
                            let radius = (r.bottom - r.top) / 2.0;
                            let over = hot(*action);
                            if i == 0 {
                                fill(r, radius, if over { color(0.29, 0.88, 0.38, 1.0) } else { green(1.0) })?;
                                text(crate::i18n::tr("Mở GitHub", "Open GitHub"), F_TITLE, r, center, color(0.02, 0.18, 0.05, 1.0))?;
                            } else {
                                if over {
                                    fill(r, radius, grey(1.0, 0.10))?;
                                }
                                outline(r, radius, 1.0, grey(1.0, 0.22))?;
                                text(crate::i18n::tr("Thoát PUW Notch", "Quit PUW Notch"), F_BODY, r, center, grey(1.0, 1.0))?;
                            }
                        }
                    }
                }
            }
            rt.SetTransform(&Matrix3x2::identity());
            rt.PopLayer();
            drop(ManuallyDrop::into_inner(layer.geometricMask));
            if v.max_scroll > 0.0 {
                let view_h = (HEIGHT - DRAG_H - 8.0) * s;
                let thumb_h = (view_h * view_h / ((HEIGHT - DRAG_H) * s + v.max_scroll * s)).max(30.0 * s);
                let top = DRAG_H * s + 4.0 * s + (view_h - thumb_h) * (v.scroll / v.max_scroll);
                fill(rect(w as f32 - 9.0 * s, top, w as f32 - 6.0 * s, top + thumb_h), 1.5 * s, grey(1.0, 0.22))?;
            }
            rt.EndDraw(None, None)?;
            #[cfg(test)]
            if self.grab {
                self.grab_frame()?;
            }
            self.surface.as_ref().unwrap().swap.Present(1, DXGI_PRESENT(0)).ok()?;
        }
        Ok(())
    }
}

fn draw_thumb(rt: &ID2D1RenderTarget, r: &Renderer, tb: D2D_RECT_F, thumb: Thumb, s: f32) -> Result<()> {
    let cx = (tb.left + tb.right) / 2.0;
    let (top, bottom) = if thumb.pill { (tb.top + 9.0 * s, tb.top + 26.0 * s) } else { (tb.top - 9.0 * s, tb.top + 17.0 * s) };
    let body = rounded(rect(cx - 35.0 * s, top, cx + 35.0 * s, bottom), 9.0 * s);
    unsafe {
        rt.FillRoundedRectangle(&body, &r.brush(grey(0.0, 1.0))?);
        if thumb.rim {
            rt.DrawRoundedRectangle(&body, &r.brush(grey(1.0, 0.4))?, 1.0, None);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notch::Style;
    use crate::render::Page;
    use crate::settings_win::{view_parts, Model, HEIGHT, SECTIONS, WIDTH};

    #[test]
    #[ignore = "writes target/settings-<n>.bmp: every section of the settings window, the Look page also scrolled"]
    fn settings_preview() {
        unsafe {
            use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let (w, h) = (WIDTH as i32, HEIGHT as i32);
            let hwnd = CreateWindowExW(WS_EX_NOREDIRECTIONBITMAP, w!("STATIC"), w!("Settings preview"), WS_POPUP, 0, 0, w, h, None, None, None, None).unwrap();
            let model = Model { minimal: false, robot: true, glass: true, flat_glass: false, style: Style::Ears, pages: 0x1FF & !Page::Usage.bit(), approve: true, volume_keys: false, activity: false, autostart: true, english: false };
            let mut renderer = Renderer::new().unwrap();
            renderer.grab = true;
            let mut shots: Vec<(Section, f32)> = SECTIONS.into_iter().map(|s| (s, 0.0)).collect();
            shots.push((Section::Look, f32::MAX));
            for (i, (section, scroll)) in shots.into_iter().enumerate() {
                let (placed, max) = view_parts(section, &model);
                let hovered = placed.iter().flat_map(|p| p.targets.iter()).nth(1).map(|(_, a)| *a);
                let hint = section.hint(env!("CARGO_PKG_VERSION"));
                let view = SettingsView { section, placed: &placed, scroll: scroll.min(max), max_scroll: max, hovered: if i == 1 { hovered } else { None }, title: section.title(), hint: &hint, version: env!("CARGO_PKG_VERSION"), pane: 1.0, flips: &[] };
                renderer.present_settings(hwnd, w, h, 1.0, &view).unwrap();
                let pixels = &renderer.grabbed[..];
                let mut bytes = Vec::new();
                bytes.extend_from_slice(b"BM");
                bytes.extend_from_slice(&(54u32 + pixels.len() as u32).to_le_bytes());
                bytes.extend_from_slice(&[0; 4]);
                bytes.extend_from_slice(&54u32.to_le_bytes());
                bytes.extend_from_slice(&40u32.to_le_bytes());
                bytes.extend_from_slice(&w.to_le_bytes());
                bytes.extend_from_slice(&(-h).to_le_bytes());
                bytes.extend_from_slice(&1u16.to_le_bytes());
                bytes.extend_from_slice(&32u16.to_le_bytes());
                bytes.extend_from_slice(&[0; 24]);
                bytes.extend_from_slice(pixels);
                std::fs::write(std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("target/settings-{i}.bmp")), bytes).unwrap();
            }
            DestroyWindow(hwnd).unwrap();
        }
    }
}

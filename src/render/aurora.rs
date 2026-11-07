use super::*;

const AURORA_MAX_LUM: f32 = 0.5;

const AURORA_STEPS: usize = 48;
const AURORA_LAYERS: [(f32, f32); 8] = [(2.4, 0.12), (2.0, 0.14), (1.65, 0.16), (1.3, 0.19), (1.0, 0.22), (0.72, 0.26), (0.48, 0.3), (0.26, 0.36)];
const AURORA_SHRINK: f32 = 8.0;
const AURORA_Y: f32 = 0.8;
const AURORA_AMP: f32 = 0.07;
const AURORA_HALO: f32 = 0.09;
const AURORA_CORE: [(f32, f32); 4] = [(9.0, 0.10), (5.0, 0.16), (2.6, 0.30), (1.2, 0.55)];
const AURORA_CORE_WHITE: f32 = 0.35;

fn at(t: f32, u: f32) -> (f32, f32) {
    use std::f32::consts::TAU;
    let c = 0.6 * (TAU * 0.7 * u + t * 0.45).sin() + 0.3 * (TAU * 1.3 * u - t * 0.3 + 1.1).sin() + 0.1 * (TAU * 2.1 * u + t * 0.6).sin();
    let w = 0.55 + 0.45 * (TAU * 1.1 * u - t * 0.25 + 0.5).sin();
    (c, w)
}

pub(super) fn tints(pixels: &[u8]) -> [(f32, f32, f32); 2] {
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

fn rotate_hue((r, g, b): (f32, f32, f32), deg: f32) -> (f32, f32, f32) {
    let (cos, sin) = (deg.to_radians().cos(), deg.to_radians().sin());
    let k = (1.0 - cos) / 3.0;
    let s3 = (1.0f32 / 3.0).sqrt() * sin;
    let m = |x: f32, y: f32, z: f32| (x * r + y * g + z * b).clamp(0.0, 1.0);
    (m(cos + k, k - s3, k + s3), m(k + s3, cos + k, k - s3), m(k - s3, k + s3, cos + k))
}

impl Renderer {
    fn band(&self, t: f32, left: f32, right: f32, yc: f32, amp: f32, thick: f32, k: f32) -> Result<ID2D1PathGeometry> {
        let pts: Vec<(f32, f32, f32)> = (0..=AURORA_STEPS).map(|i| {
            let u = i as f32 / AURORA_STEPS as f32;
            let (c, w) = at(t, u);
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

    fn line(&self, t: f32, left: f32, right: f32, yc: f32, amp: f32) -> Result<ID2D1PathGeometry> {
        unsafe {
            let geo = self.factory.CreatePathGeometry()?;
            let sink = geo.Open()?;
            let at = |i: usize| {
                let u = i as f32 / AURORA_STEPS as f32;
                Vector2 { X: left + (right - left) * u, Y: yc + at(t, u).0 * amp }
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

    pub(super) fn draw_aurora(&self, shape: &ID2D1PathGeometry, panel: (f32, f32, f32), w: i32, s: f32, t: f32, aurora_a: f32, rest: f32) {
        let (l, r, nh) = panel;
        let rt = &self.rt;
        unsafe {
            let [c1, c2] = self.cover.as_ref().map_or([GLOW_NEUTRAL; 2], |c| c.aurora);
            let (c1, c2) = (dim_to_lum(c1, AURORA_MAX_LUM), dim_to_lum(c2, AURORA_MAX_LUM));
            let stops: Vec<D2D1_GRADIENT_STOP> = (0..=5).map(|i| {
                let f = i as f32 / 5.0;
                let c = (c1.0 + (c2.0 - c1.0) * f, c1.1 + (c2.1 - c1.1) * f, c1.2 + (c2.2 - c1.2) * f);
                let pulse = 0.45 + 0.55 * (0.5 + 0.5 * (t * 0.4 + i as f32 * 1.7).sin());
                D2D1_GRADIENT_STOP { position: f, color: color(c.0, c.1, c.2, pulse) }
            }).collect();
            let line = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES { startPoint: Vector2 { X: l, Y: 0.0 }, endPoint: Vector2 { X: r, Y: 0.0 } };
            let lay = nh.min(rest);
            let (yc, amp) = (lay * AURORA_Y, lay * AURORA_AMP);
            let made = (|| -> Result<(ID2D1Bitmap, D2D_SIZE_F, ID2D1LinearGradientBrush, ID2D1PathGeometry)> {
            let small_size = D2D_SIZE_F { width: (w as f32 / AURORA_SHRINK).ceil(), height: (nh / AURORA_SHRINK).ceil() + 1.0 };
            let half = AURORA_SHRINK / 2.0;
            let key = (small_size.width as u32, small_size.height as u32);
            if self.aurora_rts.borrow().as_ref().is_none_or(|(k, _, _)| *k != key) {
                let small = rt.CreateCompatibleRenderTarget(Some(&small_size), None, None, D2D1_COMPATIBLE_RENDER_TARGET_OPTIONS_NONE)?;
                let sz = small.GetBitmap()?.GetSize();
                let mid = rt.CreateCompatibleRenderTarget(Some(&D2D_SIZE_F { width: sz.width * half, height: sz.height * half }), None, None, D2D1_COMPATIBLE_RENDER_TARGET_OPTIONS_NONE)?;
                *self.aurora_rts.borrow_mut() = Some((key, small, mid));
            }
            let rts = self.aurora_rts.borrow();
            let (_, small, mid) = rts.as_ref().unwrap();
            small.BeginDraw();
            small.Clear(Some(&color(0.0, 0.0, 0.0, 0.0)));
            small.SetTransform(&Matrix3x2 { M11: 1.0 / AURORA_SHRINK, M12: 0.0, M21: 0.0, M22: 1.0 / AURORA_SHRINK, M31: 0.0, M32: 0.0 });
            let brush = small.CreateLinearGradientBrush(&line, None, &small.CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP)?)?;
            for (k, alpha) in AURORA_LAYERS {
                let band = self.band(t, l, r, yc, amp, lay * AURORA_HALO, k)?;
                brush.SetOpacity(alpha);
                small.FillGeometry(&band, &brush, None);
            }
            small.EndDraw(None, None)?;
            let sz = small.GetBitmap()?.GetSize();
            mid.BeginDraw();
            mid.Clear(Some(&color(0.0, 0.0, 0.0, 0.0)));
            mid.DrawBitmap(&small.GetBitmap()?, Some(&rect(0.0, 0.0, sz.width * half, sz.height * half)), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None);
            mid.EndDraw(None, None)?;
            let whiten = |c: D2D1_COLOR_F| color(c.r + (1.0 - c.r) * AURORA_CORE_WHITE, c.g + (1.0 - c.g) * AURORA_CORE_WHITE, c.b + (1.0 - c.b) * AURORA_CORE_WHITE, c.a);
            let core_stops: Vec<D2D1_GRADIENT_STOP> = stops.iter().map(|st| D2D1_GRADIENT_STOP { position: st.position, color: whiten(st.color) }).collect();
            let core = rt.CreateLinearGradientBrush(&line, None, &rt.CreateGradientStopCollection(&core_stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP)?)?;
            Ok((mid.GetBitmap()?, sz, core, self.line(t, l, r, yc, amp)?))
            })();
            if let Ok((halo, sz, core, path)) = made {
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let mut px = Vec::new();
        for i in 0..64 {
            px.extend_from_slice(if i < 32 { &[255, 140, 90, 255] } else { &[170, 70, 255, 255] });
        }
        let [a, b] = tints(&px);
        assert_ne!(a, b);
        let blue = |c: (f32, f32, f32)| c.2 > c.0;
        assert!(blue(a) != blue(b), "one blue, one pink: {a:?} {b:?}");
        let [c, d] = tints(&[0, 0, 255, 255].repeat(64));
        assert_eq!(c, (1.0, 0.0, 0.0));
        assert!(d != c && d.0 > 0.5, "{d:?}");
        assert_eq!(tints(&[128, 128, 128, 255].repeat(64))[0], GLOW_NEUTRAL, "grey cover");
    }

    #[test]
    fn aurora_band_stays_in_range_and_moves() {
        for i in 0..=40 {
            let (t, u) = (i as f32 * 0.7, i as f32 / 40.0);
            let (c, w) = at(t, u);
            assert!((-1.0..=1.0).contains(&c) && (0.1 - 1e-4..=1.0 + 1e-4).contains(&w), "t {t} u {u}: {c} {w}");
        }
        assert_ne!(at(0.0, 0.3), at(1.0, 0.3), "it moves with time");
    }
}

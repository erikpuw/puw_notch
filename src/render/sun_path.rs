use super::*;

const BASELINE_Y: f32 = 122.0;
const ARC_H: f32 = 50.0;
const SUN: (f32, f32, f32) = (1.0, 0.82, 0.3);
const MOON: (f32, f32, f32) = (0.82, 0.86, 1.0);
const WALK_S: f32 = 1.4;
const PING_REACH: f32 = 16.0;
pub(super) const SWAP_STEP_S: f32 = 0.8;
const ARC_STEPS: usize = 64;
const ARC_FADE: f32 = 0.16;

fn arc_point(t: f32, x0: f32, x1: f32, baseline: f32, height: f32) -> (f32, f32) {
    let (cx, cy) = ((x0 + x1) / 2.0, baseline - 2.0 * height);
    let u = 1.0 - t;
    (u * u * x0 + 2.0 * u * t * cx + t * t * x1, u * u * baseline + 2.0 * u * t * cy + t * t * baseline)
}

fn edge_fade(t: f32) -> f32 {
    let k = (t.min(1.0 - t) / ARC_FADE).clamp(0.0, 1.0);
    0.15 + 0.85 * k * k * (3.0 - 2.0 * k)
}

impl Renderer {

    pub(super) fn sun_swap(&self, day: bool) -> Option<f32> {
        let at = match self.sun_seen.get() {
            Some((was, at)) if was == day => at,
            Some(_) => Some(Instant::now()),
            None => None,
        };
        self.sun_seen.set(Some((day, at)));
        at.map(|t| t.elapsed().as_secs_f32()).filter(|secs| *secs < 3.0 * SWAP_STEP_S)
    }

    pub(super) fn draw_sun_path(
        &self,
        text: &dyn Fn(&str, usize, D2D_RECT_F, DWRITE_TEXT_ALIGNMENT, D2D1_COLOR_F) -> Result<()>,
        wx_icon: &dyn Fn(&icons::Icon, &[ID2D1PathGeometry], f32, f32, f32, Option<((f32, f32, f32), (f32, f32, f32))>, f32, f32) -> Result<()>,
        moon: &Option<(&'static icons::Icon, Vec<ID2D1PathGeometry>)>,
        sun: weather::Sun,
        minute_of_day: u32,
        (left, right): (f32, f32),
        s: f32,
        a: f32,
        age: f32,
    ) -> Result<()> {
        let rt = &self.rt;
        let center = DWRITE_TEXT_ALIGNMENT_CENTER;
        let pos = sun.pos(minute_of_day);
        let swap = self.sun_swap(matches!(pos, weather::SunPos::Day(_)));
        let (x0, x1, yb) = (left + 10.0 * s, right - 10.0 * s, BASELINE_Y * s);
        let h = ARC_H * s;
        let pt = |t: f32| arc_point(t, x0, x1, yb, h);
        let line = |from: (f32, f32), to: (f32, f32), c: D2D1_COLOR_F, w: f32| -> Result<()> {
            unsafe { rt.DrawLine(Vector2 { X: from.0, Y: from.1 }, Vector2 { X: to.0, Y: to.1 }, &self.brush(c)?, w, &self.round) };
            Ok(())
        };
        let disc = |at: (f32, f32), c: (f32, f32, f32), r: f32, alpha: f32| -> Result<()> {
            unsafe { rt.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: at.0, Y: at.1 }, radiusX: r, radiusY: r }, &self.brush(color(c.0, c.1, c.2, alpha))?) };
            Ok(())
        };
        let halo = |at: (f32, f32), c: (f32, f32, f32), inner: f32, strength: f32| -> Result<()> {
            match anim::ping(age - WALK_S + anim::CURVE_DRAW_S) {
                Some((reach, alpha)) => disc(at, c, (inner + (PING_REACH - inner) * reach) * s, alpha * strength),
                None => Ok(()),
            }
        };
        unsafe {
            rt.FillRectangle(&rect(x0 - 4.0 * s, yb, x1 + 4.0 * s, yb + 1.0 * s), &self.brush(grey(0.14, a))?);
            for i in 0..ARC_STEPS {
                let (from, to) = (i as f32 / ARC_STEPS as f32, (i + 1) as f32 / ARC_STEPS as f32);
                line(pt(from), pt(to), grey(0.2, a * edge_fade((from + to) / 2.0)), 2.5 * s)?;
            }
            for (x, minute) in [(x0, sun.rise as u32), (x1, sun.set as u32)] {
                rt.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: x, Y: yb }, radiusX: 2.5 * s, radiusY: 2.5 * s }, &self.brush(grey(0.45, a))?);
                text(&health::hhmm(minute), F_LABEL, rect(x - 24.0 * s, yb + 11.0 * s, x + 24.0 * s, yb + 27.0 * s), center, grey(0.55, a))?;
            }
            let mid = (x0 + x1) / 2.0;
            let lit_path = |upto: f32, alpha: f32| -> Result<()> {
                let steps = ((upto * ARC_STEPS as f32).ceil() as usize).max(1);
                for i in 0..steps {
                    let (from, to) = (i as f32 / ARC_STEPS as f32, ((i + 1) as f32 / ARC_STEPS as f32).min(upto));
                    if to > from {
                        line(pt(from), pt(to), color(BLUE.0, BLUE.1, BLUE.2, alpha * a * edge_fade((from + to) / 2.0)), 2.5 * s)?;
                    }
                }
                Ok(())
            };
            let draw_sun = |at: (f32, f32), alpha: f32| -> Result<()> {
                halo(at, SUN, 4.2, alpha * a)?;
                disc(at, SUN, 4.2 * s, alpha * a)
            };
            let draw_moon = |t: f32, alpha: f32| -> Result<()> {
                if let Some((ic, geoms)) = moon {
                    let size = 12.0 * s;
                    let at = pt(t);
                    disc(at, (0.0, 0.0, 0.0), 6.5 * s, alpha * a)?;
                    halo(at, MOON, 6.5, alpha * a * 0.8)?;
                    wx_icon(ic, geoms, at.0 - size / 2.0, at.1 - size / 2.0, size, Some((MOON, MOON)), alpha * a, 9.0)?;
                }
                Ok(())
            };
            let step = |secs: f32, k: f32| anim::lerp_in(secs, k * SWAP_STEP_S, SWAP_STEP_S);
            match pos {
                weather::SunPos::Day(t) => {
                    let (sun_a, moon_a) = swap.map_or((1.0, 0.0), |secs| (step(secs, 1.0), 1.0 - step(secs, 0.0)));
                    let shown = t * ease_out(age / WALK_S);
                    lit_path(shown, sun_a)?;
                    draw_sun(pt(shown), sun_a)?;
                    if moon_a > 0.0 {
                        draw_moon(1.0, moon_a)?;
                    }
                    let left_min = sun.daylight_left(minute_of_day);
                    let words = match (left_min / 60, left_min % 60, crate::i18n::english()) {
                        (0, m, false) => format!("Nắng còn {m} phút"),
                        (h, m, false) => format!("Nắng còn {h}:{m:02}"),
                        (0, m, true) => format!("{m} min of daylight left"),
                        (h, m, true) => format!("{h}:{m:02} of daylight left"),
                    };
                    text(&words, F_LABEL, rect(mid - 80.0 * s, (BASELINE_Y - 20.0) * s, mid + 80.0 * s, BASELINE_Y * s), center, grey(0.7, a * sun_a))?;
                }
                _ => {
                    let (blue_a, sun_a, moon_a) = swap.map_or((0.0, 0.0, ease_out(age / 0.6)), |secs| (1.0 - step(secs, 0.0), 1.0 - step(secs, 1.0), step(secs, 2.0)));
                    if blue_a > 0.0 {
                        lit_path(1.0, blue_a)?;
                    }
                    if sun_a > 0.0 {
                        draw_sun(pt(1.0), sun_a)?;
                    }
                    draw_moon(sun.night_progress(minute_of_day) * ease_out(age / WALK_S), moon_a)?;
                    let dark_min = sun.dark_left(minute_of_day);
                    let words = match (dark_min / 60, dark_min % 60, crate::i18n::english()) {
                        (0, m, false) => format!("Mặt trời mọc sau {m} phút"),
                        (h, m, false) => format!("Mặt trời mọc sau {h}:{m:02}"),
                        (0, m, true) => format!("Sunrise in {m} min"),
                        (h, m, true) => format!("Sunrise in {h}:{m:02}"),
                    };
                    text(&words, F_LABEL, rect(mid - 80.0 * s, (BASELINE_Y - 20.0) * s, mid + 80.0 * s, BASELINE_Y * s), center, grey(0.7, a * moon_a))?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sunset_or_sunrise_is_told_only_when_seen_happening() {
        let r = Renderer::new().unwrap();
        assert_eq!(r.sun_swap(true), None, "opened by day: nothing to tell");
        assert_eq!(r.sun_swap(true), None);
        assert!(r.sun_swap(false).is_some_and(|secs| secs < 0.1), "it set while the view was open");
        assert!(r.sun_swap(false).is_some(), "and is still being told");
        r.sun_seen.set(None);
        assert_eq!(r.sun_swap(false), None, "opened again at night: no sunset to play");
        assert!(r.sun_swap(true).is_some(), "but a sunrise seen happening is");
    }

    #[test]
    fn the_ends_of_the_path_fade_so_it_does_not_look_cut() {
        assert!(edge_fade(0.0) < 0.2 && edge_fade(1.0) < 0.2, "faint at both ends");
        assert!((edge_fade(0.5) - 1.0).abs() < 1e-6 && (edge_fade(ARC_FADE) - 1.0).abs() < 1e-6, "full from a little in");
        assert!((edge_fade(0.3) - edge_fade(0.7)).abs() < 1e-6, "the same at both ends");
        let rising: Vec<f32> = (0..=16).map(|i| edge_fade(i as f32 * ARC_FADE / 16.0)).collect();
        assert!(rising.windows(2).all(|w| w[1] >= w[0]), "it only brightens going in");
    }

    #[test]
    fn the_arc_runs_from_end_to_end_and_peaks_in_the_middle() {
        let (x0, x1, yb, h) = (100.0, 300.0, 128.0, 36.0);
        assert_eq!(arc_point(0.0, x0, x1, yb, h), (x0, yb));
        let (ex, ey) = arc_point(1.0, x0, x1, yb, h);
        assert!((ex - x1).abs() < 1e-3 && (ey - yb).abs() < 1e-3);
        let (mx, my) = arc_point(0.5, x0, x1, yb, h);
        assert!((mx - 200.0).abs() < 1e-3 && (my - (yb - h)).abs() < 1e-3, "the apex is `height` above the ends");
        assert!((0..=20).all(|i| arc_point(i as f32 / 20.0, x0, x1, yb, h).1 <= yb + 1e-3), "never below the ground");
    }

    #[test]
    fn the_sun_and_moon_ping_only_after_they_have_walked_in() {
        assert!(anim::ping(0.0 - WALK_S + anim::CURVE_DRAW_S).is_none(), "no ping while it is still walking");
        assert!(anim::ping(WALK_S - WALK_S + anim::CURVE_DRAW_S).is_some(), "the first ping starts as it arrives");
    }
}

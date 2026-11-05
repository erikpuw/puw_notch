//! The sun's path on the Weather page: an arc from sunrise to sunset with the sun on it, lit while it is up (two rings of
//! light that fade in and out gently), a crescent moon with the same two rings on the arc at night. The arc is blue so it matches neither
//! the sun nor the moon.

use super::*;

/// Logical px from the panel's top: the arc's ends sit on this line, it rises this high above it, and the times are below.
/// Text boxes here are 16-20 px tall for 12 px text: Vietnamese marks stand above the letters and are cut by a tighter box.
const BASELINE_Y: f32 = 128.0;
const ARC_H: f32 = 36.0;
/// The sun's warm white and the moon's pale blue.
const SUN: (f32, f32, f32) = (1.0, 0.82, 0.3);
const MOON: (f32, f32, f32) = (0.82, 0.86, 1.0);
/// Seconds the sun takes to walk from sunrise to where it is now when the view opens.
const WALK_S: f32 = 1.4;
/// Seconds one fade in and out of the rings takes; the inner ring runs a third of the way behind the outer one.
const GLOW_PERIOD_S: f32 = 3.0;
/// When the sun sets or rises while the view is open, the change is told in three steps of this many seconds each: at
/// sunset the lit blue path turns grey, then the sun fades out where it stands, then the moon fades in; at sunrise the moon
/// fades out, then the sun fades in at the start of the path.
pub(super) const SWAP_STEP_S: f32 = 0.8;
/// How many straight pieces the arc is drawn with.
const ARC_STEPS: usize = 64;

/// Where the arc is at `t` (0..1) for ends `(x0, x1)` on `baseline`, rising `height`: a quadratic curve.
fn arc_point(t: f32, x0: f32, x1: f32, baseline: f32, height: f32) -> (f32, f32) {
    let (cx, cy) = ((x0 + x1) / 2.0, baseline - 2.0 * height);
    let u = 1.0 - t;
    (u * u * x0 + 2.0 * u * t * cx + t * t * x1, u * u * baseline + 2.0 * u * t * cy + t * t * baseline)
}

/// How lit a ring is `phase` seconds in, 0..1: a slow, smooth fade in and out, `lag` (0..1 of a period) behind.
fn glow(phase: f32, lag: f32) -> f32 {
    0.5 - 0.5 * (std::f32::consts::TAU * (phase / GLOW_PERIOD_S - lag)).cos()
}

impl Renderer {
    /// Seconds since the sun rose or set, while the sun view has stayed open through it; None before that, and once the three
    /// steps are told. `day` is whether the sun is up now.
    pub(super) fn sun_swap(&self, day: bool) -> Option<f32> {
        let at = match self.sun_seen.get() {
            Some((was, at)) if was == day => at,
            Some(_) => Some(Instant::now()),
            None => None,
        };
        self.sun_seen.set(Some((day, at)));
        at.map(|t| t.elapsed().as_secs_f32()).filter(|secs| *secs < 3.0 * SWAP_STEP_S)
    }

    /// `(left, right)` is the room the page leaves (window px), `phase` the animation clock and `age` seconds since the view
    /// opened. `moon` is the moon's line icon, for the night.
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
        phase: f32,
        age: f32,
    ) -> Result<()> {
        let rt = &self.rt;
        let center = DWRITE_TEXT_ALIGNMENT_CENTER;
        let pos = sun.pos(minute_of_day);
        // Seconds since the sun rose or set while this view was open (None: nothing to tell, it opened in this state).
        let swap = self.sun_swap(matches!(pos, weather::SunPos::Day(_)));
        let (x0, x1, yb) = (left + 10.0 * s, right - 10.0 * s, BASELINE_Y * s);
        let h = ARC_H * s;
        let pt = |t: f32| arc_point(t, x0, x1, yb, h);
        let line = |from: (f32, f32), to: (f32, f32), c: D2D1_COLOR_F, w: f32| -> Result<()> {
            unsafe { rt.DrawLine(Vector2 { X: from.0, Y: from.1 }, Vector2 { X: to.0, Y: to.1 }, &self.brush(c)?, w, &self.round) };
            Ok(())
        };
        // A filled circle of `c` round `at`.
        let disc = |at: (f32, f32), c: (f32, f32, f32), r: f32, alpha: f32| -> Result<()> {
            unsafe { rt.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: at.0, Y: at.1 }, radiusX: r, radiusY: r }, &self.brush(color(c.0, c.1, c.2, alpha))?) };
            Ok(())
        };
        // The light round the sun or the moon: two rings that fade in and out gently, the inner one a little behind (no blink, no
        // rays).
        let halo = |at: (f32, f32), c: (f32, f32, f32), strength: f32| -> Result<()> {
            disc(at, c, 12.5 * s, (0.05 + 0.13 * glow(phase, 0.0)) * strength)?;
            disc(at, c, 8.0 * s, (0.14 + 0.18 * glow(phase, 0.33)) * strength)
        };
        unsafe {
            // The ground, then the whole path dim.
            rt.FillRectangle(&rect(x0 - 4.0 * s, yb, x1 + 4.0 * s, yb + 1.0 * s), &self.brush(grey(0.14, a))?);
            let track = grey(0.2, a);
            for i in 0..ARC_STEPS {
                line(pt(i as f32 / ARC_STEPS as f32), pt((i + 1) as f32 / ARC_STEPS as f32), track, 2.5 * s)?;
            }
            // The two ends, with their times under them.
            for (x, minute) in [(x0, sun.rise as u32), (x1, sun.set as u32)] {
                rt.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: x, Y: yb }, radiusX: 2.5 * s, radiusY: 2.5 * s }, &self.brush(grey(0.45, a))?);
                text(&health::hhmm(minute), F_LABEL, rect(x - 24.0 * s, yb + 3.0 * s, x + 24.0 * s, yb + 19.0 * s), center, grey(0.55, a))?;
            }
            let mid = (x0 + x1) / 2.0;
            // The lit part of the path up to `upto`, in blue at `alpha`.
            let lit_path = |upto: f32, alpha: f32| -> Result<()> {
                let lit = color(BLUE.0, BLUE.1, BLUE.2, alpha * a);
                let steps = ((upto * ARC_STEPS as f32).ceil() as usize).max(1);
                for i in 0..steps {
                    let (from, to) = (i as f32 / ARC_STEPS as f32, ((i + 1) as f32 / ARC_STEPS as f32).min(upto));
                    if to > from {
                        line(pt(from), pt(to), lit, 2.5 * s)?;
                    }
                }
                Ok(())
            };
            // The sun at `at`: a warm core in two rings of light that fade in and out.
            let draw_sun = |at: (f32, f32), alpha: f32| -> Result<()> {
                halo(at, SUN, alpha * a)?;
                disc(at, SUN, 4.2 * s, alpha * a)
            };
            // A crescent (the moon's line icon) riding the top of the path, about as big as the sun with its halo, with the same
            // two rings fading in and out round it (in the moon's own pale blue).
            let draw_moon = |alpha: f32| -> Result<()> {
                if let Some((ic, geoms)) = moon {
                    let size = 16.0 * s;
                    let at = (mid, (BASELINE_Y - ARC_H + 3.0) * s);
                    halo(at, MOON, alpha * a * 0.8)?;
                    wx_icon(ic, geoms, at.0 - size / 2.0, at.1 - size / 2.0, size, Some((MOON, MOON)), alpha * a, 9.0)?;
                }
                Ok(())
            };
            let step = |secs: f32, k: f32| anim::lerp_in(secs, k * SWAP_STEP_S, SWAP_STEP_S);
            match pos {
                weather::SunPos::Day(t) => {
                    // Just risen: the moon fades out, then the sun fades in at the start of the path.
                    let (sun_a, moon_a) = swap.map_or((1.0, 0.0), |secs| (step(secs, 1.0), 1.0 - step(secs, 0.0)));
                    // The sun walks to where it is now; the part of the path behind it is lit.
                    let shown = t * ease_out(age / WALK_S);
                    lit_path(shown, sun_a)?;
                    draw_sun(pt(shown), sun_a)?;
                    if moon_a > 0.0 {
                        draw_moon(moon_a)?;
                    }
                    let left_min = sun.daylight_left(minute_of_day);
                    let words = match (left_min / 60, left_min % 60, crate::i18n::english()) {
                        (0, m, false) => format!("Nắng còn {m} phút"),
                        (h, m, false) => format!("Nắng còn {h} giờ {m}"),
                        (0, m, true) => format!("{m} min of daylight left"),
                        (h, m, true) => format!("{h} h {m} min of daylight left"),
                    };
                    text(&words, F_LABEL, rect(mid - 80.0 * s, (BASELINE_Y - 25.0) * s, mid + 80.0 * s, (BASELINE_Y - 5.0) * s), center, grey(0.7, a * sun_a))?;
                }
                pos => {
                    // Night: the path is dim, a crescent moon rides its top and the next sunrise (or the sunset that was) is told.
                    // Just set: the lit path turns grey, then the sun fades out where it stands, then the moon fades in.
                    let (blue_a, sun_a, moon_a) = swap.map_or((0.0, 0.0, ease_out(age / 0.6)), |secs| (1.0 - step(secs, 0.0), 1.0 - step(secs, 1.0), step(secs, 2.0)));
                    if blue_a > 0.0 {
                        lit_path(1.0, blue_a)?;
                    }
                    if sun_a > 0.0 {
                        draw_sun(pt(1.0), sun_a)?;
                    }
                    draw_moon(moon_a)?;
                    let words = match (pos, crate::i18n::english()) {
                        (weather::SunPos::Dawn, false) => format!("Mặt trời mọc lúc {}", health::hhmm(sun.rise as u32)),
                        (weather::SunPos::Dawn, true) => format!("Sunrise at {}", health::hhmm(sun.rise as u32)),
                        (_, false) => format!("Mặt trời lặn lúc {}", health::hhmm(sun.set as u32)),
                        (_, true) => format!("Sunset was at {}", health::hhmm(sun.set as u32)),
                    };
                    text(&words, F_LABEL, rect(mid - 80.0 * s, (BASELINE_Y - 23.0) * s, mid + 80.0 * s, (BASELINE_Y - 3.0) * s), center, grey(0.7, a * moon_a))?;
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
    fn the_rings_fade_in_and_out_smoothly_with_no_blink() {
        assert!(glow(0.0, 0.0).abs() < 1e-6 && (glow(GLOW_PERIOD_S / 2.0, 0.0) - 1.0).abs() < 1e-6, "dark, then fully lit half a period on");
        assert!((glow(GLOW_PERIOD_S, 0.0)).abs() < 1e-5, "and back to dark: one fade in and out per period");
        // Smooth: no step between two frames 16 ms apart, anywhere in the period, for either ring.
        for lag in [0.0, 0.33] {
            let step = (0..600).map(|i| (glow(i as f32 * 0.016, lag) - glow((i + 1) as f32 * 0.016, lag)).abs()).fold(0.0f32, f32::max);
            assert!(step < 0.05, "lag {lag}: a jump of {step} between frames");
        }
        assert!((0..300).all(|i| (0.0..=1.0).contains(&glow(i as f32 * 0.05, 0.33))));
    }
}

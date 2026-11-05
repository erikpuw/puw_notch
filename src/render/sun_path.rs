//! The sun's path on the Weather page: an arc from sunrise to sunset with the sun on it, lit while it is up (a halo that
//! breathes and blinks softly now and then), a crescent moon on the arc at night. The arc is blue so it matches neither
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
/// The halo blinks every this many seconds and the blink lasts `BLINK_S` of them.
const BLINK_EVERY_S: f32 = 3.2;
const BLINK_S: f32 = 0.6;
/// How many straight pieces the arc is drawn with.
const ARC_STEPS: usize = 64;

/// Where the arc is at `t` (0..1) for ends `(x0, x1)` on `baseline`, rising `height`: a quadratic curve.
fn arc_point(t: f32, x0: f32, x1: f32, baseline: f32, height: f32) -> (f32, f32) {
    let (cx, cy) = ((x0 + x1) / 2.0, baseline - 2.0 * height);
    let u = 1.0 - t;
    (u * u * x0 + 2.0 * u * t * cx + t * t * x1, u * u * baseline + 2.0 * u * t * cy + t * t * baseline)
}

/// How strong the halo's blink is `phase` seconds in: 0 most of the time, a swell and fade over `BLINK_S` once per `BLINK_EVERY_S`.
fn blink(phase: f32) -> f32 {
    let k = phase.rem_euclid(BLINK_EVERY_S);
    if k < BLINK_S { (std::f32::consts::PI * k / BLINK_S).sin() } else { 0.0 }
}

impl Renderer {
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
        let (x0, x1, yb) = (left + 10.0 * s, right - 10.0 * s, BASELINE_Y * s);
        let h = ARC_H * s;
        let pt = |t: f32| arc_point(t, x0, x1, yb, h);
        let line = |from: (f32, f32), to: (f32, f32), c: D2D1_COLOR_F, w: f32| -> Result<()> {
            unsafe { rt.DrawLine(Vector2 { X: from.0, Y: from.1 }, Vector2 { X: to.0, Y: to.1 }, &self.brush(c)?, w, &self.round) };
            Ok(())
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
            match sun.pos(minute_of_day) {
                weather::SunPos::Day(t) => {
                    // The sun walks to where it is now; the part of the path behind it is lit.
                    let shown = t * ease_out(age / WALK_S);
                    let lit = color(BLUE.0, BLUE.1, BLUE.2, a);
                    let steps = ((shown * ARC_STEPS as f32).ceil() as usize).max(1);
                    for i in 0..steps {
                        let (from, to) = (i as f32 / ARC_STEPS as f32, ((i + 1) as f32 / ARC_STEPS as f32).min(shown));
                        if to > from {
                            line(pt(from), pt(to), lit, 2.5 * s)?;
                        }
                    }
                    // The sun: a warm core in a halo that breathes and now and then blinks.
                    let (sx, sy) = pt(shown);
                    let disc = |r: f32, alpha: f32| -> Result<()> {
                        rt.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: sx, Y: sy }, radiusX: r, radiusY: r }, &self.brush(color(SUN.0, SUN.1, SUN.2, alpha * a))?);
                        Ok(())
                    };
                    let breath = 0.5 + 0.5 * (phase * 2.4).sin();
                    // Now and then the halo blinks: it swells and brightens softly (no rays), then settles.
                    let g = blink(phase);
                    disc((11.0 + 2.5 * breath + 3.0 * g) * s, 0.12 + 0.06 * breath + 0.12 * g)?;
                    disc(7.5 * s, 0.28)?;
                    disc(4.2 * s, 1.0)?;
                    let left_min = sun.daylight_left(minute_of_day);
                    let words = match (left_min / 60, left_min % 60, crate::i18n::english()) {
                        (0, m, false) => format!("Nắng còn {m} phút"),
                        (h, m, false) => format!("Nắng còn {h} giờ {m}"),
                        (0, m, true) => format!("{m} min of daylight left"),
                        (h, m, true) => format!("{h} h {m} min of daylight left"),
                    };
                    text(&words, F_LABEL, rect(mid - 80.0 * s, (BASELINE_Y - 25.0) * s, mid + 80.0 * s, (BASELINE_Y - 5.0) * s), center, grey(0.7, a))?;
                }
                pos => {
                    // Night: the path is dim, a crescent moon rides its top and the next sunrise (or the sunset that was) is told.
                    if let Some((ic, geoms)) = moon {
                        // A crescent (the moon's line icon) riding the top of the dim path, large enough to read at a glance.
                        let size = 22.0 * s;
                        wx_icon(ic, geoms, mid - size / 2.0, (BASELINE_Y - ARC_H + 3.0) * s - size / 2.0, size, Some((MOON, MOON)), a * ease_out(age / 0.6), 9.0)?;
                    }
                    let words = match (pos, crate::i18n::english()) {
                        (weather::SunPos::Dawn, false) => format!("Mặt trời mọc lúc {}", health::hhmm(sun.rise as u32)),
                        (weather::SunPos::Dawn, true) => format!("Sunrise at {}", health::hhmm(sun.rise as u32)),
                        (_, false) => format!("Mặt trời lặn lúc {}", health::hhmm(sun.set as u32)),
                        (_, true) => format!("Sunset was at {}", health::hhmm(sun.set as u32)),
                    };
                    text(&words, F_LABEL, rect(mid - 80.0 * s, (BASELINE_Y - 23.0) * s, mid + 80.0 * s, (BASELINE_Y - 3.0) * s), center, grey(0.7, a))?;
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
    fn the_halo_blinks_briefly_and_not_in_between() {
        assert_eq!(blink(0.0), 0.0);
        assert!((blink(BLINK_S / 2.0) - 1.0).abs() < 1e-5, "full in the middle of the blink");
        assert_eq!(blink(BLINK_S + 0.01), 0.0);
        assert_eq!(blink(BLINK_EVERY_S - 0.01), 0.0);
        assert!(blink(BLINK_EVERY_S + BLINK_S / 2.0) > 0.99, "and again next time");
    }
}

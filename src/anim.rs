pub const ICON_MOTION_S: f32 = 1.0;
pub const ICON_MORPH_S: f32 = 0.25;
pub const GLOW_FADE_S: f32 = 0.3;
pub const LIST_GLIDE_RATE: f32 = 14.0;
pub const BARS_RATE: f32 = 9.0;
pub const PAGE_MORPH_S: f32 = 0.32;
pub const COVER_FADE_S: f32 = 0.35;
pub const COVER_FADE_FROM: f32 = 0.96;

pub fn cover_fade(t: f32) -> (f32, f32) {
    let e = ease_out(t);
    (e, COVER_FADE_FROM + (1.0 - COVER_FADE_FROM) * e)
}

pub fn lerp_in(t: f32, from: f32, len: f32) -> f32 {
    ((t - from) / len).clamp(0.0, 1.0)
}

pub fn bump(t: f32, from: f32, len: f32) -> f32 {
    (std::f32::consts::PI * lerp_in(t, from, len)).sin()
}

pub fn ease_out(x: f32) -> f32 {
    1.0 - (1.0 - x.clamp(0.0, 1.0)).powi(3)
}

pub fn ease_out_back(x: f32) -> f32 {
    let (c1, y) = (1.3, x.clamp(0.0, 1.0) - 1.0);
    1.0 + (c1 + 1.0) * y * y * y + c1 * y * y
}

pub fn smooth(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

pub fn approach(dt: f32, secs: f32) -> f32 {
    1.0 - (-3.0 * dt / secs).exp()
}

pub fn follow(current: f32, goal: f32, dt: f32, rate: f32) -> f32 {
    current + (goal - current) * (1.0 - (-dt * rate).exp())
}

pub fn page_fades(t: f32) -> (f32, f32) {
    ((1.0 - t * 3.5).clamp(0.0, 1.0), ((t - 0.2) * 2.5).clamp(0.0, 1.0))
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tuning {
    pub k: f32,
    pub c: f32,
    pub rest_pos: f32,
    pub rest_vel: f32,
}

impl Tuning {
    pub const NOTCH: Tuning = Tuning { k: 320.0, c: 28.0, rest_pos: 0.05, rest_vel: 0.5 };
    pub const LYRIC_SCROLL: Tuning = Tuning { k: 150.0, c: 21.0, rest_pos: 0.002, rest_vel: 0.05 };
    pub const LYRIC_HIGHLIGHT: Tuning = Tuning { k: 220.0, c: 28.0, rest_pos: 0.002, rest_vel: 0.05 };
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Spring {
    pub pos: f32,
    pub vel: f32,
}

impl Spring {
    const SLICE: f32 = 0.004;

    pub fn at(pos: f32) -> Spring {
        Spring { pos, vel: 0.0 }
    }

    pub fn step(&mut self, target: f32, dt: f32, t: Tuning) -> bool {
        let mut left = dt;
        while left > 0.0 {
            let h = left.min(Self::SLICE);
            let acc = t.k * (target - self.pos) - t.c * self.vel;
            self.vel += acc * h;
            self.pos += self.vel * h;
            left -= h;
        }
        if (target - self.pos).abs() < t.rest_pos && self.vel.abs() < t.rest_vel {
            self.pos = target;
            self.vel = 0.0;
            return false;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cover_crossfade_lands_whole_and_full_size() {
        assert_eq!(cover_fade(0.0), (0.0, COVER_FADE_FROM));
        assert_eq!(cover_fade(1.0), (1.0, 1.0));
        assert_eq!(cover_fade(5.0), (1.0, 1.0), "held once done");
        let (mut alpha, mut scale) = (0.0, COVER_FADE_FROM);
        for i in 1..=100 {
            let (a, k) = cover_fade(i as f32 / 100.0);
            assert!(a >= alpha && k >= scale && k <= 1.0, "rises, never past full size");
            (alpha, scale) = (a, k);
        }
    }

    #[test]
    fn curves_start_and_end_where_they_should() {
        for f in [ease_out, ease_out_back, smooth] {
            assert!((f(0.0)).abs() < 1e-6 && (f(1.0) - 1.0).abs() < 1e-6);
            assert_eq!((f(-1.0), f(2.0)), (f(0.0), f(1.0)), "held outside 0..1");
        }
        assert!(ease_out(0.5) > 0.5, "fast at first");
        assert!((0..100).any(|i| ease_out_back(i as f32 / 100.0) > 1.0), "the pop goes past 1");
        assert!((0..100).all(|i| ease_out(i as f32 / 100.0) <= 1.0), "the slide never does");
        assert_eq!((lerp_in(0.5, 1.0, 2.0), lerp_in(2.0, 1.0, 2.0), lerp_in(9.0, 1.0, 2.0)), (0.0, 0.5, 1.0));
        assert!(bump(0.0, 0.0, 1.0).abs() < 1e-6 && (bump(0.5, 0.0, 1.0) - 1.0).abs() < 1e-6 && bump(1.0, 0.0, 1.0).abs() < 1e-5);
    }

    #[test]
    fn approach_covers_most_of_the_way_by_its_time() {
        assert_eq!(approach(0.0, 0.3), 0.0);
        let k = approach(0.3, 0.3);
        assert!(k > 0.94 && k < 0.96, "{k}");
    }

    #[test]
    fn follow_closes_in_on_its_goal_whatever_the_frame_rate() {
        let (mut a, mut b) = (0.0, 0.0);
        for _ in 0..60 {
            a = follow(a, 1.0, 1.0 / 60.0, LIST_GLIDE_RATE);
        }
        for _ in 0..30 {
            b = follow(b, 1.0, 1.0 / 30.0, LIST_GLIDE_RATE);
        }
        assert!((a - b).abs() < 1e-4 && a > 0.99, "{a} {b}");
        assert_eq!(follow(0.5, 0.5, 0.1, BARS_RATE), 0.5);
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
    fn every_tuning_settles_exactly_on_its_goal_without_a_big_overshoot() {
        for (t, travel) in [(Tuning::NOTCH, 100.0), (Tuning::LYRIC_SCROLL, 3.0), (Tuning::LYRIC_HIGHLIGHT, 1.0)] {
            for dt in [0.004, 0.016, 0.033, 0.1] {
                let mut s = Spring::at(0.0);
                let mut peak = 0.0f32;
                let mut frames = 0;
                while s.step(travel, dt, t) {
                    peak = peak.max(s.pos);
                    frames += 1;
                    assert!(frames < 5000, "{t:?} did not settle at dt={dt}");
                }
                assert_eq!((s.pos, s.vel), (travel, 0.0));
                assert!(peak <= travel * 1.05, "{t:?} dt={dt}: overshoot {peak}");
            }
        }
    }
}

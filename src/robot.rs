//! Idle robot: when nobody touches the PC, two LED eyes take the resting pill. They look around
//! below after `SHOW_S` seconds idle, fall asleep after `SLEEP_S`, and when work starts again they
//! glance at the cursor for `LOOK_S` before handing the pill back to the clock and battery.

/// Seconds without input before the eyes show / fall asleep; seconds of the glance on waking.
pub const SHOW_S: u32 = 30;
pub const SLEEP_S: u32 = 300;
const LOOK_S: f32 = 1.6;
/// Eyes fade in and out over this long.
const FADE_S: f32 = 0.35;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum State {
    /// Clock and battery: someone is working.
    Off,
    /// Looking around below.
    Awake,
    Asleep,
    /// Input again: a glance at the cursor, then Off.
    Look,
}

/// What the painter needs.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Eyes {
    /// 0..1 (fade in / out).
    pub alpha: f32,
    /// Gaze, about -1..1 sideways and -1..1.2 up / down (down is +).
    pub gaze: (f32, f32),
    /// 1 open, ~0.1 a closed line (blink, sleep).
    pub open: f32,
    pub asleep: bool,
    /// Seconds since the robot showed (drives the floating z).
    pub time: f32,
}

pub struct Robot {
    pub state: State,
    /// Seconds in the current state.
    since: f32,
    clock: f32,
    alpha: f32,
    gaze: (f32, f32),
    next_blink: f32,
    seed: u32,
}

impl Default for Robot {
    fn default() -> Self {
        Robot { state: State::Off, since: 0.0, clock: 0.0, alpha: 0.0, gaze: (0.0, 0.0), next_blink: 3.0, seed: 0x1234_5678 }
    }
}

impl Robot {
    /// Follow the idle time (whole seconds without input). `allowed` = the pill rests plainly
    /// (no music, banner, open panel, mini or fullscreen); otherwise the robot goes away at once.
    /// Returns whether the state changed.
    pub fn update(&mut self, idle_s: u32, allowed: bool) -> bool {
        let next = if !allowed {
            State::Off
        } else if idle_s >= SLEEP_S {
            State::Asleep
        } else if idle_s >= SHOW_S {
            State::Awake
        } else {
            match self.state {
                State::Awake | State::Asleep => State::Look,
                State::Look if self.since < LOOK_S => State::Look,
                _ => State::Off,
            }
        };
        let changed = next != self.state;
        if changed {
            self.state = next;
            self.since = 0.0;
        }
        changed
    }

    /// Whether frames must keep coming (eyes on screen or fading out).
    pub fn visible(&self) -> bool {
        self.state != State::Off || self.alpha > 0.0
    }

    /// Advance the animation by `dt` seconds. `cursor` = cursor relative to the pill centre in
    /// logical px, for the glance.
    pub fn step(&mut self, dt: f32, cursor: Option<(f32, f32)>) -> Eyes {
        self.since += dt;
        self.clock += dt;
        let on = self.state != State::Off;
        let fade = (if on { dt } else { -dt }) / FADE_S;
        self.alpha = (self.alpha + fade).clamp(0.0, 1.0);
        let t = self.clock;
        let target = match self.state {
            // Wandering, mostly down: the robot looks at the desk.
            State::Awake | State::Off => ((t * 0.7).sin() * 1.0 + (t * 1.9).sin() * 0.3, 0.9 + (t * 1.3).sin() * 0.25),
            State::Asleep => (0.0, 0.6),
            State::Look => match cursor {
                Some((x, y)) => ((x / 60.0).clamp(-1.2, 1.2), (y / 22.0).clamp(-0.8, 1.2)),
                None => ((self.since * 3.0).sin() * 0.9, 0.3),
            },
        };
        let k = (dt * 9.0).min(1.0);
        self.gaze.0 += (target.0 - self.gaze.0) * k;
        self.gaze.1 += (target.1 - self.gaze.1) * k;
        // Blink every 2-4.5 s for 0.12 s.
        if t > self.next_blink + 0.12 {
            self.seed = self.seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            self.next_blink = t + 2.0 + (self.seed >> 16) as f32 / 65_535.0 * 2.5;
        }
        let blinking = t >= self.next_blink && t < self.next_blink + 0.12;
        let asleep = self.state == State::Asleep;
        let open = if asleep { 0.1 } else if blinking { 0.15 } else { 1.0 };
        Eyes { alpha: self.alpha, gaze: self.gaze, open, asleep, time: t }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shows_sleeps_glances_then_gives_the_pill_back() {
        let mut r = Robot::default();
        assert!(!r.update(SHOW_S - 1, true));
        assert_eq!(r.state, State::Off, "working: clock and battery");
        assert!(r.update(SHOW_S, true));
        assert_eq!(r.state, State::Awake);
        r.update(SLEEP_S, true);
        assert_eq!(r.state, State::Asleep);
        r.update(0, true);
        assert_eq!(r.state, State::Look, "input again: glance at the cursor");
        r.step(1.0, Some((60.0, 0.0)));
        r.update(0, true);
        assert_eq!(r.state, State::Look);
        r.step(1.0, None);
        r.update(0, true);
        assert_eq!(r.state, State::Off, "then back to the clock");
    }

    #[test]
    fn goes_away_at_once_when_not_allowed() {
        let mut r = Robot::default();
        r.update(SHOW_S + 5, true);
        r.update(SHOW_S + 6, false);
        assert_eq!(r.state, State::Off, "music, banner, panel...");
        let mut r = Robot::default();
        r.update(0, true);
        assert_eq!(r.state, State::Off, "no glance without having shown");
    }

    #[test]
    fn fades_and_looks_at_the_cursor() {
        let mut r = Robot::default();
        assert!(!r.visible());
        r.update(SHOW_S, true);
        let e = r.step(0.1, None);
        assert!(e.alpha > 0.0 && e.alpha < 1.0, "fading in");
        for _ in 0..30 {
            r.step(0.05, None);
        }
        assert_eq!(r.step(0.05, None).alpha, 1.0);
        assert!(r.step(0.05, None).gaze.1 > 0.3, "looks down at the desk");
        r.update(0, true);
        for _ in 0..20 {
            r.step(0.05, Some((-200.0, -50.0)));
        }
        let g = r.step(0.05, Some((-200.0, -50.0))).gaze;
        assert!(g.0 < -1.0 && g.1 < -0.5, "looks toward the cursor, clamped: {g:?}");
        r.update(SLEEP_S, true);
        assert!(r.step(0.05, None).asleep && r.step(0.05, None).open < 0.2);
        r.update(0, false);
        for _ in 0..20 {
            r.step(0.05, None);
        }
        assert!(!r.visible(), "faded out");
    }

    #[test]
    fn blinks_now_and_then() {
        let mut r = Robot::default();
        r.update(SHOW_S, true);
        let closed = (0..400).filter(|_| r.step(0.025, None).open < 0.5).count();
        assert!((2..=40).contains(&closed), "a few short blinks in 10 s, got {closed} frames");
    }
}

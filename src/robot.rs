//! Idle robot: when nobody touches the PC, an LED face (eyes and mouth, `Face`) takes the resting
//! pill. It looks around below after `SHOW_S` seconds idle, plays a face now and then (often a
//! happy one while charging), frowns on a low battery, falls asleep after `SLEEP_S`, and when work starts again it
//! grins at the cursor for `LOOK_S` before handing the pill back to clock and battery.

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

/// Expression on the robot's LED face.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Face {
    /// Square eyes, small smile.
    Normal,
    /// ^ ^ eyes, big smile.
    Happy,
    /// One square eye, one ^.
    Wink,
    /// > < eyes, w mouth.
    Excited,
    /// You are back: plain square eyes, a big open grin.
    Greet,
    /// The eye on the side it looks toward big, the other small (`curious_eyes`), flat mouth.
    Curious,
    /// Drooping eyes, frown.
    Sad,
    /// Closed arcs, flat mouth, a z drifting up.
    Sleep,
}

/// Curious face: (left, right) eye heights in logical px for a sideways gaze; the eye on the
/// side it looks toward is the big one (looking right: right eye 11, left 6).
pub fn curious_eyes(gaze_x: f32) -> (f32, f32) {
    let t = gaze_x.clamp(-1.0, 1.0);
    (8.5 - 2.5 * t, 8.5 + 2.5 * t)
}

/// What the battery says about the robot's mood.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Mood {
    Normal,
    /// Plugged in: happy faces now and then.
    Charging,
    /// Low and not charging.
    Low,
}

/// The plain face gets a playful one every `PLAY_EVERY` (+ up to `PLAY_JITTER`) s for `PLAY_S`.
const PLAY_EVERY: f32 = 4.0;
const PLAY_JITTER: f32 = 3.0;
const PLAY_S: f32 = 1.8;

/// What the painter needs.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Eyes {
    pub face: Face,
    /// 0..1 (fade in / out).
    pub alpha: f32,
    /// Gaze, about -1..1 sideways and -1..1.2 up / down (down is +).
    pub gaze: (f32, f32),
    /// 1 open, 0.15 mid-blink (plain face only).
    pub open: f32,
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
    /// Playful face shown now, until `play_until`; the next one starts at `next_play`.
    play: Face,
    play_until: f32,
    next_play: f32,
}

impl Default for Robot {
    fn default() -> Self {
        Robot { state: State::Off, since: 0.0, clock: 0.0, alpha: 0.0, gaze: (0.0, 0.0), next_blink: 3.0, seed: 0x1234_5678, play: Face::Normal, play_until: 0.0, next_play: PLAY_EVERY }
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

    fn random(&mut self) -> f32 {
        self.seed = self.seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        (self.seed >> 16) as f32 / 65_535.0
    }

    /// Advance the animation by `dt` seconds. `cursor` = cursor relative to the pill centre in
    /// logical px, for the glance; `mood` from the battery.
    pub fn step(&mut self, dt: f32, cursor: Option<(f32, f32)>, mood: Mood) -> Eyes {
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
            self.next_blink = t + 2.0 + self.random() * 2.5;
        }
        // Now and then a playful face for a moment.
        if t >= self.next_play {
            // Charging adds a happy face to the mix.
            let plays: &[Face] = if mood == Mood::Charging {
                &[Face::Curious, Face::Wink, Face::Excited, Face::Happy, Face::Happy]
            } else {
                &[Face::Curious, Face::Wink, Face::Excited]
            };
            self.play = plays[((self.random() * plays.len() as f32) as usize).min(plays.len() - 1)];
            self.play_until = t + PLAY_S;
            self.next_play = self.play_until + PLAY_EVERY + self.random() * PLAY_JITTER;
        }
        let face = match (self.state, mood) {
            (State::Asleep, _) => Face::Sleep,
            (State::Look, _) => Face::Greet,
            (_, Mood::Low) => Face::Sad,
            _ if t < self.play_until => self.play,
            _ => Face::Normal,
        };
        let blinking = face == Face::Normal && t >= self.next_blink && t < self.next_blink + 0.12;
        let open = if blinking { 0.15 } else { 1.0 };
        Eyes { face, alpha: self.alpha, gaze: self.gaze, open, time: t }
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
        r.step(1.0, Some((60.0, 0.0)), Mood::Normal);
        r.update(0, true);
        assert_eq!(r.state, State::Look);
        r.step(1.0, None, Mood::Normal);
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
        let e = r.step(0.1, None, Mood::Normal);
        assert!(e.alpha > 0.0 && e.alpha < 1.0, "fading in");
        for _ in 0..30 {
            r.step(0.05, None, Mood::Normal);
        }
        assert_eq!(r.step(0.05, None, Mood::Normal).alpha, 1.0);
        assert!(r.step(0.05, None, Mood::Normal).gaze.1 > 0.3, "looks down at the desk");
        r.update(0, true);
        for _ in 0..20 {
            r.step(0.05, Some((-200.0, -50.0)), Mood::Normal);
        }
        let g = r.step(0.05, Some((-200.0, -50.0)), Mood::Normal).gaze;
        assert!(g.0 < -1.0 && g.1 < -0.5, "looks toward the cursor, clamped: {g:?}");
        r.update(SLEEP_S, true);
        assert_eq!(r.step(0.05, None, Mood::Normal).face, Face::Sleep);
        r.update(0, false);
        for _ in 0..20 {
            r.step(0.05, None, Mood::Normal);
        }
        assert!(!r.visible(), "faded out");
    }

    #[test]
    fn face_follows_the_situation() {
        let face = |r: &mut Robot, mood| r.step(0.05, None, mood).face;
        let mut r = Robot::default();
        r.update(SHOW_S, true);
        assert_eq!(face(&mut r, Mood::Low), Face::Sad, "battery low, not charging: sad");
        let charging: Vec<Face> = (0..2400).map(|_| face(&mut r, Mood::Charging)).collect();
        assert!(charging.contains(&Face::Happy), "charging: happy now and then");
        assert!(charging.iter().filter(|f| **f == Face::Normal).count() > charging.len() / 2, "but not a face stuck on happy");
        r.update(SLEEP_S, true);
        assert_eq!(face(&mut r, Mood::Low), Face::Sleep);
        r.update(0, true);
        assert_eq!(face(&mut r, Mood::Normal), Face::Greet, "you are back: plain eyes, a big grin");
    }

    #[test]
    fn idle_face_is_mostly_normal_with_a_little_play() {
        let mut r = Robot::default();
        r.update(SHOW_S, true);
        let faces: Vec<Face> = (0..1200).map(|_| r.step(0.05, None, Mood::Normal).face).collect();
        let normal = faces.iter().filter(|f| **f == Face::Normal).count();
        let play = faces.iter().filter(|f| matches!(f, Face::Curious | Face::Wink | Face::Excited)).count();
        assert!(normal > faces.len() / 2, "mostly the plain face: {normal}/{}", faces.len());
        assert!(play > 0, "now and then curious / wink / excited");
        assert_eq!(normal + play, faces.len(), "no other face while idle and fine");
    }

    #[test]
    fn curious_eye_on_the_side_it_looks_is_bigger() {
        let (l, r) = curious_eyes(1.0);
        assert!(r > l, "looking right: right eye big ({r}), left small ({l})");
        let (l, r) = curious_eyes(-1.0);
        assert!(l > r, "looking left: left eye big");
        let (l, r) = curious_eyes(0.0);
        assert_eq!(l, r, "straight ahead: same size");
        assert_eq!(curious_eyes(5.0), curious_eyes(1.0), "clamped");
    }

    #[test]
    fn blinks_now_and_then() {
        let mut r = Robot::default();
        r.update(SHOW_S, true);
        let closed = (0..400).filter(|_| r.step(0.025, None, Mood::Normal).open < 0.5).count();
        assert!((2..=40).contains(&closed), "a few short blinks in 10 s, got {closed} frames");
    }
}

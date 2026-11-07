pub const SHOW_S: u32 = 30;
pub const SLEEP_S: u32 = 300;
const LOOK_S: f32 = 1.6;
const GLANCE_S: f32 = 1.1;
const FADE_S: f32 = 0.35;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum State {
    Off,
    Awake,
    Asleep,
    Look,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Face {
    Normal,
    Happy,
    Greet,
    Curious,
    Sad,
    Sleep,
}

pub const EYE_H: f32 = 12.0;

pub fn curious_eyes(gaze_x: f32) -> (f32, f32) {
    let t = gaze_x.clamp(-1.0, 1.0);
    let (mid, swing) = (EYE_H * 8.5 / 9.0, EYE_H * 2.5 / 9.0);
    (mid - swing * t, mid + swing * t)
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Mood {
    Normal,
    Low,
}

const PLAY_EVERY: f32 = 4.0;
const PLAY_JITTER: f32 = 3.0;
const PLAY_S: f32 = 1.8;
const HAPPY_S: f32 = 1.8;

const BLINK_CLOSE_S: f32 = 0.07;
const BLINK_OPEN_S: f32 = 0.11;
const DOUBLE_BLINK: f32 = 0.2;

pub fn blink_open(t: f32) -> f32 {
    if !(0.0..BLINK_CLOSE_S + BLINK_OPEN_S).contains(&t) {
        return 1.0;
    }
    let x = if t < BLINK_CLOSE_S { t / BLINK_CLOSE_S } else { 1.0 - (t - BLINK_CLOSE_S) / BLINK_OPEN_S };
    1.0 - 0.9 * crate::anim::smooth(x)
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Eyes {
    pub face: Face,
    pub alpha: f32,
    pub gaze: (f32, f32),
    pub curious: f32,
    pub open: f32,
    pub time: f32,
}

pub struct Robot {
    pub state: State,
    since: f32,
    clock: f32,
    alpha: f32,
    gaze: (f32, f32),
    glance: f32,
    next_blink: f32,
    seed: u32,
    side: f32,
    play_until: f32,
    next_play: f32,
    full_pending: bool,
    happy_until: f32,
}

impl Default for Robot {
    fn default() -> Self {
        Robot { state: State::Off, since: 0.0, clock: 0.0, alpha: 0.0, gaze: (0.0, 0.0), glance: 0.0, next_blink: 3.0, seed: 0x1234_5678, side: 1.0, play_until: 0.0, next_play: PLAY_EVERY, full_pending: false, happy_until: 0.0 }
    }
}

impl Robot {
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

    pub fn full_battery(&mut self, full: bool) {
        self.full_pending = full;
    }

    pub fn visible(&self) -> bool {
        self.state != State::Off || self.alpha > 0.0
    }

    fn random(&mut self) -> f32 {
        self.seed = self.seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        (self.seed >> 16) as f32 / 65_535.0
    }

    pub fn step(&mut self, dt: f32, cursor: Option<(f32, f32)>, mood: Mood) -> Eyes {
        self.since += dt;
        self.clock += dt;
        let on = self.state != State::Off;
        let fade = (if on { dt } else { -dt }) / FADE_S;
        self.alpha = (self.alpha + fade).clamp(0.0, 1.0);
        let t = self.clock;
        if t >= self.next_play {
            self.side = if self.random() < 0.5 { -1.0 } else { 1.0 };
            self.play_until = t + PLAY_S;
            self.next_play = self.play_until + PLAY_EVERY + self.random() * PLAY_JITTER;
        }
        if self.full_pending && self.state == State::Awake {
            self.full_pending = false;
            self.happy_until = t + HAPPY_S;
        }
        let glance_to = if self.state == State::Awake && t < self.play_until && t >= self.happy_until { self.side } else { 0.0 };
        let target = match self.state {
            State::Awake => {
                let (x, y) = wander(t);
                (x + glance_to, y)
            }
            State::Asleep => (0.0, 0.6),
            State::Look if self.since >= GLANCE_S => (0.0, 0.0),
            State::Look => cursor.map_or(((self.since * 3.0).sin() * 0.9, 0.3), toward),
            State::Off => (0.0, 0.0),
        };
        let k = (dt * 9.0).min(1.0);
        self.gaze.0 += (target.0 - self.gaze.0) * k;
        self.gaze.1 += (target.1 - self.gaze.1) * k;
        self.glance += (glance_to - self.glance) * k;
        if t > self.next_blink + BLINK_CLOSE_S + BLINK_OPEN_S {
            let gap = if self.random() < DOUBLE_BLINK { 0.08 } else { 2.0 + self.random() * 2.5 };
            self.next_blink = t + gap;
        }
        let face = match (self.state, mood) {
            (State::Asleep, _) => Face::Sleep,
            (State::Look, _) => Face::Greet,
            (_, Mood::Low) => Face::Sad,
            _ if t < self.happy_until => Face::Happy,
            _ if t < self.play_until => Face::Curious,
            _ => Face::Normal,
        };
        let blinks = matches!(face, Face::Normal | Face::Curious | Face::Sad);
        let open = if blinks { blink_open(t - self.next_blink) } else { 1.0 };
        Eyes { face, alpha: self.alpha, gaze: self.gaze, curious: self.glance, open, time: t }
    }
}

fn wander(t: f32) -> (f32, f32) {
    ((t * 0.7).sin() + (t * 1.9).sin() * 0.3, 0.9 + (t * 1.3).sin() * 0.25)
}

fn toward((x, y): (f32, f32)) -> (f32, f32) {
    let root = |v: f32| v.signum() * v.abs().sqrt();
    ((1.2 * root(x)).clamp(-1.2, 1.2), (1.2 * root(y)).clamp(-0.8, 1.2))
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
        r.step(1.0, Some((0.5, 0.0)), Mood::Normal);
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
        assert!(r.step(0.05, Some((-0.25, 0.2)), Mood::Normal).gaze.1 > 0.3, "idle: looks around down at the desk, not at the cursor");
        r.update(0, true);
        for _ in 0..20 {
            r.step(0.05, Some((-1.0, -0.3)), Mood::Normal);
        }
        let g = r.step(0.05, Some((-1.0, -0.3)), Mood::Normal).gaze;
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
        r.update(SLEEP_S, true);
        assert_eq!(face(&mut r, Mood::Low), Face::Sleep);
        r.update(0, true);
        assert_eq!(face(&mut r, Mood::Normal), Face::Greet, "you are back: ^ ^ eyes");
    }

    #[test]
    fn full_battery_cheers_once_when_the_eyes_are_up() {
        let mut r = Robot::default();
        r.full_battery(true);
        assert_eq!(r.step(0.05, None, Mood::Normal).face, Face::Normal, "working: nobody to cheer for");
        r.update(SHOW_S, true);
        let faces: Vec<Face> = (0..2400).map(|_| r.step(0.05, None, Mood::Normal).face).collect();
        let happy = faces.iter().filter(|f| **f == Face::Happy).count();
        assert!(happy > 0 && faces[0] == Face::Happy, "cheers as soon as the eyes are up");
        assert!((HAPPY_S / 0.05) as usize - 2 <= happy && happy <= (HAPPY_S / 0.05) as usize + 2, "for about {HAPPY_S} s: {happy} frames");
        assert!(faces[happy..].iter().all(|f| *f != Face::Happy), "and only once");
        let mut r = Robot::default();
        r.full_battery(true);
        r.full_battery(false);
        r.update(SHOW_S, true);
        assert!((0..2400).all(|_| r.step(0.05, None, Mood::Normal).face != Face::Happy), "unplugged first: no cheer");
    }

    #[test]
    fn idle_face_is_mostly_normal_with_a_little_curiosity() {
        let mut r = Robot::default();
        r.update(SHOW_S, true);
        let faces: Vec<Face> = (0..1200).map(|_| r.step(0.05, None, Mood::Normal).face).collect();
        let normal = faces.iter().filter(|f| **f == Face::Normal).count();
        let curious = faces.iter().filter(|f| **f == Face::Curious).count();
        assert!(normal > faces.len() / 2, "mostly the plain face: {normal}/{}", faces.len());
        assert!(curious > 0, "now and then curious");
        assert_eq!(normal + curious, faces.len(), "no other face while idle and fine");
    }

    #[test]
    fn the_curious_glance_steps_aside_from_where_the_eyes_look() {
        let mut r = Robot::default();
        r.update(SHOW_S, true);
        let (mut left, mut right) = (false, false);
        for i in 0..4800 {
            let e = r.step(0.05, None, Mood::Normal);
            let t = (i + 1) as f32 * 0.05;
            if e.face == Face::Curious && e.curious.abs() > 0.95 {
                let step = e.gaze.0 - wander(t).0;
                assert!((step - e.curious).abs() < 0.5, "a step of about {} from the wandering look, got {step}", e.curious);
                (left, right) = (left || e.curious < 0.0, right || e.curious > 0.0);
            }
        }
        assert!(left && right, "glances left and right");
    }

    #[test]
    fn coming_back_the_eyes_glance_at_the_cursor_over_the_whole_screen() {
        let look = |c: (f32, f32)| {
            let mut r = Robot::default();
            r.update(SHOW_S, true);
            r.step(0.05, None, Mood::Normal);
            r.update(0, true);
            assert_eq!(r.state, State::Look);
            for _ in 0..15 {
                r.step(0.05, Some(c), Mood::Normal);
            }
            r.step(0.05, Some(c), Mood::Normal).gaze
        };
        let (near, mid, far) = (look((0.1, 0.1)), look((0.4, 0.4)), look((1.0, 0.95)));
        assert!(near.0 < mid.0 && mid.0 < far.0, "further right, further the eyes turn: {near:?} {mid:?} {far:?}");
        assert!(near.1 < mid.1 && mid.1 < far.1, "lower on the screen, lower the eyes look");
        assert!(far.1 > 1.0 && look((-1.0, 0.95)).0 < -1.0 && far.0 > 1.0, "the bottom corners are told apart from the middle");
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
        assert!(curious_eyes(1.0).1 < EYE_H * 1.3 && curious_eyes(1.0).0 > EYE_H * 0.6, "big and small stay in scale with the plain eye");
    }

    #[test]
    fn face_returns_to_the_centre_before_giving_the_pill_back() {
        let mut r = Robot::default();
        r.update(SHOW_S, true);
        for _ in 0..60 {
            r.step(0.05, None, Mood::Normal);
        }
        r.update(0, true);
        assert_eq!(r.state, State::Look);
        let mut gaze = (0.0, 0.0);
        for _ in 0..30 {
            gaze = r.step(0.02, Some((-1.0, -0.3)), Mood::Normal).gaze;
        }
        assert!(gaze.0 < -1.0, "glances at the cursor first: {gaze:?}");
        let mut frames = 0;
        while r.state == State::Look && frames < 400 {
            r.update(0, true);
            gaze = r.step(0.02, Some((-1.0, -0.3)), Mood::Normal).gaze;
            frames += 1;
        }
        assert_eq!(r.state, State::Off, "the glance ends");
        assert!(gaze.0.abs() < 0.12 && gaze.1.abs() < 0.12, "centred when the pill is handed back: {gaze:?}");
        for _ in 0..10 {
            gaze = r.step(0.02, None, Mood::Normal).gaze;
        }
        assert!(gaze.0.abs() < 0.12 && gaze.1.abs() < 0.12, "no wandering while fading out: {gaze:?}");
    }

    #[test]
    fn blinks_now_and_then() {
        let mut r = Robot::default();
        r.update(SHOW_S, true);
        let opens: Vec<f32> = (0..400).map(|_| r.step(0.025, None, Mood::Normal).open).collect();
        let closed = opens.iter().filter(|o| **o < 0.5).count();
        assert!((2..=40).contains(&closed), "a few short blinks in 10 s, got {closed} frames");
        assert!(opens.iter().any(|o| *o > 0.3 && *o < 0.8), "the lid passes half-way: a smooth blink, not a cut");
    }

    #[test]
    fn blink_closes_fast_and_opens_slower() {
        assert_eq!(blink_open(-0.1), 1.0);
        assert!(blink_open(BLINK_CLOSE_S * 0.5) < 1.0 && blink_open(BLINK_CLOSE_S * 0.5) > 0.2);
        assert!(blink_open(BLINK_CLOSE_S) < 0.15, "shut at the bottom");
        assert!(blink_open(BLINK_CLOSE_S + BLINK_OPEN_S * 0.5) > blink_open(BLINK_CLOSE_S));
        assert_eq!(blink_open(BLINK_CLOSE_S + BLINK_OPEN_S), 1.0);
    }

    #[test]
    fn open_eyes_blink_and_shut_ones_do_not() {
        let mut r = Robot::default();
        r.update(SHOW_S, true);
        r.step(1.0, None, Mood::Normal);
        r.update(0, true);
        let greet: Vec<Eyes> = (0..80).map(|_| r.step(0.02, None, Mood::Normal)).collect();
        assert_eq!(greet[0].face, Face::Greet);
        assert!(greet.iter().all(|e| e.open == 1.0), "^ ^ is already shut");
        let mut r = Robot::default();
        r.update(SHOW_S, true);
        let sad = (0..400).filter(|_| r.step(0.025, None, Mood::Low).open < 0.5).count();
        assert!(sad > 0, "the sad face blinks too");
    }

    #[test]
    fn sometimes_blinks_twice() {
        let mut r = Robot::default();
        r.update(SHOW_S, true);
        let (mut blinks, mut doubles, mut last, mut shut) = (0, 0, -9.0f32, false);
        for i in 0..24_000 {
            let e = r.step(0.025, None, Mood::Normal);
            if e.open < 0.5 && !shut {
                let t = i as f32 * 0.025;
                if t - last < 0.5 {
                    doubles += 1;
                }
                (blinks, last) = (blinks + 1, t);
            }
            shut = e.open < 0.5;
        }
        assert!(blinks > 50 && doubles > 0 && doubles * 2 < blinks, "{blinks} blinks, {doubles} double");
    }
}

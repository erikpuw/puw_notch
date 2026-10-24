//! Notch state machine and spring animation. Pure logic, no Win32.
//! All sizes are logical pixels (multiply by DPI scale to draw).

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Mode {
    /// Normal pill under the top edge.
    Collapsed,
    /// Thin line, used when the window under the notch is busy or "minimal" is on.
    Mini,
    /// Mouse just arrived; grows a little while we wait to see if it stays.
    Peek,
    /// Mouse arrived on the mini line.
    PeekMini,
    /// Full panel.
    Expanded,
    /// Banner for a new Windows notification; ends by timer.
    Alert,
}

/// (width, height, corner radius) the notch animates towards in each mode.
/// `live` = music is playing: wider pill (cover + bars). The panel is one size; it pages its content.
pub fn target(mode: Mode, live: bool) -> [f32; 3] {
    match (mode, live) {
        (Mode::Collapsed, false) => [150.0, 30.0, 14.0],
        (Mode::Collapsed, true) => [216.0, 30.0, 14.0],
        (Mode::Mini, _) => [44.0, 4.0, 2.0],
        (Mode::Peek, false) => [166.0, 34.0, 15.0],
        (Mode::Peek, true) => [232.0, 34.0, 15.0],
        (Mode::PeekMini, _) => [72.0, 8.0, 4.0],
        (Mode::Expanded, _) => [440.0, 170.0, 28.0],
        (Mode::Alert, _) => [360.0, 64.0, 22.0],
    }
}

/// Window must fit the largest target plus the hover margin.
pub const MAX_W: f32 = 460.0;
pub const MAX_H: f32 = 190.0;
/// How long a notification banner stays.
pub const ALERT_MS: u32 = 5000;
/// How long a status banner (Caps Lock, brightness) stays.
pub const STATUS_MS: u32 = 1500;
/// Invisible hover margin around the pill (not used in mini: only the line itself reacts).
pub const HOVER_MARGIN: f32 = 4.0;
/// How long the mouse must stay before Peek turns into Expanded.
pub const DWELL_MS: u32 = 200;

#[derive(Clone, Copy, Default)]
struct Spring {
    pos: f32,
    vel: f32,
}

impl Spring {
    const K: f32 = 230.0;
    const C: f32 = 24.0;

    fn step(&mut self, target: f32, dt: f32) -> bool {
        let acc = Self::K * (target - self.pos) - Self::C * self.vel;
        self.vel += acc * dt;
        self.pos += self.vel * dt;
        if (target - self.pos).abs() < 0.05 && self.vel.abs() < 0.5 {
            self.pos = target;
            self.vel = 0.0;
            return false;
        }
        true
    }
}

pub struct Notch {
    pub mode: Mode,
    pub minimal: bool,
    pub live: bool,
    obstructed: bool,
    clear_checks: u8,
    size: [Spring; 3],
}

impl Notch {
    pub fn new(minimal: bool) -> Self {
        let mut n = Notch { mode: Mode::Collapsed, minimal, live: false, obstructed: false, clear_checks: 0, size: [Spring::default(); 3] };
        n.mode = n.rest_mode();
        for (s, t) in n.size.iter_mut().zip(target(n.mode, false)) {
            s.pos = t;
        }
        n
    }

    fn rest_mode(&self) -> Mode {
        if self.obstructed || self.minimal { Mode::Mini } else { Mode::Collapsed }
    }

    pub fn is_resting(&self) -> bool {
        matches!(self.mode, Mode::Collapsed | Mode::Mini)
    }

    /// Resting at its pill size, springs stopped: the only time the area beside it is our own business.
    pub fn is_settled_rest(&self) -> bool {
        self.is_resting() && self.size() == target(self.mode, self.live)
    }

    /// Current (w, h, r).
    pub fn size(&self) -> [f32; 3] {
        [self.size[0].pos, self.size[1].pos, self.size[2].pos]
    }

    /// Music started/stopped. Returns true if the target size changed (caller animates).
    pub fn set_live(&mut self, live: bool) -> bool {
        let changed = self.live != live;
        self.live = live;
        changed
    }

    /// Mouse entered the drawn shape. Returns true if a dwell timer should start.
    pub fn on_enter(&mut self) -> bool {
        self.mode = match self.mode {
            Mode::Collapsed => Mode::Peek,
            Mode::Mini => Mode::PeekMini,
            // Banner stays as is; resting on it opens the panel.
            Mode::Alert => Mode::Alert,
            _ => return false,
        };
        true
    }

    /// Dwell timer fired.
    pub fn on_dwell(&mut self, inside: bool) {
        match self.mode {
            Mode::Peek | Mode::PeekMini => self.mode = if inside { Mode::Expanded } else { self.rest_mode() },
            Mode::Alert if inside => self.mode = Mode::Expanded,
            _ => {}
        }
    }

    pub fn on_leave(&mut self) {
        // A banner ends by its own timer, not by the mouse passing over it.
        if self.mode != Mode::Alert {
            self.mode = self.rest_mode();
        }
    }

    /// New notification. Returns true if the banner is shown (not while the panel is open).
    pub fn on_alert(&mut self) -> bool {
        if self.mode == Mode::Expanded {
            return false;
        }
        self.mode = Mode::Alert;
        true
    }

    pub fn on_alert_end(&mut self) {
        if self.mode == Mode::Alert {
            self.mode = self.rest_mode();
        }
    }

    pub fn set_minimal(&mut self, on: bool) {
        self.minimal = on;
        if self.is_resting() {
            self.mode = self.rest_mode();
        }
    }

    /// Result of one obstruction check. Busy applies at once, clear needs two checks in a row
    /// so a single quiet frame does not make the notch jump back.
    pub fn report_busy(&mut self, busy: bool) {
        if busy {
            self.obstructed = true;
            self.clear_checks = 0;
        } else if self.obstructed {
            self.clear_checks += 1;
            if self.clear_checks >= 2 {
                self.obstructed = false;
            }
        }
        if self.is_resting() {
            self.mode = self.rest_mode();
        }
    }

    /// Advance the animation. Returns true while still moving.
    pub fn step(&mut self, dt: f32) -> bool {
        let t = target(self.mode, self.live);
        let mut moving = false;
        for (s, t) in self.size.iter_mut().zip(t) {
            moving |= s.step(t, dt);
        }
        moving
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settle(n: &mut Notch) {
        for _ in 0..600 {
            if !n.step(1.0 / 60.0) {
                return;
            }
        }
        panic!("spring did not settle");
    }

    #[test]
    fn hover_flow() {
        let mut n = Notch::new(false);
        assert_eq!(n.mode, Mode::Collapsed);
        assert!(n.on_enter());
        assert_eq!(n.mode, Mode::Peek);
        n.on_dwell(true);
        assert_eq!(n.mode, Mode::Expanded);
        settle(&mut n);
        assert_eq!(n.size(), target(Mode::Expanded, false));
        assert!(!n.is_settled_rest());
        n.on_leave();
        n.step(1.0 / 60.0);
        assert!(!n.is_settled_rest(), "still shrinking");
        assert_eq!(n.mode, Mode::Collapsed);
        settle(&mut n);
        assert_eq!(n.size(), target(Mode::Collapsed, false));
        assert!(n.is_settled_rest());
        // music starting widens the resting pill
        assert!(n.set_live(true));
        assert!(!n.set_live(true));
        settle(&mut n);
        assert_eq!(n.size(), target(Mode::Collapsed, true));
    }

    #[test]
    fn short_visit_does_not_expand() {
        let mut n = Notch::new(false);
        n.on_enter();
        n.on_dwell(false);
        assert_eq!(n.mode, Mode::Collapsed);
    }

    #[test]
    fn busy_goes_mini_and_needs_two_clear_checks() {
        let mut n = Notch::new(false);
        n.report_busy(true);
        assert_eq!(n.mode, Mode::Mini);
        n.report_busy(false);
        assert_eq!(n.mode, Mode::Mini);
        n.report_busy(false);
        assert_eq!(n.mode, Mode::Collapsed);
    }

    #[test]
    fn busy_does_not_interrupt_expanded() {
        let mut n = Notch::new(false);
        n.on_enter();
        n.on_dwell(true);
        n.report_busy(true);
        assert_eq!(n.mode, Mode::Expanded);
        n.on_leave();
        assert_eq!(n.mode, Mode::Mini);
    }

    #[test]
    fn alert_banner_flow() {
        let mut n = Notch::new(false);
        n.report_busy(true);
        assert!(n.on_alert());
        assert_eq!(n.mode, Mode::Alert);
        // passing over the banner does not end it
        assert!(n.on_enter());
        n.on_leave();
        assert_eq!(n.mode, Mode::Alert);
        n.on_alert_end();
        assert_eq!(n.mode, Mode::Mini);
        // resting on the banner opens the panel, and the panel is not replaced by a banner
        n.on_alert();
        n.on_enter();
        n.on_dwell(true);
        assert_eq!(n.mode, Mode::Expanded);
        assert!(!n.on_alert());
        n.on_alert_end();
        assert_eq!(n.mode, Mode::Expanded);
    }

    #[test]
    fn minimal_setting_rests_in_mini() {
        let mut n = Notch::new(true);
        assert_eq!(n.mode, Mode::Mini);
        assert!(n.on_enter());
        assert_eq!(n.mode, Mode::PeekMini);
        n.set_minimal(false);
        assert_eq!(n.mode, Mode::PeekMini);
        n.on_leave();
        assert_eq!(n.mode, Mode::Collapsed);
    }
}

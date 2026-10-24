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
}

/// (width, height, corner radius) the notch animates towards in each mode.
pub fn target(mode: Mode) -> [f32; 3] {
    match mode {
        Mode::Collapsed => [150.0, 30.0, 10.0],
        Mode::Mini => [44.0, 4.0, 2.0],
        Mode::Peek => [166.0, 34.0, 12.0],
        Mode::PeekMini => [72.0, 8.0, 4.0],
        Mode::Expanded => [420.0, 140.0, 28.0],
    }
}

/// Window must fit the largest target plus the hover margin.
pub const MAX_W: f32 = 440.0;
pub const MAX_H: f32 = 160.0;
/// Invisible hover margin around the pill (not used in mini: only the line itself reacts).
pub const HOVER_MARGIN: f32 = 4.0;
/// Notch wider than this would cover the side strips the obstruction check samples.
pub const WIDE: f32 = 180.0;
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
    obstructed: bool,
    clear_checks: u8,
    size: [Spring; 3],
}

impl Notch {
    pub fn new(minimal: bool) -> Self {
        let mut n = Notch { mode: Mode::Collapsed, minimal, obstructed: false, clear_checks: 0, size: [Spring::default(); 3] };
        n.mode = n.rest_mode();
        for (s, t) in n.size.iter_mut().zip(target(n.mode)) {
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

    /// Current (w, h, r).
    pub fn size(&self) -> [f32; 3] {
        [self.size[0].pos, self.size[1].pos, self.size[2].pos]
    }

    pub fn is_wide(&self) -> bool {
        self.size[0].pos > WIDE
    }

    /// Mouse entered the drawn shape. Returns true if a dwell timer should start.
    pub fn on_enter(&mut self) -> bool {
        self.mode = match self.mode {
            Mode::Collapsed => Mode::Peek,
            Mode::Mini => Mode::PeekMini,
            _ => return false,
        };
        true
    }

    /// Dwell timer fired.
    pub fn on_dwell(&mut self, inside: bool) {
        if matches!(self.mode, Mode::Peek | Mode::PeekMini) {
            self.mode = if inside { Mode::Expanded } else { self.rest_mode() };
        }
    }

    pub fn on_leave(&mut self) {
        self.mode = self.rest_mode();
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
        let t = target(self.mode);
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
        assert!(n.is_wide());
        n.on_leave();
        assert_eq!(n.mode, Mode::Collapsed);
        settle(&mut n);
        assert_eq!(n.size(), target(Mode::Collapsed));
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

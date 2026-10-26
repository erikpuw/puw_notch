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
        (Mode::Expanded, _) => [440.0, 186.0, 28.0],
        (Mode::Alert, _) => [360.0, 64.0, 22.0],
    }
}

/// A notification banner shows at most this many lines of its text; longer text ends with "…".
pub const BANNER_MAX_LINES: u32 = 4;
/// Text line height in the notification banner (13 px Segoe UI) and where the text starts.
pub const BANNER_LINE_H: f32 = 18.0;
pub const BANNER_TEXT_TOP: f32 = 43.0;

/// Banner size for `lines` lines of notification text (0 = status banner: volume, brightness...).
/// Wider than the status banner and as tall as its text, capped at `BANNER_MAX_LINES`.
pub fn alert_size(lines: u8) -> [f32; 3] {
    if lines == 0 {
        return target(Mode::Alert, false);
    }
    let n = (lines as u32).min(BANNER_MAX_LINES) as f32;
    [420.0, BANNER_TEXT_TOP + n * BANNER_LINE_H + 7.0, 24.0]
}

/// Window must fit the largest target plus the hover margin.
/// Includes room for the spring overshoot and the ears.
pub const MAX_W: f32 = 530.0;
pub const MAX_H: f32 = 210.0;
/// How long a notification banner stays.
pub const ALERT_MS: u32 = 5000;
/// How long a status banner (Caps Lock, brightness) stays.
pub const STATUS_MS: u32 = 1500;
/// Concave flares joining the notch sides to the screen edge: grow with the notch, capped.
const EAR_RATIO: f32 = 0.35;
const EAR_MAX: f32 = 14.0;

/// Ear size (logical px) for a notch `h` tall.
pub fn ear(h: f32) -> f32 {
    (h * EAR_RATIO).min(EAR_MAX)
}

/// Area that reacts to the mouse, logical px: (half width from the centre, height from the top).
/// Only the pill body: the ears are drawn (so Windows reports the mouse over them) but entering
/// and the dwell check both test this, so the rim neither opens the panel nor bounces the pill.
pub fn hover_area(size: [f32; 3]) -> (f32, f32) {
    (size[0] / 2.0, size[1])
}
/// The system's mouse-leave message can go missing (the window was hidden or pushed under a
/// fullscreen app and brought back), which leaves the panel open with the cursor far away. True
/// when the panel, or the "mouse is inside" flag, outlived the real cursor: release it.
/// Typing keeps the panel open on purpose; a banner ends by its own timer.
pub fn hover_stale(mode: Mode, hover_in: bool, editing: bool, inside: bool) -> bool {
    !editing && !inside && mode != Mode::Alert && (hover_in || matches!(mode, Mode::Peek | Mode::PeekMini | Mode::Expanded))
}

/// How long the mouse must stay before Peek turns into Expanded.
pub const DWELL_MS: u32 = 80;

#[derive(Clone, Copy, Default)]
struct Spring {
    pos: f32,
    vel: f32,
}

impl Spring {
    /// Damping ratio ~0.78 keeps a subtle bounce (under 2% of the travel) in both directions.
    const K: f32 = 320.0;
    const C: f32 = 28.0;
    /// Integrate in small slices so a late frame does not change the motion.
    const SLICE: f32 = 0.004;

    fn step(&mut self, target: f32, dt: f32) -> bool {
        let mut left = dt;
        while left > 0.0 {
            let h = left.min(Self::SLICE);
            let acc = Self::K * (target - self.pos) - Self::C * self.vel;
            self.vel += acc * h;
            self.pos += self.vel * h;
            left -= h;
        }
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
    /// Lines of text in the current notification banner (0 = status banner), see `alert_size`.
    alert_lines: u8,
    obstructed: bool,
    clear_checks: u8,
    busy_checks: u8,
    size: [Spring; 3],
}

impl Notch {
    pub fn new(minimal: bool) -> Self {
        let mut n = Notch { mode: Mode::Collapsed, minimal, live: false, alert_lines: 0, obstructed: false, clear_checks: 0, busy_checks: 0, size: [Spring::default(); 3] };
        n.mode = n.rest_mode();
        // Start as the thin mini line and let the spring grow it into the pill, the way Nusa appears.
        for (s, t) in n.size.iter_mut().zip(target(Mode::Mini, false)) {
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

    /// Current (w, h, r). The bounce can swing past zero when shrinking to the mini line; clamp.
    pub fn size(&self) -> [f32; 3] {
        [self.size[0].pos.max(0.0), self.size[1].pos.max(0.0), self.size[2].pos.max(0.0)]
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

    /// New banner with `lines` lines of notification text (0 = status banner). Returns true if
    /// shown (not while the panel is open).
    pub fn on_alert(&mut self, lines: u8) -> bool {
        if self.mode == Mode::Expanded {
            return false;
        }
        self.mode = Mode::Alert;
        self.alert_lines = lines;
        true
    }

    /// Size the springs head for right now.
    fn goal(&self) -> [f32; 3] {
        if self.mode == Mode::Alert { alert_size(self.alert_lines) } else { target(self.mode, self.live) }
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

    /// Result of one obstruction check. Both directions need two checks in a row, so one odd
    /// frame (a window mid-animation, a tooltip) neither shrinks the notch nor brings it back.
    pub fn report_busy(&mut self, busy: bool) {
        if busy {
            self.clear_checks = 0;
            self.busy_checks = self.busy_checks.saturating_add(1);
            if self.busy_checks >= 2 {
                self.obstructed = true;
            }
        } else {
            self.busy_checks = 0;
            if self.obstructed {
                self.clear_checks += 1;
                if self.clear_checks >= 2 {
                    self.obstructed = false;
                }
            }
        }
        if self.is_resting() {
            self.mode = self.rest_mode();
        }
    }

    /// Advance the animation. Returns true while still moving.
    pub fn step(&mut self, dt: f32) -> bool {
        let t = self.goal();
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

    #[test]
    fn stale_hover_is_an_open_panel_with_the_cursor_gone() {
        use Mode::*;
        for mode in [Peek, PeekMini, Expanded] {
            assert!(hover_stale(mode, true, false, false), "{mode:?}: cursor left, no leave message");
            assert!(!hover_stale(mode, true, false, true), "{mode:?}: cursor still on it");
            assert!(!hover_stale(mode, true, true, false), "{mode:?}: typing keeps the panel open");
        }
        for mode in [Collapsed, Mini, Alert] {
            assert!(!hover_stale(mode, false, false, false), "{mode:?}: nothing to release");
        }
        assert!(hover_stale(Collapsed, true, false, false), "mouse remembered as inside while the pill rests");
        assert!(!hover_stale(Alert, false, false, false), "a banner ends by its own timer");
    }

    fn settle(n: &mut Notch) {
        for _ in 0..600 {
            if !n.step(1.0 / 60.0) {
                return;
            }
        }
        panic!("spring did not settle");
    }

    #[test]
    fn hover_area_is_the_pill_body() {
        // Ears and the space beside them do not count: only the pill itself opens the panel.
        let pill = target(Mode::Collapsed, false);
        assert_eq!(hover_area(pill), (pill[0] / 2.0, pill[1]));
        let line = target(Mode::Mini, false);
        assert_eq!(hover_area(line), (line[0] / 2.0, line[1]));
        assert_eq!(ear(200.0), EAR_MAX, "ears are capped");
    }

    #[test]
    fn starts_as_the_mini_line_and_grows_into_the_pill() {
        let mut n = Notch::new(false);
        assert_eq!(n.size(), target(Mode::Mini, false), "appears from the thin line, like Nusa");
        assert!(!n.is_settled_rest());
        for _ in 0..120 {
            n.step(1.0 / 120.0);
        }
        assert!(n.is_settled_rest());
        assert_eq!(n.size(), target(Mode::Collapsed, false));
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
    fn spring_stays_within_two_percent_on_open_and_close() {
        let sizes = [target(Mode::Collapsed, false), target(Mode::Expanded, false),
            target(Mode::Mini, false), alert_size(BANNER_MAX_LINES as u8)];
        for from in sizes {
            for to in sizes {
                for dt in [1.0 / 120.0, 1.0 / 60.0, 0.05] {
                    for (start, goal) in from.into_iter().zip(to) {
                        let mut spring = Spring { pos: start, vel: 0.0 };
                        let margin = (goal - start).abs() * 0.02 + 0.001;
                        for _ in 0..(2.0 / dt) as usize {
                            spring.step(goal, dt);
                            assert!(spring.pos >= start.min(goal) - margin
                                && spring.pos <= start.max(goal) + margin,
                                "{start} -> {goal}, dt={dt}: {}", spring.pos);
                            assert!(spring.pos >= 0.0);
                        }
                        assert_eq!(spring.pos, goal);
                        assert_eq!(spring.vel, 0.0);
                    }
                }
            }
        }
    }

    #[test]
    fn short_visit_does_not_expand() {
        let mut n = Notch::new(false);
        n.on_enter();
        n.on_dwell(false);
        assert_eq!(n.mode, Mode::Collapsed);
    }

    #[test]
    fn busy_and_clear_both_need_two_checks() {
        let mut n = Notch::new(false);
        n.report_busy(true);
        assert_eq!(n.mode, Mode::Collapsed, "one busy frame (window mid-animation) is not enough");
        n.report_busy(false);
        n.report_busy(true);
        assert_eq!(n.mode, Mode::Collapsed, "busy checks must be in a row");
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
        n.report_busy(true);
        assert_eq!(n.mode, Mode::Expanded);
        n.on_leave();
        assert_eq!(n.mode, Mode::Mini);
    }

    #[test]
    fn alert_banner_flow() {
        let mut n = Notch::new(false);
        n.report_busy(true);
        n.report_busy(true);
        assert!(n.on_alert(0));
        assert_eq!(n.mode, Mode::Alert);
        // passing over the banner does not end it
        assert!(n.on_enter());
        n.on_leave();
        assert_eq!(n.mode, Mode::Alert);
        n.on_alert_end();
        assert_eq!(n.mode, Mode::Mini);
        // resting on the banner opens the panel, and the panel is not replaced by a banner
        n.on_alert(0);
        n.on_enter();
        n.on_dwell(true);
        assert_eq!(n.mode, Mode::Expanded);
        assert!(!n.on_alert(0));
        n.on_alert_end();
        assert_eq!(n.mode, Mode::Expanded);
        // a notification with text grows to fit its lines
        let mut n = Notch::new(false);
        n.on_alert(3);
        settle(&mut n);
        assert_eq!(n.size(), alert_size(3));
    }

    #[test]
    fn banner_grows_with_its_text_up_to_four_lines() {
        assert_eq!(alert_size(1), [420.0, 68.0, 24.0]);
        assert_eq!(alert_size(2), [420.0, 86.0, 24.0], "two lines: the old tall banner");
        assert_eq!(alert_size(4), [420.0, 122.0, 24.0]);
        assert_eq!(alert_size(9), alert_size(BANNER_MAX_LINES as u8), "longer text ends with ... instead");
        assert_eq!(alert_size(0), target(Mode::Alert, false), "status banner (volume, brightness) keeps its size");
        assert!(alert_size(4)[1] < MAX_H);
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

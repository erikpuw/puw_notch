use crate::anim::{Spring, Tuning};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Mode {
    Collapsed,
    Mini,
    Peek,
    PeekMini,
    Expanded,
    Alert,
    Hidden,
}

pub fn target(mode: Mode, live: bool) -> [f32; 3] {
    match (mode, live) {
        (Mode::Collapsed, false) => [150.0, 30.0, 14.0],
        (Mode::Collapsed, true) => [216.0, 30.0, 14.0],
        (Mode::Mini, _) => [44.0, 4.0, 2.0],
        (Mode::Peek, false) => [166.0, 34.0, 15.0],
        (Mode::Peek, true) => [232.0, 34.0, 15.0],
        (Mode::PeekMini, _) => [72.0, 8.0, 4.0],
        (Mode::Expanded, _) => [480.0, 186.0, 28.0],
        (Mode::Alert, _) => [360.0, 52.0, 22.0],
        (Mode::Hidden, _) => [0.0; 3],
    }
}

pub const BANNER_MAX_LINES: u32 = 4;
pub const BANNER_LINE_H: f32 = 18.0;
pub const BANNER_TEXT_TOP: f32 = 43.0;

pub fn alert_size_choice(lines: u8) -> [f32; 3] {
    [520.0, BANNER_TEXT_TOP + lines.clamp(1, 3) as f32 * BANNER_LINE_H + 42.0, 24.0]
}

pub const ROW_H: f32 = 22.0;

pub fn alert_size(lines: u8) -> [f32; 3] {
    if lines == 0 {
        return target(Mode::Alert, false);
    }
    let n = (lines as u32).min(BANNER_MAX_LINES) as f32;
    [420.0, BANNER_TEXT_TOP + n * BANNER_LINE_H + 7.0, 24.0]
}

pub const LYRICS_EXTRA_W: f32 = 284.0;

pub const MAX_W: f32 = 820.0;
pub const MAX_H: f32 = 330.0;
pub const ALERT_MS: u32 = 5000;
pub const STATUS_MS: u32 = 1500;
const EAR_RATIO: f32 = 0.35;
const EAR_MAX: f32 = 14.0;

pub fn ear(h: f32) -> f32 {
    (h * EAR_RATIO).min(EAR_MAX)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Style {
    #[default]
    Ears,
    Pill,
}

const PILL_GAP: f32 = 6.0;

impl Style {
    pub fn gap(self) -> f32 {
        if self == Style::Pill { PILL_GAP } else { 0.0 }
    }

    pub fn gap_at(self, height: f32) -> f32 {
        self.gap() * ((height - 4.0) / 26.0).clamp(0.0, 1.0)
    }

    pub fn tag(self) -> &'static str {
        if self == Style::Pill { "pill" } else { "ears" }
    }

    pub fn from_tag(tag: &str) -> Option<Style> {
        match tag {
            "ears" => Some(Style::Ears),
            "pill" => Some(Style::Pill),
            _ => None,
        }
    }
}

pub fn hover_area(size: [f32; 3]) -> (f32, f32) {
    (size[0] / 2.0, size[1])
}
pub fn hover_stale(mode: Mode, hover_in: bool, editing: bool, inside: bool) -> bool {
    !editing && !inside && mode != Mode::Alert && (hover_in || matches!(mode, Mode::Peek | Mode::PeekMini | Mode::Expanded))
}

pub const DWELL_MS: u32 = 80;

pub struct Notch {
    pub mode: Mode,
    pub minimal: bool,
    pub live: bool,
    alert_lines: u8,
    alert_width: f32,
    alert_rows: u8,
    alert_choice: bool,
    wide: bool,
    panel: [f32; 2],
    obstructed: bool,
    clear_checks: u8,
    size: [Spring; 3],
}

impl Notch {
    pub fn new(minimal: bool) -> Self {
        let mut n = Notch { mode: Mode::Collapsed, minimal, live: false, alert_lines: 0, alert_width: 0.0, alert_rows: 0, alert_choice: false, wide: false, panel: [target(Mode::Expanded, false)[0], target(Mode::Expanded, false)[1]], obstructed: false, clear_checks: 0, size: [Spring::default(); 3] };
        n.mode = n.rest_mode();
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

    pub fn is_settled_rest(&self) -> bool {
        self.is_resting() && self.size() == self.goal()
    }

    pub fn set_hidden(&mut self, hidden: bool) {
        self.mode = if hidden { Mode::Hidden } else { self.rest_mode() };
        self.wide = false;
    }

    pub fn is_hidden(&self) -> bool {
        self.mode == Mode::Hidden && self.size.iter().all(|s| s.pos == 0.0 && s.vel == 0.0)
    }

    pub fn size(&self) -> [f32; 3] {
        [self.size[0].pos.max(0.0), self.size[1].pos.max(0.0), self.size[2].pos.max(0.0)]
    }

    pub fn set_live(&mut self, live: bool) -> bool {
        let changed = self.live != live;
        self.live = live;
        changed
    }

    pub fn on_enter(&mut self) -> bool {
        self.mode = match self.mode {
            Mode::Collapsed => Mode::Peek,
            Mode::Mini => Mode::PeekMini,
            Mode::Alert => Mode::Alert,
            _ => return false,
        };
        true
    }

    pub fn on_dwell(&mut self, inside: bool) {
        match self.mode {
            Mode::Peek | Mode::PeekMini => self.mode = if inside { Mode::Expanded } else { self.rest_mode() },
            Mode::Alert if inside => self.mode = Mode::Expanded,
            _ => {}
        }
    }

    pub fn on_leave(&mut self) {
        if !matches!(self.mode, Mode::Alert | Mode::Hidden) {
            self.mode = self.rest_mode();
        }
    }

    pub fn on_alert(&mut self, lines: u8) -> bool {
        if matches!(self.mode, Mode::Expanded | Mode::Hidden) {
            return false;
        }
        self.mode = Mode::Alert;
        self.alert_lines = lines;
        self.alert_width = 0.0;
        self.alert_rows = 0;
        self.alert_choice = false;
        true
    }

    pub fn set_alert_rows(&mut self, rows: u8) {
        self.alert_rows = rows;
    }

    pub fn set_alert_status(&mut self, rows: u8) {
        self.alert_lines = 0;
        self.alert_rows = rows;
    }

    pub fn set_alert_width(&mut self, width: f32) {
        self.alert_width = width;
    }

    pub fn on_alert_choice(&mut self, lines: u8) -> bool {
        let shown = self.on_alert(lines);
        self.alert_choice = shown;
        shown
    }

    fn goal(&self) -> [f32; 3] {
        if self.mode == Mode::Alert {
            let mut size = if self.alert_choice { alert_size_choice(self.alert_lines) } else { alert_size(self.alert_lines) };
            if self.alert_width > 0.0 {
                size[0] = self.alert_width;
            }
            size[1] += ROW_H * self.alert_rows as f32;
            return size;
        }
        let mut t = target(self.mode, self.live);
        if self.mode == Mode::Expanded {
            (t[0], t[1]) = (self.panel[0], self.panel[1]);
            if self.wide {
                t[0] += LYRICS_EXTRA_W;
            }
        }
        t
    }

    pub fn set_panel(&mut self, size: [f32; 2]) -> bool {
        let changed = self.panel != size;
        self.panel = size;
        changed
    }

    pub fn set_wide(&mut self, wide: bool) -> bool {
        let changed = self.wide != wide;
        self.wide = wide;
        changed
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

    pub fn obstructed(&self) -> bool {
        self.obstructed
    }

    pub fn report_busy(&mut self, busy: bool) {
        if busy {
            self.clear_checks = 0;
            self.obstructed = true;
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

    pub fn step(&mut self, dt: f32) -> bool {
        let t = self.goal();
        let mut moving = false;
        for (s, t) in self.size.iter_mut().zip(t) {
            moving |= s.step(t, dt, Tuning::NOTCH);
        }
        moving
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fullscreen_shrinks_to_zero_and_restores_the_resting_size() {
        for minimal in [false, true] {
            let mut notch = Notch::new(minimal);
            for _ in 0..240 { notch.step(1.0 / 120.0); }
            let before = notch.size();
            notch.set_hidden(true);
            assert_eq!(notch.size(), before, "requesting fullscreen must not jump the size");
            assert!(!notch.is_hidden());
            assert!(notch.step(1.0 / 120.0));
            assert!(notch.size()[0] > 0.0 && notch.size()[0] < before[0]);
            assert!(!notch.on_enter());
            assert!(!notch.on_alert(0));
            notch.on_leave();
            notch.on_dwell(true);
            for _ in 0..240 { notch.step(1.0 / 120.0); }
            assert!(notch.is_hidden());
            assert_eq!(notch.size(), [0.0; 3]);
            assert!(!notch.step(1.0 / 120.0), "fully hidden must stop rendering");
            notch.set_hidden(false);
            assert_eq!(notch.size(), [0.0; 3], "restoring must animate out of zero");
            for _ in 0..240 { notch.step(1.0 / 120.0); }
            let mode = if minimal { Mode::Mini } else { Mode::Collapsed };
            assert_eq!(notch.mode, mode);
            assert_eq!(notch.size(), target(mode, false));
        }
    }

    #[test]
    fn leaving_fullscreen_mid_shrink_reverses_without_a_size_jump() {
        let mut notch = Notch::new(false);
        notch.on_enter();
        notch.on_dwell(true);
        for _ in 0..240 { notch.step(1.0 / 120.0); }
        notch.set_hidden(true);
        for _ in 0..8 { notch.step(1.0 / 120.0); }
        let shrinking = notch.size();
        notch.set_hidden(false);
        assert_eq!(notch.size(), shrinking);
        for _ in 0..240 { notch.step(1.0 / 120.0); }
        assert_eq!(notch.mode, Mode::Collapsed);
        assert_eq!(notch.size(), target(Mode::Collapsed, false));
    }

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
    fn style_tags_round_trip_and_only_the_pill_floats() {
        for style in [Style::Ears, Style::Pill] {
            assert_eq!(Style::from_tag(style.tag()), Some(style));
        }
        assert_eq!(Style::from_tag("x"), None);
        assert_eq!(Style::default(), Style::Ears, "the original look unless chosen");
        assert_eq!(Style::Ears.gap(), 0.0);
        assert!(Style::Pill.gap() > 0.0);
        assert_eq!(Style::Pill.gap_at(30.0), Style::Pill.gap(), "the resting pill floats");
        assert_eq!(Style::Pill.gap_at(4.0), 0.0, "the thin line sits on the edge");
        assert!(Style::Pill.gap_at(17.0) > 0.0 && Style::Pill.gap_at(17.0) < Style::Pill.gap());
        assert_eq!(Style::Ears.gap_at(30.0), 0.0);
    }

    #[test]
    fn hover_area_is_the_pill_body() {
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
        assert!(n.set_live(true));
        assert!(!n.set_live(true));
        settle(&mut n);
        assert_eq!(n.size(), target(Mode::Collapsed, true));
    }

    #[test]
    fn lyrics_widen_only_the_open_panel_and_keep_its_height() {
        let mut n = Notch::new(false);
        assert!(n.set_wide(true));
        assert!(!n.set_wide(true), "no change the second time");
        settle(&mut n);
        assert_eq!(n.size(), target(Mode::Collapsed, false));
        n.on_enter();
        n.on_dwell(true);
        settle(&mut n);
        let [w, h, r] = target(Mode::Expanded, false);
        assert_eq!(n.size(), [w + LYRICS_EXTRA_W, h, r]);
        n.set_wide(false);
        settle(&mut n);
        assert_eq!(n.size(), [w, h, r]);
        assert!(w + LYRICS_EXTRA_W + 2.0 * 14.0 <= MAX_W);
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
                            spring.step(goal, dt, Tuning::NOTCH);
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
    fn busy_shrinks_at_once_and_clearing_needs_two_checks() {
        let mut n = Notch::new(false);
        n.report_busy(true);
        assert_eq!(n.mode, Mode::Mini, "a busy window shrinks the notch straight away");
        n.report_busy(false);
        assert_eq!(n.mode, Mode::Mini, "one quiet check is not enough to bring it back");
        n.report_busy(true);
        n.report_busy(false);
        assert_eq!(n.mode, Mode::Mini, "quiet checks must be in a row");
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
        assert!(n.on_enter());
        n.on_leave();
        assert_eq!(n.mode, Mode::Alert);
        n.on_alert_end();
        assert_eq!(n.mode, Mode::Mini);
        n.on_alert(0);
        n.on_enter();
        n.on_dwell(true);
        assert_eq!(n.mode, Mode::Expanded);
        assert!(!n.on_alert(0));
        n.on_alert_end();
        assert_eq!(n.mode, Mode::Expanded);
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

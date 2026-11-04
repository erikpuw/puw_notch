//! Pomodoro: one cycle of eight segments (focus 25, short break 5, ... long break 15). The timer lives in
//! memory only: a cycle in progress is not worth keeping across a restart.

use std::time::{Duration, Instant};

/// Minutes of a segment kind.
pub const FOCUS_MIN: u32 = 25;
pub const SHORT_MIN: u32 = 5;
pub const LONG_MIN: u32 = 15;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    Focus,
    Short,
    Long,
}

/// Four focus rounds, a short break after each of the first three, the long one at the end.
pub const CYCLE: [Kind; 8] = [Kind::Focus, Kind::Short, Kind::Focus, Kind::Short, Kind::Focus, Kind::Short, Kind::Focus, Kind::Long];

impl Kind {
    pub fn minutes(self) -> u32 {
        match self {
            Kind::Focus => FOCUS_MIN,
            Kind::Short => SHORT_MIN,
            Kind::Long => LONG_MIN,
        }
    }
}

/// A segment ran out: the one that ended and the one that comes now.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Done {
    pub ended: Kind,
    pub next: Kind,
}

#[derive(Clone, Debug)]
pub struct Focus {
    /// Index into `CYCLE`.
    pub seg: usize,
    /// Time left in the segment as of `since` (or now, while paused).
    left: Duration,
    /// Set while the clock runs.
    since: Option<Instant>,
}

impl Default for Focus {
    fn default() -> Self {
        Focus { seg: 0, left: seg_len(0), since: None }
    }
}

fn seg_len(seg: usize) -> Duration {
    Duration::from_secs(CYCLE[seg].minutes() as u64 * 60)
}

impl Focus {
    pub fn kind(&self) -> Kind {
        CYCLE[self.seg]
    }

    pub fn running(&self) -> bool {
        self.since.is_some()
    }

    /// Time left in the current segment.
    pub fn remaining(&self, now: Instant) -> Duration {
        self.since.map_or(self.left, |s| self.left.saturating_sub(now.saturating_duration_since(s)))
    }

    /// 0.0 .. 1.0 of a segment done (the segments before the current one are full, after it empty).
    pub fn fill(&self, seg: usize, now: Instant) -> f32 {
        match seg.cmp(&self.seg) {
            std::cmp::Ordering::Less => 1.0,
            std::cmp::Ordering::Greater => 0.0,
            std::cmp::Ordering::Equal => 1.0 - self.remaining(now).as_secs_f32() / seg_len(seg).as_secs_f32(),
        }
    }

    /// Time left in the whole cycle.
    pub fn cycle_left(&self, now: Instant) -> Duration {
        self.remaining(now) + (self.seg + 1..CYCLE.len()).map(seg_len).sum::<Duration>()
    }

    /// Which focus round (1..=4) the current segment belongs to.
    pub fn round(&self) -> usize {
        self.seg / 2 + 1
    }

    pub fn toggle(&mut self, now: Instant) {
        self.left = self.remaining(now);
        self.since = if self.since.is_some() { None } else { Some(now) };
    }

    /// Back to the start of the current segment, paused.
    pub fn reset(&mut self) {
        *self = Focus { seg: self.seg, left: seg_len(self.seg), since: None };
    }

    /// Start `seg` over, keeping the clock running if it was.
    pub fn jump(&mut self, seg: usize, now: Instant) {
        let running = self.running();
        *self = Focus { seg: seg % CYCLE.len(), left: seg_len(seg % CYCLE.len()), since: None };
        if running {
            self.since = Some(now);
        }
    }

    pub fn skip(&mut self, now: Instant) {
        self.jump(self.seg + 1, now);
    }

    /// The chips: the first segment of that kind from the current one on, wrapping round.
    pub fn pick(&mut self, kind: Kind, now: Instant) {
        if let Some(seg) = (0..CYCLE.len()).map(|d| (self.seg + d) % CYCLE.len()).find(|s| CYCLE[*s] == kind) {
            self.jump(seg, now);
        }
    }

    /// A running segment that has run out moves on to the next one, which starts at once: the user is told at
    /// every change. After the long break the cycle is over: it waits at the start, paused, for a new one.
    pub fn tick(&mut self, now: Instant) -> Option<Done> {
        if !self.running() || !self.remaining(now).is_zero() {
            return None;
        }
        let ended = self.kind();
        let seg = (self.seg + 1) % CYCLE.len();
        *self = Focus { seg, left: seg_len(seg), since: (seg != 0).then_some(now) };
        Some(Done { ended, next: self.kind() })
    }
}

#[cfg(test)]
impl Focus {
    /// A picture of the timer: segment `seg`, `elapsed` into it, running.
    pub fn demo(seg: usize, elapsed: Duration) -> Focus {
        Focus { seg, left: seg_len(seg).saturating_sub(elapsed), since: Some(Instant::now()) }
    }
}

/// "17:42" from a duration (rounded up, so the clock shows 00:01 until it is really over).
pub fn mmss(d: Duration) -> String {
    let s = d.as_secs() + u64::from(d.subsec_nanos() > 0);
    format!("{:02}:{:02}", s / 60, s % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cycle_is_130_minutes_and_ends_with_the_long_break() {
        let total: u32 = CYCLE.iter().map(|k| k.minutes()).sum();
        assert_eq!(total, 130);
        assert_eq!(CYCLE[7], Kind::Long);
        assert_eq!(CYCLE.iter().filter(|k| **k == Kind::Focus).count(), 4);
    }

    #[test]
    fn a_segment_runs_out_the_next_starts_and_the_cycle_ends_paused() {
        let t0 = Instant::now();
        let mut f = Focus::default();
        f.toggle(t0);
        assert_eq!(f.tick(t0 + Duration::from_secs(60)), None);
        let done = f.tick(t0 + Duration::from_secs(25 * 60)).unwrap();
        assert_eq!(done, Done { ended: Kind::Focus, next: Kind::Short });
        assert!(f.running(), "the break starts by itself");
        assert_eq!(f.remaining(t0 + Duration::from_secs(25 * 60)), Duration::from_secs(5 * 60));
        f.seg = 7;
        f.left = Duration::ZERO;
        f.since = Some(t0);
        assert_eq!(f.tick(t0).unwrap().next, Kind::Focus);
        assert_eq!(f.seg, 0);
        assert!(!f.running(), "after the long break a new cycle waits to be started");
    }

    #[test]
    fn pause_keeps_the_time_and_skip_keeps_the_clock_running() {
        let t0 = Instant::now();
        let mut f = Focus::default();
        f.toggle(t0);
        f.toggle(t0 + Duration::from_secs(90));
        assert_eq!(f.remaining(t0 + Duration::from_secs(500)), Duration::from_secs(25 * 60 - 90));
        f.toggle(t0 + Duration::from_secs(500));
        f.skip(t0 + Duration::from_secs(501));
        assert!(f.running());
        assert_eq!((f.kind(), f.round()), (Kind::Short, 1));
        assert_eq!(f.remaining(t0 + Duration::from_secs(501)), Duration::from_secs(5 * 60));
    }

    #[test]
    fn chips_go_to_the_next_segment_of_their_kind() {
        let t0 = Instant::now();
        let mut f = Focus::default();
        f.pick(Kind::Long, t0);
        assert_eq!(f.seg, 7);
        f.pick(Kind::Focus, t0);
        assert_eq!(f.seg, 0, "wraps round to the start");
        f.seg = 5;
        f.pick(Kind::Focus, t0);
        assert_eq!(f.seg, 6);
    }

    #[test]
    fn fill_and_cycle_left_follow_the_segments() {
        let t0 = Instant::now();
        let mut f = Focus::default();
        f.jump(2, t0);
        assert_eq!((f.fill(1, t0), f.fill(2, t0), f.fill(3, t0)), (1.0, 0.0, 0.0));
        assert_eq!(f.cycle_left(t0), Duration::from_secs((130 - 30) * 60));
        assert_eq!(mmss(Duration::from_millis(1500)), "00:02");
        assert_eq!(mmss(Duration::from_secs(25 * 60)), "25:00");
    }
}

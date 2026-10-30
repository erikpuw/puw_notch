//! Select lyrics from the media clock; advance their motion from the frame clock.

const LEAD_MS: u64 = 80;
/// Brightness of a line that is not the one being sung.
pub const DIM: f32 = 0.42;

fn smooth(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

pub fn current_index(lines: &[(u64, String)], pos_ms: u64) -> Option<usize> {
    lines.partition_point(|(t, _)| *t <= pos_ms.saturating_add(LEAD_MS)).checked_sub(1)
}

/// Target line, held until the next timestamp. Motion is independent of media polling.
pub fn phase(lines: &[(u64, String)], pos_ms: u64) -> f32 {
    current_index(lines, pos_ms).unwrap_or(0) as f32
}

#[derive(Default)]
struct Spring {
    x: f32,
    v: f32,
}

impl Spring {
    fn step(&mut self, goal: f32, dt: f32, k: f32, c: f32) -> bool {
        // Match the notch's small integration slices so low-refresh frames stay stable.
        let mut left = dt.clamp(0.0, 0.1);
        while left > 0.0 {
            let h = left.min(0.004);
            self.v += (k * (goal - self.x) - c * self.v) * h;
            self.x += self.v * h;
            left -= h;
        }
        if (self.x - goal).abs() < 0.002 && self.v.abs() < 0.05 {
            self.x = goal;
            self.v = 0.0;
            false
        } else {
            true
        }
    }
}

#[derive(Default)]
pub struct Motion {
    scroll: Spring,
    highlight: Spring,
    position: Option<u64>,
}

impl Motion {
    pub fn focus(&self) -> f32 { self.scroll.x }
    pub fn phase(&self) -> f32 { self.highlight.x }

    /// First display and real seeks land directly; ordinary playback retargets the springs.
    pub fn step(&mut self, focus: f32, phase: f32, position: u64, dt: f32, rate: f64) -> bool {
        let extra = (dt.max(0.0) as f64 * rate.max(0.0) * 1000.0) as u64;
        let reset = self.position.is_none_or(|last| last.saturating_add(extra).abs_diff(position) > 1000)
            || dt > 0.25;
        self.position = Some(position);
        if reset {
            self.scroll = Spring { x: focus, v: 0.0 };
            self.highlight = Spring { x: phase, v: 0.0 };
            return false;
        }
        let moving = self.scroll.step(focus, dt, 150.0, 21.0);
        self.highlight.step(phase, dt, 220.0, 28.0) || moving
    }
}

/// Top of each line when they are stacked with `gap` between them.
pub fn tops(heights: &[f32], gap: f32) -> Vec<f32> {
    let mut y = 0.0;
    heights
        .iter()
        .map(|h| {
            let top = y;
            y += h + gap;
            top
        })
        .collect()
}

/// Centre (in the stacked lines' own coordinates) of the line in focus at `phase`, moving
/// between the centres of the two lines it is gliding across.
pub fn focus(tops: &[f32], heights: &[f32], phase: f32) -> f32 {
    let n = tops.len().min(heights.len());
    if n == 0 {
        return 0.0;
    }
    let i = (phase.floor().max(0.0) as usize).min(n - 1);
    let centre = |k: usize| tops[k] + heights[k] / 2.0;
    if i + 1 >= n {
        return centre(i);
    }
    centre(i) + (centre(i + 1) - centre(i)) * (phase - i as f32).clamp(0.0, 1.0)
}

/// Brightness interpolates with the animated highlight, separately from line selection.
pub fn line_alpha(j: usize, phase: f32) -> f32 {
    DIM + (1.0 - DIM) * (1.0 - (j as f32 - phase).abs()).clamp(0.0, 1.0)
}

/// Fade towards the top and bottom of the lyrics area: 1 in the middle, 0 at the edge, over `edge`
/// px, for a line centred at `y` in an area from `top` to `bottom`.
pub fn edge_fade(y: f32, top: f32, bottom: f32, edge: f32) -> f32 {
    smooth((y - top).min(bottom - y) / edge.max(1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(times: &[u64]) -> Vec<(u64, String)> {
        times.iter().map(|&t| (t, format!("line at {t}"))).collect()
    }

    #[test]
    fn phase_selects_the_last_line_with_an_eighty_ms_lead() {
        let l = lines(&[1000, 5000, 9000]);
        assert_eq!(phase(&[], 3000), 0.0);
        assert_eq!(phase(&l, 0), 0.0, "before the first line");
        assert_eq!(phase(&l, 1000), 0.0, "first line starts");
        assert_eq!(phase(&l, 4000), 0.0, "still on line 0, before the glide");
        assert_eq!(phase(&l, 4919), 0.0, "animation must not select a future line early");
        assert_eq!(phase(&l, 4920), 1.0, "the next line is selected 80 ms early");
        assert_eq!(phase(&l, 5000), 1.0, "next line starts: focus is on it");
        assert_eq!(phase(&l, 7000), 1.0);
        assert_eq!(phase(&l, 99_000), 2.0, "after the last line");
    }

    #[test]
    fn selection_handles_close_and_duplicate_timestamps_and_seeks() {
        let l = lines(&[0, 200, 400, 5000]);
        let mut last = 0.0f32;
        for pos in (0..6000).step_by(10) {
            let p = phase(&l, pos);
            assert!(p >= last - 1e-6, "went back at {pos}: {last} -> {p}");
            last = p;
        }
        assert_eq!(phase(&lines(&[0, 200, 200, 400]), 120), 2.0);
        assert_eq!(phase(&l, 0), 0.0, "backward seek selects its destination directly");
    }

    #[test]
    fn motion_continues_between_media_updates_and_settles_when_paused() {
        let mut m = Motion::default();
        assert!(!m.step(10.0, 0.0, 4900, 0.016, 1.0));
        assert!(m.step(50.0, 1.0, 4920, 0.020, 1.0));
        let first = m.focus();
        assert!(first > 10.0 && first < 50.0);
        assert!(m.step(50.0, 1.0, 4920, 0.016, 0.0));
        assert!(m.focus() > first, "motion must continue without a new playback position");
        let mut moving = true;
        for _ in 0..120 { moving = m.step(50.0, 1.0, 4920, 0.016, 0.0); }
        assert!(!moving);
        assert_eq!((m.focus(), m.phase()), (50.0, 1.0));
    }

    #[test]
    fn seeks_and_long_frame_gaps_do_not_scroll_through_unrelated_lines() {
        let mut m = Motion::default();
        m.step(10.0, 0.0, 1000, 0.016, 1.0);
        assert!(!m.step(900.0, 20.0, 90_000, 0.016, 1.0));
        assert_eq!((m.focus(), m.phase()), (900.0, 20.0));
        assert!(!m.step(50.0, 1.0, 5000, 0.016, 1.0));
        assert_eq!((m.focus(), m.phase()), (50.0, 1.0));
        assert!(!m.step(80.0, 2.0, 5500, 0.5, 1.0));
        assert_eq!(m.focus(), 80.0);
    }

    #[test]
    fn scroll_is_consistent_across_refresh_rates_and_playback_speeds() {
        let sample = |fps: u32| {
            let mut m = Motion::default();
            m.step(0.0, 0.0, 1000, 0.0, 2.0);
            for frame in 1..=fps / 2 {
                m.step(80.0, 1.0, 1000 + frame as u64 * 2000 / fps as u64, 1.0 / fps as f32, 2.0);
            }
            m.focus()
        };
        let (slow, fast) = (sample(30), sample(144));
        assert!(slow > 79.0 && fast > 79.0);
        assert!((slow - fast).abs() < 0.2, "30 Hz {slow}, 144 Hz {fast}");
    }

    #[test]
    fn focus_moves_between_line_centres() {
        let heights = [20.0, 40.0, 20.0];
        let tops = tops(&heights, 10.0);
        assert_eq!(tops, vec![0.0, 30.0, 80.0]);
        assert_eq!(focus(&tops, &heights, 0.0), 10.0);
        assert_eq!(focus(&tops, &heights, 1.0), 50.0);
        assert_eq!(focus(&tops, &heights, 0.5), 30.0, "halfway between 10 and 50");
        assert_eq!(focus(&tops, &heights, 2.0), 90.0, "last line");
        assert_eq!(focus(&tops, &heights, 7.0), 90.0, "past the end stays on the last");
        assert_eq!(focus(&[], &[], 1.0), 0.0);
    }

    #[test]
    fn the_line_in_focus_is_brightest_and_the_rest_dim() {
        assert_eq!(line_alpha(3, 3.0), 1.0);
        assert_eq!(line_alpha(0, 3.0), DIM);
        assert_eq!(line_alpha(5, 3.0), DIM);
        let (a, b) = (line_alpha(3, 3.5), line_alpha(4, 3.5));
        assert!((a - b).abs() < 1e-6 && a > DIM && a < 1.0, "halfway: both partly lit, equally");
    }

    #[test]
    fn lines_fade_out_towards_the_edges() {
        assert_eq!(edge_fade(65.0, 0.0, 130.0, 24.0), 1.0, "middle");
        assert_eq!(edge_fade(0.0, 0.0, 130.0, 24.0), 0.0, "on the top edge");
        assert_eq!(edge_fade(130.0, 0.0, 130.0, 24.0), 0.0, "on the bottom edge");
        let (up, down) = (edge_fade(12.0, 0.0, 130.0, 24.0), edge_fade(118.0, 0.0, 130.0, 24.0));
        assert!((up - down).abs() < 1e-6 && up > 0.0 && up < 1.0);
        assert_eq!(edge_fade(-30.0, 0.0, 130.0, 24.0), 0.0, "outside the area");
    }
}

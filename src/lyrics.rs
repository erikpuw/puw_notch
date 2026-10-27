//! Where the lyrics scroll to and how bright each line is, from the song position. Pure maths: the
//! renderer measures how tall each wrapped line is and draws; this decides what is centred.

/// A line glides into the focus this long before its time stamp, so the scroll is a short smooth
/// move that ends as the line starts, not a slow creep.
pub const GLIDE_MS: u64 = 380;
/// Brightness of a line that is not the one being sung.
pub const DIM: f32 = 0.42;

fn smooth(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// Fractional index of the line in focus at `pos_ms`: line `i` while it is sung, gliding to
/// `i + 1` over the last GLIDE_MS before that line starts. Before the first line it sits on 0.
pub fn phase(lines: &[(u64, String)], pos_ms: u64) -> f32 {
    let started = lines.partition_point(|(t, _)| *t <= pos_ms);
    let Some(i) = started.checked_sub(1) else { return 0.0 };
    let mut p = i as f32;
    if let Some(&(next, _)) = lines.get(i + 1) {
        let start = next.saturating_sub(GLIDE_MS).max(lines[i].0);
        if pos_ms > start {
            p += smooth((pos_ms - start) as f32 / (next - start).max(1) as f32);
        }
    }
    p
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

/// Brightness of line `j`: full on the focus line, DIM two lines away and beyond, in between
/// while the focus glides.
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
    fn phase_sits_on_a_line_then_glides_to_the_next_as_it_starts() {
        let l = lines(&[1000, 5000, 9000]);
        assert_eq!(phase(&[], 3000), 0.0);
        assert_eq!(phase(&l, 0), 0.0, "before the first line");
        assert_eq!(phase(&l, 1000), 0.0, "first line starts");
        assert_eq!(phase(&l, 4000), 0.0, "still on line 0, before the glide");
        let mid = phase(&l, 5000 - GLIDE_MS / 2);
        assert!(mid > 0.3 && mid < 0.7, "halfway through the glide: {mid}");
        assert_eq!(phase(&l, 5000), 1.0, "next line starts: focus is on it");
        assert_eq!(phase(&l, 7000), 1.0);
        assert_eq!(phase(&l, 99_000), 2.0, "after the last line");
    }

    #[test]
    fn phase_never_goes_backwards_or_jumps() {
        // Lines closer together than the glide still move forward smoothly, in whole steps of at most one line.
        let l = lines(&[0, 200, 400, 5000]);
        let mut last = 0.0f32;
        for pos in (0..6000).step_by(10) {
            let p = phase(&l, pos);
            assert!(p >= last - 1e-6, "went back at {pos}: {last} -> {p}");
            assert!(p - last <= 0.6, "jumped at {pos}: {last} -> {p}");
            last = p;
        }
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

//! The Limits page: a tab per app (a dot says whether it has counted), a bar per limit under it.

use super::*;
use crate::usage::{self, APPS};

/// Where the rows start, their height, and the widths beside the bar (logical px).
const ROW_TOP: f32 = 62.0;
const ROW_H: f32 = 19.0;
const LABEL_W: f32 = 118.0;
const RIGHT_W: f32 = 92.0;
const RED: (f32, f32, f32) = (1.0, 0.35, 0.35);

/// Green while there is room, orange getting close, red nearly out.
fn bar_colour(used: f32) -> (f32, f32, f32) {
    if used < 60.0 { GREEN } else if used < 85.0 { ORANGE } else { RED }
}

impl Renderer {
    /// `tabs` are the page's buttons, `(cl, cr)` the content's left and right edge, `a` its opacity.
    pub(super) fn draw_usage(
        &self,
        text: &dyn Fn(&str, usize, D2D_RECT_F, DWRITE_TEXT_ALIGNMENT, D2D1_COLOR_F) -> Result<()>,
        tabs: &[(Button, D2D_RECT_F)],
        (cl, cr): (f32, f32),
        s: f32,
        a: f32,
        hovered: Option<Button>,
    ) -> Result<()> {
        let rt = &self.rt;
        let (lead, center, trail) = (DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_TRAILING);
        let solid = |c: (f32, f32, f32), a: f32| color(c.0, c.1, c.2, a);
        let tab = self.usage_tab.min(APPS.len() - 1);
        unsafe {
            for (i, app) in APPS.iter().enumerate() {
                let Some(&(_, b)) = tabs.iter().find(|(x, _)| *x == Button::UsageTab(i)) else { continue };
                let shade = if i == tab { 0.22 } else if hovered == Some(Button::UsageTab(i)) { 0.17 } else { 0.11 };
                rt.FillRoundedRectangle(&rounded(b, 11.0 * s), &self.brush(grey(shade, a))?);
                let dot = D2D1_ELLIPSE { point: Vector2 { X: b.left + 11.0 * s, Y: (b.top + b.bottom) / 2.0 }, radiusX: 3.0 * s, radiusY: 3.0 * s };
                let counted = self.usage.get(*app).is_some();
                rt.FillEllipse(&dot, &self.brush(if counted { solid(GREEN, a) } else { grey(0.35, a) })?);
                text(app.name(), F_LABEL, rect(b.left + 18.0 * s, b.top, b.right - 4.0 * s, b.bottom), center, grey(if i == tab { 1.0 } else { 0.65 }, a))?;
            }
            let now = usage::now_secs();
            let Some(bars) = self.usage.get(APPS[tab]) else {
                text(APPS[tab].open_hint(), F_LABEL, rect(cl, ROW_TOP * s, cr, (ROW_TOP + 60.0) * s), center, grey(0.6, a))?;
                return Ok(());
            };
            for (k, bar) in bars.iter().enumerate() {
                let y = (ROW_TOP + k as f32 * ROW_H) * s;
                text(&bar.label, F_LABEL, rect(cl, y, cl + LABEL_W * s, y + ROW_H * s), lead, grey(0.85, a))?;
                let mid = y + ROW_H * s / 2.0;
                let track = rect(cl + (LABEL_W + 6.0) * s, mid - 3.0 * s, cr - (RIGHT_W + 6.0) * s, mid + 3.0 * s);
                rt.FillRoundedRectangle(&rounded(track, 3.0 * s), &self.brush(grey(0.2, a))?);
                let filled = (bar.used / 100.0).clamp(0.0, 1.0);
                if filled > 0.0 {
                    // Never thinner than a dot, so a little use still shows.
                    let to = track.left + (track.right - track.left) * filled.max(0.03);
                    rt.FillRoundedRectangle(&rounded(rect(track.left, track.top, to, track.bottom), 3.0 * s), &self.brush(solid(bar_colour(bar.used), a))?);
                }
                let right = if bar.resets_at > 0 { format!("{}% · {}", bar.used.round(), usage::in_words(bar.resets_at, now)) } else { format!("{}%", bar.used.round()) };
                text(&right, F_LABEL, rect(cr - RIGHT_W * s, y, cr, y + ROW_H * s), trail, grey(0.7, a))?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bars_turn_orange_then_red() {
        assert_eq!(bar_colour(10.0), GREEN);
        assert_eq!(bar_colour(70.0), ORANGE);
        assert_eq!(bar_colour(95.0), RED);
    }
}

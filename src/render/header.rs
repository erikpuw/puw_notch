//! The panel's header: the page's name (it morphs with the page), the date and the time, the battery ring and its percentage.

use super::*;

impl Renderer {
    pub(super) fn draw_header(
        &self,
        c: &Ctx,
        a: f32,
    ) -> Result<()> {
        let Ctx { v, s, l, r, cx, text, width, .. } = *c;
        let lead = DWRITE_TEXT_ALIGNMENT_LEADING;
        let trail = DWRITE_TEXT_ALIGNMENT_TRAILING;
        let (hl, hr) = (l + PAD * s, r - PAD * s);
        // Header: the page's name on the left (it morphs with the page), the date and the time
        // in the middle, the battery ring and its percentage on the right.
        let (ht, hb) = (6.0 * s, 28.0 * s);
        let draw_title = |page: Page, dx: f32, ta: f32| -> Result<()> {
            if ta <= 0.01 {
                return Ok(());
            }
            text(page.name(), F_HEAD, rect(hl + dx, ht, hl + dx + 170.0 * s, hb), lead, grey(1.0, a * ta))
        };
        match v.leaving {
            Some((from, t, dir)) => {
                let d = dir as f32 * 24.0 * s;
                let (out_a, in_a) = page_fades(t);
                draw_title(from, -d * t.min(1.0), out_a)?;
                draw_title(v.page, d * (1.0 - ease_out(t)), in_a)?;
            }
            None => draw_title(v.page, 0.0, 1.0)?,
        }
        // The same battery ring as the pill's, then its percentage.
        if let Some(b) = &v.battery {
            let pct = format!("{}%", b.percent);
            let tx = hr - width(&pct, F_LABEL);
            text(&pct, F_LABEL, rect(tx, ht, hr, hb), trail, grey(0.85, a))?;
            let d = 20.0 * s;
            self.draw_battery_ring((tx - 3.0 * s - d / 2.0, (ht + hb) / 2.0), d, b, a)?;
        }
        // Date and time in the middle, the time bolder.
        {
            let (date, time) = (v.date, v.time);
            let (dw, tw) = (width(date, F_CLOCK), width(time, F_CLOCK_BOLD));
            let gap = 9.0 * s;
            let x = cx - (dw + gap + tw) / 2.0;
            text(date, F_CLOCK, rect(x, ht, x + dw + 2.0 * s, hb), lead, grey(0.75, a))?;
            text(time, F_CLOCK_BOLD, rect(x + dw + gap, ht, x + dw + gap + tw + 2.0 * s, hb), lead, grey(1.0, a))?;
        }
        Ok(())
    }
}

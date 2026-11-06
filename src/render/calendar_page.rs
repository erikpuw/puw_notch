//! The Calendar page: the day picked, big, with its weekday on the left (and the Solar | Lunar choice where the
//! region keeps a lunar calendar); the month on the right, weeks from Monday.

use super::*;
use crate::calendar::{self as cal, CalView};

impl Renderer {
    pub(super) fn draw_calendar(
        &self,
        text: &dyn Fn(&str, usize, D2D_RECT_F, DWRITE_TEXT_ALIGNMENT, D2D1_COLOR_F) -> Result<()>,
        width: &dyn Fn(&str, usize) -> f32,
        hover_disc: &dyn Fn(Button) -> Result<()>,
        btns: &[(Button, D2D_RECT_F)],
        c: &CalView,
        (cl, _cr): (f32, f32),
        s: f32,
        a: f32,
        hovered: Option<Button>,
    ) -> Result<()> {
        let rt = &self.rt;
        let center = DWRITE_TEXT_ALIGNMENT_CENTER;
        let english = crate::i18n::english();
        let rect_of = |b: Button| btns.iter().find(|(x, _)| *x == b).map(|(_, r)| *r);
        // The lunar numbers show only where the region has them and they are chosen.
        let zone = c.zone.filter(|_| c.lunar_on);
        unsafe {
            // Left column: the day picked, its weekday, the other calendar's date, the choice.
            let sel = c.sel.max(1);
            let big = zone.map_or(sel, |z| cal::lunar(c.year, c.month, sel, z).day);
            let total = CAL_LEFT_H + if c.zone.is_some() { CAL_SWITCH_H } else { 0.0 };
            let top = CAL_TOP + (CAL_BLOCK_H - total) / 2.0;
            let col_box = |y0: f32, y1: f32| rect(cl, (top + y0) * s, cl + CAL_LEFT_W * s, (top + y1) * s);
            text(&big.to_string(), F_DAY, col_box(0.0, 52.0), center, grey(1.0, a))?;
            text(cal::weekday_long(cal::weekday(c.year, c.month, sel), english), F_TITLE, col_box(58.0, 76.0), center, grey(1.0, a))?;
            if zone.is_some() {
                let other = if english { format!("Solar {sel}/{}", c.month) } else { format!("{sel}/{} dương lịch", c.month) };
                text(&other, F_LABEL, col_box(78.0, 94.0), center, grey(0.6, a))?;
            }
            if c.zone.is_some() {
                let pill = col_box(102.0, 124.0);
                let px = cl + (CAL_LEFT_W - 2.0 * CAL_SEG_W - 4.0) / 2.0 * s;
                let bar = rect(px, pill.top, px + (2.0 * CAL_SEG_W + 4.0) * s, pill.bottom);
                rt.FillRoundedRectangle(&rounded(bar, (bar.bottom - bar.top) / 2.0), &self.brush(grey(0.12, a))?);
                for (k, label) in [(false, tr("Dương", "Solar")), (true, tr("Âm", "Lunar"))] {
                    let x = bar.left + 2.0 * s + if k { CAL_SEG_W * s } else { 0.0 };
                    let seg = rect(x, bar.top + 2.0 * s, x + CAL_SEG_W * s, bar.bottom - 2.0 * s);
                    let on = c.lunar_on == k;
                    if on {
                        rt.FillRoundedRectangle(&rounded(seg, (seg.bottom - seg.top) / 2.0), &self.brush(grey(1.0, a))?);
                    }
                    let ink = if on { grey(0.05, a) } else if hovered == Some(Button::CalMode(k)) { grey(1.0, a) } else { grey(0.75, a) };
                    text(label, F_LABEL, seg, center, ink)?;
                }
            }

            // Right column: the month's title between its two arrows, the weekday heads, the days.
            let gx = cl + CAL_GRID_X * s;
            let head = rect(gx + CAL_COL_W * s, CAL_TITLE_Y * s, gx + 6.0 * CAL_COL_W * s, (CAL_TITLE_Y + 20.0) * s);
            let title = match zone {
                Some(z) => cal::lunar_title(c.year, c.month, z, english),
                None => cal::month_title(c.year, c.month, english),
            };
            text(&title, F_TITLE, head, center, grey(1.0, a))?;
            for (b, glyph) in [(Button::CalPrev, ICON_LEFT), (Button::CalNext, ICON_RIGHT)] {
                if let Some(r) = rect_of(b) {
                    hover_disc(b)?;
                    text(glyph, F_SMALL_ICON, r, center, if hovered == Some(b) { grey(1.0, a) } else { grey(0.55, a) })?;
                }
            }
            for k in 0..7u32 {
                let x = gx + k as f32 * CAL_COL_W * s;
                text(cal::weekday_short(k, english), F_BODY, rect(x, CAL_HEAD_Y * s, x + CAL_COL_W * s, (CAL_HEAD_Y + CAL_ROW_H) * s), center, grey(0.5, a))?;
            }
            let (first, days) = (cal::first_column(c.year, c.month) as i32, cal::days_in_month(c.year, c.month) as i32);
            for i in 0..42usize {
                let day = i as i32 - first + 1;
                if !(1..=days).contains(&day) {
                    continue;
                }
                let day = day as u32;
                let (col, row) = ((i % 7) as f32, (i / 7) as f32);
                let cell = rect(gx + col * CAL_COL_W * s, (CAL_ROW0 + row * CAL_ROW_H) * s, gx + (col + 1.0) * CAL_COL_W * s, (CAL_ROW0 + (row + 1.0) * CAL_ROW_H) * s);
                let l = zone.map(|z| cal::lunar(c.year, c.month, day, z));
                // A new lunar month says which one it is ("1/10"); every other day is its number.
                let label = match l {
                    Some(l) if l.day == 1 => format!("1/{}", l.month),
                    Some(l) => l.day.to_string(),
                    None => day.to_string(),
                };
                let font = if l.is_some_and(|l| l.day == 1) { F_CLOCK_BOLD } else { F_CLOCK };
                let (mx, my) = ((cell.left + cell.right) / 2.0, (cell.top + cell.bottom) / 2.0);
                let half = ((width(&label, font) / s + 8.0).max(CAL_CHIP_H) / 2.0) * s;
                let chip = rect(mx - half, my - CAL_CHIP_H * s / 2.0, mx + half, my + CAL_CHIP_H * s / 2.0);
                let today = c.today == (c.year, c.month, day);
                if today {
                    rt.FillRoundedRectangle(&rounded(chip, CAL_CHIP_H * s / 2.0), &self.brush(color(GREEN.0, GREEN.1, GREEN.2, a))?);
                } else if day == sel {
                    let inner = rect(chip.left + 0.5, chip.top + 0.5, chip.right - 0.5, chip.bottom - 0.5);
                    rt.DrawRoundedRectangle(&rounded(inner, CAL_CHIP_H * s / 2.0), &self.brush(grey(0.7, a))?, 1.0 * s, None);
                } else if hovered == Some(Button::CalCell(i)) {
                    rt.FillRoundedRectangle(&rounded(chip, CAL_CHIP_H * s / 2.0), &self.brush(grey(0.14, a))?);
                }
                // Today is dark on its green disc; the first day of a lunar month ("1/10") is yellow, so it stands out where a
                // bolder weight at 14 px would not.
                let ink = if today { color(0.02, 0.13, 0.04, a) } else if l.is_some_and(|l| l.day == 1) { color(YELLOW.0, YELLOW.1, YELLOW.2, a) } else { grey(1.0, a) };
                text(&label, if today { F_CLOCK_BOLD } else { font }, cell, center, ink)?;
            }
        }
        Ok(())
    }
}

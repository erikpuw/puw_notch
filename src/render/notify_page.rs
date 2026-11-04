//! The Notifications page: the count and Clear all on the left, the list of the latest Windows notifications.

use super::*;

impl Renderer {
    pub(super) fn draw_notify(
        &self,
        text: &dyn Fn(&str, usize, D2D_RECT_F, DWRITE_TEXT_ALIGNMENT, D2D1_COLOR_F) -> Result<()>,
        scrollbar: &dyn Fn(usize, usize, f32, f32) -> Result<()>,
        btns: &[(Button, D2D_RECT_F)],
        v: &View,
        lv: &ListView,
        current: bool,
        note_icons: &[Option<ID2D1Bitmap>],
        (cl, cr): (f32, f32),
        s: f32,
        a: f32,
        hovered: Option<Button>,
    ) -> Result<()> {
        let rt = &self.rt;
        let (center, lead, trail) = (DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_ALIGNMENT_TRAILING);
        let solid = |c: (f32, f32, f32), a: f32| color(c.0, c.1, c.2, a);
        let rect_of = |b: Button| btns.iter().find(|(x, _)| *x == b).map(|(_, r)| *r);
        unsafe {
            let (x0, xr) = (cl + NOTE_X * s, cr);
            let today = (lv.now / 10_000) as u32;
            // An app's mark: its own icon as it is, or its initial on a colour taken from its name.
            let note_tile = |row: usize, tile: D2D_RECT_F| -> Result<()> {
                let Some(n) = v.notes.get(row) else { return Ok(()) };
                match note_icons.get(row).and_then(|i| i.as_ref()) {
                    Some(icon) => {
                        // The icon itself, as Windows draws it: no tile behind it, no margin around it.
                        rt.DrawBitmap(icon, Some(&tile), a, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None);
                    }
                    None => {
                        let (tr_, tg, tb) = app_tint(&n.app);
                        rt.FillRoundedRectangle(&rounded(tile, 5.0 * s), &self.brush(color(tr_, tg, tb, a))?);
                        text(&app_initial(&n.app), F_LABEL, tile, center, grey(1.0, a))?;
                    }
                }
                Ok(())
            };
            // Left: how many notifications, and Clear all.
            text(&v.notes.len().to_string(), F_FIGURE, rect(cl, 56.0 * s, cl + (NOTE_X - 8.0) * s, 92.0 * s), lead, grey(1.0, a))?;
            text(tr("thông báo", "notifications"), F_LABEL, rect(cl, 92.0 * s, cl + (NOTE_X - 4.0) * s, 110.0 * s), lead, grey(0.9, a))?;
            if let Some(cb) = rect_of(Button::NotesClear) {
                let hot = hovered == Some(Button::NotesClear);
                text(tr("Xóa hết", "Clear all"), F_LABEL, cb, lead, if hot { color(0.75, 0.85, 1.0, a) } else { solid(BLUE, a) })?;
            }
            if v.notes.is_empty() {
                let area = rect(x0, NOTE_Y * s, xr, (NOTE_Y + NOTE_H * NOTE_ROWS as f32) * s);
                text(tr("Chưa có thông báo", "No notifications"), F_BODY, area, center, grey(0.55, a))?;
            }
            // Right: the same for every notification, its tile and two lines (the title and its time, then the app and
            // the text, cut with "..." when long). Rows glide to their place after a wheel step: drawn
            // from the fractional position, inside the list's own clip.
            let pos = if current { lv.note_pos } else { 0.0 };
            let first = pos.floor().max(0.0) as usize;
            rt.PushAxisAlignedClip(&rect(x0 - 8.0 * s, NOTE_Y * s, xr + 10.0 * s, (NOTE_Y + NOTE_H * NOTE_ROWS as f32 + 2.0) * s), D2D1_ANTIALIAS_MODE_ALIASED);
            for (row, n) in v.notes.iter().enumerate().skip(first).take(NOTE_ROWS + 1) {
                let y = (NOTE_Y + (row as f32 - pos) * NOTE_H) * s;
                // The row under the mouse lights up: clicking it opens the notification.
                if hovered == Some(Button::NoteOpen(row)) {
                    rt.FillRoundedRectangle(&rounded(rect(x0 - 6.0 * s, y - 1.0 * s, xr + 2.0 * s, y + (NOTE_H - 3.0) * s), 8.0 * s), &self.brush(grey(0.08, a))?);
                }
                if row + 1 < v.notes.len() {
                    rt.FillRectangle(&rect(x0, y + (NOTE_H - 2.0) * s, xr, y + (NOTE_H - 1.0) * s), &self.brush(grey(0.11, a))?);
                }
                let day = (n.at / 10_000) as u32;
                let when = if day == today { format!("{:02}:{:02}", n.at / 100 % 100, n.at % 100) }
                    else if todo::next_day(day) == today { tr("Hôm qua", "Yesterday").to_string() }
                    else { format!("{:02}/{:02}", day % 100, day / 100 % 100) };
                // The app's tile, so it is plain whose message it is, then the two lines.
                note_tile(row, rect(x0, y + 8.5 * s, x0 + 16.0 * s, y + 24.5 * s))?;
                let tx = x0 + 24.0 * s;
                text(&when, F_LABEL, rect(xr - 64.0 * s, y, xr, y + 18.0 * s), trail, grey(0.55, a))?;
                text(&n.title, F_TITLE, rect(tx, y, xr - 68.0 * s, y + 18.0 * s), lead, grey(1.0, a))?;
                let line2 = if n.body.is_empty() { n.app.clone() } else { format!("{} · {}", n.app, n.body) };
                text(&line2, F_BODY, rect(tx, y + 15.0 * s, xr, y + 32.0 * s), lead, grey(0.65, a))?;
            }
            rt.PopAxisAlignedClip();
            scrollbar(v.notes.len(), NOTE_ROWS, NOTE_Y, NOTE_H)?;
        }
        Ok(())
    }
}

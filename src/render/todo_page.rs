use super::*;

impl Renderer {
    pub(super) fn draw_todo(
        &self,
        text: &dyn Fn(&str, usize, D2D_RECT_F, DWRITE_TEXT_ALIGNMENT, D2D1_COLOR_F) -> Result<()>,
        width: &dyn Fn(&str, usize) -> f32,
        micon: &dyn Fn(&'static str, &'static str, usize, D2D_RECT_F, D2D1_COLOR_F) -> Result<()>,
        draw_input: &dyn Fn(&str) -> Result<()>,
        hover_disc: &dyn Fn(Button) -> Result<()>,
        row_tools: &dyn Fn(usize, bool) -> Result<()>,
        row_hovered: &dyn Fn(usize) -> bool,
        scrollbar: &dyn Fn(usize, usize, f32, f32) -> Result<()>,
        time_color: &dyn Fn(u64, bool) -> D2D1_COLOR_F,
        btns: &[(Button, D2D_RECT_F)],
        v: &View,
        lv: &ListView,
        current: bool,
        settings: Option<Card>,
        (cl, cr): (f32, f32),
        s: f32,
        a: f32,
        hovered: Option<Button>,
    ) -> Result<()> {
        let rt = &self.rt;
        let (center, lead, trail) = (DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_ALIGNMENT_TRAILING);
        let solid = |c: (f32, f32, f32), a: f32| color(c.0, c.1, c.2, a);
        let minutes = todo::minutes;
        let rect_of = |b: Button| btns.iter().find(|(x, _)| *x == b).map(|(_, r)| *r);
        unsafe {
            let (x0, xr) = (cl + LIST_X * s, cr);
            draw_input(tr("Thêm ghi chú… (vd: Họp 20:00)", "Add a note… (e.g. Call 20:00)"))?;
            let today = (lv.now / 10_000) as u32;
            let (open, later) = if current { lv.counts } else { (0, 0) };
            let hub = (cl + HERO_W / 2.0 * s, 80.0 * s);
            let done = v.todos.iter().filter(|t| t.done.is_some()).count();
            let frac = if v.todos.is_empty() { 0.0 } else { done as f32 / v.todos.len() as f32 };
            self.draw_ring_on(hub, 84.0 * s, 7.0 * s, frac, GREEN, color(GREEN.0 * 0.2, GREEN.1 * 0.2, GREEN.2 * 0.2, a), a)?;
            text(&open.to_string(), F_FIGURE, rect(hub.0 - 40.0 * s, hub.1 - 22.0 * s, hub.0 + 40.0 * s, hub.1 + 22.0 * s), center, grey(1.0, a))?;
            text(tr("còn lại", "left"), F_LABEL, rect(cl, 122.0 * s, cl + HERO_W * s, 137.0 * s), center, grey(0.9, a))?;
            text(&format!("{later} {}", tr("sắp tới", "upcoming")), F_LABEL, rect(cl, 136.0 * s, cl + HERO_W * s, 151.0 * s), center, grey(0.5, a))?;
            let open_set = settings == Some(Card::Tasks);
            if let Some(gb) = rect_of(Button::Gear(Card::Tasks)) {
                let hot = hovered == Some(Button::Gear(Card::Tasks));
                let (icon, c) = if open_set { (ICON_CHECK, solid(GREEN, a)) } else { (ICON_GEAR, grey(if hot { 1.0 } else { 0.45 }, a)) };
                hover_disc(Button::Gear(Card::Tasks))?;
                micon("tasks-gear", icon, F_SMALL_ICON, gb, c)?;
            }
            if open_set {
                let y = ROW_Y * s;
                text(tr("Nhắc trước", "Remind before"), F_BODY, rect(x0, y, xr - 112.0 * s, y + 22.0 * s), lead, grey(0.9, a))?;
                let value = if lv.remind_before == 0 { tr("Tắt", "Off").to_string() } else { format!("{} {}", lv.remind_before, tr("phút", "min")) };
                text(&value, F_BODY, rect(xr - 88.0 * s, y, xr - 26.0 * s, y + 22.0 * s), center, grey(1.0, a))?;
                for (b, icon) in [(Button::RemindStep(-1), ICON_MINUS), (Button::RemindStep(1), ICON_PLUS)] {
                    if let Some(sb) = rect_of(b) {
                        rt.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: (sb.left + sb.right) / 2.0, Y: (sb.top + sb.bottom) / 2.0 }, radiusX: 11.0 * s, radiusY: 11.0 * s },
                            &self.brush(grey(if hovered == Some(b) { 0.3 } else { 0.2 }, a))?);
                        text(icon, F_TINY_ICON, sb, center, grey(1.0, a))?;
                    }
                }
                text(tr("Ghi chú có giờ còn được nhắc đúng giờ", "Timed notes also ring on time"), F_LABEL,
                    rect(x0, y + 26.0 * s, xr, y + 44.0 * s), lead, grey(0.5, a))?;
            } else {
                if v.todos.is_empty() {
                    let mid = (x0 + xr) / 2.0;
                    let ring = D2D1_ELLIPSE { point: Vector2 { X: mid, Y: (ROW_Y + 16.0) * s }, radiusX: 11.0 * s, radiusY: 11.0 * s };
                    rt.DrawEllipse(&ring, &self.brush(grey(0.4, a))?, 1.5 * s, None);
                    let w = (2.0 * s).round().max(2.0);
                    let (cx, cy) = (mid.round(), ((ROW_Y + 16.0) * s).round());
                    let ink = self.brush(grey(0.6, a))?;
                    let at = |dx: f32, dy: f32| Vector2 { X: cx + dx * s, Y: cy + dy * s };
                    rt.DrawLine(at(-4.5, 0.5), at(-1.5, 3.5), &ink, w, &self.round);
                    rt.DrawLine(at(-1.5, 3.5), at(4.5, -3.0), &ink, w, &self.round);
                    text(tr("Chưa có ghi chú nào", "No notes yet"), F_BODY, rect(x0, (ROW_Y + 32.0) * s, xr, (ROW_Y + 50.0) * s), center, grey(0.7, a))?;
                    text(tr("Gõ vào ô trên rồi nhấn Enter", "Type above and press Enter"), F_LABEL, rect(x0, (ROW_Y + 50.0) * s, xr, (ROW_Y + 66.0) * s), center, grey(0.5, a))?;
                }
                let rows_on: Vec<_> = btns.iter().filter(|(b, _)| matches!(b, Button::ListToggle(_))).collect();
                for (n, (b, br)) in rows_on.iter().enumerate() {
                    let Button::ListToggle(i) = *b else { continue };
                    let Some(item) = v.todos.get(i) else { continue };
                    let hot = row_hovered(i);
                    if hot {
                        rt.FillRoundedRectangle(&rounded(rect(br.left - 6.0 * s, br.top + 1.0 * s, xr + 2.0 * s, br.bottom - 1.0 * s), 8.0 * s), &self.brush(grey(0.08, a))?);
                    }
                    if n + 1 < rows_on.len() {
                        rt.FillRectangle(&rect(br.left, br.bottom - 1.0 * s, xr, br.bottom), &self.brush(grey(0.12, a))?);
                    }
                    let d = 13.0 * s;
                    let (ccx, ccy) = (br.left + d / 2.0, (br.top + br.bottom) / 2.0);
                    let circle = D2D1_ELLIPSE { point: Vector2 { X: ccx, Y: ccy }, radiusX: d / 2.0 - 0.75 * s, radiusY: d / 2.0 - 0.75 * s };
                    let done = item.done.is_some();
                    if done {
                        rt.FillEllipse(&circle, &self.brush(solid(GREEN, a))?);
                        text(ICON_CHECK, F_TINY_ICON, rect(ccx - d / 2.0, ccy - d / 2.0, ccx + d / 2.0, ccy + d / 2.0), center, grey(0.0, a))?;
                    } else {
                        let ring = match item.at {
                            Some(at) if minutes(at) - minutes(lv.now) <= lv.remind_before.max(15) as i64 => time_color(at, false),
                            _ => grey(if hot { 0.85 } else { 0.5 }, a),
                        };
                        rt.DrawEllipse(&circle, &self.brush(ring)?, 1.5 * s, None);
                    }
                    let mut text_right = br.right;
                    if let Some(at) = item.at {
                        let (day, h, m) = ((at / 10_000) as u32, at / 100 % 100, at % 100);
                        let when = if day == today { format!("{h:02}:{m:02}") }
                            else if day == todo::next_day(today) { format!("{} {h:02}:{m:02}", tr("Mai", "Tmrw")) }
                            else { format!("{:02}/{:02} {h:02}:{m:02}", day % 100, day / 100 % 100) };
                        let left = minutes(at) - minutes(lv.now);
                        let flag = if done { "" } else if left <= 0 { tr("Trễ", "Late") } else if left <= lv.remind_before.max(15) as i64 { tr("Sắp", "Soon") } else { "" };
                        let when = if !flag.is_empty() && day != today { format!("{:02}/{:02}", day % 100, day / 100 % 100) } else { when };
                        let label = if flag.is_empty() { when } else { format!("{flag} {when}") };
                        let lw = width(&label, F_LABEL) + 4.0 * s;
                        text(&label, F_LABEL, rect(br.right - lw, br.top, br.right - 2.0 * s, br.bottom), trail, time_color(at, done))?;
                        text_right = br.right - lw - 6.0 * s;
                    }
                    let tb = rect(br.left + d + 10.0 * s, br.top, text_right, br.bottom);
                    text(&item.text, F_BODY, tb, lead, grey(if done { 0.5 } else { 1.0 }, a))?;
                    if done {
                        let tw = width(&item.text, F_BODY).min(tb.right - tb.left);
                        let my = (br.top + br.bottom) / 2.0 + 0.5 * s;
                        rt.FillRectangle(&rect(tb.left, my, tb.left + tw, my + 1.0 * s), &self.brush(grey(0.5, a))?);
                    }
                    if hot {
                        row_tools(i, hot)?;
                    }
                }
                scrollbar(v.todos.len(), LIST_ROWS, ROW_Y, ROW_H)?;
            }
        }
        Ok(())
    }
}

use super::*;
use crate::task::{self, Status, Tasks};

pub(super) const WEEK_W: f32 = 170.0;
pub(super) const DIVIDER_X: f32 = 182.0;
pub(super) const LIST_X: f32 = 194.0;
pub(super) const TASK_ROW_Y: f32 = 58.0;
pub(super) const TASK_ROW_H: f32 = 26.0;
pub(super) const TASK_ROWS: usize = 4;
pub(super) const HEAD_Y: f32 = 32.0;
pub(super) const SWITCH_W: f32 = 30.0;
pub(super) const SWITCH_H: f32 = 18.0;
const DOT: f32 = 4.0;
const DAYS_Y: f32 = 78.0;
pub(super) const RED: (f32, f32, f32) = (1.0, 0.35, 0.35);
const EDIT_LIST_W: f32 = 164.0;
const EDIT_DIVIDER_X: f32 = 172.0;
const PANE_X: f32 = 184.0;
const PANE_LABEL_W: f32 = 54.0;
const EDIT_ROW_Y: f32 = 60.0;
const EDIT_ROW_H: f32 = 24.0;
const FIELD_H: f32 = 22.0;
const STEP: f32 = 18.0;
const CHIP_GAP: f32 = 3.0;
const NAME_Y: f32 = 32.0;
const REPEAT_Y: f32 = 58.0;
const DUE_Y: f32 = 80.0;
const SLOT_Y: f32 = 102.0;
const SNOOZE_Y: f32 = 124.0;
const DATE_W: f32 = 52.0;
const TIME_W: f32 = 38.0;
const SNOOZE_W: f32 = 56.0;
const REPEATS: [(task::Repeat, f32); 4] = [(task::Repeat::Once, 40.0), (task::Repeat::Weekly, 40.0), (task::Repeat::Monthly, 44.0), (task::Repeat::Quarterly, 36.0)];
const SLOTS: [(u8, f32); 3] = [(task::SLOT_BOOT, 54.0), (task::SLOT_NOON, 44.0), (task::SLOT_FOUR, 44.0)];

fn repeat_label(r: task::Repeat) -> &'static str {
    match r {
        task::Repeat::Once => tr("1 lần", "Once"),
        task::Repeat::Weekly => tr("Tuần", "Week"),
        task::Repeat::Monthly => tr("Tháng", "Month"),
        task::Repeat::Quarterly => tr("Quý", "Qtr"),
    }
}

fn slot_label(slot: u8) -> &'static str {
    match slot {
        task::SLOT_BOOT => tr("Mở máy", "Startup"),
        task::SLOT_NOON => "12:00",
        _ => "16:00",
    }
}

pub(super) fn edit_buttons(r: &dyn Fn(f32, f32, f32, f32) -> D2D_RECT_F, cl: f32, cw: f32, rows: usize, scroll: usize) -> Vec<(Button, D2D_RECT_F)> {
    let mut out = vec![(Button::ListInput, r(cl, NAME_Y, EDIT_LIST_W, FIELD_H))];
    for (row, i) in (scroll..rows.min(scroll + TASK_ROWS)).enumerate() {
        out.push((Button::TaskPick(i), r(cl, EDIT_ROW_Y + row as f32 * EDIT_ROW_H, EDIT_LIST_W, EDIT_ROW_H)));
    }
    let px = cl + PANE_X;
    let pw = cw - PANE_X;
    out.push((Button::TaskBack, r(cl + cw - 44.0, NAME_Y, 44.0, FIELD_H)));
    if rows == 0 {
        return out;
    }
    out.push((Button::TaskName, r(px, NAME_Y, pw - 50.0, FIELD_H)));
    let mut x = px + PANE_LABEL_W;
    for (rep, w) in REPEATS {
        out.push((Button::TaskRepeat(rep), r(x, REPEAT_Y, w, STEP)));
        x += w + CHIP_GAP;
    }
    let x = px + PANE_LABEL_W;
    out.push((Button::TaskDue(-1), r(x, DUE_Y, STEP, STEP)));
    out.push((Button::TaskDue(1), r(x + STEP + DATE_W, DUE_Y, STEP, STEP)));
    let hx = x + 2.0 * STEP + DATE_W + 6.0;
    out.push((Button::TaskHour(-1), r(hx, DUE_Y, STEP, STEP)));
    out.push((Button::TaskHour(1), r(hx + STEP + TIME_W, DUE_Y, STEP, STEP)));
    let mut x = px + PANE_LABEL_W;
    for (slot, w) in SLOTS {
        out.push((Button::TaskSlot(slot), r(x, SLOT_Y, w, STEP)));
        x += w + CHIP_GAP;
    }
    let x = px + PANE_LABEL_W;
    out.push((Button::TaskSnooze(-1), r(x, SNOOZE_Y, STEP, STEP)));
    out.push((Button::TaskSnooze(1), r(x + STEP + SNOOZE_W, SNOOZE_Y, STEP, STEP)));
    out.push((Button::TaskDelete, r(cl + cw - 36.0, SNOOZE_Y, 36.0, STEP)));
    out
}

pub fn picked(sel: Option<usize>, n: usize) -> Option<usize> {
    sel.filter(|&i| i < n).or((n > 0).then_some(0))
}

pub(super) fn status_color(st: Status) -> (f32, f32, f32) {
    match st {
        Status::Late(_) => RED,
        Status::Today | Status::Waiting => YELLOW,
        Status::Done => GREEN,
    }
}

pub(super) fn due_label(tasks: &Tasks, i: usize, now: u64) -> String {
    let due = tasks.items[i].due;
    match tasks.status(i, now) {
        Status::Done => tr("đã làm", "done").to_string(),
        Status::Late(d) => if crate::i18n::english() { format!("{d} day{} late", if d == 1 { "" } else { "s" }) } else { format!("trễ {d} ngày") },
        Status::Today => format!("{:02}:{:02}", due / 100 % 100, due % 100),
        Status::Waiting => {
            let date = task::date_of(due);
            let (y, m, d) = ((date / 10_000) as i32, date / 100 % 100, date % 100);
            format!("{} {d}/{m}", crate::calendar::weekday_short(crate::calendar::weekday(y, m, d), crate::i18n::english()))
        }
    }
}

impl Renderer {
    pub(super) fn draw_task(
        &self,
        text: &dyn Fn(&str, usize, D2D_RECT_F, DWRITE_TEXT_ALIGNMENT, D2D1_COLOR_F) -> Result<()>,
        width: &dyn Fn(&str, usize) -> f32,
        hover_disc: &dyn Fn(Button) -> Result<()>,
        scrollbar: &dyn Fn(usize, usize, f32, f32) -> Result<()>,
        btns: &[(Button, D2D_RECT_F)],
        fonts: &[IDWriteTextFormat],
        draw_input: &dyn Fn(&str) -> Result<()>,
        tv: &TaskView,
        scroll: usize,
        settings: Option<Card>,
        (cl, cr): (f32, f32),
        s: f32,
        a: f32,
        hovered: Option<Button>,
    ) -> Result<()> {
        let Some(tasks) = tv.tasks else { return Ok(()) };
        if settings == Some(Card::TaskEdit) {
            return self.draw_task_edit(text, draw_input, btns, tasks, tv, scroll, (cl, cr), s, a, hovered);
        }
        let rt = &self.rt;
        let (center, lead, trail) = (DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_ALIGNMENT_TRAILING);
        let solid = |c: (f32, f32, f32), a: f32| color(c.0, c.1, c.2, a);
        let rect_of = |b: Button| btns.iter().find(|(x, _)| *x == b).map(|(_, r)| *r);
        let english = crate::i18n::english();
        let now = tv.now;
        let today = task::date_of(now);
        unsafe {
            text(tr("TUẦN NÀY", "THIS WEEK"), F_LABEL, rect(cl, HEAD_Y * s, cl + WEEK_W * s, (HEAD_Y + 16.0) * s), lead, grey(0.45, a))?;
            let monday = task::week_start(today);
            let dots = tasks.week(monday, now);
            let col = WEEK_W / 7.0;
            for k in 0..7u32 {
                let x = cl + k as f32 * col * s;
                text(crate::calendar::weekday_short(k, english), F_LABEL, rect(x, 52.0 * s, x + col * s, 67.0 * s), center, grey(0.55, a))?;
                let date = task::days_after(monday, k);
                let mx = (x + col * s / 2.0).round();
                let my = (DAYS_Y * s).round();
                let is_today = date == today;
                if is_today {
                    let half = (CAL_CHIP_H * s / 2.0).round();
                    rt.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: mx, Y: my }, radiusX: half, radiusY: half }, &self.brush(solid(BLUE, a))?);
                }
                let ink = if is_today { grey(1.0, a) } else if date < today { grey(0.4, a) } else { grey(0.92, a) };
                self.day_number(&(date % 100).to_string(), &fonts[if is_today { F_CAL_BOLD } else { F_CAL }], mx, my, ink)?;
                if let Some(st) = dots[k as usize] {
                    let r = DOT / 2.0 * s;
                    rt.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: mx, Y: my + 15.0 * s }, radiusX: r, radiusY: r }, &self.brush(solid(status_color(st), a))?);
                }
            }
            let due_today = (0..tasks.items.len()).filter(|&i| tasks.status(i, now) == Status::Today).count();
            let summary = match (due_today, english) {
                (0, false) => "Hôm nay trống".to_string(),
                (0, true) => "Nothing today".to_string(),
                (n, false) => format!("{n} task hôm nay"),
                (n, true) => format!("{n} task{} today", if n == 1 { "" } else { "s" }),
            };
            let sy = 112.0;
            let dot_c = if due_today > 0 { YELLOW } else { GREEN };
            rt.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: cl + 4.0 * s, Y: (sy + 9.0) * s }, radiusX: 4.0 * s, radiusY: 4.0 * s }, &self.brush(solid(dot_c, a))?);
            text(&summary, F_TITLE, rect(cl + 14.0 * s, sy * s, cl + WEEK_W * s, (sy + 18.0) * s), lead, grey(1.0, a))?;
            let next = tasks.order(now).into_iter().find(|&i| matches!(tasks.status(i, now), Status::Today | Status::Waiting));
            let next_line = match next {
                Some(i) => format!("{} {}, {}", tr("Kế tiếp:", "Next:"), tasks.items[i].title, due_label(tasks, i, now)),
                None => tr("Không có task sắp tới", "Nothing coming up").to_string(),
            };
            text(&next_line, F_LABEL, rect(cl + 14.0 * s, (sy + 20.0) * s, cl + WEEK_W * s, (sy + 36.0) * s), lead, grey(0.55, a))?;

            let dx = cl + DIVIDER_X * s;
            rt.FillRectangle(&rect(dx, HEAD_Y * s, dx + 1.0 * s, 158.0 * s), &self.brush(grey(0.1, a))?);

            let x0 = cl + LIST_X * s;
            let n = tasks.items.len();
            text(&format!("{} · {n}", tr("Đã lên lịch", "Scheduled")), F_LABEL, rect(x0, HEAD_Y * s, cr - 70.0 * s, (HEAD_Y + 20.0) * s), lead, grey(0.6, a))?;
            if let Some(sb) = rect_of(Button::TaskSwitch) {
                let rad = (sb.bottom - sb.top) / 2.0;
                let on = tasks.enabled;
                rt.FillRoundedRectangle(&rounded(sb, rad), &self.brush(if on { solid(GREEN, a) } else { grey(0.3, a) })?);
                let kx = if on { sb.right - rad } else { sb.left + rad };
                rt.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: kx, Y: (sb.top + sb.bottom) / 2.0 }, radiusX: rad - 2.0 * s, radiusY: rad - 2.0 * s }, &self.brush(grey(1.0, a))?);
            }
            if let Some(eb) = rect_of(Button::TaskEdit) {
                hover_disc(Button::TaskEdit)?;
                text(ICON_EDIT, F_SMALL_ICON, eb, center, grey(if hovered == Some(Button::TaskEdit) { 1.0 } else { 0.6 }, a))?;
            }
            if n == 0 {
                text(tr("Chưa có task nào", "No tasks yet"), F_BODY, rect(x0, (TASK_ROW_Y + 22.0) * s, cr, (TASK_ROW_Y + 40.0) * s), center, grey(0.7, a))?;
                text(tr("Bấm nút sửa để thêm", "Tap edit to add one"), F_LABEL, rect(x0, (TASK_ROW_Y + 40.0) * s, cr, (TASK_ROW_Y + 56.0) * s), center, grey(0.5, a))?;
                return Ok(());
            }
            let order = tasks.order(now);
            for (row, &i) in order.iter().skip(scroll).take(TASK_ROWS).enumerate() {
                let top = (TASK_ROW_Y + row as f32 * TASK_ROW_H) * s;
                let bottom = top + TASK_ROW_H * s;
                let st = tasks.status(i, now);
                let done = st == Status::Done;
                if hovered == Some(Button::TaskRow(i)) {
                    rt.FillRoundedRectangle(&rounded(rect(x0 - 4.0 * s, top + 1.0 * s, cr + 2.0 * s, bottom - 1.0 * s), 6.0 * s), &self.brush(grey(0.1, a))?);
                }
                let my = (top + bottom) / 2.0;
                rt.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: x0 + 3.0 * s, Y: my }, radiusX: 3.0 * s, radiusY: 3.0 * s }, &self.brush(solid(status_color(st), if done { a * 0.6 } else { a }))?);
                let label = due_label(tasks, i, now);
                let label_c = match st {
                    Status::Late(_) => solid(RED, a),
                    Status::Today => solid(YELLOW, a),
                    Status::Waiting => grey(0.55, a),
                    Status::Done => grey(0.4, a),
                };
                let lw = width(&label, F_LABEL) + 4.0 * s;
                text(&label, F_LABEL, rect(cr - lw, top, cr, bottom), trail, label_c)?;
                let tb = rect(x0 + 12.0 * s, top, cr - lw - 6.0 * s, bottom);
                text(&tasks.items[i].title, F_BODY, tb, lead, grey(if done { 0.4 } else { 1.0 }, a))?;
                if done {
                    let tw = width(&tasks.items[i].title, F_BODY).min(tb.right - tb.left);
                    rt.FillRectangle(&rect(tb.left, my, tb.left + tw, my + 1.0 * s), &self.brush(grey(0.4, a))?);
                }
                if row + 1 < TASK_ROWS.min(order.len() - scroll) {
                    rt.FillRectangle(&rect(x0, bottom - 0.5 * s, cr, bottom + 0.5 * s), &self.brush(grey(0.08, a))?);
                }
            }
            scrollbar(n, TASK_ROWS, TASK_ROW_Y, TASK_ROW_H)?;
        }
        Ok(())
    }

    fn draw_task_edit(
        &self,
        text: &dyn Fn(&str, usize, D2D_RECT_F, DWRITE_TEXT_ALIGNMENT, D2D1_COLOR_F) -> Result<()>,
        draw_input: &dyn Fn(&str) -> Result<()>,
        btns: &[(Button, D2D_RECT_F)],
        tasks: &Tasks,
        tv: &TaskView,
        scroll: usize,
        (cl, cr): (f32, f32),
        s: f32,
        a: f32,
        hovered: Option<Button>,
    ) -> Result<()> {
        let rt = &self.rt;
        let (center, lead) = (DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING);
        let solid = |c: (f32, f32, f32), a: f32| color(c.0, c.1, c.2, a);
        let rect_of = |b: Button| btns.iter().find(|(x, _)| *x == b).map(|(_, r)| *r);
        let n = tasks.items.len();
        let sel = picked(tv.sel, n);
        unsafe {
            draw_input(tr("Thêm task…", "Add a task…"))?;
            for (row, i) in (scroll..n.min(scroll + TASK_ROWS)).enumerate() {
                let top = (EDIT_ROW_Y + row as f32 * EDIT_ROW_H) * s;
                let row_r = rect(cl, top, cl + EDIT_LIST_W * s, top + EDIT_ROW_H * s);
                if sel == Some(i) || hovered == Some(Button::TaskPick(i)) {
                    let fill = if sel == Some(i) { 0.14 } else { 0.08 };
                    rt.FillRoundedRectangle(&rounded(rect(row_r.left, row_r.top + 1.0 * s, row_r.right, row_r.bottom - 1.0 * s), 6.0 * s), &self.brush(grey(fill, a))?);
                }
                let st = tasks.status(i, tv.now);
                rt.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: cl + 9.0 * s, Y: (row_r.top + row_r.bottom) / 2.0 }, radiusX: 3.0 * s, radiusY: 3.0 * s }, &self.brush(solid(status_color(st), a))?);
                text(&tasks.items[i].title, F_BODY, rect(cl + 18.0 * s, row_r.top, row_r.right - 6.0 * s, row_r.bottom), lead, grey(if sel == Some(i) { 1.0 } else { 0.75 }, a))?;
            }
            if n > TASK_ROWS {
                let (top, bottom) = (EDIT_ROW_Y * s, (EDIT_ROW_Y + EDIT_ROW_H * TASK_ROWS as f32) * s);
                let x = cl + (EDIT_LIST_W + 2.0) * s;
                let len = (bottom - top) * TASK_ROWS as f32 / n as f32;
                let start = top + (bottom - top - len) * scroll as f32 / (n - TASK_ROWS) as f32;
                rt.FillRoundedRectangle(&rounded(rect(x, start, x + 3.0 * s, start + len), 1.5 * s), &self.brush(grey(0.5, a))?);
            }
            let dx = cl + EDIT_DIVIDER_X * s;
            rt.FillRectangle(&rect(dx, NAME_Y * s, dx + 1.0 * s, 158.0 * s), &self.brush(grey(0.1, a))?);

            if let Some(bb) = rect_of(Button::TaskBack) {
                let hot = hovered == Some(Button::TaskBack);
                rt.FillRoundedRectangle(&rounded(bb, (bb.bottom - bb.top) / 2.0), &self.brush(if hot { solid(GREEN, a) } else { color(GREEN.0, GREEN.1, GREEN.2, a * 0.85) })?);
                text(tr("Xong", "Done"), F_LABEL, rect(bb.left, bb.top - 1.0 * s, bb.right, bb.bottom - 1.0 * s), center, color(0.02, 0.13, 0.04, a))?;
            }
            let px = cl + PANE_X * s;
            let Some(i) = sel else {
                text(tr("Gõ tên task vào ô bên trái", "Type a task on the left"), F_BODY, rect(px, 70.0 * s, cr, 90.0 * s), center, grey(0.7, a))?;
                text(tr("rồi nhấn Enter", "and press Enter"), F_LABEL, rect(px, 90.0 * s, cr, 106.0 * s), center, grey(0.5, a))?;
                return Ok(());
            };
            let t = &tasks.items[i];
            let chip = |b: Button, label: &str, on: bool| -> Result<()> {
                let Some(cb) = rect_of(b) else { return Ok(()) };
                let rad = (cb.bottom - cb.top) / 2.0;
                if on {
                    rt.FillRoundedRectangle(&rounded(cb, rad), &self.brush(solid(GREEN, a))?);
                } else {
                    let inner = rect(cb.left + 0.5, cb.top + 0.5, cb.right - 0.5, cb.bottom - 0.5);
                    if hovered == Some(b) {
                        rt.FillRoundedRectangle(&rounded(cb, rad), &self.brush(grey(0.14, a))?);
                    }
                    rt.DrawRoundedRectangle(&rounded(inner, rad - 0.5), &self.brush(grey(0.28, a))?, 1.0 * s, None);
                }
                let ink = if on { color(0.02, 0.13, 0.04, a) } else { grey(0.85, a) };
                text(label, F_LABEL, rect(cb.left, cb.top - 1.0 * s, cb.right, cb.bottom - 1.0 * s), center, ink)
            };
            let stepper = |b: Button, glyph: &str| -> Result<()> {
                let Some(sb) = rect_of(b) else { return Ok(()) };
                let c = Vector2 { X: (sb.left + sb.right) / 2.0, Y: (sb.top + sb.bottom) / 2.0 };
                rt.FillEllipse(&D2D1_ELLIPSE { point: c, radiusX: 9.0 * s, radiusY: 9.0 * s }, &self.brush(grey(if hovered == Some(b) { 0.3 } else { 0.18 }, a))?);
                text(glyph, F_TINY_ICON, sb, center, grey(1.0, a))
            };
            let label = |y: f32, l: &str| text(l, F_LABEL, rect(px, (y - 1.0) * s, px + PANE_LABEL_W * s, (y + STEP - 1.0) * s), lead, grey(0.6, a));

            if let Some(nb) = rect_of(Button::TaskName) {
                let hot = hovered == Some(Button::TaskName);
                rt.FillRoundedRectangle(&rounded(nb, 6.0 * s), &self.brush(grey(if hot { 0.14 } else { 0.1 }, a))?);
                text(&t.title, F_TITLE, rect(nb.left + 8.0 * s, nb.top - 1.0 * s, nb.right - 6.0 * s, nb.bottom - 1.0 * s), lead, grey(1.0, a))?;
            }
            label(REPEAT_Y, tr("Lặp", "Repeat"))?;
            for (rep, _) in REPEATS {
                chip(Button::TaskRepeat(rep), repeat_label(rep), t.repeat == rep)?;
            }
            label(DUE_Y, tr("Hạn", "Due"))?;
            stepper(Button::TaskDue(-1), ICON_MINUS)?;
            stepper(Button::TaskDue(1), ICON_PLUS)?;
            stepper(Button::TaskHour(-1), ICON_MINUS)?;
            stepper(Button::TaskHour(1), ICON_PLUS)?;
            let date = task::date_of(t.due);
            let (y, m, d) = ((date / 10_000) as i32, date / 100 % 100, date % 100);
            let day = format!("{} {d}/{m}", crate::calendar::weekday_short(crate::calendar::weekday(y, m, d), crate::i18n::english()));
            let x = px + (PANE_LABEL_W + STEP) * s;
            text(&day, F_LABEL, rect(x, (DUE_Y - 1.0) * s, x + DATE_W * s, (DUE_Y + STEP - 1.0) * s), center, grey(1.0, a))?;
            let hx = px + (PANE_LABEL_W + 3.0 * STEP + DATE_W + 6.0) * s;
            text(&format!("{:02}:{:02}", t.due / 100 % 100, t.due % 100), F_LABEL, rect(hx, (DUE_Y - 1.0) * s, hx + TIME_W * s, (DUE_Y + STEP - 1.0) * s), center, grey(1.0, a))?;
            label(SLOT_Y, tr("Nhắc", "Remind"))?;
            for (slot, _) in SLOTS {
                chip(Button::TaskSlot(slot), slot_label(slot), t.slots & slot != 0)?;
            }
            label(SNOOZE_Y, tr("Chưa làm", "Not yet"))?;
            stepper(Button::TaskSnooze(-1), ICON_MINUS)?;
            stepper(Button::TaskSnooze(1), ICON_PLUS)?;
            let x = px + (PANE_LABEL_W + STEP) * s;
            text(&format!("{} {}", t.snooze, tr("phút", "min")), F_LABEL, rect(x, (SNOOZE_Y - 1.0) * s, x + SNOOZE_W * s, (SNOOZE_Y + STEP - 1.0) * s), center, grey(1.0, a))?;
            if let Some(db) = rect_of(Button::TaskDelete) {
                let hot = hovered == Some(Button::TaskDelete);
                text(tr("Xoá", "Delete"), F_LABEL, rect(db.left, db.top - 1.0 * s, db.right, db.bottom - 1.0 * s), DWRITE_TEXT_ALIGNMENT_TRAILING, solid(RED, if hot { a } else { a * 0.8 }))?;
            }
        }
        Ok(())
    }
}

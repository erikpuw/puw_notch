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
        tv: &TaskView,
        scroll: usize,
        (cl, cr): (f32, f32),
        s: f32,
        a: f32,
        hovered: Option<Button>,
    ) -> Result<()> {
        let Some(tasks) = tv.tasks else { return Ok(()) };
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
}

use super::*;
use crate::task::Status;
use crate::task_alert::{card_height, AlertRow, Choice, CARD_W, LIST_ROW_H, LIST_TOP, LIST_W, MAX_ROWS};

const PAD: f32 = 24.0;
const BTN_H: f32 = 26.0;
const BTN_GAP: f32 = 4.0;
const DONE_W: f32 = 84.0;
const NOT_YET_W: f32 = 150.0;
const ROW_BTN_H: f32 = 22.0;
const ROW_DONE_W: f32 = 64.0;
const ROW_NOT_YET_W: f32 = 76.0;

pub struct AlertView<'a> {
    pub rows: &'a [AlertRow],
    pub appear: f32,
    pub hovered: Option<Choice>,
    pub locked: bool,
}

fn card_size(n: usize) -> (f32, f32) {
    (if n <= 1 { CARD_W } else { LIST_W }, card_height(n))
}

fn card_rect(w: f32, h: f32, s: f32, n: usize) -> D2D_RECT_F {
    let (cw, ch) = card_size(n);
    let (x, y) = ((w - cw * s) / 2.0, (h - ch * s) / 2.0);
    rect(x.round(), y.round(), (x + cw * s).round(), (y + ch * s).round())
}

pub fn alert_buttons(w: f32, h: f32, s: f32, tasks: &[usize]) -> Vec<(Choice, D2D_RECT_F)> {
    let c = card_rect(w, h, s, tasks.len());
    if let [one] = tasks {
        let (right, top) = (c.right - PAD * s, c.bottom - (20.0 + BTN_H) * s);
        let later = rect(right - NOT_YET_W * s, top, right, top + BTN_H * s);
        let done = rect(later.left - (BTN_GAP + DONE_W) * s, top, later.left - BTN_GAP * s, top + BTN_H * s);
        return vec![(Choice::Done(*one), done), (Choice::NotYet(*one), later)];
    }
    let mut out = Vec::new();
    for (row, &i) in tasks.iter().take(MAX_ROWS).enumerate() {
        let top = c.top + (LIST_TOP + row as f32 * LIST_ROW_H + (LIST_ROW_H - ROW_BTN_H) / 2.0) * s;
        let right = c.right - PAD * s;
        let later = rect(right - ROW_NOT_YET_W * s, top, right, top + ROW_BTN_H * s);
        let done = rect(later.left - (BTN_GAP + ROW_DONE_W) * s, top, later.left - BTN_GAP * s, top + ROW_BTN_H * s);
        out.push((Choice::Done(i), done));
        out.push((Choice::NotYet(i), later));
    }
    out
}

fn status_rgb(st: Status) -> (f32, f32, f32) {
    match st {
        Status::Late(_) => task_page::RED,
        Status::Today => ORANGE,
        _ => YELLOW,
    }
}

impl Renderer {
    pub fn present_alert(&mut self, hwnd: HWND, w: i32, h: i32, s: f32, v: &AlertView) -> Result<()> {
        self.ensure_surface(hwnd, w, h)?;
        let fonts = self.fonts(s)?;
        let rt = self.rt.clone();
        let (wf, hf) = (w as f32, h as f32);
        let n = v.rows.len();
        let tasks: Vec<usize> = v.rows.iter().map(|r| r.task).collect();
        let btns = alert_buttons(wf, hf, s, &tasks);
        let english = crate::i18n::english();
        let ease = 1.0 - (1.0 - v.appear.clamp(0.0, 1.0)).powi(3);
        let a = ease;
        unsafe {
            let text = |t: &str, f: usize, b: D2D_RECT_F, align: DWRITE_TEXT_ALIGNMENT, c: D2D1_COLOR_F| -> Result<()> {
                let t: Vec<u16> = t.encode_utf16().collect();
                fonts[f].SetTextAlignment(align)?;
                rt.DrawText(&t, &fonts[f], &b, &self.brush(c)?, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
                Ok(())
            };
            let width = |t: &str, f: usize| -> f32 {
                let u: Vec<u16> = t.encode_utf16().collect();
                let mut m = DWRITE_TEXT_METRICS::default();
                match self.dw.CreateTextLayout(&u, &fonts[f], 10_000.0, 100.0) {
                    Ok(layout) if layout.GetMetrics(&mut m).is_ok() => m.widthIncludingTrailingWhitespace,
                    _ => 0.0,
                }
            };
            let (center, lead) = (DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING);
            let solid = |c: (f32, f32, f32), a: f32| color(c.0, c.1, c.2, a);
            rt.BeginDraw();
            rt.Clear(Some(&color(0.0, 0.0, 0.0, 0.0)));
            let k = 0.96 + 0.04 * ease;
            let (mx, my) = (wf / 2.0, hf / 2.0);
            rt.SetTransform(&Matrix3x2 { M11: k, M12: 0.0, M21: 0.0, M22: k, M31: mx * (1.0 - k), M32: my * (1.0 - k) });
            let c = card_rect(wf, hf, s, n);
            rt.FillRoundedRectangle(&rounded(c, 28.0 * s), &self.brush(color(0.0, 0.0, 0.0, a))?);
            let edge = rect(c.left + 0.5, c.top + 0.5, c.right - 0.5, c.bottom - 0.5);
            rt.DrawRoundedRectangle(&rounded(edge, 28.0 * s), &self.brush(grey(1.0, a * 0.08))?, 1.0 * s, None);

            let pill = |choice: Choice, label: &str, filled: bool| -> Result<()> {
                let Some(b) = btns.iter().find(|(x, _)| *x == choice).map(|(_, r)| *r) else { return Ok(()) };
                let ba = if v.locked { a * 0.45 } else { a };
                let hot = !v.locked && v.hovered == Some(choice);
                let rad = (b.bottom - b.top) / 2.0;
                if filled {
                    let g = if hot { (0.32, 0.92, 0.42) } else { GREEN };
                    rt.FillRoundedRectangle(&rounded(b, rad), &self.brush(solid(g, ba))?);
                    text(label, F_LABEL, rect(b.left, b.top - 1.0 * s, b.right, b.bottom - 1.0 * s), center, color(0.02, 0.13, 0.04, ba))?;
                } else {
                    if hot {
                        rt.FillRoundedRectangle(&rounded(b, rad), &self.brush(grey(0.14, ba))?);
                    }
                    let inner = rect(b.left + 0.5, b.top + 0.5, b.right - 0.5, b.bottom - 0.5);
                    rt.DrawRoundedRectangle(&rounded(inner, rad - 0.5), &self.brush(grey(0.28, ba))?, 1.0 * s, None);
                    text(label, F_LABEL, rect(b.left, b.top - 1.0 * s, b.right, b.bottom - 1.0 * s), center, grey(1.0, ba))?;
                }
                Ok(())
            };
            let bell = |cx: f32, cy: f32, r: f32, rgb: (f32, f32, f32)| -> Result<()> {
                rt.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cy }, radiusX: r, radiusY: r }, &self.brush(solid(rgb, a * 0.2))?);
                text(ICON_BELL, F_ICON, rect(cx - r, cy - r, cx + r, cy + r), center, solid(rgb, a))
            };

            if let [row] = v.rows {
                let rgb = status_rgb(row.status);
                let x = c.left + PAD * s;
                bell(x + 18.0 * s, c.top + 40.0 * s, 18.0 * s, rgb)?;
                text(&format!("Task · {}", row.kind), F_LABEL, rect(x + 48.0 * s, c.top + 30.0 * s, c.right - PAD * s, c.top + 50.0 * s), lead, grey(0.6, a))?;
                text(&row.title, F_HEAD, rect(x, c.top + 70.0 * s, c.right - PAD * s, c.top + 96.0 * s), lead, grey(1.0, a))?;
                text(&row.due, F_BODY, rect(x, c.top + 96.0 * s, c.right - PAD * s, c.top + 116.0 * s), lead, grey(0.6, a))?;
                text(&row.big, F_FIGURE, rect(x, c.top + 118.0 * s, c.right - PAD * s, c.top + 158.0 * s), lead, solid(rgb, a))?;
                let told = if english { format!("Reminded {} time{} · next {}", row.told, if row.told == 1 { "" } else { "s" }, row.next) } else { format!("Đã nhắc {} lần · lần sau {}", row.told, row.next) };
                text(&told, F_LABEL, rect(x, c.top + 158.0 * s, c.right - PAD * s, c.top + 176.0 * s), lead, grey(0.45, a))?;
                let later = if english { format!("Remind in {} min", row.snooze) } else { format!("Nhắc lại sau {} phút", row.snooze) };
                pill(Choice::NotYet(row.task), &later, false)?;
                pill(Choice::Done(row.task), tr("Đã làm", "Done"), true)?;
            } else {
                let x = c.left + PAD * s;
                let worst = if v.rows.iter().any(|r| matches!(r.status, Status::Late(_))) { task_page::RED } else { ORANGE };
                bell(x + 14.0 * s, c.top + 34.0 * s, 14.0 * s, worst)?;
                let head = if english { format!("{n} tasks to do") } else { format!("{n} task cần làm") };
                text(&head, F_HEAD, rect(x + 38.0 * s, c.top + 20.0 * s, c.right - PAD * s, c.top + 48.0 * s), lead, grey(1.0, a))?;
                for (k, row) in v.rows.iter().take(MAX_ROWS).enumerate() {
                    let top = c.top + (LIST_TOP + k as f32 * LIST_ROW_H) * s;
                    if k > 0 {
                        rt.FillRectangle(&rect(x, top, c.right - PAD * s, top + 1.0 * s), &self.brush(grey(0.1, a))?);
                    }
                    let rgb = status_rgb(row.status);
                    rt.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: x + 4.0 * s, Y: top + 15.0 * s }, radiusX: 4.0 * s, radiusY: 4.0 * s }, &self.brush(solid(rgb, a))?);
                    let tr_right = c.right - (PAD + ROW_DONE_W + ROW_NOT_YET_W + BTN_GAP + 10.0) * s;
                    text(&row.title, F_TITLE, rect(x + 16.0 * s, top + 4.0 * s, tr_right, top + 24.0 * s), lead, grey(1.0, a))?;
                    let big_w = width(&row.big, F_LABEL);
                    text(&row.big, F_LABEL, rect(x + 16.0 * s, top + 22.0 * s, tr_right, top + 40.0 * s), lead, solid(rgb, a))?;
                    text(&format!(" · {}", row.when), F_LABEL, rect(x + 16.0 * s + big_w, top + 22.0 * s, tr_right, top + 40.0 * s), lead, grey(0.55, a))?;
                    pill(Choice::NotYet(row.task), tr("Chưa làm", "Not yet"), false)?;
                    pill(Choice::Done(row.task), tr("Đã làm", "Done"), true)?;
                }
                let mut y = c.top + (LIST_TOP + n.min(MAX_ROWS) as f32 * LIST_ROW_H) * s;
                if n > MAX_ROWS {
                    let more = if english { format!("and {} more", n - MAX_ROWS) } else { format!("và {} task nữa", n - MAX_ROWS) };
                    text(&more, F_LABEL, rect(x + 16.0 * s, y, c.right - PAD * s, y + 20.0 * s), lead, grey(0.55, a))?;
                    y += 20.0 * s;
                }
                text(tr("Esc: nhắc lại tất cả sau", "Esc: remind me later for all"), F_LABEL, rect(x, y + 4.0 * s, c.right - PAD * s, y + 24.0 * s), lead, grey(0.4, a))?;
            }
            rt.SetTransform(&Matrix3x2::identity());
            rt.EndDraw(None, None)?;
            #[cfg(test)]
            if self.grab {
                self.grab_frame()?;
            }
            self.surface.as_ref().unwrap().swap.Present(1, DXGI_PRESENT(0)).ok()?;
        }
        Ok(())
    }
}

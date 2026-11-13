use super::*;

impl Renderer {
    pub(super) fn draw_pages(
        &self,
        c: &Ctx,
        p: &Prep,
        a: f32,
    ) -> Result<()> {
        let rt = &self.rt;
        let Prep { weather_line, weather_empty, hour_lines, focus_icons, figure_icons, player_icons, note_icons, lyric_heights, lyric_tops, moon_line, tint, level, fonts } = *p;
        let Ctx { v, s, l, r, cx, gy, text, width, micon, wx_icon, cover, .. } = *c;
        let center = DWRITE_TEXT_ALIGNMENT_CENTER;
        let lead = DWRITE_TEXT_ALIGNMENT_LEADING;
        let trail = DWRITE_TEXT_ALIGNMENT_TRAILING;
        unsafe {
            let (cl, cr) = (l + SIDE * s, r - SIDE * s);
            let lv = &v.list;
            let rows_of = |page: Page| match page {
                Page::Todo => v.todos.len(),
                Page::Task => v.task.tasks.map_or(0, |t| t.items.len()),
                Page::Notify => v.notes.len(),
                Page::Weather => usize::from(v.weather.now.is_some()),
                _ => 0,
            };
            let minutes = todo::minutes;
            let solid = |c: (f32, f32, f32), a: f32| color(c.0, c.1, c.2, a);

            let draw_page = |page: Page, dx: f32, pa: f32| -> Result<()> {
                let a = a * pa;
                if a <= 0.01 {
                    return Ok(());
                }
                let current = page == v.page;
                let scroll = if current { lv.scroll } else { 0 };
                let settings = if current { v.health.settings } else { None };
                let hovered = if current { v.hovered } else { None };
                let btns = page_buttons(cx, s, page, rows_of(page), scroll, settings, v.lyrics);
                let rect_of = |b: Button| btns.iter().find(|(x, _)| *x == b).map(|(_, r)| *r);
                let hover_disc = |b: Button| -> Result<()> {
                    if let (true, Some(r)) = (hovered == Some(b), rect_of(b)) {
                        let point = Vector2 { X: (r.left + r.right) / 2.0, Y: (r.top + r.bottom) / 2.0 };
                        rt.FillEllipse(&D2D1_ELLIPSE { point, radiusX: 11.0 * s, radiusY: 11.0 * s }, &self.brush(grey(0.18, a))?);
                    }
                    Ok(())
                };
                rt.SetTransform(&Matrix3x2 { M11: 1.0, M12: 0.0, M21: 0.0, M22: 1.0, M31: dx, M32: gy });

                let draw_input = |placeholder: &str| -> Result<()> {
                    let Some(ib) = rect_of(Button::ListInput) else { return Ok(()) };
                    let editing = current && lv.editing;
                    let edit_row = if current { lv.edit_item } else { None };
                    let focus = editing || hovered == Some(Button::ListInput);
                    rt.FillRoundedRectangle(&rounded(ib, 13.0 * s), &self.brush(grey(if focus { 0.16 } else { 0.12 }, a))?);
                    if editing || edit_row.is_some() {
                        let inner = rect(ib.left + 0.5, ib.top + 0.5, ib.right - 0.5, ib.bottom - 0.5);
                        let c = if edit_row.is_some() { solid(YELLOW, a * 0.8) } else { grey(1.0, a * 0.35) };
                        rt.DrawRoundedRectangle(&rounded(inner, 13.0 * s), &self.brush(c)?, 1.0 * s, None);
                    }
                    let tag_w = if edit_row.is_some() { 58.0 * s } else { 0.0 };
                    if edit_row.is_some() {
                        text(tr("Đang sửa", "Editing"), F_LABEL, rect(ib.right - tag_w - 8.0 * s, ib.top, ib.right - 10.0 * s, ib.bottom), trail, solid(YELLOW, a))?;
                    }
                    let icon_box = rect(ib.left + 8.0 * s, ib.top, ib.left + 26.0 * s, ib.bottom);
                    text(ICON_ADD, F_SMALL_ICON, icon_box, center, grey(0.6, a))?;
                    let draft = if current { lv.draft } else { "" };
                    let clear = !draft.is_empty() && edit_row.is_none();
                    if clear {
                        if let Some(cb) = rect_of(Button::ListClear) {
                            hover_disc(Button::ListClear)?;
                            text(ICON_CLOSE, F_SMALL_ICON, cb, center, if hovered == Some(Button::ListClear) { grey(1.0, a) } else { grey(0.55, a) })?;
                        }
                    }
                    let tb = rect(icon_box.right + 4.0 * s, ib.top, ib.right - if clear { 26.0 } else { 12.0 } * s - tag_w, ib.bottom);
                    if draft.is_empty() && !editing {
                        text(placeholder, F_BODY, tb, lead, grey(0.55, a))?;
                    } else {
                        let tw = width(draft, F_BODY);
                        let room = tb.right - tb.left - 4.0 * s;
                        text(draft, F_BODY, tb, if tw > room { trail } else { lead }, grey(1.0, a))?;
                        if editing && lv.caret {
                            let x = if tw > room { tb.right - 2.0 * s } else { tb.left + tw + 1.0 * s };
                            let my = (ib.top + ib.bottom) / 2.0;
                            rt.FillRectangle(&rect(x, my - 7.0 * s, x + 1.5 * s, my + 7.0 * s), &self.brush(grey(1.0, a))?);
                        }
                    }
                    Ok(())
                };
                let row_tools = |i: usize, lit: bool| -> Result<()> {
                    let rest = if lit { 0.65 } else { 0.4 };
                    let edit_hot = hovered == Some(Button::ListEdit(i)) || (current && lv.edit_item == Some(i));
                    if let Some(eb) = btns.iter().filter(|(b, _)| *b == Button::ListEdit(i)).map(|(_, r)| *r).last() {
                        hover_disc(Button::ListEdit(i))?;
                        text(ICON_EDIT, F_SMALL_ICON, eb, center, if edit_hot { solid(YELLOW, a) } else { grey(rest, a) })?;
                    }
                    if let Some(db) = rect_of(Button::ListDelete(i)) {
                        let c = if hovered == Some(Button::ListDelete(i)) { color(1.0, 0.45, 0.4, a) } else { grey(rest, a) };
                        hover_disc(Button::ListDelete(i))?;
                        text(ICON_CLOSE, F_SMALL_ICON, db, center, c)?;
                    }
                    Ok(())
                };
                let row_hovered = |i: usize| {
                    matches!(hovered, Some(Button::ListToggle(j)) | Some(Button::ListEdit(j)) | Some(Button::ListDelete(j)) if j == i)
                        || (current && lv.edit_item == Some(i))
                };
                let scrollbar = |n: usize, rows: usize, y0: f32, row_h: f32| -> Result<()> {
                    if n > rows {
                        let (top, bottom) = (y0 * s, (y0 + row_h * rows as f32) * s);
                        let x = cr + 6.0 * s;
                        rt.FillRoundedRectangle(&rounded(rect(x, top, x + 3.0 * s, bottom), 1.5 * s), &self.brush(grey(0.2, a))?);
                        let len = (bottom - top) * rows as f32 / n as f32;
                        let start = top + (bottom - top - len) * scroll as f32 / (n - rows) as f32;
                        rt.FillRoundedRectangle(&rounded(rect(x, start, x + 3.0 * s, start + len), 1.5 * s), &self.brush(grey(0.6, a))?);
                    }
                    Ok(())
                };
                let time_color = |due: u64, quiet: bool| {
                    let left = minutes(due) - minutes(lv.now);
                    if quiet { grey(0.4, a) } else if left <= 0 { color(1.0, 0.35, 0.3, a) }
                    else if left <= lv.remind_before.max(15) as i64 { color(1.0, 0.62, 0.2, a) } else { grey(0.6, a) }
                };

                match page {
                    Page::Music => self.draw_music(&text, &micon, &cover, &wx_icon, &player_icons, &fonts, &lyric_heights, &lyric_tops, v, l, tint, level, cl, s, a, hovered)?,
                    Page::Todo => self.draw_todo(&text, &width, &micon, &draw_input, &hover_disc, &row_tools, &row_hovered, &scrollbar, &time_color, &btns, v, lv, current, settings, (cl, cr), s, a, hovered)?,
                    Page::Notify => self.draw_notify(&text, &scrollbar, &btns, v, lv, current, &note_icons, (cl, cr), s, a, hovered)?,
                    Page::Usage => self.draw_usage(&text, &width, &btns, (cl, cr), s, a, hovered)?,
                    Page::Weather => self.draw_weather(&text, &width, &wx_icon, &draw_input, &hover_disc, &btns, v, lv, current, &weather_line, &weather_empty, &moon_line, &hour_lines, &figure_icons, (cl, cr), s, a, hovered)?,
                    Page::Calendar => self.draw_calendar(&text, &width, &hover_disc, &btns, &fonts, &v.cal, (cl, cr), s, a, hovered)?,
                    Page::Focus => self.draw_focus(&text, &micon, &wx_icon, &focus_icons, &btns, v.focus, v.health.minute_of_day, (cl, cr), s, a, hovered)?,
                    Page::Task => self.draw_task(&text, &width, &hover_disc, &scrollbar, &btns, &fonts, &draw_input, &v.task, scroll, settings, (cl, cr), s, a, hovered)?,
                    Page::Health => self.draw_health(&text, &micon, &hover_disc, &btns, &width, &v.health, settings, (cl, cr), s, a, hovered)?,
                }
                rt.SetTransform(&Matrix3x2::translation(0.0, gy));
                Ok(())
            };

            let ph = panel_size(v.page)[1];
            let clip_r = if v.size[0] > notch::target(Mode::Expanded, false)[0] + 1.0 { LYRIC_CLIP } else { PAGE_CLIP };
            rt.PushAxisAlignedClip(&rect(l + PAGE_CLIP * s, 30.0 * s, r - clip_r * s, (dots_y(ph) - 3.0) * s), D2D1_ANTIALIAS_MODE_ALIASED);
            match v.leaving {
                Some((from, t, dir)) => {
                    let span = (cr - cl) * 0.55;
                    let d = dir as f32;
                    let (out_a, in_a) = page_fades(t);
                    draw_page(from, -d * span * t.min(1.0), out_a)?;
                    draw_page(v.page, d * span * (1.0 - ease_out(t)), in_a)?;
                }
                None => draw_page(v.page, 0.0, 1.0)?,
            }
            rt.PopAxisAlignedClip();

        }
        Ok(())
    }
}

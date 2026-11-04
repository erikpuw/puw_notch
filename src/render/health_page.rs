//! The Health page: a card each for water and sitting, with a figure, a bar and a round button; the gear opens the card's settings.

use super::*;

impl Renderer {
    pub(super) fn draw_health(
        &self,
        text: &dyn Fn(&str, usize, D2D_RECT_F, DWRITE_TEXT_ALIGNMENT, D2D1_COLOR_F) -> Result<()>,
        micon: &dyn Fn(&'static str, &'static str, usize, D2D_RECT_F, D2D1_COLOR_F) -> Result<()>,
        hover_disc: &dyn Fn(Button) -> Result<()>,
        btns: &[(Button, D2D_RECT_F)],
        width: &dyn Fn(&str, usize) -> f32,
        hv: &HealthView,
        settings: Option<Card>,
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
            let h = hv.h;
            // The two cards share the content's width and end above the panel's bottom.
            let card_w = ((cr - cl) / s - CARD_GAP) / 2.0;
            let bottom = card_bottom(panel_size(Page::Health)[1]);
            for card in [Card::Water, Card::Sit] {
                let x = if card == Card::Water { cl } else { cl + (card_w + CARD_GAP) * s };
                let cb = rect(x, CARD_TOP * s, x + card_w * s, bottom * s);
                let accent = if card == Card::Water { BLUE } else { ORANGE };
                // A neutral tile with a hairline of light along its top edge.
                rt.FillRoundedRectangle(&rounded(cb, 14.0 * s), &self.brush(grey(0.067, a))?);
                rt.FillRectangle(&rect(cb.left + 12.0 * s, cb.top, cb.right - 12.0 * s, cb.top + 1.0 * s), &self.brush(grey(1.0, a * 0.08))?);
                // Title row: icon, name, gear.
                let ty = (CARD_TOP + 4.0) * s;
                let title = if card == Card::Water { tr("Uống nước", "Water") } else { tr("Đứng dậy", "Stand up") };
                text(title, F_TITLE, rect(x + 14.0 * s, ty, x + card_w * s - 34.0 * s, ty + 22.0 * s), lead, grey(1.0, a))?;
                let open = settings == Some(card);
                if let Some(gb) = rect_of(Button::Gear(card)) {
                    let hot = hovered == Some(Button::Gear(card));
                    let (icon, c) = if open { (ICON_CHECK, solid(GREEN, a)) } else { (ICON_GEAR, grey(if hot { 1.0 } else { 0.6 }, a)) };
                    hover_disc(Button::Gear(card))?;
                    micon(if card == Card::Water { "gear_water" } else { "gear_sit" }, icon, F_SMALL_ICON, gb, c)?;
                }
                if open {
                    for (k, &(set, switch)) in setting_rows(card).iter().enumerate() {
                        let y = (SET_Y + k as f32 * SET_ROW) * s;
                        let row = rect(x + 12.0 * s, y, x + card_w * s - 12.0 * s, y + SET_ROW * s);
                        let label = match set {
                            Setting::WaterOn | Setting::SitOn => tr("Nhắc", "Remind"),
                            Setting::Cup => tr("Mỗi lần", "Per drink"),
                            Setting::Goal => tr("Mục tiêu", "Goal"),
                            Setting::From => tr("Từ", "From"),
                            Setting::To => tr("Đến", "To"),
                            Setting::SitEvery => tr("Mỗi", "Every"),
                        };
                        text(label, F_LABEL, row, lead, grey(0.65, a))?;
                        if switch {
                            if let Some(sb) = rect_of(Button::Toggle(set)) {
                                let on = if set == Setting::WaterOn { h.water_on } else { h.sit_on };
                                let rad = (sb.bottom - sb.top) / 2.0;
                                rt.FillRoundedRectangle(&rounded(sb, rad), &self.brush(if on { solid(GREEN, a) } else { grey(0.3, a) })?);
                                let kx = if on { sb.right - rad } else { sb.left + rad };
                                let knob = D2D1_ELLIPSE { point: Vector2 { X: kx, Y: (sb.top + sb.bottom) / 2.0 }, radiusX: rad - 2.0 * s, radiusY: rad - 2.0 * s };
                                rt.FillEllipse(&knob, &self.brush(grey(1.0, a))?)
                            }
                        } else {
                            let value = match set {
                                Setting::Cup => format!("{} ml", h.cup_ml),
                                Setting::Goal => format!("{:.1} L", h.goal_ml as f32 / 1000.0),
                                Setting::From => health::hhmm(h.from_min),
                                Setting::To => health::hhmm(h.to_min),
                                _ => format!("{} {}", h.sit_every_min, tr("phút", "min")),
                            };
                            if let (Some(mb), Some(pb)) = (rect_of(Button::Step(set, -1)), rect_of(Button::Step(set, 1))) {
                                for (bb, icon, dir) in [(mb, ICON_MINUS, -1), (pb, ICON_PLUS, 1)] {
                                    let hot = hovered == Some(Button::Step(set, dir));
                                    let c = D2D1_ELLIPSE { point: Vector2 { X: (bb.left + bb.right) / 2.0, Y: (bb.top + bb.bottom) / 2.0 }, radiusX: 8.0 * s, radiusY: 8.0 * s };
                                    rt.FillEllipse(&c, &self.brush(grey(if hot { 0.32 } else { 0.2 }, a))?);
                                    // The sign is drawn as lines, so it sits exactly in the circle's middle (the
                                    // icon font's minus rides about a pixel high). Whole-pixel stroke, centred on a
                                    // pixel edge (even width) or a pixel's middle (odd), so it stays crisp.
                                    let w = (1.75 * s).round().max(1.0);
                                    let snap = |v: f32| if w % 2.0 == 0.0 { v.round() } else { v.floor() + 0.5 };
                                    let (mx, my, k) = (snap(c.point.X), snap(c.point.Y), 3.0 * s);
                                    let ink = self.brush(grey(1.0, a))?;
                                    rt.DrawLine(Vector2 { X: mx - k, Y: my }, Vector2 { X: mx + k, Y: my }, &ink, w, &self.round);
                                    if icon == ICON_PLUS {
                                        rt.DrawLine(Vector2 { X: mx, Y: my - k }, Vector2 { X: mx, Y: my + k }, &ink, w, &self.round);
                                    }
                                }
                                text(&value, F_LABEL, rect(mb.right, mb.top, pb.left, pb.bottom), center, grey(1.0, a))?;
                            }
                        }
                    }
                    if card == Card::Sit {
                        let y = (SET_Y + 2.0 * SET_ROW) * s;
                        text(tr("Rời máy ≥ 5 phút: tính lại", "Away ≥ 5 min: starts over"), F_LABEL, rect(x + 12.0 * s, y, x + card_w * s - 8.0 * s, y + SET_ROW * s), lead, grey(0.5, a))?;
                    }
                    continue;
                }
                // The figure, the share of the day done, and the status line.
                let (big, frac, status) = if card == Card::Water {
                    let status = if !h.water_on { String::new() }
                        else if h.taken_ml >= h.goal_ml { tr("đạt mục tiêu", "goal reached").to_string() }
                        else if hv.minute_of_day < h.from_min { format!("{} {}", tr("bắt đầu", "starts"), health::hhmm(h.from_min)) }
                        else {
                            match h.next_drink(hv.now, hv.minute_of_day) {
                                Some(at) => format!("{} {}", tr("nhắc lúc", "next at"), health::hhmm((at - (hv.now - hv.minute_of_day as i64)).max(0) as u32)),
                                None => tr("hết giờ nhắc", "after hours").to_string(),
                            }
                        };
                    (h.taken_ml.to_string(), h.taken_ml as f32 / h.goal_ml.max(1) as f32, status)
                } else {
                    let sat = h.sitting_minutes(hv.now);
                    let status = if !h.sit_on { String::new() }
                        else if h.sitting_since.is_none() { tr("đang rời máy", "away from the PC").to_string() }
                        else { format!("{} {} {}", tr("nhắc sau", "reminder in"), (h.sit_every_min as i64 - sat).max(0), tr("phút", "min")) };
                    (sat.to_string(), sat as f32 / h.sit_every_min.max(1) as f32, status)
                };
                // The card's own picture, big and faint in its bottom right corner, cut by the tile.
                rt.PushAxisAlignedClip(&cb, D2D1_ANTIALIAS_MODE_ALIASED);
                if card == Card::Water {
                    let g = self.water_drop(cb.right - 16.0 * s, cb.bottom - 10.0 * s, 96.0 * s)?;
                    rt.FillGeometry(&g, &self.brush(solid(accent, a * 0.09))?, None);
                } else {
                    text(ICON_WALK, F_MARK, rect(cb.right - 84.0 * s, cb.bottom - 90.0 * s, cb.right + 30.0 * s, cb.bottom + 24.0 * s), center, solid(accent, a * 0.08))?;
                }
                rt.PopAxisAlignedClip();
                // The figure large; under it what it is of and when the next reminder is.
                let (bx, by) = (x + 14.0 * s, (CARD_TOP + 30.0) * s);
                // Two short lines: what the figure is of, then the status; a switched-off tile
                // leaves the second empty and says so under its gear.
                let off = status.is_empty();
                let (unit_inline, line1) = if card == Card::Water {
                    (String::new(), format!("{} {} ml", tr("trên", "of"), h.goal_ml))
                } else {
                    (tr("phút", "min").to_string(), tr("đã ngồi", "sitting").to_string())
                };
                text(&big, F_FIGURE, rect(bx, by, x + card_w * s - 12.0 * s, by + 36.0 * s), lead, grey(if off { 0.5 } else { 1.0 }, a))?;
                if !unit_inline.is_empty() {
                    let bw = width(&big, F_FIGURE);
                    text(&unit_inline, F_BODY, rect(bx + bw + 5.0 * s, by + 8.0 * s, x + card_w * s - 12.0 * s, by + 32.0 * s), lead, grey(0.6, a))?;
                }
                let line_r = x + (card_w - 54.0) * s;
                text(&line1, F_LABEL, rect(bx, by + 38.0 * s, line_r, by + 54.0 * s), lead, grey(0.6, a))?;
                text(&status, F_LABEL, rect(bx, by + 54.0 * s, line_r, by + 70.0 * s), lead, grey(0.75, a))?;
                // The day's progress as a thin bar along the tile's bottom, in its colour.
                let (track_l, track_r, track_y) = (bx, x + (card_w - 64.0) * s, (bottom - 16.0) * s);
                let track = |right: f32| rounded(rect(track_l, track_y, right, track_y + 4.0 * s), 2.0 * s);
                rt.FillRoundedRectangle(&track(track_r), &self.brush(color(accent.0 * 0.22, accent.1 * 0.22, accent.2 * 0.22, a))?);
                rt.FillRoundedRectangle(&track(track_l + (track_r - track_l) * frac.clamp(0.0, 1.0)), &self.brush(solid(accent, a))?);
                // The round action button: + for a drink, a tick for having stood up.
                let ab = if card == Card::Water { Button::Drink } else { Button::Stood };
                if let Some(pb) = rect_of(ab) {
                    let k = (if hovered == Some(ab) { 1.0 } else { 0.9 }) * if off { 0.6 } else { 1.0 };
                    let c = D2D1_ELLIPSE { point: Vector2 { X: (pb.left + pb.right) / 2.0, Y: (pb.top + pb.bottom) / 2.0 }, radiusX: (pb.right - pb.left) / 2.0, radiusY: (pb.bottom - pb.top) / 2.0 };
                    rt.FillEllipse(&c, &self.brush(color(accent.0 * k, accent.1 * k, accent.2 * k, a))?);
                    // Dark on both: white on the orange is only 3.1:1. The sign is drawn as strokes of
                    // whole pixels (2 px at 100%) so it is bold, centred and crisp; the icon font's
                    // glyph was one thin pixel and half a pixel off.
                    let w = (2.0 * s).round().max(2.0);
                    let snap = |v: f32| if w % 2.0 == 0.0 { v.round() } else { v.floor() + 0.5 };
                    let (mx, my) = (snap(c.point.X), snap(c.point.Y));
                    let ink = self.brush(grey(0.06, a))?;
                    let at = |dx: f32, dy: f32| Vector2 { X: mx + dx * s, Y: my + dy * s };
                    if card == Card::Water {
                        rt.DrawLine(at(-6.0, 0.0), at(6.0, 0.0), &ink, w, &self.round);
                        rt.DrawLine(at(0.0, -6.0), at(0.0, 6.0), &ink, w, &self.round);
                    } else {
                        rt.DrawLine(at(-6.0, 0.5), at(-2.0, 4.5), &ink, w, &self.round);
                        rt.DrawLine(at(-2.0, 4.5), at(6.5, -4.5), &ink, w, &self.round);
                    }
                }
                // Under the gear: "Later" for a water reminder, or that the tile is off.
                let under = rect(x + (card_w - 72.0) * s, (CARD_TOP + 28.0) * s, x + (card_w - 12.0) * s, (CARD_TOP + 46.0) * s);
                if off {
                    text(tr("Đang tắt", "Off"), F_LABEL, under, trail, grey(0.55, a))?;
                } else if card == Card::Water {
                    let hot = hovered == Some(Button::Snooze);
                    text(tr("Để sau", "Later"), F_LABEL, under, trail, grey(if hot { 1.0 } else { 0.6 }, a))?;
                }
            }
        }
        Ok(())
    }
}

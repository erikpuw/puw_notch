use super::*;

impl Renderer {
    pub(super) fn draw_weather(
        &self,
        text: &dyn Fn(&str, usize, D2D_RECT_F, DWRITE_TEXT_ALIGNMENT, D2D1_COLOR_F) -> Result<()>,
        width: &dyn Fn(&str, usize) -> f32,
        wx_icon: &dyn Fn(&icons::Icon, &[ID2D1PathGeometry], f32, f32, f32, Option<((f32, f32, f32), (f32, f32, f32))>, f32, f32) -> Result<()>,
        draw_input: &dyn Fn(&str) -> Result<()>,
        hover_disc: &dyn Fn(Button) -> Result<()>,
        btns: &[(Button, D2D_RECT_F)],
        v: &View,
        lv: &ListView,
        current: bool,
        weather_line: &Option<(&'static icons::Icon, Vec<ID2D1PathGeometry>)>,
        weather_empty: &Option<(&'static icons::Icon, Vec<ID2D1PathGeometry>)>,
        moon_line: &Option<(&'static icons::Icon, Vec<ID2D1PathGeometry>)>,
        hour_lines: &[(weather::Hour, &'static icons::Icon, Vec<ID2D1PathGeometry>)],
        figure_icons: &[(&'static icons::Icon, Vec<ID2D1PathGeometry>)],
        (cl, cr): (f32, f32),
        s: f32,
        a: f32,
        hovered: Option<Button>,
    ) -> Result<()> {
        let rt = &self.rt;
        let (center, lead) = (DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING);
        let solid = |c: (f32, f32, f32), a: f32| color(c.0, c.1, c.2, a);
        let rect_of = |b: Button| btns.iter().find(|(x, _)| *x == b).map(|(_, r)| *r);
        let figure = |slot: usize, x: f32, y: f32, size: f32, alpha: f32| -> Result<()> {
            match figure_icons.get(slot) {
                Some((ic, geoms)) => wx_icon(ic, geoms, x, y, size, None, a * alpha, 9.0),
                None => Ok(()),
            }
        };
        unsafe {
            let wx = v.weather;
            match (&wx.place, wx.now, weather_line) {
                (Some(place), Some(n), Some((ic, geoms))) => {
                    let stale = wx.is_stale(lv.now);
                    let fade = if stale { 0.55 } else { 1.0 };
                    let buttons_l = cr - (CHIPS_R + 2.0 * UNIT_W + SUN_GAP + SUN_W + 8.0) * s;
                    figure(0, cl, (CITY_Y + 5.5) * s, 13.0 * s, 0.55)?;
                    text(&place.name, F_BODY, rect(cl + 16.0 * s, (CITY_Y + 2.0) * s, buttons_l, (CITY_Y + 22.0) * s), lead, grey(0.8, a))?;
                    if let Some(ob) = rect_of(Button::CityOff) {
                        let hot = hovered == Some(Button::CityOff);
                        hover_disc(Button::CityOff)?;
                        text(ICON_CLOSE, F_SMALL_ICON, ob, center, if hot { color(1.0, 0.45, 0.4, a) } else { grey(0.55, a) })?;
                    }
                    if let (Some(first), Some(last)) = (rect_of(Button::Unit(weather::Unit::C)), rect_of(Button::Unit(weather::Unit::F))) {
                        rt.FillRoundedRectangle(&rounded(rect(first.left, first.top, last.right, last.bottom), UNIT_H / 2.0 * s), &self.brush(grey(0.1, a))?);
                    }
                    for unit in [weather::Unit::C, weather::Unit::F] {
                        let Some(chip) = rect_of(Button::Unit(unit)) else { continue };
                        let on = v.unit == unit;
                        if on || hovered == Some(Button::Unit(unit)) {
                            rt.FillRoundedRectangle(&rounded(chip, UNIT_H / 2.0 * s), &self.brush(grey(1.0, a * if on { 0.2 } else { 0.08 }))?);
                        }
                        text(unit.symbol(), F_LABEL, chip, center, grey(if on { 1.0 } else { 0.55 }, a))?;
                    }
                    if let Some(chip) = rect_of(Button::SunToggle) {
                        let (on, hot) = (v.sun_view, hovered == Some(Button::SunToggle));
                        rt.FillRoundedRectangle(&rounded(chip, UNIT_H / 2.0 * s), &self.brush(grey(if on { 0.22 } else if hot { 0.16 } else { 0.1 }, a))?);
                        text("\u{E706}", F_SMALL_ICON, chip, center, grey(if on { 1.0 } else if hot { 0.85 } else { 0.6 }, a))?;
                    }
                    wx_icon(ic, geoms, cl, ICON_Y * s, WX_ICON * s, Some(weather::tints(n.sky, n.day)), a * fade, if current { v.page_age } else { 9.0 })?;
                    let temp = format!("{}°", v.unit.number(n.temp));
                    let (tx, ty) = (cl + TEMP_X * s, (ICON_Y + 22.0) * s);
                    let k = ((HOURS_X - TEMP_X - 8.0) * s / width(&temp, F_TEMP)).min(1.0);
                    let base = { let mut m = Matrix3x2::default(); rt.GetTransform(&mut m); m };
                    rt.SetTransform(&(translate(-tx, -ty) * scaling(k) * translate(tx, ty) * base));
                    text(&temp, F_TEMP, rect(tx, ICON_Y * s, tx + width(&temp, F_TEMP) + 6.0 * s, (ICON_Y + 44.0) * s), lead, grey(1.0, a * fade))?;
                    rt.SetTransform(&base);
                    let words_r = cl + (HOURS_X - 8.0) * s;
                    text(weather::describe(n.sky), F_BODY, rect(tx, (ICON_Y + 48.0) * s, words_r, (ICON_Y + 64.0) * s), lead, grey(0.9, a))?;
                    let bl = cl + HOURS_X * s;
                    let columns = if v.sun_view { 0 } else { hour_lines.len().min(5) };
                    if v.sun_view {
                        match wx.sun {
                            Some(sun) => self.draw_sun_path(text, wx_icon, moon_line, sun, v.health.minute_of_day, (bl, cr), s, a * fade, v.phase, if current { v.page_age } else { 9.0 })?,
                            None => text(tr("Đang lấy giờ mặt trời…", "Getting the sun's times…"), F_LABEL, rect(bl, SUN_WAIT_Y * s, cr, (SUN_WAIT_Y + 20.0) * s), center, grey(0.55, a))?,
                        }
                    }
                    if columns > 0 {
                        let shown = &hour_lines[..columns];
                        let sw = (cr - bl) / columns as f32;
                        let (lo, hi) = shown.iter().fold((f32::MAX, f32::MIN), |(lo, hi), (h, _, _)| (lo.min(h.temp), hi.max(h.temp)));
                        let per_degree = if hi > lo { ((CURVE_BOTTOM - CURVE_TOP) / (hi - lo)).min(4.0) } else { 0.0 };
                        let mid = (CURVE_TOP + CURVE_BOTTOM) / 2.0;
                        let points: Vec<(f32, f32)> = shown.iter().enumerate()
                            .map(|(i, (h, _, _))| (bl + (i as f32 + 0.5) * sw, (mid - (h.temp - (lo + hi) / 2.0) * per_degree) * s))
                            .collect();
                        let ink = self.brush(grey(1.0, a * fade))?;
                        if points.len() > 1 {
                            let at = |i: isize| points[i.clamp(0, points.len() as isize - 1) as usize];
                            let geo = self.factory.CreatePathGeometry()?;
                            let sink = geo.Open()?;
                            sink.BeginFigure(Vector2 { X: points[0].0, Y: points[0].1 }, D2D1_FIGURE_BEGIN_HOLLOW);
                            for i in 0..points.len() as isize - 1 {
                                let (p0, p1, p2, p3) = (at(i - 1), at(i), at(i + 1), at(i + 2));
                                sink.AddBezier(&D2D1_BEZIER_SEGMENT {
                                    point1: Vector2 { X: p1.0 + (p2.0 - p0.0) / 6.0, Y: p1.1 + (p2.1 - p0.1) / 6.0 },
                                    point2: Vector2 { X: p2.0 - (p3.0 - p1.0) / 6.0, Y: p2.1 - (p3.1 - p1.1) / 6.0 },
                                    point3: Vector2 { X: p2.0, Y: p2.1 },
                                });
                            }
                            sink.EndFigure(D2D1_FIGURE_END_OPEN);
                            sink.Close()?;
                            rt.DrawGeometry(&geo, &ink, 2.0 * s, &self.round);
                        }
                        for (i, ((h, ic, geoms), (px, py))) in shown.iter().zip(&points).enumerate() {
                            rt.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: *px, Y: *py }, radiusX: 3.0 * s, radiusY: 3.0 * s }, &ink);
                            let x = bl + i as f32 * sw;
                            text(&format!("{}h", h.hour), F_LABEL, rect(x, HOURS_Y * s, x + sw, (HOURS_Y + 14.0) * s), center, grey(0.55, a * fade))?;
                            text(&format!("{}°", v.unit.number(h.temp)), F_LABEL, rect(x, py - 19.0 * s, x + sw, py - 5.0 * s), center, grey(1.0, a * fade))?;
                            wx_icon(ic, geoms, px - HOUR_ICON / 2.0 * s, HOUR_ICON_Y * s, HOUR_ICON * s, Some(weather::tints(h.sky, h.day)), a * fade, 9.0)?;
                            if h.rain >= 20 {
                                text(&format!("{}%", h.rain), F_LABEL, rect(x, RAIN_Y * s, x + sw, (RAIN_Y + 13.0) * s), center, solid(BLUE, a * fade))?;
                            }
                        }
                    }
                    let high = weather::outlook(&wx.hours).high.map(|h| format!("{} {}°", tr("Cao", "High"), v.unit.number(h.temp)));
                    let figures = [
                        Some(format!("{} {}°", tr("Cảm giác", "Feels like"), v.unit.number(n.feels))),
                        Some(format!("{:.0}%", n.humidity)),
                        Some(format!("{:.0} km/h", n.wind)),
                        high,
                    ];
                    for (slot, label) in figures.into_iter().enumerate() {
                        let Some(label) = label else { continue };
                        let (column, row) = (slot % 2, slot / 2);
                        let x = cl + column as f32 * FIG_COL * s;
                        let y = (FIG_ROW_Y + row as f32 * FIG_ROW_H) * s;
                        let right = if column == 0 { cl + (FIG_COL - 4.0) * s } else { cl + (HOURS_X - 8.0) * s };
                        figure(slot + 1, x, y + 1.5 * s, 13.0 * s, 0.55 * fade)?;
                        text(&label, F_LABEL, rect(x + 18.0 * s, y, right, y + FIG_ROW_H * s), lead, grey(0.8, a * fade))?;
                    }
                }
                _ => {
                    draw_input(tr("Nhập thành phố (vd: Hà Nội, Đà Nẵng)", "Type a city (e.g. Hanoi, Da Nang)"))?;
                    let (first, second) = if wx.looking {
                        (tr("Đang tìm thành phố…", "Looking for the city…").to_string(), "")
                    } else if let Some(city) = &wx.not_found {
                        (format!("{} “{city}”", tr("Không tìm thấy", "Not found:")), tr("Thử viết khác, hoặc tên tỉnh lân cận", "Try another spelling, or a nearby city"))
                    } else if wx.place.is_some() {
                        (tr("Đang lấy thời tiết…", "Getting the weather…").to_string(), "")
                    } else {
                        (tr("Nhập tên thành phố rồi nhấn Enter", "Type a city and press Enter").to_string(), tr("Có dấu hay không dấu đều được", "With or without accents"))
                    };
                    if let Some((ic, geoms)) = weather_empty {
                        let mid = (cl + cr) / 2.0;
                        wx_icon(ic, geoms, mid - 16.0 * s, 72.0 * s, 32.0 * s, Some(((0.4, 0.4, 0.4), (0.4, 0.4, 0.4))), a, 9.0)?;
                    }
                    text(&first, F_BODY, rect(cl, 110.0 * s, cr, 130.0 * s), center, grey(0.75, a))?;
                    text(second, F_LABEL, rect(cl, 130.0 * s, cr, 150.0 * s), center, grey(0.5, a))?;
                }
            }
        }
        Ok(())
    }
}

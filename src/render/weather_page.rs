//! The Weather page: the city line, the big icon and temperature, the hours ahead and a line of figures.

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
        hour_lines: &[(weather::Hour, &'static icons::Icon, Vec<ID2D1PathGeometry>)],
        (cl, cr): (f32, f32),
        s: f32,
        a: f32,
        hovered: Option<Button>,
    ) -> Result<()> {
        let rt = &self.rt;
        let (center, lead) = (DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING);
        let solid = |c: (f32, f32, f32), a: f32| color(c.0, c.1, c.2, a);
        let rect_of = |b: Button| btns.iter().find(|(x, _)| *x == b).map(|(_, r)| *r);
        unsafe {
            draw_input(tr("Nhập thành phố (vd: Hà Nội, Đà Nẵng)", "Type a city (e.g. Hanoi, Da Nang)"))?;
            let wx = v.weather;
            match (&wx.place, wx.now, weather_line) {
                (Some(place), Some(n), Some((ic, geoms))) => {
                    // Conditions older than STALE_MIN are dimmed and say so in words.
                    let stale = wx.is_stale(lv.now);
                    let fade = if stale { 0.55 } else { 1.0 };
                    // City line: the name, then the unit chips and x at its end.
                    let buttons_l = cr - (30.0 + 8.0 + 2.0 * UNIT_W + 8.0) * s;
                    text(&place.name, F_TITLE, rect(cl, (CITY_Y + 2.0) * s, buttons_l, (CITY_Y + 22.0) * s), lead, grey(1.0, a))?;
                    if let Some(ob) = rect_of(Button::CityOff) {
                        let hot = hovered == Some(Button::CityOff);
                        hover_disc(Button::CityOff)?;
                        text(ICON_CLOSE, F_SMALL_ICON, ob, center, if hot { color(1.0, 0.45, 0.4, a) } else { grey(0.55, a) })?;
                    }
                    // The unit to show it in: one capsule holding both, the chosen one lit, the
                    // other lighting on hover.
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
                    // Left block, bare on the panel: the big icon, the temperature beside it, the
                    // sky's words and the feels-like under that.
                    wx_icon(ic, geoms, cl, ICON_Y * s, WX_ICON * s, Some(weather::tints(n.sky, n.day)), a * fade, if current { v.page_age } else { 9.0 })?;
                    // The temperature shrinks to fit the left block when it is long ("-128°"), turning
                    // about its left end.
                    let temp = format!("{}°", v.unit.number(n.temp));
                    let (tx, ty) = (cl + TEMP_X * s, (ICON_Y + 20.0) * s);
                    let k = ((STRIP_X - TEMP_X - 8.0) * s / width(&temp, F_TEMP)).min(1.0);
                    let base = { let mut m = Matrix3x2::default(); rt.GetTransform(&mut m); m };
                    rt.SetTransform(&(translate(-tx, -ty) * scaling(k) * translate(tx, ty) * base));
                    text(&temp, F_TEMP, rect(tx, (ICON_Y - 2.0) * s, tx + width(&temp, F_TEMP) + 6.0 * s, (ICON_Y + 42.0) * s), lead, grey(1.0, a * fade))?;
                    rt.SetTransform(&base);
                    let words_r = cl + (STRIP_X - 8.0) * s;
                    text(weather::describe(n.sky), F_BODY, rect(tx, (ICON_Y + 42.0) * s, words_r, (ICON_Y + 58.0) * s), lead, grey(0.9, a))?;
                    // Right block: the next five slots, three hours apart, as columns.
                    let bl = cl + STRIP_X * s;
                    let columns = hour_lines.len().min(5);
                    if columns > 0 {
                        let sw = (cr - bl) / columns as f32;
                        for (i, (h, ic, geoms)) in hour_lines.iter().take(columns).enumerate() {
                            let x = bl + i as f32 * sw;
                            text(&format!("{}h", h.hour), F_LABEL, rect(x, STRIP_Y * s, x + sw, (STRIP_Y + 13.0) * s), center, grey(0.55, a * fade))?;
                            wx_icon(ic, geoms, x + sw / 2.0 - 9.0 * s, (STRIP_Y + 15.0) * s, 18.0 * s, Some(weather::tints(h.sky, h.day)), a * fade, 9.0)?;
                            text(&format!("{}°", v.unit.number(h.temp)), F_LABEL, rect(x, (STRIP_Y + 35.0) * s, x + sw, (STRIP_Y + 49.0) * s), center, grey(1.0, a * fade))?;
                            if h.rain >= 20 {
                                text(&format!("{}%", h.rain), F_LABEL, rect(x, (STRIP_Y + 48.0) * s, x + sw, (STRIP_Y + 62.0) * s), center, solid(BLUE, a * fade))?;
                            }
                        }
                    }
                    // A hairline, then one line of figures across the whole width: the first at the left
                    // edge, under the icon, the last at the right end, the rest spread evenly between.
                    // Kept clear of the page dots below.
                    rt.FillRectangle(&rect(cl, (FIGURES_Y - 3.0) * s, cr, (FIGURES_Y - 2.0) * s), &self.brush(grey(0.12, a))?);
                    let high = weather::outlook(&wx.hours).high.map(|h| format!("{} {}°", tr("Cao nhất", "High"), v.unit.number(h.temp)));
                    let figures: Vec<String> = [
                        Some(format!("{} {}°", tr("Cảm giác", "Feels like"), v.unit.number(n.feels))),
                        Some(format!("{} {:.0}%", tr("Độ ẩm", "Humidity"), n.humidity)),
                        Some(format!("{} {:.0} km/h", tr("Gió", "Wind"), n.wind)),
                        high,
                    ].into_iter().flatten().collect();
                    let widths: Vec<f32> = figures.iter().map(|f| width(f, F_LABEL) + 2.0 * s).collect();
                    let gap = ((cr - cl) - widths.iter().sum::<f32>()).max(0.0) / (figures.len().max(2) - 1) as f32;
                    let mut fx = cl;
                    for (label, fw) in figures.iter().zip(&widths) {
                        text(label, F_LABEL, rect(fx, (FIGURES_Y - 2.0) * s, fx + fw, (FIGURES_Y + 16.0) * s), lead, grey(0.65, a * fade))?;
                        fx += fw + gap;
                    }
                }
                _ => {
                    let (first, second) = if wx.looking {
                        (tr("Đang tìm thành phố…", "Looking for the city…").to_string(), "")
                    } else if let Some(city) = &wx.not_found {
                        (format!("{} “{city}”", tr("Không tìm thấy", "Not found:")), tr("Thử viết khác, hoặc tên tỉnh lân cận", "Try another spelling, or a nearby city"))
                    } else if wx.place.is_some() {
                        (tr("Đang lấy thời tiết…", "Getting the weather…").to_string(), "")
                    } else {
                        (tr("Nhập tên thành phố rồi nhấn Enter", "Type a city and press Enter").to_string(), tr("Có dấu hay không dấu đều được", "With or without accents"))
                    };
                    // A muted sun above the words, as the Tasks page has its ring.
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

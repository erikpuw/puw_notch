use super::*;

fn lerp_cell(from: D2D_RECT_F, to: D2D_RECT_F, k: f32) -> D2D_RECT_F {
    let l = |a: f32, b: f32| a + (b - a) * k;
    rect(l(from.left, to.left), l(from.top, to.top), l(from.right, to.right), l(from.bottom, to.bottom))
}

impl Renderer {
    pub(super) fn draw_nav(
        &self,
        c: &Ctx,
        weather_empty: &Option<(&'static icons::Icon, Vec<ID2D1PathGeometry>)>,
        a: f32,
        ph: f32,
    ) -> Result<()> {
        let rt = &self.rt;
        let Ctx { v, s, l, r, cx, nh, text, wx_icon, .. } = *c;
        let center = DWRITE_TEXT_ALIGNMENT_CENTER;
        let trail = DWRITE_TEXT_ALIGNMENT_TRAILING;
        unsafe {
            let nav = [
                (Button::PagePrev, rect(l + 10.0 * s, arrow_y(ph) * s, l + (10.0 + ARROW) * s, (arrow_y(ph) + ARROW) * s)),
                (Button::PageNext, rect(r - (10.0 + ARROW) * s, arrow_y(ph) * s, r - 10.0 * s, (arrow_y(ph) + ARROW) * s)),
            ];
            let arrow_a = a * (1.0 - ((v.size[0] - notch::target(Mode::Expanded, false)[0]) / notch::LYRICS_EXTRA_W).clamp(0.0, 1.0));
            for (b, icon) in [(Button::PagePrev, ICON_LEFT), (Button::PageNext, ICON_RIGHT)] {
                if arrow_a < 0.01 {
                    break;
                }
                let a = arrow_a;
                if let Some((_, br)) = nav.iter().find(|(x, _)| *x == b) {
                    let hot = v.hovered == Some(b);
                    let circle = D2D1_ELLIPSE {
                        point: Vector2 { X: (br.left + br.right) / 2.0, Y: (br.top + br.bottom) / 2.0 },
                        radiusX: (br.right - br.left) / 2.0,
                        radiusY: (br.bottom - br.top) / 2.0,
                    };
                    rt.FillEllipse(&circle, &self.brush(grey(if hot { 0.26 } else { 0.15 }, a))?);
                    text(icon, F_SMALL_ICON, *br, center, grey(if hot { 1.0 } else { 0.75 }, a))?;
                }
            }
            let (dot, wide, gap) = (6.0 * s, 18.0 * s, 6.0 * s);
            let kept = PAGES.iter().filter(|p| page_shown(**p)).count();
            let total = wide + (kept as f32 - 1.0) * (dot + gap);
            let mut dx = cx - total / 2.0;
            let arriving = v.leaving.map_or(1.0, |(_, t, _)| ease_out(t));
            let nav_fade = ((nh - ph * s - 4.0 * s) / (8.0 * s)).clamp(0.0, 1.0);
            for p in PAGES.into_iter().filter(|p| page_shown(*p)) {
                let weight = match v.leaving {
                    _ if p == v.page => arriving,
                    Some((from, _, _)) if p == from => 1.0 - arriving,
                    _ => 0.0,
                };
                let dw = dot + (wide - dot) * weight;
                let shade = 0.38 + 0.62 * weight;
                rt.FillRoundedRectangle(&rounded(rect(dx, dots_y(ph) * s, dx + dw, dots_y(ph) * s + dot), dot / 2.0), &self.brush(grey(shade, a * (1.0 - nav_fade)))?);
                dx += dw + gap;
            }
            if nav_fade > 0.01 {
                let cells = nav_cells(cx, s, ph);
                let cell_of = |page: Page| cells.iter().find(|(p, _)| *p == page).map(|(_, c)| *c);
                if let Some(to) = cell_of(v.page) {
                    let from = v.leaving.and_then(|(from, _, _)| cell_of(from)).unwrap_or(to);
                    rt.FillRoundedRectangle(&rounded(lerp_cell(from, to, arriving), 8.0 * s), &self.brush(grey(0.17, a * nav_fade))?);
                }
                for (p, cell) in cells {
                    let hot = v.hovered == Some(Button::GoPage(p));
                    if hot && p != v.page {
                        rt.FillRoundedRectangle(&rounded(cell, 8.0 * s), &self.brush(grey(0.1, a * nav_fade))?);
                    }
                    let weight = match v.leaving {
                        _ if p == v.page => arriving,
                        Some((from, _, _)) if p == from => 1.0 - arriving,
                        _ => 0.0,
                    };
                    let ink = grey(if hot { 1.0 } else { 0.6 + 0.4 * weight }, a * nav_fade);
                    match (weather_empty, p) {
                        (Some((ic, geoms)), Page::Weather) => {
                            let (mx, my) = ((cell.left + cell.right) / 2.0, (cell.top + cell.bottom) / 2.0);
                            let g = if hot { 1.0 } else { 0.6 + 0.4 * weight };
                            let tint = (g, g, g);
                            wx_icon(ic, geoms, mx - 8.0 * s, my - 8.0 * s, 16.0 * s, Some((tint, tint)), a * nav_fade, 9.0)?;
                        }
                        _ => text(nav_icon(p), F_ICON, cell, center, ink)?,
                    }
                }
            }
            if let (Page::Weather, None, Some(_), Some(u)) = (v.page, v.leaving, v.weather.now, v.weather.updated) {
                let stale = v.weather.is_stale(v.list.now);
                let when = format!("{}{:02}:{:02}", if stale { format!("{} ", tr("cũ", "old")) } else { String::new() }, u / 100 % 100, u % 100);
                let ca = a * (1.0 - nav_fade);
                let ink = if stale { color(ORANGE.0, ORANGE.1, ORANGE.2, ca) } else { grey(0.4, ca) };
                text(&format!("Open-Meteo · {when}"), F_LABEL, rect(cx + 36.0 * s, (dots_y(ph) - 8.0) * s, r - 32.0 * s, (dots_y(ph) + 7.0) * s), trail, ink)?;
            }
        }
        Ok(())
    }
}

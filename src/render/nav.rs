//! The panel's side arrows, page dots, the row of page icons that opens under them, and the Weather page's source credit.

use super::*;

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
            // Side arrows and page dots.
            // They sit on the panel's own edges, so they follow it while it widens or narrows.
            let nav = [
                (Button::PagePrev, rect(l + 10.0 * s, arrow_y(ph) * s, l + (10.0 + ARROW) * s, (arrow_y(ph) + ARROW) * s)),
                (Button::PageNext, rect(r - (10.0 + ARROW) * s, arrow_y(ph) * s, r - 10.0 * s, (arrow_y(ph) + ARROW) * s)),
            ];
            // While the lyrics are open the arrows are hidden (they fade as the panel widens), so the
            // way back is the lyrics button: shrinking the panel never leaves the mouse outside it.
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
            // The page dots: the long one is the page on show. On a page change the old one shrinks
            // while the new one grows (the weights below), so the pill seems to travel and stretch.
            let (dot, wide, gap) = (6.0 * s, 18.0 * s, 6.0 * s);
            let kept = PAGES.iter().filter(|p| page_shown(**p)).count();
            let total = wide + (kept as f32 - 1.0) * (dot + gap);
            let mut dx = cx - total / 2.0;
            let arriving = v.leaving.map_or(1.0, |(_, t, _)| ease_out(t));
            // The row of page icons comes as the panel grows by NAV_EXTRA, the dots give way to it. The icons only show
            // once they are inside the panel (they reach 10 px under its resting height, so none show before the panel is
            // 4 px past it, all of them 12 px past it), so they fade out before the shrinking edge gets to them instead of
            // poking out of it.
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
                for (p, cell) in nav_cells(cx, s, ph) {
                    let (on, hot) = (p == v.page, v.hovered == Some(Button::GoPage(p)));
                    if on || hot {
                        rt.FillRoundedRectangle(&rounded(cell, 8.0 * s), &self.brush(grey(if on { 0.17 } else { 0.1 }, a * nav_fade))?);
                    }
                    let ink = grey(if on || hot { 1.0 } else { 0.6 }, a * nav_fade);
                    match (weather_empty, p) {
                        (Some((ic, geoms)), Page::Weather) => {
                            let (mx, my) = ((cell.left + cell.right) / 2.0, (cell.top + cell.bottom) / 2.0);
                            let tint = if on || hot { (1.0, 1.0, 1.0) } else { (0.6, 0.6, 0.6) };
                            wx_icon(ic, geoms, mx - 8.0 * s, my - 8.0 * s, 16.0 * s, Some((tint, tint)), a * nav_fade, 9.0)?;
                        }
                        _ => text(nav_icon(p), F_ICON, cell, center, ink)?,
                    }
                }
            }
            // The Weather page's source (Open-Meteo asks for the credit, CC BY 4.0) and when it was
            // fetched, "old" when stale: quiet in the bottom-right corner, level with the dots, clear
            // of the page's own content.
            if let (Page::Weather, None, Some(_), Some(u)) = (v.page, v.leaving, v.weather.now, v.weather.updated) {
                let stale = v.weather.is_stale(v.list.now);
                let when = format!("{}{:02}:{:02}", if stale { format!("{} ", tr("cũ", "old")) } else { String::new() }, u / 100 % 100, u % 100);
                let ink = if stale { color(ORANGE.0, ORANGE.1, ORANGE.2, a) } else { grey(0.4, a) };
                text(&format!("Open-Meteo · {when}"), F_LABEL, rect(cx + 36.0 * s, (dots_y(ph) - 8.0) * s, r - 32.0 * s, (dots_y(ph) + 7.0) * s), trail, ink)?;
            }
        }
        Ok(())
    }
}

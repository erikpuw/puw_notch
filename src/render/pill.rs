//! The resting pill: the clock, the battery ring, the weather, the cover and bars while music plays, the idle robot, and the Pomodoro line along its bottom.

use super::*;

impl Renderer {
    pub(super) fn draw_pill(
        &self,
        c: &Ctx,
        weather_line: &Option<(&'static icons::Icon, Vec<ID2D1PathGeometry>)>,
        bars_level: f32,
        small_a: f32,
    ) -> Result<()> {
        let rt = &self.rt;
        let Ctx { v, s, l, r, cx, nh, shape, text, width, wx_icon, cover, .. } = *c;
        let center = DWRITE_TEXT_ALIGNMENT_CENTER;
        let lead = DWRITE_TEXT_ALIGNMENT_LEADING;
        unsafe {
            let pill_music = v.music.filter(|_| v.live);
            // The idle robot fades the clock and battery out as its eyes fade in.
            let robot_a = v.robot.map_or(0.0, |e| e.alpha);
            let pill_a = small_a;
            let small_a = small_a * (1.0 - robot_a);
            text(v.time, F_SMALL, rect(l, 0.0, r, nh), center, grey(1.0, small_a))?;
            if let Some(b) = &v.battery {
                // A battery ring left of the time; with music the cover takes that spot, so it
                // sits before the bars.
                let d = 22.0 * s;
                let bx = if pill_music.is_some() { r - 61.0 * s } else { l + 13.0 * s };
                self.draw_battery_ring((bx + 10.0 * s, nh / 2.0), d, b, small_a)?;
            }
            // Weather: icon and temperature in the room the clock leaves, to its right on the plain
            // pill (balancing the battery), after the cover on the music pill. Left out
            // when it would touch the clock.
            if let (Some((ic, geoms)), Some(n)) = (&weather_line, v.weather.now) {
                let (icon, gap) = (14.0 * s, 2.0 * s);
                let clock_half = width(v.time, F_SMALL) / 2.0 + 3.0 * s;
                // Where the icon and `label` shrunk by `k` go, and the room left between them and the clock.
                let place = |label: &str, k: f32| {
                    let total = icon + gap + width(label, F_LABEL) * k;
                    if pill_music.is_some() {
                        let left = l + 38.0 * s;
                        (left, total, cx - clock_half - (left + total))
                    } else {
                        let left = r - 11.0 * s - total;
                        (left, total, left - (cx + clock_half))
                    }
                };
                // "−12°" is wider than "29°": it keeps its degree sign by shrinking a little (down
                // to 80%); only a number that still does not fit loses the sign, not the weather.
                let number = v.unit.number(n.temp);
                let with_sign = format!("{number}°");
                let tries = [(&with_sign, 1.0), (&with_sign, 0.9), (&with_sign, 0.8), (&number, 1.0)];
                let fitted = tries.into_iter().map(|(label, k)| (place(label, k), label, k)).find(|((_, _, room), _, _)| *room >= 0.0);
                if let Some(((left, _, _), label, k)) = fitted {
                    wx_icon(ic, geoms, left, (nh - icon) / 2.0, icon, None, small_a * 0.9, 9.0)?;
                    let (tx, ty) = (left + icon + gap, nh / 2.0);
                    let base = { let mut m = Matrix3x2::default(); rt.GetTransform(&mut m); m };
                    rt.SetTransform(&(translate(-tx, -ty) * scaling(k) * translate(tx, ty) * base));
                    text(label, F_LABEL, rect(tx, 0.0, tx + width(label, F_LABEL) + 6.0 * s, nh), lead, grey(0.9, small_a))?;
                    rt.SetTransform(&base);
                }
            }
            if pill_music.is_some() {
                // Live pill: cover on the left, equaliser on the right.
                let c = 20.0 * s;
                let top = (nh - c) / 2.0;
                cover(rect(l + 10.0 * s, top, l + 10.0 * s + c, top + c), 5.0 * s, small_a)?;
                let bw = 2.0 * s;
                for i in 0..4 {
                    let k = i as f32;
                    let amp = bars_level * (0.5 + 0.5 * (v.phase * (6.0 + k * 1.7) + k * 1.3).sin());
                    let bh = (4.0 + 10.0 * amp) * s;
                    let bx = r - 14.0 * s - (4.0 - k) * (bw + 2.0 * s);
                    let my = nh / 2.0;
                    rt.FillRoundedRectangle(&rounded(rect(bx, my - bh / 2.0, bx + bw, my + bh / 2.0), bw / 2.0), &self.brush(grey(1.0, small_a * 0.9))?);
                }
            }
            if let Some(e) = v.robot.filter(|e| e.alpha > 0.0) {
                self.draw_face(&e, cx, nh / 2.0, s, pill_a * e.alpha)?;
            }
            // Pomodoro: the pill is too full for another mark, so a thin line along its bottom fills as the segment runs
            // (dimmer while paused part-way). Like the approval banner's countdown it is cut to the outline, so its
            // start curves up with the corner.
            let done = v.focus.fill(v.focus.seg, std::time::Instant::now());
            if v.focus.running() || done > 0.0 {
                let ink = if v.focus.kind() == focus::Kind::Focus { CORAL } else { GREEN };
                let top = nh - FOCUS_LINE_H * s;
                let params = D2D1_LAYER_PARAMETERS { contentBounds: rect(l, top, r, nh), geometricMask: std::mem::ManuallyDrop::new(Some(shape.clone().into())),
                    maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, maskTransform: Matrix3x2::identity(), opacity: 1.0,
                    opacityBrush: std::mem::ManuallyDrop::new(None), layerOptions: D2D1_LAYER_OPTIONS_NONE };
                rt.PushLayer(&params, None);
                rt.FillRectangle(&rect(l, top, l + (r - l) * done.max(0.01), nh), &self.brush(color(ink.0, ink.1, ink.2, pill_a * if v.focus.running() { 1.0 } else { 0.5 }))?);
                rt.PopLayer();
                drop(std::mem::ManuallyDrop::into_inner(params.geometricMask));
            }
        }
        Ok(())
    }
}

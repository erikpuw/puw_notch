use super::*;

impl Renderer {
    pub(super) fn draw_focus(
        &self,
        text: &dyn Fn(&str, usize, D2D_RECT_F, DWRITE_TEXT_ALIGNMENT, D2D1_COLOR_F) -> Result<()>,
        micon: &dyn Fn(&'static str, &'static str, usize, D2D_RECT_F, D2D1_COLOR_F) -> Result<()>,
        wx_icon: &dyn Fn(&icons::Icon, &[ID2D1PathGeometry], f32, f32, f32, Option<((f32, f32, f32), (f32, f32, f32))>, f32, f32) -> Result<()>,
        icons: &[(&'static icons::Icon, Vec<ID2D1PathGeometry>)],
        btns: &[(Button, D2D_RECT_F)],
        focus: &Focus,
        minute_of_day: u32,
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
            let f = focus;
            let now = std::time::Instant::now();
            let kc = |k: focus::Kind| if k == focus::Kind::Focus { CORAL } else { GREEN };
            let running = f.running();
            let cw_l = (cr - cl) / s;
            text(&focus::mmss(f.remaining(now)), F_TIME, rect(cl, 32.0 * s, cl + 200.0 * s, 88.0 * s), lead, grey(1.0, a))?;
            for kind in [focus::Kind::Focus, focus::Kind::Short, focus::Kind::Long] {
                let Some(cb) = rect_of(Button::FocusMode(kind)) else { continue };
                let on = f.kind() == kind;
                let kcol = kc(kind);
                let hot = hovered == Some(Button::FocusMode(kind));
                let fill = if on { color(kcol.0 * 0.22, kcol.1 * 0.22, kcol.2 * 0.22, a) } else { grey(if hot { 0.16 } else { 0.086 }, a) };
                rt.FillRoundedRectangle(&rounded(cb, (cb.bottom - cb.top) / 2.0), &self.brush(fill)?);
                text(focus_chip_label(kind), F_LABEL, cb, center, if on { solid(kcol, a) } else { grey(if hot { 0.85 } else { 0.6 }, a) })?;
            }
            if let (Some(rb), Some(pb), Some(nb)) = (rect_of(Button::FocusReset), rect_of(Button::FocusPlay), rect_of(Button::FocusSkip)) {
                for (b, br, which) in [(Button::FocusReset, rb, 0), (Button::FocusSkip, nb, 1)] {
                    let Some((ic, geoms)) = icons.get(which) else { continue };
                    let size = 17.0 * s;
                    let g = if hovered == Some(b) { 1.0 } else { 0.78 };
                    let (cx0, cy0) = ((br.left + br.right) / 2.0, (br.top + br.bottom) / 2.0);
                    wx_icon(ic, geoms, cx0 - size / 2.0, cy0 - size / 2.0, size, Some(((g, g, g), (g, g, g))), a, anim::ICON_MOTION_S)?;
                }
                let fill = if hovered == Some(Button::FocusPlay) { grey(0.85, a) } else { grey(1.0, a) };
                let disc = D2D1_ELLIPSE { point: Vector2 { X: (pb.left + pb.right) / 2.0, Y: (pb.top + pb.bottom) / 2.0 }, radiusX: (pb.right - pb.left) / 2.0, radiusY: (pb.bottom - pb.top) / 2.0 };
                rt.FillEllipse(&disc, &self.brush(fill)?);
                let nudge = if running { 0.0 } else { 1.5 * s };
                micon("play", if running { ICON_PAUSE } else { ICON_PLAY }, F_ICON, rect(pb.left + nudge, pb.top, pb.right + nudge, pb.bottom), grey(0.05, a))?;
            }
            for (i, (x0, x1)) in focus_segments(cw_l).into_iter().enumerate() {
                let kcol = kc(focus::CYCLE[i]);
                let hot = hovered == Some(Button::FocusSeg(i));
                let seg = rect(cl + x0 * s, FOCUS_STRIP_Y * s, cl + x1 * s, (FOCUS_STRIP_Y + FOCUS_STRIP_H) * s);
                let radius = FOCUS_STRIP_H * s / 2.0;
                let k = if hot { 0.34 } else { 0.22 };
                rt.FillRoundedRectangle(&rounded(seg, radius), &self.brush(color(kcol.0 * k, kcol.1 * k, kcol.2 * k, a))?);
                let done = f.fill(i, now);
                if done > 0.0 {
                    let w = ((x1 - x0) * done * s).max(FOCUS_STRIP_H * s);
                    rt.FillRoundedRectangle(&rounded(rect(seg.left, seg.top, seg.left + w, seg.bottom), radius), &self.brush(solid(kcol, a))?);
                }
            }
            let left_min = f.cycle_left(now).as_secs().div_ceil(60) as u32;
            let ends = health::hhmm((minute_of_day + left_min) % 1440);
            let left_txt = if left_min >= 60 {
                if crate::i18n::english() { format!("{} h {} min left", left_min / 60, left_min % 60) } else { format!("còn {} giờ {}", left_min / 60, left_min % 60) }
            } else if crate::i18n::english() { format!("{left_min} min left") } else { format!("còn {left_min} phút") };
            let round = if crate::i18n::english() { format!("Round {} of 4", f.round()) } else { format!("Vòng {} trên 4", f.round()) };
            let foot = rect(cl, (FOCUS_STRIP_Y + 13.0) * s, cr, (FOCUS_STRIP_Y + 29.0) * s);
            text(&round, F_LABEL, foot, lead, grey(0.55, a))?;
            let end_txt = if crate::i18n::english() { format!("Done at {ends} · {left_txt}") } else { format!("Xong lúc {ends} · {left_txt}") };
            text(&end_txt, F_LABEL, foot, trail, grey(0.55, a))?;
        }
        Ok(())
    }
}

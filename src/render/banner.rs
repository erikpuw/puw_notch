//! A banner, drawn by its kind: a status (an icon and its line), a notification (an app's message), an approval (a question with buttons and a countdown). Returns whether an icon is still moving.

use super::*;

impl Renderer {
    pub(super) fn draw_banner(
        &self,
        c: &Ctx,
        t: &Toast,
        banner_line: &Option<(&'static icons::Icon, Vec<ID2D1PathGeometry>)>,
        banner_image: &Option<ID2D1Bitmap>,
    ) -> Result<bool> {
        let rt = &self.rt;
        let Ctx { v, s, l, r, cx, nh, shape, text, micon, .. } = *c;
        let center = DWRITE_TEXT_ALIGNMENT_CENTER;
        let lead = DWRITE_TEXT_ALIGNMENT_LEADING;
        let trail = DWRITE_TEXT_ALIGNMENT_TRAILING;
        let mut icon_busy = false;
        let fade = |from: f32, span: f32| ((nh - from * s) / (span * s)).clamp(0.0, 1.0);
        unsafe {
            let a = fade(30.0, 18.0);
            let kind = t.kind();
            // Icon on the left, text beside it. Status and device icons, and a notification's
            // bell, are line icons whose parts move for a moment after the banner appears
            // (icons.rs); the water drop hops.
            let d = 36.0 * s;
            // A question's icon sits level with its text (app name down to the last line), not the
            // banner with its buttons.
            let icon_mid = match kind {
                BannerKind::Approval => (10.0 + v.size[1] - 42.0) / 2.0 * s,
                BannerKind::Status => nh / 2.0,
                BannerKind::Notification => nh / 2.0 + 2.0 * s,
            };
            let (ix, iy) = (l + 18.0 * s, icon_mid - d / 2.0);
            let icon = t.icon.unwrap_or(ICON_BELL);
            let (mx, my) = (ix + d / 2.0, iy + d / 2.0);
            let base = { let mut b = Matrix3x2::default(); rt.GetTransform(&mut b); b };
            let age = v.banner_age;
            let (pop_scale, pop_alpha) = icons::pop(age);
            let pop = translate(-mx, -my) * scaling(pop_scale) * translate(mx, my);
            if let Some((ic, geoms)) = &banner_line {
                icon_busy = age < anim::ICON_MOTION_S;
                let size = LINE_ICON * s;
                let view = scaling(size / ic.vb) * translate(mx - size / 2.0, my - size / 2.0);
                let groups = icons::motion(ic.anim, age);
                for (part, geo) in ic.parts.iter().zip(geoms) {
                    let g = groups.get(part.g as usize).copied().unwrap_or(icons::Motion::STILL);
                    let (ox, oy) = ic.origins.get(part.g as usize).copied().unwrap_or((0.0, 0.0));
                    let local = translate(-ox, -oy) * rotation(g.rot) * scaling(g.scale) * translate(ox + g.dx, oy + g.dy);
                    let alpha = a * pop_alpha * g.alpha * groups.get(part.a as usize).map_or(1.0, |m| m.alpha);
                    rt.SetTransform(&(local * view * pop * base));
                    rt.DrawGeometry(geo, &self.brush(grey(1.0, alpha))?, ic.sw, &self.round);
                }
                rt.SetTransform(&base);
            } else if icon == ICON_DROP {
                icon_busy = age < anim::ICON_MOTION_S;
                let hop = icons::drop_motion(age).dy * (LINE_ICON * s / 24.0);
                rt.SetTransform(&(pop * translate(0.0, hop) * base));
                let g = self.water_drop(mx, my, 24.0 * s)?;
                rt.FillGeometry(&g, &self.brush(color(BLUE.0, BLUE.1, BLUE.2, a * pop_alpha))?, None);
                rt.SetTransform(&base);
            } else {
                micon("banner", icon, F_BANNER_ICON, rect(ix, iy, ix + d, iy + d), grey(1.0, a))?;
            }
            let tl = ix + d + 12.0 * s;
            let mut tr = r - 22.0 * s;
            match kind {
                // A status banner (the icon says what it is) is just its lines, centred on the icon.
                BannerKind::Status => {
                    // A badge (a Bluetooth device's battery) stands large at the right end; text stops before it.
                    if let Some(b) = &t.badge {
                        text(b, F_BADGE, rect(tr - 80.0 * s, 0.0, tr, nh + 4.0 * s), trail, grey(1.0, a))?;
                        tr -= 86.0 * s;
                    }
                    let mid = nh / 2.0;
                    let (title_top, body_top, bar_y) = if t.level.is_some() {
                        (mid - 15.0 * s, 0.0, mid + 9.0 * s)
                    } else if t.body.is_empty() {
                        (mid - 9.0 * s, 0.0, 0.0)
                    } else {
                        (mid - 17.0 * s, mid + 1.0 * s, 0.0)
                    };
                    text(&t.title, F_TITLE, rect(tl, title_top, tr, title_top + 18.0 * s), lead, grey(1.0, a))?;
                    match t.level {
                        Some(level) => {
                            let th = 5.0 * s;
                            let bar = |right: f32| rounded(rect(tl, bar_y - th / 2.0, right, bar_y + th / 2.0), th / 2.0);
                            rt.FillRoundedRectangle(&bar(tr), &self.brush(grey(0.25, a))?);
                            rt.FillRoundedRectangle(&bar(tl + (tr - tl) * level.clamp(0.0, 1.0)), &self.brush(grey(1.0, a))?);
                        }
                        // One line under the title.
                        None => text(&t.body, F_BODY, rect(tl, body_top, tr, body_top + 16.0 * s), lead, grey(0.75, a))?,
                    }
                }
                // A notification names its app above the title and wraps its text over the banner's height.
                BannerKind::Notification => {
                    if t.image.is_some() {
                        // The picture sits at the right edge, centred on the banner; text ends before it.
                        let (bw, bh) = (BANNER_IMAGE_W * s, BANNER_IMAGE_H * s);
                        let (bx, by) = (r - 18.0 * s - bw, nh / 2.0 - bh / 2.0 + 2.0 * s);
                        tr = bx - BANNER_IMAGE_GAP * s;
                        match &banner_image {
                            Some(img) => {
                                let size = img.GetSize();
                                let k = (bw / size.width).min(bh / size.height);
                                let (w, h) = (size.width * k, size.height * k);
                                let (x, y) = (bx + (bw - w) / 2.0, by + (bh - h) / 2.0);
                                let brush = rt.CreateBitmapBrush(img, None, None)?;
                                brush.SetTransform(&(scaling(k) * translate(x, y)));
                                brush.SetOpacity(a);
                                rt.FillRoundedRectangle(&rounded(rect(x, y, x + w, y + h), 5.0 * s), &brush);
                            }
                            // Still loading (or gone): a quiet placeholder of the same size.
                            None => rt.FillRoundedRectangle(&rounded(rect(bx, by, bx + bw, by + bh), 5.0 * s), &self.brush(grey(0.2, a))?),
                        }
                    }
                    if let Some(b) = &t.badge {
                        text(b, F_BADGE, rect(tr - 80.0 * s, 0.0, tr, nh + 4.0 * s), trail, grey(1.0, a))?;
                        tr -= 86.0 * s;
                    }
                    // A notification a click opens gets an x to close it early. Status
                    // banners have none: resting on them opens the panel within a moment.
                    if t.image.is_some() || t.open.is_some() {
                        let b = banner_close(cx, s, v.size[0], self.style == notch::Style::Pill);
                        let (x, y, k) = ((b.left + b.right) / 2.0, (b.top + b.bottom) / 2.0, 4.0 * s);
                        if self.close_hot {
                            rt.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: x, Y: y }, radiusX: 10.0 * s, radiusY: 10.0 * s }, &self.brush(grey(0.22, a))?);
                        }
                        let ink = self.brush(grey(if self.close_hot { 0.95 } else { 0.55 }, a))?;
                        rt.DrawLine(Vector2 { X: x - k, Y: y - k }, Vector2 { X: x + k, Y: y + k }, &ink, 1.5 * s, &self.round);
                        rt.DrawLine(Vector2 { X: x - k, Y: y + k }, Vector2 { X: x + k, Y: y - k }, &ink, 1.5 * s, &self.round);
                    }
                    text(&t.app, F_LABEL, rect(tl, 8.0 * s, tr, 24.0 * s), lead, grey(0.6, a))?;
                    text(&t.title, F_TITLE, rect(tl, 24.0 * s, tr, 42.0 * s), lead, grey(1.0, a))?;
                    // Wraps over the banner's height (sized to its lines, at most `BANNER_MAX_LINES`);
                    // what still does not fit ends with "…".
                    let top = notch::BANNER_TEXT_TOP * s;
                    let bottom = (nh - 7.0 * s).max(top + notch::BANNER_LINE_H * s);
                    text(&t.body, F_BODY_WRAP, rect(tl, top, tr, bottom), lead, grey(0.75, a))?;
                }
                // An approval waits `approve::BANNER`: a bright bar along the very bottom edge runs
                // down meanwhile, and when it is gone the question goes back to its app.
                BannerKind::Approval => {
                    let left = (1.0 - age / approve::BANNER.as_secs_f32()).clamp(0.0, 1.0);
                    icon_busy |= left > 0.0;
                    if left > 0.0 {
                        // Green, then yellow and red as the time runs out.
                        let mix = |p: (f32, f32, f32), q: (f32, f32, f32), k: f32| (p.0 + (q.0 - p.0) * k, p.1 + (q.1 - p.1) * k, p.2 + (q.2 - p.2) * k);
                        let c = if left > 0.45 { GREEN } else if left > 0.25 { mix(YELLOW, GREEN, (left - 0.25) / 0.2) } else { mix((1.0, 0.3, 0.25), YELLOW, (left / 0.25).min(1.0)) };
                        // The bar is as wide as the banner and cut to its outline, so its ends curve up with the corners.
                        let top = nh - COUNTDOWN_H * s;
                        let params = D2D1_LAYER_PARAMETERS { contentBounds: rect(l, top, r, nh), geometricMask: std::mem::ManuallyDrop::new(Some(shape.clone().into())),
                            maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, maskTransform: Matrix3x2::identity(), opacity: 1.0,
                            opacityBrush: std::mem::ManuallyDrop::new(None), layerOptions: D2D1_LAYER_OPTIONS_NONE };
                        rt.PushLayer(&params, None);
                        rt.FillRectangle(&rect(l, top, l + (r - l) * left, nh), &self.brush(color(c.0, c.1, c.2, a))?);
                        rt.PopLayer();
                        drop(std::mem::ManuallyDrop::into_inner(params.geometricMask));
                    }
                    for (k, (b, label)) in approve_buttons(cx, s, v.size[0], v.size[1], &t.choices).into_iter().zip(&t.choices).enumerate() {
                        // The first answer is the filled one, the others are outlined.
                        let ink = if k == 0 { color(0.02, 0.13, 0.04, a) } else { grey(1.0, a) };
                        let r = (b.bottom - b.top) / 2.0;
                        if k == 0 {
                            rt.FillRoundedRectangle(&rounded(b, r), &self.brush(color(GREEN.0, GREEN.1, GREEN.2, a))?);
                        } else {
                            let edge = 0.5 * s;
                            rt.DrawRoundedRectangle(&rounded(rect(b.left + edge, b.top + edge, b.right - edge, b.bottom - edge), r - edge), &self.brush(grey(0.28, a))?, 1.0 * s, None);
                        }
                        // The font sits a little low in its line box: lift the label by a pixel to the pill's middle.
                        text(label, F_LABEL, rect(b.left, b.top - 1.0 * s, b.right, b.bottom - 1.0 * s), center, ink)?;
                    }
                    text(&t.app, F_LABEL, rect(tl, 8.0 * s, tr, 24.0 * s), lead, grey(0.6, a))?;
                    text(&t.title, F_TITLE, rect(tl, 24.0 * s, tr, 42.0 * s), lead, grey(1.0, a))?;
                    // A question wraps over up to 3 lines, the buttons below.
                    text(&t.body, F_BODY_WRAP, rect(tl, notch::BANNER_TEXT_TOP * s, tr, (nh - 40.0 * s).max(notch::BANNER_TEXT_TOP * s + notch::BANNER_LINE_H * s)), lead, grey(0.75, a))?;
                }
            }
        }
        Ok(icon_busy)
    }
}

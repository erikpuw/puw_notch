//! A banner, drawn by its kind: a status (an icon and its line), a notification (an app's message), an approval (a question with buttons and a countdown). Returns whether an icon is still moving.

use super::*;

impl Renderer {
    pub(super) fn draw_banner(
        &self,
        c: &Ctx,
        t: &Toast,
        banner_line: &Option<(&'static icons::Icon, Vec<ID2D1PathGeometry>)>,
        banner_image: &Option<ID2D1Bitmap>,
        banner_app: &Option<ID2D1Bitmap>,
    ) -> Result<bool> {
        let rt = &self.rt;
        let Ctx { v, s, l, r, cx, nh, shape, text, width, micon, .. } = *c;
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
            if let (BannerKind::Notification, Some(img)) = (kind, banner_app) {
                // The app's own icon in a rounded square, popping in like the line icons.
                let size = img.GetSize();
                let k = d / size.width.max(size.height);
                let brush = rt.CreateBitmapBrush(img, None, None)?;
                brush.SetTransform(&(scaling(k) * translate(ix, iy)));
                brush.SetOpacity(a * pop_alpha);
                rt.SetTransform(&(pop * base));
                rt.FillRoundedRectangle(&rounded(rect(ix, iy, ix + d, iy + d), 9.0 * s), &brush);
                rt.SetTransform(&base);
            } else if let Some((ic, geoms)) = &banner_line {
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
                // A finished task's check-in-a-ring is green; the glyph it replaces morphs into it.
                let c = match icon {
                    ICON_DONE => color(GREEN.0, GREEN.1, GREEN.2, a),
                    ICON_FAILED => color(LOW_RED.0, LOW_RED.1, LOW_RED.2, a),
                    _ => grey(1.0, a),
                };
                micon("banner", icon, F_BANNER_ICON, rect(ix, iy, ix + d, iy + d), c)?;
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
                    // With lines under it the title keeps the place it has in a one-line banner.
                    let mid = if t.rows.is_empty() { nh / 2.0 } else { 26.0 * s };
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
                    // A checklist: a green dot with a tick (it pops in as its line arrives), a thin line
                    // down to the next dot, the text beside it (the timeline of the jarvis island).
                    for (i, row) in t.rows.iter().enumerate() {
                        let top = (40.0 + notch::ROW_H * i as f32) * s;
                        let age = row.at.elapsed().as_secs_f32();
                        icon_busy |= age < 0.4;
                        let (k, alpha) = icons::pop(age);
                        let (dx, dy) = (tl + 7.5 * s, top + 10.5 * s);
                        let ra = a * alpha;
                        if let Some(glyph) = row.icon {
                            // A device: its icon, its name, its battery at the right end (red when low). One still
                            // to connect is grey, with a spinning ring where its battery will show.
                            let waiting = row.state == RowState::Wait;
                            let (icon_ink, name_ink) = if waiting { (grey(0.45, ra), color(0.58, 0.64, 0.72, ra * 0.8)) } else { (grey(1.0, ra), grey(0.9, ra)) };
                            text(glyph, F_ICON, rect(tl - 1.0 * s, top + 2.0 * s, tl + 17.0 * s, top + 20.0 * s), center, icon_ink)?;
                            let end = if row.value.is_some() || waiting { tr - 52.0 * s } else { tr };
                            text(&row.text, F_BODY, rect(tl + ROW_DOT_W * s, top + 3.0 * s, end, top + 19.0 * s), lead, name_ink)?;
                            if waiting {
                                icon_busy = true;
                                self.draw_spinner((tr - 9.0 * s, top + 11.0 * s), 5.5 * s, 1.5 * s, age, ra)?;
                            } else if let Some(v) = &row.value {
                                let ink = if row.low { color(LOW_RED.0, LOW_RED.1, LOW_RED.2, ra) } else { grey(1.0, ra) };
                                text(v, F_TITLE, rect(tr - 50.0 * s, top + 2.0 * s, tr, top + 20.0 * s), trail, ink)?;
                            }
                            continue;
                        }
                        if i + 1 < t.rows.len() {
                            let hue = if row.state == RowState::Done { color(GREEN.0, GREEN.1, GREEN.2, ra * 0.35) } else { color(0.58, 0.64, 0.72, ra * 0.2) };
                            rt.DrawLine(Vector2 { X: dx, Y: dy + 9.5 * s }, Vector2 { X: dx, Y: top + (notch::ROW_H + 1.0) * s }, &self.brush(hue)?, 1.5 * s, &self.round);
                        }
                        let r = 7.5 * s;
                        let ring = D2D1_ELLIPSE { point: Vector2 { X: dx, Y: dy }, radiusX: r - 0.75 * s, radiusY: r - 0.75 * s };
                        let track = self.brush(color(0.58, 0.64, 0.72, ra * if row.state == RowState::Wait { 0.3 } else { 0.35 }))?;
                        match row.state {
                            RowState::Done | RowState::Failed => {
                                // A ring in the colour of the state, black inside, the mark in the same colour: a white mark on a
                                // filled disc is about 2:1 against its green and sinks; this is about 10:1.
                                let tint = if row.state == RowState::Done { GREEN } else { LOW_RED };
                                let hue = self.brush(color(tint.0, tint.1, tint.2, ra))?;
                                let ring_k = D2D1_ELLIPSE { radiusX: ring.radiusX * k, radiusY: ring.radiusY * k, ..ring };
                                rt.FillEllipse(&ring_k, &self.brush(color(0.0, 0.0, 0.0, ra))?);
                                rt.DrawEllipse(&ring_k, &hue, 1.5 * s, None);
                                let (ink, u) = (hue, 0.9 * k * s);
                                let mark: [((f32, f32), (f32, f32)); 2] = if row.state == RowState::Done {
                                    [((-3.5, 0.2), (-1.2, 2.5)), ((-1.2, 2.5), (3.5, -2.5))]
                                } else {
                                    [((-2.8, -2.8), (2.8, 2.8)), ((-2.8, 2.8), (2.8, -2.8))]
                                };
                                for ((x0, y0), (x1, y1)) in mark {
                                    rt.DrawLine(Vector2 { X: dx + x0 * u, Y: dy + y0 * u }, Vector2 { X: dx + x1 * u, Y: dy + y1 * u }, &ink, 1.7 * s, &self.round);
                                }
                            }
                            RowState::Todo => rt.DrawEllipse(&ring, &track, 1.5 * s, None),
                            RowState::Wait => {
                                icon_busy = true;
                                self.draw_spinner((dx, dy), r - 0.75 * s, 1.5 * s, age, ra)?;
                            }
                        }
                        let ink = match row.state {
                            RowState::Todo => grey(0.6, ra),
                            RowState::Failed => color(1.0, 0.65, 0.65, ra),
                            _ => grey(0.88, ra),
                        };
                        text(&row.text, F_BODY, rect(tl + ROW_DOT_W * s, top + 3.0 * s, tr, top + 19.0 * s), lead, ink)?;
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
                    // The time it arrived, large and bold at the right of the title row (a banner with a picture has no room).
                    let mut head = tr;
                    if t.image.is_none() {
                        let w = width(v.time, F_CLOCK_BOLD) + 14.0 * s;
                        text(v.time, F_CLOCK_BOLD, rect(tr - w, 24.0 * s, tr, 42.0 * s), trail, grey(1.0, a))?;
                        head = tr - w;
                    }
                    text(&t.app, F_LABEL, rect(tl, 8.0 * s, head, 24.0 * s), lead, grey(0.6, a))?;
                    text(&t.title, F_TITLE, rect(tl, 24.0 * s, head, 42.0 * s), lead, grey(1.0, a))?;
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

impl Renderer {
    /// A faint ring with a quarter of it turning (0.85 s a turn): something is being waited for.
    fn draw_spinner(&self, (x, y): (f32, f32), radius: f32, width: f32, age: f32, a: f32) -> Result<()> {
        let rt = &self.rt;
        let ring = D2D1_ELLIPSE { point: Vector2 { X: x, Y: y }, radiusX: radius, radiusY: radius };
        let head = age / 0.85 * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
        let step = std::f32::consts::FRAC_PI_2 / 6.0;
        let on_ring = |a: f32| Vector2 { X: x + radius * a.cos(), Y: y + radius * a.sin() };
        unsafe {
            rt.DrawEllipse(&ring, &self.brush(color(0.58, 0.64, 0.72, a * 0.3))?, width, None);
            let arc = self.brush(color(BLUE.0, BLUE.1, BLUE.2, a))?;
            for j in 0..6 {
                let a0 = head + j as f32 * step;
                rt.DrawLine(on_ring(a0), on_ring(a0 + step), &arc, width * 1.07, &self.round);
            }
        }
        Ok(())
    }
}

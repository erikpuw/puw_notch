//! The Music page: the cover, the song and artist, the seek bar and the controls; the lyrics column beside them when open.

use super::*;

impl Renderer {
    pub(super) fn draw_music(
        &self,
        text: &dyn Fn(&str, usize, D2D_RECT_F, DWRITE_TEXT_ALIGNMENT, D2D1_COLOR_F) -> Result<()>,
        micon: &dyn Fn(&'static str, &'static str, usize, D2D_RECT_F, D2D1_COLOR_F) -> Result<()>,
        draw_cover: &dyn Fn(D2D_RECT_F, f32, f32) -> Result<()>,
        wx_icon: &dyn Fn(&icons::Icon, &[ID2D1PathGeometry], f32, f32, f32, Option<((f32, f32, f32), (f32, f32, f32))>, f32, f32) -> Result<()>,
        player_icons: &[(&'static icons::Icon, Vec<ID2D1PathGeometry>)],
        fonts: &[IDWriteTextFormat],
        lyric_heights: &[f32],
        lyric_tops: &[f32],
        v: &View,
        l: f32,
        tint: (f32, f32, f32),
        level: f32,
        cl: f32,
        s: f32,
        a: f32,
        hovered: Option<Button>,
    ) -> Result<()> {
        let rt = &self.rt;
        let (center, lead, trail) = (DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_ALIGNMENT_TRAILING);
        unsafe {
            let top = COVER_TOP * s;
            // The player is the left 480 px of the panel (the standard width), wider or not.
            let cr = l + (notch::target(Mode::Expanded, false)[0] - SIDE) * s;
            draw_cover(rect(cl, top, cl + COVER * s, top + COVER * s), 14.0 * s, a)?;
            let tl = cl + (COVER + COVER_GAP) * s;
            match v.music {
                Some(t) => {
                    let title = if t.title.is_empty() { tr("Đang phát", "Playing") } else { &t.title };
                    text(title, F_SONG, rect(tl, top - 2.0 * s, cr, top + 28.0 * s), lead, grey(1.0, a))?;
                    text(&t.artist, F_BODY, rect(tl, top + 25.0 * s, cr, top + 42.0 * s), lead, soft_text(tint, level, 0.65, a))?;
                    // The line being sung, unless the lyrics panel beside already shows it.
                    let lyric = t.lyric_now().filter(|_| !v.lyrics).unwrap_or_else(|| hovered.map_or(
                        if t.duration_ms == 0 { tr("Trình phát chưa cung cấp thời gian", "The player gives no timeline") }
                        else if t.lyrics.is_empty() || v.lyrics { "" } else { "♪" }, Button::label));
                    text(t.command_error.unwrap_or(lyric), F_BODY, rect(tl, top + 42.0 * s, cr, top + 60.0 * s), lead,
                        if t.command_error.is_some() { color(1.0, 0.5, 0.4, a) } else { grey(0.95, a) })?;
                }
                None => text(tr("Không có bài", "Nothing playing"), F_TITLE, rect(tl, top, cr, top + 40.0 * s), lead, grey(1.0, a))?,
            }
            let pos = v.music.map_or(0, |t| t.position_ms());
            let dur = v.music.map_or(0, |t| t.duration_ms);
            let by = BAR_Y * s;
            let (bl, brr) = (tl, cr);
            let times = soft_text(tint, level, 0.6, a);
            text(&if dur > 0 { clock(pos) } else { "--:--".into() }, F_LABEL, rect(bl, by + 4.0 * s, bl + 60.0 * s, by + 20.0 * s), lead, times)?;
            text(&if dur > 0 { format!("-{}", clock(dur.saturating_sub(pos))) } else { "--:--".into() }, F_LABEL, rect(brr - 60.0 * s, by + 4.0 * s, brr, by + 20.0 * s), trail, times)?;
            let th = 6.0 * s;
            let bar = |right: f32| rounded(rect(bl, by - th / 2.0, right, by + th / 2.0), th / 2.0);
            rt.FillRoundedRectangle(&bar(brr), &self.brush({ let g = 0.25 + 0.75 * level; color(g, g, g, a * (1.0 - 0.7 * level)) })?);
            if dur > 0 {
                let p = (pos as f32 / dur as f32).clamp(0.0, 1.0);
                rt.FillRoundedRectangle(&bar(bl + (brr - bl) * p), &self.brush(grey(1.0, a))?);
                if Button::Seek.enabled(v.music) {
                    let radius = if hovered == Some(Button::Seek) { 5.0 } else { 3.0 } * s;
                    rt.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: bl + (brr - bl) * p, Y: by }, radiusX: radius, radiusY: radius }, &self.brush(grey(1.0, a))?);
                }
            }
            let playing = v.music.is_some_and(|t| t.playing);
            for (b, br) in buttons_from(l / s, s) {
                let on = b.enabled(v.music);
                if b == Button::Lyrics {
                    // A speech bubble with two lines; filled while the lyrics are open.
                    let (bx, by) = ((br.left + br.right) / 2.0, (br.top + br.bottom) / 2.0);
                    let bubble = rect(bx - 9.0 * s, by - 7.0 * s, bx + 9.0 * s, by + 7.0 * s);
                    let ink = if !on { grey(0.3, a) } else if v.lyrics { grey(0.05, a) } else if hovered == Some(b) { color(0.25, 0.85, 0.5, a) } else { grey(1.0, a) };
                    if on && v.lyrics {
                        rt.FillRoundedRectangle(&rounded(bubble, 4.0 * s), &self.brush(grey(1.0, a))?);
                    } else {
                        rt.DrawRoundedRectangle(&rounded(bubble, 4.0 * s), &self.brush(ink)?, 1.6 * s, None);
                    }
                    for (k, len) in [10.0, 6.0].into_iter().enumerate() {
                        let y = by - 2.2 * s + k as f32 * 4.4 * s;
                        rt.DrawLine(Vector2 { X: bx - 5.0 * s, Y: y }, Vector2 { X: bx - 5.0 * s + len * s, Y: y }, &self.brush(ink)?, 1.5 * s, &self.round);
                    }
                    continue;
                }
                if b == Button::PlayPause {
                    // The main action: a white disc with a dark glyph.
                    let icon = if playing { ICON_PAUSE } else { ICON_PLAY };
                    let fill = if !on { grey(0.25, a) } else if hovered == Some(b) { grey(0.85, a) } else { grey(1.0, a) };
                    let disc = D2D1_ELLIPSE { point: Vector2 { X: (br.left + br.right) / 2.0, Y: (br.top + br.bottom) / 2.0 },
                        radiusX: (br.right - br.left) / 2.0, radiusY: (br.bottom - br.top) / 2.0 };
                    rt.FillEllipse(&disc, &self.brush(fill)?);
                    // A triangle's weight sits left of its box: the play glyph moves 1.5 px right so
                    // it looks centred in the disc (the pause bars need no shift).
                    let nudge = if playing { 0.0 } else { 1.5 * s };
                    micon("play", icon, F_ICON, rect(br.left + nudge, br.top, br.right + nudge, br.bottom), if on { grey(0.05, a) } else { grey(0.5, a) })?;
                    continue;
                }
                // Previous, next, shuffle and repeat are plain Tabler line icons (no disc): shuffle and repeat dimmer at
                // rest, green while they are on (no other mark), white under the mouse.
                let (which, active) = match b {
                    Button::Shuffle => (0, v.music.is_some_and(|t| t.shuffle)),
                    Button::Previous => (1, false),
                    Button::Next => (2, false),
                    Button::Repeat => (if v.music.is_some_and(|t| t.repeat == Repeat::One) { 4 } else { 3 }, v.music.is_some_and(|t| t.repeat != Repeat::Off)),
                    _ => continue,
                };
                let Some((ic, geoms)) = player_icons.get(which) else { continue };
                let ((r, g, bl), alpha) = if !on { ((1.0, 1.0, 1.0), 0.3) } else if active { ((0.25, 0.85, 0.5), 1.0) }
                    else if hovered == Some(b) || !matches!(b, Button::Shuffle | Button::Repeat) { ((1.0, 1.0, 1.0), 1.0) } else { ((0.78, 0.78, 0.78), 1.0) };
                let size = 17.0 * s;
                let (cx0, cy0) = ((br.left + br.right) / 2.0, (br.top + br.bottom) / 2.0);
                wx_icon(ic, geoms, cx0 - size / 2.0, cy0 - size / 2.0, size, Some(((r, g, bl), (r, g, bl))), a * alpha, icons::DUR)?;
            }
            // Lyrics beside the player, fading in as the panel widens: the line being
            // sung is bright and centred, the others dim; the whole column glides up
            // as the song moves on. Long lines wrap onto more lines.
            let player_w = notch::target(Mode::Expanded, false)[0];
            let grow = ((v.size[0] - player_w) / LYRICS_EXTRA_W).clamp(0.0, 1.0);
            if grow > 0.02 {
                let appear = grow * grow * (3.0 - 2.0 * grow);
                // Right after the controls, past a thin divider.
                let divider = l + (player_w - SIDE + LYRIC_GAP) * s;
                let (x0, y0, y1) = (divider + LYRIC_GAP * s, LYRIC_TOP * s, LYRIC_BOTTOM * s);
                let x1 = x0 + LYRIC_W * s;
                // Fading out at both ends.
                let peak = 0.18 * a * appear;
                let stops = rt.CreateGradientStopCollection(&[
                    D2D1_GRADIENT_STOP { position: 0.0, color: grey(1.0, 0.0) },
                    D2D1_GRADIENT_STOP { position: 0.5, color: grey(1.0, peak) },
                    D2D1_GRADIENT_STOP { position: 1.0, color: grey(1.0, 0.0) },
                ], D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP)?;
                let down = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES { startPoint: Vector2 { X: divider, Y: y0 }, endPoint: Vector2 { X: divider, Y: y1 } };
                rt.FillRectangle(&rect(divider, y0, divider + 1.0 * s, y1), &rt.CreateLinearGradientBrush(&down, None, &stops)?);
                match v.music {
                    Some(t) if !lyric_heights.is_empty() => {
                        let phase = self.lyric_motion.phase();
                        let focus = self.lyric_motion.focus();
                        let middle = (y0 + y1) / 2.0;
                        for (j, (_, line)) in t.lyrics.iter().enumerate() {
                            let (top, h) = (middle + lyric_tops[j] - focus, lyric_heights[j]);
                            if top + h < y0 || top > y1 {
                                continue;
                            }
                            let alpha = a * appear * lyrics::edge_fade(top + h / 2.0, y0, y1, 28.0 * s) * lyrics::line_alpha(j, phase);
                            if alpha > 0.01 {
                                // Not clipped to the box: a line wraps to fit by the letters' advance,
                                // and its last letter's ink ("y", a tone mark) can stick out past it.
                                let units: Vec<u16> = line.encode_utf16().collect();
                                fonts[F_LYRIC].SetTextAlignment(lead)?;
                                rt.DrawText(&units, &fonts[F_LYRIC], &rect(x0, top, x1, top + h), &self.brush(soft_text(tint, level, 1.0, alpha))?, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
                            }
                        }
                    }
                    _ => text(tr("Không có lời cho bài này", "No lyrics for this song"), F_BODY, rect(x0, y0, x1, y1), center, grey(0.6, a * appear))?,
                }
            }
        }
        Ok(())
    }
}

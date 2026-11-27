use super::*;
use crate::settings_win::{nav_rects, Action, Item, Placed, Rect, Section, CLOSE, CW, CX, HEIGHT, NAV_W, WIDTH};

const ICON_CLOSE: &str = "\u{E711}";

pub struct SettingsView<'a> {
    pub section: Section,
    pub placed: &'a [Placed],
    pub hovered: Option<Action>,
    pub title: &'a str,
    pub hint: &'a str,
    pub version: &'a str,
}

fn scaled(q: Rect, s: f32) -> D2D_RECT_F {
    rect(q.x * s, q.y * s, (q.x + q.w) * s, (q.y + q.h) * s)
}

impl Renderer {
    pub fn present_settings(&mut self, hwnd: HWND, w: i32, h: i32, s: f32, v: &SettingsView) -> Result<()> {
        self.ensure_surface(hwnd, w, h)?;
        let fonts = self.fonts(s)?;
        let rt = self.rt.clone();
        let (center, lead) = (DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING);
        unsafe {
            let text = |t: &str, f: usize, b: D2D_RECT_F, align: DWRITE_TEXT_ALIGNMENT, c: D2D1_COLOR_F| -> Result<()> {
                let t: Vec<u16> = t.encode_utf16().collect();
                fonts[f].SetTextAlignment(align)?;
                rt.DrawText(&t, &fonts[f], &b, &self.brush(c)?, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
                Ok(())
            };
            let fill = |b: D2D_RECT_F, radius: f32, c: D2D1_COLOR_F| -> Result<()> {
                rt.FillRoundedRectangle(&rounded(b, radius), &self.brush(c)?);
                Ok(())
            };
            let outline = |b: D2D_RECT_F, radius: f32, c: D2D1_COLOR_F| -> Result<()> {
                let inner = rect(b.left + 0.5, b.top + 0.5, b.right - 0.5, b.bottom - 0.5);
                rt.DrawRoundedRectangle(&rounded(inner, radius - 0.5), &self.brush(c)?, 1.0, None);
                Ok(())
            };
            let hot = |a: Action| v.hovered == Some(a);
            let green = |a: f32| color(GREEN.0, GREEN.1, GREEN.2, a);

            rt.BeginDraw();
            rt.Clear(Some(&color(0.0, 0.0, 0.0, 0.0)));
            let window = rect(0.0, 0.0, w as f32, h as f32);
            fill(window, 14.0 * s, grey(0.043, 1.0))?;
            outline(window, 14.0 * s, grey(0.2, 1.0))?;
            rt.FillRectangle(&rect(NAV_W * s, 1.0 * s, NAV_W * s + 1.0, (HEIGHT - 1.0) * s), &self.brush(grey(0.12, 1.0))?);

            text(crate::i18n::tr("Cài đặt", "Settings"), F_TITLE, rect(24.0 * s, 14.0 * s, (NAV_W - 8.0) * s, 40.0 * s), lead, grey(1.0, 1.0))?;
            for (section, q) in nav_rects() {
                let b = scaled(q, s);
                let active = section == v.section;
                let over = hot(Action::Go(section));
                if active {
                    fill(b, 8.0 * s, grey(0.15, 1.0))?;
                } else if over {
                    fill(b, 8.0 * s, grey(0.09, 1.0))?;
                }
                let ink = if active { grey(1.0, 1.0) } else if over { grey(0.85, 1.0) } else { grey(0.55, 1.0) };
                text(section.icon(), F_SMALL_ICON, rect(b.left + 10.0 * s, b.top, b.left + 30.0 * s, b.bottom), center, ink)?;
                text(section.title(), F_BODY, rect(b.left + 38.0 * s, b.top, b.right - 6.0 * s, b.bottom), lead, ink)?;
            }
            text(&format!("PUW Notch {}", v.version), F_LABEL, rect(24.0 * s, (HEIGHT - 34.0) * s, (NAV_W - 8.0) * s, (HEIGHT - 14.0) * s), lead, grey(0.35, 1.0))?;

            let close = scaled(CLOSE, s);
            if hot(Action::Close) {
                rt.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: (close.left + close.right) / 2.0, Y: (close.top + close.bottom) / 2.0 }, radiusX: 12.0 * s, radiusY: 12.0 * s }, &self.brush(grey(0.14, 1.0))?);
            }
            text(ICON_CLOSE, F_SMALL_ICON, close, center, grey(if hot(Action::Close) { 1.0 } else { 0.6 }, 1.0))?;

            text(v.title, F_HEAD, rect(CX * s, 14.0 * s, (CX + CW - 34.0) * s, 44.0 * s), lead, grey(1.0, 1.0))?;
            text(v.hint, F_LABEL, rect(CX * s, 44.0 * s, (CX + CW) * s, 62.0 * s), lead, grey(0.55, 1.0))?;

            for p in v.placed {
                let q = scaled(p.rect, s);
                match &p.item {
                    Item::Heading(t) => text(t, F_LABEL, rect(q.left, q.top + 8.0 * s, q.right, q.bottom), lead, grey(0.55, 1.0))?,
                    Item::Card { title, value, note, go } => {
                        let over = hot(Action::Go(*go));
                        fill(q, 12.0 * s, grey(if over { 0.11 } else { 0.08 }, 1.0))?;
                        outline(q, 12.0 * s, grey(if over { 0.26 } else { 0.135 }, 1.0))?;
                        let x = q.left + 14.0 * s;
                        text(title, F_LABEL, rect(x, q.top + 9.0 * s, q.right - 10.0 * s, q.top + 25.0 * s), lead, grey(0.55, 1.0))?;
                        text(value, F_TITLE, rect(x, q.top + 25.0 * s, q.right - 10.0 * s, q.top + 46.0 * s), lead, grey(1.0, 1.0))?;
                        text(note, F_LABEL, rect(x, q.top + 45.0 * s, q.right - 10.0 * s, q.top + 61.0 * s), lead, green(1.0))?;
                    }
                    Item::Switch { label, note, on, action } => {
                        if hot(*action) {
                            fill(q, 8.0 * s, grey(0.08, 1.0))?;
                        }
                        rt.FillRectangle(&rect(q.left, q.bottom - 1.0, q.right, q.bottom), &self.brush(grey(0.11, 1.0))?);
                        let track = rect(q.right - 12.0 * s - 34.0 * s, (q.top + q.bottom) / 2.0 - 10.0 * s, q.right - 12.0 * s, (q.top + q.bottom) / 2.0 + 10.0 * s);
                        let text_right = track.left - 12.0 * s;
                        match note {
                            Some(n) => {
                                text(label, F_BODY, rect(q.left + 2.0 * s, q.top + 5.0 * s, text_right, q.top + 24.0 * s), lead, grey(1.0, 1.0))?;
                                text(n, F_LABEL, rect(q.left + 2.0 * s, q.top + 24.0 * s, text_right, q.top + 40.0 * s), lead, grey(0.55, 1.0))?;
                            }
                            None => text(label, F_BODY, rect(q.left + 2.0 * s, q.top, text_right, q.bottom), lead, grey(1.0, 1.0))?,
                        }
                        fill(track, 10.0 * s, if *on { green(1.0) } else { grey(0.28, 1.0) })?;
                        let knob_x = if *on { track.right - 10.0 * s } else { track.left + 10.0 * s };
                        rt.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: knob_x, Y: (track.top + track.bottom) / 2.0 }, radiusX: 8.0 * s, radiusY: 8.0 * s }, &self.brush(grey(1.0, 1.0))?);
                    }
                    Item::Choice { label, options } => {
                        rt.FillRectangle(&rect(q.left, q.bottom - 1.0, q.right, q.bottom), &self.brush(grey(0.11, 1.0))?);
                        let (first, last) = (scaled(p.targets[0].0, s), scaled(p.targets[p.targets.len() - 1].0, s));
                        let holder = rect(first.left - 2.0 * s, first.top - 2.0 * s, last.right + 2.0 * s, last.bottom + 2.0 * s);
                        text(label, F_BODY, rect(q.left + 2.0 * s, q.top, holder.left - 12.0 * s, q.bottom), lead, grey(1.0, 1.0))?;
                        fill(holder, 8.0 * s, grey(0.1, 1.0))?;
                        for (b, (name, active, action)) in p.targets.iter().zip(options) {
                            let seg = scaled(b.0, s);
                            if *active {
                                fill(seg, 6.0 * s, grey(0.22, 1.0))?;
                            }
                            let ink = if *active { grey(1.0, 1.0) } else if hot(*action) { grey(0.85, 1.0) } else { grey(0.55, 1.0) };
                            text(name, F_LABEL, seg, center, ink)?;
                        }
                    }
                    Item::Chip { label, on, action } => {
                        let over = hot(*action);
                        let radius = (q.bottom - q.top) / 2.0;
                        if *on {
                            fill(q, radius, green(0.16))?;
                            outline(q, radius, green(1.0))?;
                        } else {
                            if over {
                                fill(q, radius, grey(0.09, 1.0))?;
                            }
                            outline(q, radius, grey(1.0, if over { 0.4 } else { 0.28 }))?;
                        }
                        text(label, F_LABEL, q, center, if *on { grey(1.0, 1.0) } else if over { grey(0.85, 1.0) } else { grey(0.6, 1.0) })?;
                    }
                    Item::Button { label, button, action } => {
                        rt.FillRectangle(&rect(q.left, q.bottom - 1.0, q.right, q.bottom), &self.brush(grey(0.11, 1.0))?);
                        let b = scaled(p.targets[0].0, s);
                        text(label, F_BODY, rect(q.left + 2.0 * s, q.top, b.left - 12.0 * s, q.bottom), lead, grey(1.0, 1.0))?;
                        let radius = (b.bottom - b.top) / 2.0;
                        if hot(*action) {
                            fill(b, radius, grey(0.14, 1.0))?;
                        }
                        outline(b, radius, grey(1.0, 0.28))?;
                        text(button, F_LABEL, rect(b.left, b.top - 1.0 * s, b.right, b.bottom - 1.0 * s), center, grey(1.0, 1.0))?;
                    }
                }
            }
            rt.EndDraw(None, None)?;
            #[cfg(test)]
            if self.grab {
                self.grab_frame()?;
            }
            self.surface.as_ref().unwrap().swap.Present(1, DXGI_PRESENT(0)).ok()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notch::Style;
    use crate::settings_win::{place, section_items, Model, SECTIONS};

    #[test]
    #[ignore = "writes target/settings-<n>.bmp: every section of the settings window"]
    fn settings_preview() {
        unsafe {
            use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let (w, h) = (WIDTH as i32, HEIGHT as i32);
            let hwnd = CreateWindowExW(WS_EX_NOREDIRECTIONBITMAP, w!("STATIC"), w!("Settings preview"), WS_POPUP, 0, 0, w, h, None, None, None, None).unwrap();
            let model = Model { minimal: false, robot: true, glass: true, flat_glass: false, style: Style::Ears, pages: 0x1FF & !Page::Usage.bit(), approve: true, volume_keys: false, activity: false, autostart: true, english: false };
            let mut renderer = Renderer::new().unwrap();
            renderer.grab = true;
            for (i, section) in SECTIONS.into_iter().enumerate() {
                let placed = place(section_items(section, &model));
                let hovered = placed.iter().flat_map(|p| p.targets.iter()).nth(1).map(|(_, a)| *a);
                let hint = section.hint(env!("CARGO_PKG_VERSION"));
                let view = SettingsView { section, placed: &placed, hovered: if i == 1 { hovered } else { None }, title: section.title(), hint: &hint, version: env!("CARGO_PKG_VERSION") };
                renderer.present_settings(hwnd, w, h, 1.0, &view).unwrap();
                let pixels = &renderer.grabbed[..];
                let mut bytes = Vec::new();
                bytes.extend_from_slice(b"BM");
                bytes.extend_from_slice(&(54u32 + pixels.len() as u32).to_le_bytes());
                bytes.extend_from_slice(&[0; 4]);
                bytes.extend_from_slice(&54u32.to_le_bytes());
                bytes.extend_from_slice(&40u32.to_le_bytes());
                bytes.extend_from_slice(&w.to_le_bytes());
                bytes.extend_from_slice(&(-h).to_le_bytes());
                bytes.extend_from_slice(&1u16.to_le_bytes());
                bytes.extend_from_slice(&32u16.to_le_bytes());
                bytes.extend_from_slice(&[0; 24]);
                bytes.extend_from_slice(pixels);
                std::fs::write(std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("target/settings-{i}.bmp")), bytes).unwrap();
            }
            DestroyWindow(hwnd).unwrap();
        }
    }
}

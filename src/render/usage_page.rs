//! The Limits page: a tab per app (a dot says whether it has counted), a ring per limit under it:
//! the percent used in the middle, the window and its group below, and when it starts over.

use super::*;
use crate::usage::{self, APPS};

/// Logical px: where the group names sit, the ring's centre and the label lines under it.
const GROUP_Y: f32 = 60.0;
const RING_Y: f32 = 98.0;
const LABEL_H: f32 = 14.0;
/// Widest a ring's column gets, so two or three rings sit together instead of far apart.
const SLOT_W: f32 = 112.0;
const RED: (f32, f32, f32) = (1.0, 0.35, 0.35);

/// Green while there is room, orange getting close, red nearly out.
fn ring_colour(used: f32) -> (f32, f32, f32) {
    if used < 60.0 { GREEN } else if used < 85.0 { ORANGE } else { RED }
}

/// "Gemini · Week" -> (Some("Gemini"), "Week"); "5 h" -> (None, "5 h").
fn split_label(label: &str) -> (Option<&str>, &str) {
    match label.rsplit_once(" · ") {
        Some((group, window)) => (Some(group), window),
        None => (None, label),
    }
}

impl Renderer {
    /// `tabs` are the page's buttons, `(cl, cr)` the content's left and right edge, `a` its opacity.
    pub(super) fn draw_usage(
        &self,
        text: &dyn Fn(&str, usize, D2D_RECT_F, DWRITE_TEXT_ALIGNMENT, D2D1_COLOR_F) -> Result<()>,
        tabs: &[(Button, D2D_RECT_F)],
        (cl, cr): (f32, f32),
        s: f32,
        a: f32,
        hovered: Option<Button>,
    ) -> Result<()> {
        let rt = &self.rt;
        let center = DWRITE_TEXT_ALIGNMENT_CENTER;
        let solid = |c: (f32, f32, f32), a: f32| color(c.0, c.1, c.2, a);
        let tab = self.usage_tab.min(APPS.len() - 1);
        unsafe {
            for (i, app) in APPS.iter().enumerate() {
                let Some(&(_, b)) = tabs.iter().find(|(x, _)| *x == Button::UsageTab(i)) else { continue };
                let shade = if i == tab { 0.22 } else if hovered == Some(Button::UsageTab(i)) { 0.17 } else { 0.11 };
                rt.FillRoundedRectangle(&rounded(b, 11.0 * s), &self.brush(grey(shade, a))?);
                let dot = D2D1_ELLIPSE { point: Vector2 { X: b.left + 11.0 * s, Y: (b.top + b.bottom) / 2.0 }, radiusX: 3.0 * s, radiusY: 3.0 * s };
                let counted = self.usage.get(*app).is_some();
                rt.FillEllipse(&dot, &self.brush(if counted { solid(GREEN, a) } else { grey(0.35, a) })?);
                text(app.name(), F_LABEL, rect(b.left + 18.0 * s, b.top, b.right - 4.0 * s, b.bottom), center, grey(if i == tab { 1.0 } else { 0.65 }, a))?;
            }
            let now = usage::now_secs();
            let Some(bars) = self.usage.get(APPS[tab]) else {
                text(APPS[tab].open_hint(), F_LABEL, rect(cl, 62.0 * s, cr, 122.0 * s), center, grey(0.6, a))?;
                return Ok(());
            };
            // One column per limit, together in the middle; more of them (Antigravity) get smaller rings.
            let n = bars.len().max(1);
            let slot = ((cr - cl) / n as f32).min(SLOT_W * s);
            let d = if n <= 3 { 54.0 * s } else { 46.0 * s };
            let mut x = cl + ((cr - cl) - slot * n as f32) / 2.0;
            let grouped = bars.iter().any(|b| split_label(&b.label).0.is_some());
            let down = if grouped { 0.0 } else { 6.0 * s };
            for bar in bars {
                let (group, window) = split_label(&bar.label);
                let mid = x + slot / 2.0;
                if let Some(group) = group {
                    text(&group.replace(" · ", "·"), F_LABEL, rect(x, GROUP_Y * s, x + slot, (GROUP_Y + LABEL_H) * s), center, grey(0.6, a))?;
                }
                let cy = RING_Y * s - if n <= 3 { 0.0 } else { 3.0 * s } + down;
                self.draw_ring((mid, cy), d, d * 0.15, bar.used / 100.0, ring_colour(bar.used), a)?;
                text(&format!("{}%", bar.used.round()), F_TITLE, rect(mid - d / 2.0, cy - d / 2.0, mid + d / 2.0, cy + d / 2.0), center, grey(1.0, a))?;
                let below = cy + d / 2.0 + 3.0 * s;
                text(window, F_LABEL, rect(x, below, x + slot, below + LABEL_H * s), center, grey(0.9, a))?;
                if bar.resets_at > 0 {
                    text(&usage::in_words(bar.resets_at, now), F_LABEL, rect(x, below + LABEL_H * s, x + slot, below + 2.0 * LABEL_H * s), center, grey(0.5, a))?;
                }
                x += slot;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rings_turn_orange_then_red() {
        assert_eq!(ring_colour(10.0), GREEN);
        assert_eq!(ring_colour(70.0), ORANGE);
        assert_eq!(ring_colour(95.0), RED);
    }

    #[test]
    fn a_label_splits_into_group_and_window() {
        assert_eq!(split_label("Gemini · Tuần"), (Some("Gemini"), "Tuần"));
        assert_eq!(split_label("Claude · GPT · 5 giờ"), (Some("Claude · GPT"), "5 giờ"));
        assert_eq!(split_label("5 giờ"), (None, "5 giờ"));
    }

    #[test]
    #[ignore = "writes target/usage-preview.bmp: the Limits page on each app's tab, stacked"]
    fn usage_preview() {
        use crate::usage::{Bar, Snapshot};
        unsafe {
            use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let (w, h) = (530i32, 210i32);
            let hwnd = CreateWindowExW(WS_EX_NOREDIRECTIONBITMAP, w!("STATIC"), w!("Usage preview"), WS_POPUP, 0, 0, w, h, None, None, None, None).unwrap();
            let now = usage::now_secs();
            let bar = |label: &str, weekly, used, hours: i64| Bar { label: label.into(), weekly, used, resets_at: now + hours * 3600 };
            let mut snap = Snapshot::default();
            snap.apps[0] = Some(vec![bar("5 giờ", false, 6.0, 3), bar("Tuần", true, 4.0, 150)]);
            snap.apps[1] = Some(vec![bar("Tuần", true, 99.0, 120)]);
            snap.apps[2] = Some(vec![bar("Gemini · Tuần", true, 0.4, 150), bar("Gemini · 5 giờ", false, 0.0, 4), bar("Claude · GPT · Tuần", true, 64.0, 160),
                bar("Claude · GPT · 5 giờ", false, 30.0, 4), bar("Argon · Tuần", true, 89.0, 150)]);
            let mut renderer = Renderer::new().unwrap();
            renderer.grab = true;
            renderer.usage = snap;
            let mut sheet = Vec::new();
            for tab in 0..3 {
                renderer.usage_tab = tab;
                let view = View { mode: Mode::Expanded, size: target(Mode::Expanded, false), time: "23:48", date: "T7, 31/10",
                    battery: Some(Battery { percent: 77, charging: true, saver: false }), banner: None, music: None, live: false, robot: None,
                    phase: 0.0, banner_age: 0.0, lyrics: false, hours: false, hovered: None, page: Page::Usage, leaving: None, todos: &[], notes: &[],
                    weather: &weather::State::default(), page_age: 9.0, unit: weather::Unit::C,
                    list: ListView { counts: (0, 0), remind_before: 15, now: 0, draft: "", editing: false, edit_item: None, caret: false, scroll: 0, note_pos: 0.0 },
                    health: HealthView { h: &Health::default(), now: 0, minute_of_day: 0, settings: None } };
                for _ in 0..40 {
                    renderer.present(hwnd, 0, 0, w, h, 1.0, &view).unwrap();
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                sheet.extend_from_slice(&renderer.grabbed[..]);
            }
            let sh = h * 3;
            let mut bytes = Vec::new(); bytes.extend_from_slice(b"BM");
            bytes.extend_from_slice(&(54u32 + sheet.len() as u32).to_le_bytes()); bytes.extend_from_slice(&[0; 4]);
            bytes.extend_from_slice(&54u32.to_le_bytes()); bytes.extend_from_slice(&40u32.to_le_bytes());
            bytes.extend_from_slice(&w.to_le_bytes()); bytes.extend_from_slice(&(-sh).to_le_bytes());
            bytes.extend_from_slice(&1u16.to_le_bytes()); bytes.extend_from_slice(&32u16.to_le_bytes());
            bytes.extend_from_slice(&[0; 24]); bytes.extend_from_slice(&sheet);
            std::fs::write(std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/usage-preview.bmp"), bytes).unwrap();
            DestroyWindow(hwnd).unwrap();
        }
    }
}

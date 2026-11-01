//! The Limits page: the apps as a segmented control (dimmer while one has not counted), then
//! cards on the same grey as the other pages. One or two limits (Claude, Codex) get a card each
//! with a ring and the time left; three or more (Antigravity) get a card per group with a bar per
//! window, so the numbers stay readable.

use super::*;
use crate::usage::{self, Bar, APPS};

/// Logical px: the cards' top and height (the tabs end at 56), the gap between cards, their padding.
const CARDS_TOP: f32 = 64.0;
const CARDS_H: f32 = 92.0;
const GAP: f32 = 6.0;
const PAD: f32 = 14.0;
/// Widest a card gets, so one or two limits do not stretch over the whole page.
const CARD_MAX_W: f32 = 200.0;
const RED: (f32, f32, f32) = (1.0, 0.35, 0.35);

/// Green while there is room, orange getting close, red nearly out.
fn ring_colour(used: f32) -> (f32, f32, f32) {
    if used < 60.0 { GREEN } else if used < 85.0 { ORANGE } else { RED }
}

/// Each app's own colour: the chosen tab and the cards carry a wash of it.
fn app_accent(app: usage::App) -> (f32, f32, f32) {
    match app {
        usage::App::Claude => (0.85, 0.47, 0.34),
        usage::App::Codex => (0.25, 0.8, 0.65),
        usage::App::Antigravity => (0.55, 0.5, 1.0),
    }
}

/// "Gemini · Week" -> (Some("Gemini"), "Week"); "5 h" -> (None, "5 h").
fn split_label(label: &str) -> (Option<&str>, &str) {
    match label.rsplit_once(" · ") {
        Some((group, window)) => (Some(group), window),
        None => (None, label),
    }
}

/// The limits grouped by their group name, in order ("" for those without one).
fn groups(bars: &[Bar]) -> Vec<(String, Vec<&Bar>)> {
    let mut out: Vec<(String, Vec<&Bar>)> = Vec::new();
    for bar in bars {
        let group = split_label(&bar.label).0.unwrap_or("").replace(" · ", "·");
        match out.iter_mut().find(|(g, _)| *g == group) {
            Some((_, list)) => list.push(bar),
            None => out.push((group, vec![bar])),
        }
    }
    // The short window first, then the week.
    for (_, list) in &mut out {
        list.sort_by_key(|b| b.weekly);
    }
    out
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
        let (center, lead, trail) = (DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_ALIGNMENT_TRAILING);
        let solid = |c: (f32, f32, f32), a: f32| color(c.0, c.1, c.2, a);
        let segment = |i: usize| tabs.iter().find(|(x, _)| *x == Button::UsageTab(i)).map(|(_, b)| *b);
        // The chosen app, or the first one that has a tab.
        let tab = if segment(self.usage_tab).is_some() { self.usage_tab } else { (0..APPS.len()).find(|&i| segment(i).is_some()).unwrap_or(0) };
        let now = usage::now_secs();
        // What is shown (the app's own way: the part left, or the part used) and how full it looks.
        let shown = |used: f32| if APPS[tab].counts_down() { 100.0 - used } else { used }.clamp(0.0, 100.0);
        unsafe {
            // The segmented control: one dark track, the chosen app lifted on it.
            let ends = (0..APPS.len()).filter_map(segment).fold(None, |ends: Option<(D2D_RECT_F, D2D_RECT_F)>, b| Some(ends.map_or((b, b), |(first, _)| (first, b))));
            let Some((first, last)) = ends else {
                // None of the three is on this PC.
                let r = rect(cl, CARDS_TOP * s, cr, (CARDS_TOP + CARDS_H) * s);
                rt.FillRoundedRectangle(&rounded(r, 14.0 * s), &self.brush(grey(0.11, a))?);
                text(crate::i18n::tr("Chưa thấy Claude, Codex hay Antigravity trên máy này", "No Claude, Codex or Antigravity found on this PC"), F_LABEL, r, center, grey(0.6, a))?;
                return Ok(());
            };
            rt.FillRoundedRectangle(&rounded(rect(first.left, first.top, last.right, first.bottom), 11.0 * s), &self.brush(grey(0.11, a))?);
            for (i, app) in APPS.iter().enumerate() {
                let Some(b) = segment(i) else { continue };
                let inner = rect(b.left + 2.0 * s, b.top + 2.0 * s, b.right - 2.0 * s, b.bottom - 2.0 * s);
                if i == tab {
                    let k = app_accent(*app);
                    rt.FillRoundedRectangle(&rounded(inner, 9.0 * s), &self.brush(color(k.0 * 0.45 + 0.08, k.1 * 0.45 + 0.08, k.2 * 0.45 + 0.08, a))?);
                } else if hovered == Some(Button::UsageTab(i)) {
                    rt.FillRoundedRectangle(&rounded(inner, 9.0 * s), &self.brush(grey(0.17, a))?);
                }
                // An app that has not counted yet is dimmer; the label is centred in its segment.
                let counted = self.usage.get(*app).is_some();
                let shade = match (i == tab, counted) { (true, true) => 1.0, (true, false) => 0.7, (false, true) => 0.62, (false, false) => 0.38 };
                let name = match self.usage.plan(*app) {
                    Some(plan) => format!("{} · {}", app.name(), plan),
                    None => app.name().to_string(),
                };
                text(&name, F_LABEL, b, center, grey(shade, a))?;
            }

            let top = CARDS_TOP * s;
            let card = |left: f32, width: f32| -> Result<D2D_RECT_F> {
                let r = rect(left, top, left + width, top + CARDS_H * s);
                let k = app_accent(APPS[tab]);
                self.rt.FillRoundedRectangle(&rounded(r, 14.0 * s), &self.brush(color(k.0 * 0.17 + 0.05, k.1 * 0.17 + 0.05, k.2 * 0.17 + 0.05, a))?);
                Ok(r)
            };
            let Some(bars) = self.usage.get(APPS[tab]) else {
                let r = card(cl, cr - cl)?;
                text(APPS[tab].open_hint(), F_LABEL, rect(r.left + PAD * s, r.top, r.right - PAD * s, r.bottom), center, grey(0.6, a))?;
                return Ok(());
            };

            if bars.len() <= 2 {
                // A card per limit: the ring with its percent, the window and the time left beside it.
                // One limit gets a wider card, so its name never has to be cut.
                let cap = if bars.len() == 1 { CARD_MAX_W * 1.4 } else { CARD_MAX_W };
                let room = if bars.len() == 1 { cr - cl } else { ((cr - cl) - GAP * s) / 2.0 };
                let width = room.min(cap * s);
                let mut x = cl + ((cr - cl) - width * bars.len() as f32 - GAP * s * (bars.len() as f32 - 1.0)) / 2.0;
                for bar in bars {
                    let r = card(x, width)?;
                    let d = 72.0 * s;
                    let mid = (r.top + r.bottom) / 2.0;
                    let ring_x = r.left + PAD * s + d / 2.0;
                    self.draw_ring((ring_x, mid), d, d * 0.12, shown(bar.used) / 100.0, ring_colour(bar.used), a)?;
                    text(&format!("{}%", shown(bar.used).round()), F_TITLE, rect(ring_x - d / 2.0, mid - d / 2.0, ring_x + d / 2.0, mid + d / 2.0), center, grey(1.0, a))?;
                    let tx = ring_x + d / 2.0 + 12.0 * s;
                    let (_, window) = split_label(&bar.label);
                    let line = 22.0 * s;
                    // A reading the app made a while ago says so: what it shows now may be further on.
                    let stale = self.usage.updated(APPS[tab]).filter(|&at| now - at > APPS[tab].stale_after());
                    let up = if stale.is_some() { 8.0 * s } else { 0.0 };
                    let (y0, y1) = if bar.resets_at > 0 { (mid - line - up, mid - up) } else { (mid - line / 2.0 - up, mid + line / 2.0 - up) };
                    // Taller than its line, so the tail of a g or a y is not cut.
                    text(window, F_BIG, rect(tx, y0 - 8.0 * s, r.right - PAD * s, y1 + 4.0 * s), lead, grey(1.0, a))?;
                    if bar.resets_at > 0 {
                        let left = format!("{} {}", crate::i18n::tr("Còn", "In"), usage::in_words(bar.resets_at, now));
                        text(&left, F_LABEL, rect(tx, mid + 5.0 * s - up, r.right - PAD * s, mid + line + 4.0 * s - up), lead, grey(0.6, a))?;
                    }
                    if let Some(at) = stale {
                        let ago = format!("{} {}", crate::i18n::tr("Số cũ", "Old"), usage::in_words(now, at));
                        text(&ago, F_LABEL, rect(tx, mid + line - up + 1.0 * s, r.right - PAD * s, mid + 2.0 * line - up), lead, grey(0.42, a))?;
                    }
                    x += width + GAP * s;
                }
                return Ok(());
            }

            // A card per group: its name, then a bar per window with the percent at the right.
            let list = groups(bars);
            let width = (((cr - cl) - GAP * s * (list.len() as f32 - 1.0)) / list.len() as f32).min(CARD_MAX_W * s);
            let mut x = cl + ((cr - cl) - width * list.len() as f32 - GAP * s * (list.len() as f32 - 1.0)) / 2.0;
            for (name, rows) in &list {
                let r = card(x, width)?;
                let inner = (r.left + PAD * s, r.right - PAD * s);
                text(name, F_TITLE, rect(inner.0, r.top + 6.0 * s, inner.1, r.top + 26.0 * s), lead, grey(1.0, a))?;
                for (k, bar) in rows.iter().enumerate() {
                    let y = r.top + (30.0 + k as f32 * 29.0) * s;
                    let (_, window) = split_label(&bar.label);
                    text(window, F_LABEL, rect(inner.0, y, inner.1, y + 15.0 * s), lead, grey(0.7, a))?;
                    text(&format!("{}%", shown(bar.used).round()), F_LABEL, rect(inner.0, y, inner.1, y + 15.0 * s), trail, grey(1.0, a))?;
                    let track = rect(inner.0, y + 18.0 * s, inner.1, y + 22.0 * s);
                    rt.FillRoundedRectangle(&rounded(track, 2.0 * s), &self.brush(grey(0.26, a))?);
                    let to = track.left + (track.right - track.left) * (shown(bar.used) / 100.0).max(0.03);
                    rt.FillRoundedRectangle(&rounded(rect(track.left, track.top, to, track.bottom), 2.0 * s), &self.brush(solid(ring_colour(bar.used), a))?);
                }
                x += width + GAP * s;
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
    fn limits_group_by_their_name_in_order() {
        let bar = |label: &str| Bar { label: label.into(), weekly: false, used: 1.0, resets_at: 0 };
        let bars = [bar("Gemini · Tuần"), bar("Gemini · 5 giờ"), bar("Claude · GPT · Tuần"), bar("Argon · Tuần")];
        let list = groups(&bars);
        assert_eq!(list.iter().map(|(g, r)| (g.as_str(), r.len())).collect::<Vec<_>>(), vec![("Gemini", 2), ("Claude·GPT", 1), ("Argon", 1)]);
        assert_eq!(groups(&[bar("5 giờ"), bar("Tuần")]).len(), 1, "no group names: one group");
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
            snap.apps[1] = Some(vec![bar("30 ngày", true, 0.0, 720)]);
            snap.updated[1] = Some(now - 3 * 3600);
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

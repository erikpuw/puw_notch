use std::time::{Duration, Instant};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::UI::Input::KeyboardAndMouse::{TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, KillTimer, SetForegroundWindow, SetTimer, ShowWindow, SW_SHOW, WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP};

use crate::i18n::{english, tr};
use crate::notch::Style;
use crate::render::{Page, Renderer, SettingsView, PAGES};

pub const CLASS: PCWSTR = w!("PuwNotchSettings");
pub const WIDTH: f32 = 760.0;
pub const HEIGHT: f32 = 560.0;
pub const NAV_W: f32 = 212.0;
pub const NAV_TOP: f32 = 78.0;
pub const NAV_H: f32 = 36.0;
pub const NAV_GAP: f32 = 2.0;
pub const DRAG_H: f32 = 44.0;
pub const CX: f32 = NAV_W + 30.0;
pub const CW: f32 = WIDTH - CX - 30.0;
pub const TITLE_H: f32 = 72.0;
pub const ITEMS_TOP: f32 = DRAG_H + TITLE_H;
pub const BOTTOM_PAD: f32 = 26.0;
pub const CLOSE: Rect = Rect { x: WIDTH - 38.0, y: 9.0, w: 26.0, h: 26.0 };
pub const STAGE_H: f32 = 150.0;
pub const ROW_H: f32 = 46.0;
pub const ROW_NOTE_H: f32 = 54.0;
pub const SEG_W: f32 = 96.0;
pub const SEG_H: f32 = 30.0;
pub const BUTTON_H: f32 = 34.0;
pub const PRIMARY_W: f32 = 130.0;
pub const SECONDARY_W: f32 = 150.0;
const HEAD_H: f32 = 38.0;
const OPTION_H: f32 = 118.0;
const TILE_H: f32 = 84.0;
const HERO_H: f32 = 112.0;
const STAT_H: f32 = 51.0;
const GAP: f32 = 10.0;
const ABOUT_H: f32 = 230.0;
const FEATURES: usize = 3;
const WHEEL_STEP: f32 = 48.0;
const FLIP: Duration = Duration::from_millis(250);
const PANE: Duration = Duration::from_millis(300);
const TIMER: usize = 1;
const FRAME_MS: u32 = 16;

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Section {
    Home,
    Look,
    Pages,
    Features,
    System,
    About,
}

pub const SECTIONS: [Section; 6] = [Section::Home, Section::Look, Section::Pages, Section::Features, Section::System, Section::About];

impl Section {
    pub fn title(self) -> &'static str {
        match self {
            Section::Home => tr("Tổng quan", "Overview"),
            Section::Look => tr("Giao diện", "Look"),
            Section::Pages => tr("Trang hiển thị", "Pages"),
            Section::Features => tr("Tính năng", "Features"),
            Section::System => tr("Hệ thống", "System"),
            Section::About => tr("Giới thiệu", "About"),
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Section::Home => "\u{E80F}",
            Section::Look => "\u{E790}",
            Section::Pages => "\u{E71D}",
            Section::Features => "\u{EA80}",
            Section::System => "\u{E713}",
            Section::About => "\u{E946}",
        }
    }

    pub fn hint(self, version: &str) -> String {
        match self {
            Section::Home => tr("Tình trạng hiện tại của notch. Bấm một thẻ để chỉnh phần đó.", "How the notch is set up now. Click a card to change that part.").into(),
            Section::Look => tr("Hình dạng notch và nền kính của các trang. Xem thay đổi ngay bên dưới.", "The notch shape and the glass behind pages. See the change right below.").into(),
            Section::Pages => tr("Chọn các trang xuất hiện khi mở notch. Luôn giữ lại ít nhất một trang.", "Pick the pages that show when the notch opens. At least one always stays.").into(),
            Section::Features => tr("Những việc chạy thêm ngoài các trang.", "Extras that run outside the pages.").into(),
            Section::System => tr("Cách PUW Notch chạy trên máy.", "How PUW Notch runs on this PC.").into(),
            Section::About => format!("{} {version}", tr("Phiên bản", "Version")),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Action {
    Go(Section),
    Close,
    Minimal,
    Robot,
    Glass,
    GlassFlat(bool),
    NotchStyle(Style),
    Page(Page),
    Approve,
    VolumeKeys,
    Activity,
    Autostart,
    English(bool),
    OpenRepo,
    Quit,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Model {
    pub minimal: bool,
    pub robot: bool,
    pub glass: bool,
    pub flat_glass: bool,
    pub style: Style,
    pub pages: u16,
    pub approve: bool,
    pub volume_keys: bool,
    pub activity: bool,
    pub autostart: bool,
    pub english: bool,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Thumb {
    pub pill: bool,
    pub rim: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Heading { text: &'static str, aside: Option<String> },
    Stage { pill: bool, raised: bool, glass: bool },
    Option { title: &'static str, note: &'static str, thumb: Thumb, on: bool, action: Action },
    Tile { label: &'static str, glyph: &'static str, on: bool, action: Action },
    Hero { title: &'static str, value: String, note: String, go: Section },
    Stat { title: &'static str, value: String, go: Section },
    Switch { label: &'static str, note: Option<&'static str>, glyph: Option<&'static str>, on: bool, action: Action },
    Choice { label: &'static str, note: Option<&'static str>, options: Vec<(&'static str, bool, Action)> },
    About { version: String },
}

impl Item {
    pub fn is_row(&self) -> bool {
        matches!(self, Item::Switch { .. } | Item::Choice { .. })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Placed {
    pub item: Item,
    pub rect: Rect,
    pub targets: Vec<(Rect, Action)>,
}

pub fn toggled_pages(mask: u16, page: Page) -> u16 {
    if mask == page.bit() { mask } else { mask ^ page.bit() }
}

pub fn nav_rects() -> Vec<(Section, Rect)> {
    SECTIONS
        .iter()
        .enumerate()
        .map(|(i, &s)| (s, Rect { x: 10.0, y: NAV_TOP + i as f32 * (NAV_H + NAV_GAP), w: NAV_W - 20.0, h: NAV_H }))
        .collect()
}

pub fn page_glyph(page: Page) -> &'static str {
    match page {
        Page::Music => "\u{E8D6}",
        Page::Todo => "\u{E8A5}",
        Page::Notify => "\u{EA8F}",
        Page::Health => "\u{EB51}",
        Page::Weather => "\u{E706}",
        Page::Calendar => "\u{E787}",
        Page::Task => "\u{E73A}",
        Page::Focus => "\u{E916}",
        Page::Usage => "\u{E9D2}",
    }
}

fn style_name(style: Style) -> &'static str {
    match style {
        Style::Ears => tr("Tai thỏ", "Ears"),
        Style::Pill => tr("Viên thuốc", "Pill"),
    }
}

fn glass_note(m: &Model) -> String {
    match (m.glass, m.flat_glass) {
        (false, _) => tr("Kính tắt", "Glass off").into(),
        (true, true) => tr("Kính phẳng, đang bật", "Flat glass, on").into(),
        (true, false) => tr("Kính nổi, đang bật", "Raised glass, on").into(),
    }
}

fn count_text(shown: usize) -> String {
    if english() { format!("{shown} of {}", PAGES.len()) } else { format!("{shown} trên {}", PAGES.len()) }
}

fn quick(m: &Model, notes: bool) -> Vec<Item> {
    let note = |vi: &'static str, en: &'static str| notes.then_some(tr(vi, en));
    vec![
        Item::Switch { label: tr("Kính mờ cho trang", "Frosted glass pages"), note: note("Tắt thì trang quay về nền đen đặc", "Off brings back the solid black page"), glyph: None, on: m.glass, action: Action::Glass },
        Item::Switch { label: tr("Notch tối giản", "Minimal notch"), note: note("Chỉ một vạch mảnh khi nghỉ", "Just a thin line at rest"), glyph: None, on: m.minimal, action: Action::Minimal },
        Item::Switch { label: tr("Robot khi để yên", "Idle robot"), note: note("Robot hiện trên viên thuốc khi không có gì", "A robot shows on the pill when nothing is going on"), glyph: None, on: m.robot, action: Action::Robot },
    ]
}

fn home(m: &Model) -> Vec<Item> {
    let shown = PAGES.iter().filter(|p| m.pages & p.bit() != 0).count();
    let mut items = vec![
        Item::Hero { title: tr("Giao diện", "Look"), value: style_name(m.style).into(), note: glass_note(m), go: Section::Look },
        Item::Stat { title: tr("Trang hiển thị", "Pages shown"), value: count_text(shown), go: Section::Pages },
        Item::Stat { title: tr("Ngôn ngữ", "Language"), value: if m.english { "English".into() } else { "Tiếng Việt".into() }, go: Section::System },
        Item::Heading { text: tr("Bật nhanh", "Quick switches"), aside: None },
    ];
    items.extend(quick(m, false));
    items
}

fn look(m: &Model) -> Vec<Item> {
    let option = |title, note, pill: bool, rim: bool, on: bool, action| Item::Option { title, note, thumb: Thumb { pill, rim }, on, action };
    let mut items = vec![
        Item::Stage { pill: m.style == Style::Pill, raised: !m.flat_glass, glass: m.glass },
        Item::Heading { text: tr("Kiểu notch", "Notch style"), aside: None },
        option(style_name(Style::Ears), tr("Dính mép trên màn hình", "Flush with the top edge"), false, true, m.style == Style::Ears, Action::NotchStyle(Style::Ears)),
        option(style_name(Style::Pill), tr("Nổi cách mép trên", "Floats below the top edge"), true, true, m.style == Style::Pill, Action::NotchStyle(Style::Pill)),
        Item::Heading { text: tr("Kiểu kính", "Glass style"), aside: None },
        option(tr("Kính nổi", "Raised"), tr("Viền và ánh sáng ở mép", "A rim and edge light"), m.style == Style::Pill, true, !m.flat_glass, Action::GlassFlat(false)),
        option(tr("Kính phẳng", "Flat"), tr("Không viền, chỉ có nền mờ", "No rim, just the blurred backdrop"), m.style == Style::Pill, false, m.flat_glass, Action::GlassFlat(true)),
        Item::Heading { text: tr("Hiệu ứng", "Effects"), aside: None },
    ];
    items.extend(quick(m, true));
    items
}

fn pages(m: &Model) -> Vec<Item> {
    let shown = PAGES.iter().filter(|p| m.pages & p.bit() != 0).count();
    let mut items = vec![Item::Heading { text: tr("Đang bật", "Shown"), aside: Some(count_text(shown)) }];
    items.extend(PAGES.iter().map(|&p| Item::Tile { label: p.name(), glyph: page_glyph(p), on: m.pages & p.bit() != 0, action: Action::Page(p) }));
    items
}

fn features(m: &Model) -> Vec<Item> {
    vec![
        Item::Switch { label: tr("Duyệt ngay trên notch", "Approve on the notch"), note: Some(tr("Trả lời yêu cầu của Claude và Antigravity ở banner", "Answer Claude and Antigravity requests on the banner")), glyph: Some("\u{E73E}"), on: m.approve, action: Action::Approve },
        Item::Switch { label: tr("Ẩn bảng âm lượng của Windows", "Hide Windows volume popup"), note: Some(tr("Chỉ hiện banner âm lượng của notch", "Only the notch shows the volume banner")), glyph: Some("\u{E767}"), on: m.volume_keys, action: Action::VolumeKeys },
        Item::Switch { label: tr("Tác vụ nền trực tiếp", "Live activities"), note: Some(tr("Tải file, sao chép và các việc đang chạy", "File downloads, copies and other running jobs")), glyph: Some("\u{E896}"), on: m.activity, action: Action::Activity },
    ]
}

fn system(m: &Model) -> Vec<Item> {
    vec![
        Item::Switch { label: tr("Khởi động cùng Windows", "Start with Windows"), note: Some(tr("Tự mở khi đăng nhập", "Opens when you sign in")), glyph: None, on: m.autostart, action: Action::Autostart },
        Item::Choice {
            label: tr("Ngôn ngữ", "Language"),
            note: Some(tr("Áp dụng ngay cho toàn bộ giao diện", "Applies to the whole interface right away")),
            options: vec![("Tiếng Việt", !m.english, Action::English(false)), ("English", m.english, Action::English(true))],
        },
    ]
}

pub fn section_items(section: Section, m: &Model) -> Vec<Item> {
    match section {
        Section::Home => home(m),
        Section::Look => look(m),
        Section::Pages => pages(m),
        Section::Features => features(m),
        Section::System => system(m),
        Section::About => vec![Item::About { version: env!("CARGO_PKG_VERSION").into() }],
    }
}

pub fn features_on(m: &Model) -> usize {
    [m.approve, m.volume_keys, m.activity].iter().filter(|on| **on).count().min(FEATURES)
}

fn height(item: &Item) -> f32 {
    match item {
        Item::Heading { .. } => HEAD_H,
        Item::Stage { .. } => STAGE_H,
        Item::Option { .. } => OPTION_H,
        Item::Tile { .. } => TILE_H,
        Item::Hero { .. } => HERO_H,
        Item::Stat { .. } => STAT_H,
        Item::Switch { note: Some(_), .. } | Item::Choice { note: Some(_), .. } => ROW_NOTE_H,
        Item::Switch { .. } | Item::Choice { .. } => ROW_H,
        Item::About { .. } => ABOUT_H,
    }
}

pub fn place(items: Vec<Item>) -> Vec<Placed> {
    let mut out = Vec::with_capacity(items.len());
    let mut y = ITEMS_TOP;
    let (mut kind, mut count, mut start) = (0u8, 0usize, ITEMS_TOP);
    let hero_w = (CW - GAP) * 1.4 / 2.4;
    for item in items {
        let h = height(&item);
        let rect = match &item {
            Item::Option { .. } | Item::Tile { .. } => {
                let (k, cols) = if matches!(item, Item::Option { .. }) { (1, 2) } else { (2, 3) };
                if k != kind {
                    (kind, count, start) = (k, 0, y);
                }
                let cell = (CW - (cols - 1) as f32 * GAP) / cols as f32;
                let (row, col) = (count / cols, count % cols);
                count += 1;
                let rect = Rect { x: CX + col as f32 * (cell + GAP), y: start + row as f32 * (h + GAP), w: cell, h };
                y = rect.y + h;
                rect
            }
            Item::Hero { .. } => {
                (kind, count, start) = (3, 0, y);
                y = start + HERO_H;
                Rect { x: CX, y: start, w: hero_w, h }
            }
            Item::Stat { .. } => {
                let idx = count;
                count += 1;
                Rect { x: CX + hero_w + GAP, y: start + idx as f32 * (STAT_H + GAP), w: CW - hero_w - GAP, h }
            }
            _ => {
                (kind, count) = (0, 0);
                let rect = Rect { x: CX, y, w: CW, h };
                y += h;
                rect
            }
        };
        let targets = match &item {
            Item::Hero { go, .. } | Item::Stat { go, .. } => vec![(rect, Action::Go(*go))],
            Item::Option { action, .. } | Item::Tile { action, .. } | Item::Switch { action, .. } => vec![(rect, *action)],
            Item::Heading { .. } | Item::Stage { .. } => Vec::new(),
            Item::Choice { options, .. } => {
                let total = options.len() as f32 * SEG_W + 4.0;
                let left = rect.x + rect.w - 14.0 - total + 2.0;
                let top = rect.y + (rect.h - SEG_H) / 2.0 + 2.0;
                options.iter().enumerate().map(|(i, (_, _, a))| (Rect { x: left + i as f32 * SEG_W, y: top, w: SEG_W, h: SEG_H - 4.0 }, *a)).collect()
            }
            Item::About { .. } => {
                let total = PRIMARY_W + GAP + SECONDARY_W;
                let left = rect.x + (rect.w - total) / 2.0;
                let top = rect.y + 168.0;
                vec![
                    (Rect { x: left, y: top, w: PRIMARY_W, h: BUTTON_H }, Action::OpenRepo),
                    (Rect { x: left + PRIMARY_W + GAP, y: top, w: SECONDARY_W, h: BUTTON_H }, Action::Quit),
                ]
            }
        };
        out.push(Placed { item, rect, targets });
    }
    out
}

pub fn max_scroll(placed: &[Placed]) -> f32 {
    let bottom = placed.iter().map(|p| p.rect.y + p.rect.h).fold(ITEMS_TOP, f32::max);
    (bottom + BOTTOM_PAD - HEIGHT).max(0.0)
}

pub fn view_parts(section: Section, m: &Model) -> (Vec<Placed>, f32) {
    let placed = place(section_items(section, m));
    let max = max_scroll(&placed);
    (placed, max)
}

pub fn hit_at(section: Section, m: &Model, x: f32, y: f32, scroll: f32) -> Option<Action> {
    if CLOSE.contains(x, y) {
        return Some(Action::Close);
    }
    if let Some((s, _)) = nav_rects().into_iter().find(|(_, r)| r.contains(x, y)) {
        return Some(Action::Go(s));
    }
    if x < NAV_W || y < DRAG_H {
        return None;
    }
    place(section_items(section, m)).into_iter().flat_map(|p| p.targets).find(|(r, _)| r.contains(x, y + scroll)).map(|(_, a)| a)
}

pub fn caption_at(x: f32, y: f32) -> bool {
    (0.0..WIDTH).contains(&x) && (0.0..DRAG_H).contains(&y) && !CLOSE.contains(x, y)
}

fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

pub struct SettingsWin {
    pub hwnd: HWND,
    renderer: Renderer,
    size: (i32, i32),
    scale: f32,
    pub section: Section,
    pub hovered: Option<Action>,
    pub model: Model,
    pub scroll: f32,
    pane_since: Instant,
    flips: Vec<(Action, Instant)>,
    tracking: bool,
    timer: bool,
}

impl SettingsWin {
    pub fn open(monitor: RECT, scale: f32, model: Model) -> Option<Self> {
        let size = ((WIDTH * scale).round() as i32, (HEIGHT * scale).round() as i32);
        let x = monitor.left + (monitor.right - monitor.left - size.0) / 2;
        let y = monitor.top + (monitor.bottom - monitor.top - size.1) / 2;
        unsafe {
            let hwnd = CreateWindowExW(WS_EX_NOREDIRECTIONBITMAP | WS_EX_TOPMOST | WS_EX_TOOLWINDOW, CLASS, w!("PUW Notch settings"), WS_POPUP, x, y, size.0, size.1, None, None, None, None).ok()?;
            let renderer = match Renderer::new() {
                Ok(r) => r,
                Err(_) => {
                    let _ = DestroyWindow(hwnd);
                    return None;
                }
            };
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
            let mut win = SettingsWin { hwnd, renderer, size, scale, section: Section::Home, hovered: None, model, scroll: 0.0, pane_since: Instant::now(), flips: Vec::new(), tracking: false, timer: false };
            win.draw();
            Some(win)
        }
    }

    pub fn go(&mut self, section: Section) {
        self.section = section;
        self.hovered = None;
        self.scroll = 0.0;
        self.pane_since = Instant::now();
    }

    pub fn flipped(&mut self, action: Action) {
        self.flips.retain(|(_, at)| at.elapsed() < FLIP);
        self.flips.push((action, Instant::now()));
    }

    pub fn wheel(&mut self, notches: f32) {
        let (_, max) = view_parts(self.section, &self.model);
        self.scroll = (self.scroll - notches * WHEEL_STEP).clamp(0.0, max);
    }

    pub fn draw(&mut self) {
        let (placed, max) = view_parts(self.section, &self.model);
        self.scroll = self.scroll.clamp(0.0, max);
        self.flips.retain(|(_, at)| at.elapsed() < FLIP);
        let flips: Vec<(Action, f32)> = self.flips.iter().map(|(a, at)| (*a, ease_out(at.elapsed().as_secs_f32() / FLIP.as_secs_f32()))).collect();
        let pane_raw = self.pane_since.elapsed().as_secs_f32() / PANE.as_secs_f32();
        let animating = !flips.is_empty() || pane_raw < 1.0;
        let version = env!("CARGO_PKG_VERSION");
        let (title, hint) = (self.section.title(), self.section.hint(version));
        let view = SettingsView { section: self.section, placed: &placed, scroll: self.scroll, max_scroll: max, hovered: self.hovered, title, hint: &hint, version, pane: ease_out(pane_raw), flips: &flips };
        let _ = self.renderer.present_settings(self.hwnd, self.size.0, self.size.1, self.scale, &view);
        self.set_timer(animating);
    }

    fn set_timer(&mut self, on: bool) {
        if on != self.timer {
            self.timer = on;
            unsafe {
                if on {
                    SetTimer(Some(self.hwnd), TIMER, FRAME_MS, None);
                } else {
                    let _ = KillTimer(Some(self.hwnd), TIMER);
                }
            }
        }
    }

    pub fn is_timer(&self, id: usize) -> bool {
        id == TIMER
    }

    pub fn hit(&self, px: f32, py: f32) -> Option<Action> {
        hit_at(self.section, &self.model, px / self.scale, py / self.scale, self.scroll)
    }

    pub fn caption(&self, px: f32, py: f32) -> bool {
        caption_at(px / self.scale, py / self.scale)
    }

    pub fn track_leave(&mut self) {
        if !self.tracking {
            self.tracking = true;
            let mut event = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: self.hwnd, dwHoverTime: 0 };
            unsafe {
                let _ = TrackMouseEvent(&mut event);
            }
        }
    }

    pub fn left(&mut self) {
        self.tracking = false;
        self.hovered = None;
    }
}

impl Drop for SettingsWin {
    fn drop(&mut self) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIMER);
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model() -> Model {
        Model { minimal: false, robot: true, glass: true, flat_glass: false, style: Style::Ears, pages: 0x1FF & !Page::Usage.bit(), approve: true, volume_keys: false, activity: false, autostart: true, english: false }
    }

    #[test]
    fn the_overview_shows_a_hero_two_stats_and_three_quick_switches() {
        let items = section_items(Section::Home, &model());
        assert!(matches!(&items[0], Item::Hero { value, note, .. } if value == "Tai thỏ" && note == "Kính nổi, đang bật"));
        assert!(matches!(&items[1], Item::Stat { value, .. } if value == "8 trên 9"));
        assert!(matches!(&items[2], Item::Stat { value, .. } if value == "Tiếng Việt"));
        assert_eq!(items.iter().filter(|i| i.is_row()).count(), 3);
        let placed = place(items);
        let (hero, a, b) = (&placed[0].rect, &placed[1].rect, &placed[2].rect);
        assert_eq!(hero.y, a.y);
        assert!(a.x > hero.x + hero.w && a.x == b.x && b.y > a.y, "stats sit in a column right of the hero");
        assert!((b.y + b.h - (hero.y + hero.h)).abs() < 0.01, "the two stats end level with the hero");
    }

    #[test]
    fn a_click_on_a_row_a_card_or_a_segment_gives_its_action_even_when_scrolled() {
        let m = model();
        let placed = place(section_items(Section::Look, &m));
        let row = placed.iter().find(|p| matches!(&p.item, Item::Switch { action: Action::Glass, .. })).unwrap();
        assert_eq!(hit_at(Section::Look, &m, row.rect.x + 40.0, row.rect.y + 10.0, 0.0), Some(Action::Glass));
        let scroll = 60.0;
        assert_eq!(hit_at(Section::Look, &m, row.rect.x + 40.0, row.rect.y + 10.0 - scroll, scroll), Some(Action::Glass), "the click is measured after the scroll offset");
        let card = placed.iter().find(|p| matches!(&p.item, Item::Option { action: Action::GlassFlat(true), .. })).unwrap();
        assert_eq!(hit_at(Section::Look, &m, card.rect.x + 8.0, card.rect.y + 8.0, 0.0), Some(Action::GlassFlat(true)));
        let language = place(section_items(Section::System, &m));
        let seg = language.iter().find_map(|p| matches!(&p.item, Item::Choice { .. }).then(|| p.targets[1])).unwrap();
        assert_eq!(seg.1, Action::English(true));
        assert_eq!(hit_at(Section::System, &m, seg.0.x + 5.0, seg.0.y + 5.0, 0.0), Some(Action::English(true)));
        assert_eq!(hit_at(Section::Look, &m, NAV_W + 2.0, HEIGHT - 2.0, 0.0), None, "empty corner");
    }

    #[test]
    fn the_sidebar_the_close_button_and_the_drag_strip() {
        let m = model();
        for (s, r) in nav_rects() {
            assert_eq!(hit_at(Section::Home, &m, r.x + r.w / 2.0, r.y + r.h / 2.0, 0.0), Some(Action::Go(s)));
        }
        assert_eq!(hit_at(Section::Pages, &m, CLOSE.x + 4.0, CLOSE.y + 4.0, 0.0), Some(Action::Close));
        assert!(caption_at(400.0, 20.0) && !caption_at(CLOSE.x + 4.0, CLOSE.y + 4.0) && !caption_at(400.0, 200.0), "only the top strip drags the window");
        assert_eq!(hit_at(Section::Look, &m, CX + 20.0, DRAG_H - 4.0, 80.0), None, "content scrolled under the drag strip is not clickable");
    }

    #[test]
    fn every_section_stays_inside_the_window_sideways_and_scrolls_when_long() {
        let m = model();
        for s in SECTIONS {
            let (placed, max) = view_parts(s, &m);
            for p in &placed {
                assert!(p.rect.x >= CX - 0.01 && p.rect.x + p.rect.w <= CX + CW + 0.01, "{s:?} item sticks out sideways: {:?}", p.rect);
            }
            let bottom = placed.iter().map(|p| p.rect.y + p.rect.h).fold(0.0, f32::max);
            assert!(bottom - max <= HEIGHT - BOTTOM_PAD + 0.01, "{s:?} last item must be reachable by scrolling");
        }
        assert!(view_parts(Section::Look, &m).1 > 0.0, "the Look page is longer than the window");
        assert_eq!(view_parts(Section::About, &m).1, 0.0);
    }

    #[test]
    fn the_nine_page_tiles_form_a_grid_of_three_columns() {
        let placed = place(section_items(Section::Pages, &model()));
        let tiles: Vec<&Placed> = placed.iter().filter(|p| matches!(p.item, Item::Tile { .. })).collect();
        assert_eq!(tiles.len(), PAGES.len());
        assert_eq!(tiles[0].rect.x, tiles[3].rect.x);
        assert!(tiles[0].rect.x < tiles[1].rect.x && tiles[1].rect.x < tiles[2].rect.x);
        assert!(tiles[3].rect.y > tiles[0].rect.y && tiles[0].rect.y == tiles[2].rect.y);
    }

    #[test]
    fn the_glass_style_cards_follow_the_notch_shape() {
        let mut m = model();
        m.style = Style::Pill;
        let items = section_items(Section::Look, &m);
        assert!(matches!(&items[0], Item::Stage { pill: true, raised: true, glass: true }));
        let thumbs: Vec<Thumb> = items.iter().filter_map(|i| match i { Item::Option { thumb, .. } => Some(*thumb), _ => None }).collect();
        assert_eq!(thumbs.len(), 4);
        assert!(!thumbs[0].pill && thumbs[1].pill, "the notch cards show both shapes");
        assert!(thumbs[2].pill && thumbs[3].pill && thumbs[2].rim && !thumbs[3].rim, "the glass cards use the current shape, raised with a rim and flat without");
    }

    #[test]
    fn the_last_page_cannot_be_switched_off() {
        assert_eq!(toggled_pages(Page::Music.bit(), Page::Music), Page::Music.bit());
        assert_eq!(toggled_pages(Page::Music.bit() | Page::Todo.bit(), Page::Todo), Page::Music.bit());
        assert_eq!(toggled_pages(Page::Music.bit(), Page::Todo), Page::Music.bit() | Page::Todo.bit());
    }

    #[test]
    fn features_on_counts_the_three_switches() {
        assert_eq!(features_on(&model()), 1);
    }

    #[test]
    #[ignore = "shows a real window for a moment"]
    fn the_settings_window_opens_draws_scrolls_and_closes() {
        use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
        use windows::Win32::UI::WindowsAndMessaging::{DefWindowProcW, IsWindow, RegisterClassExW, WNDCLASSEXW};
        unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        unsafe {
            let wc = WNDCLASSEXW { cbSize: size_of::<WNDCLASSEXW>() as u32, lpfnWndProc: Some(proc), lpszClassName: CLASS, ..Default::default() };
            assert_ne!(RegisterClassExW(&wc), 0);
            let mut win = SettingsWin::open(RECT { left: 0, top: 0, right: 1920, bottom: 1080 }, 1.0, model()).expect("the window opens");
            let hwnd = win.hwnd;
            win.go(Section::Look);
            win.wheel(-3.0);
            assert!(win.scroll > 0.0, "scrolled down");
            win.flipped(Action::Glass);
            win.hovered = win.hit(CX + 20.0, DRAG_H + 120.0);
            win.draw();
            assert!(IsWindow(Some(hwnd)).as_bool());
            assert!(win.caption(300.0, 20.0) && !win.caption(300.0, 200.0));
            drop(win);
            assert!(!IsWindow(Some(hwnd)).as_bool(), "dropping the window destroys it");
        }
    }
}

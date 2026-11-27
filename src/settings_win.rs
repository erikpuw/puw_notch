use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::UI::Input::KeyboardAndMouse::{TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, SetForegroundWindow, ShowWindow, SW_SHOW, WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP};

use crate::i18n::tr;
use crate::notch::Style;
use crate::render::{Page, Renderer, SettingsView, PAGES};

pub const CLASS: PCWSTR = w!("PuwNotchSettings");
pub const WIDTH: f32 = 660.0;
pub const HEIGHT: f32 = 500.0;
pub const NAV_W: f32 = 178.0;
pub const NAV_TOP: f32 = 52.0;
pub const NAV_H: f32 = 34.0;
pub const NAV_GAP: f32 = 2.0;
pub const CX: f32 = NAV_W + 18.0;
pub const CW: f32 = WIDTH - CX - 18.0;
pub const TOP: f32 = 76.0;
pub const CAPTION_H: f32 = 48.0;
pub const CLOSE: Rect = Rect { x: WIDTH - 36.0, y: 12.0, w: 24.0, h: 24.0 };
const ROW_H: f32 = 36.0;
const ROW_NOTE_H: f32 = 46.0;
const HEAD_H: f32 = 30.0;
const CARD_H: f32 = 66.0;
const CARD_GAP: f32 = 10.0;
const CHIP_H: f32 = 30.0;
const CHIP_GAP: f32 = 8.0;
const SEG_W: f32 = 104.0;
const SEG_H: f32 = 28.0;
const BUTTON_W: f32 = 150.0;
const BUTTON_H: f32 = 26.0;
const FEATURES: usize = 3;

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
            Section::Home => tr("Tình trạng hiện tại, bấm một thẻ để chỉnh.", "Where things stand. Click a card to change it.").into(),
            Section::Look => tr("Hình dạng notch và kiểu nền kính của trang.", "The notch shape and the glass behind pages.").into(),
            Section::Pages => tr("Bấm để bật hoặc tắt từng trang. Cần ít nhất một trang.", "Click to show or hide a page. At least one stays on.").into(),
            Section::Features => tr("Những thứ chạy thêm ngoài các trang.", "Extras that run outside the pages.").into(),
            Section::System => tr("Cách PUW Notch chạy trên máy.", "How PUW Notch runs on this PC.").into(),
            Section::About => format!("PUW Notch {version}"),
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

#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Card { title: &'static str, value: String, note: String, go: Section },
    Heading(&'static str),
    Switch { label: &'static str, note: Option<&'static str>, on: bool, action: Action },
    Choice { label: &'static str, options: Vec<(&'static str, bool, Action)> },
    Chip { label: &'static str, on: bool, action: Action },
    Button { label: &'static str, button: &'static str, action: Action },
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
        .map(|(i, &s)| (s, Rect { x: 8.0, y: NAV_TOP + i as f32 * (NAV_H + NAV_GAP), w: NAV_W - 16.0, h: NAV_H }))
        .collect()
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

fn features_on(m: &Model) -> Vec<&'static str> {
    [
        (m.approve, tr("Duyệt ngay trên notch", "Approve on the notch")),
        (m.volume_keys, tr("Ẩn bảng âm lượng của Windows", "Hide Windows volume popup")),
        (m.activity, tr("Tác vụ nền trực tiếp", "Live activities")),
    ]
    .into_iter()
    .filter_map(|(on, name)| on.then_some(name))
    .collect()
}

fn home(m: &Model) -> Vec<Item> {
    let hidden: Vec<&str> = PAGES.iter().filter(|p| m.pages & p.bit() == 0).map(|p| p.name()).collect();
    let shown = PAGES.len() - hidden.len();
    let on = features_on(m);
    vec![
        Item::Card { title: tr("Kiểu notch", "Notch style"), value: style_name(m.style).into(), note: glass_note(m), go: Section::Look },
        Item::Card {
            title: tr("Trang hiển thị", "Pages shown"),
            value: if crate::i18n::english() { format!("{shown} of {}", PAGES.len()) } else { format!("{shown} trên {}", PAGES.len()) },
            note: if hidden.is_empty() { tr("Hiện tất cả", "All shown").into() } else { format!("{} {}", tr("Ẩn:", "Hidden:"), hidden.join(", ")) },
            go: Section::Pages,
        },
        Item::Card {
            title: tr("Tính năng", "Features"),
            value: if crate::i18n::english() { format!("{} of {FEATURES} on", on.len()) } else { format!("{} trên {FEATURES} đang bật", on.len()) },
            note: on.first().map_or_else(|| tr("Chưa bật gì", "None on").into(), |name| name.to_string()),
            go: Section::Features,
        },
        Item::Card {
            title: tr("Hệ thống", "System"),
            value: if m.english { "English".into() } else { "Tiếng Việt".into() },
            note: if m.autostart { tr("Khởi động cùng Windows", "Starts with Windows").into() } else { tr("Không tự khởi động", "Does not start with Windows").into() },
            go: Section::System,
        },
        Item::Heading(tr("Bật nhanh", "Quick switches")),
        Item::Switch { label: tr("Kính mờ cho trang", "Frosted glass pages"), note: None, on: m.glass, action: Action::Glass },
        Item::Switch { label: tr("Notch tối giản", "Minimal notch"), note: None, on: m.minimal, action: Action::Minimal },
        Item::Switch { label: tr("Robot khi để yên", "Idle robot"), note: None, on: m.robot, action: Action::Robot },
    ]
}

fn look(m: &Model) -> Vec<Item> {
    vec![
        Item::Choice {
            label: tr("Kiểu notch", "Notch style"),
            options: [Style::Ears, Style::Pill].into_iter().map(|s| (style_name(s), m.style == s, Action::NotchStyle(s))).collect(),
        },
        Item::Choice {
            label: tr("Kiểu kính", "Glass style"),
            options: vec![
                (tr("Kính nổi", "Raised"), !m.flat_glass, Action::GlassFlat(false)),
                (tr("Kính phẳng", "Flat"), m.flat_glass, Action::GlassFlat(true)),
            ],
        },
        Item::Switch { label: tr("Kính mờ cho trang", "Frosted glass pages"), note: Some(tr("Tắt thì trang quay về nền đen đặc", "Off brings back the solid black page")), on: m.glass, action: Action::Glass },
        Item::Switch { label: tr("Notch tối giản", "Minimal notch"), note: Some(tr("Chỉ một vạch mảnh khi nghỉ", "Just a thin line at rest")), on: m.minimal, action: Action::Minimal },
        Item::Switch { label: tr("Robot khi để yên", "Idle robot"), note: Some(tr("Robot hiện trên viên thuốc khi không có gì", "A robot shows on the pill when nothing is going on")), on: m.robot, action: Action::Robot },
    ]
}

fn pages(m: &Model) -> Vec<Item> {
    PAGES.iter().map(|&p| Item::Chip { label: p.name(), on: m.pages & p.bit() != 0, action: Action::Page(p) }).collect()
}

fn features(m: &Model) -> Vec<Item> {
    vec![
        Item::Switch { label: tr("Duyệt ngay trên notch", "Approve on the notch"), note: Some(tr("Trả lời yêu cầu của Claude và Antigravity ở banner", "Answer Claude and Antigravity requests on the banner")), on: m.approve, action: Action::Approve },
        Item::Switch { label: tr("Ẩn bảng âm lượng của Windows", "Hide Windows volume popup"), note: Some(tr("Chỉ hiện banner âm lượng của notch", "Only the notch shows the volume banner")), on: m.volume_keys, action: Action::VolumeKeys },
        Item::Switch { label: tr("Tác vụ nền trực tiếp", "Live activities"), note: Some(tr("Tải file, sao chép và các việc đang chạy", "File downloads, copies and other running jobs")), on: m.activity, action: Action::Activity },
    ]
}

fn system(m: &Model) -> Vec<Item> {
    vec![
        Item::Switch { label: tr("Khởi động cùng Windows", "Start with Windows"), note: None, on: m.autostart, action: Action::Autostart },
        Item::Choice {
            label: tr("Ngôn ngữ", "Language"),
            options: vec![("Tiếng Việt", !m.english, Action::English(false)), ("English", m.english, Action::English(true))],
        },
    ]
}

fn about() -> Vec<Item> {
    vec![
        Item::Button { label: tr("Mã nguồn và bản phát hành", "Source code and releases"), button: tr("Mở GitHub", "Open GitHub"), action: Action::OpenRepo },
        Item::Button { label: tr("Đóng ứng dụng", "Close the app"), button: tr("Thoát PUW Notch", "Quit PUW Notch"), action: Action::Quit },
    ]
}

pub fn section_items(section: Section, m: &Model) -> Vec<Item> {
    match section {
        Section::Home => home(m),
        Section::Look => look(m),
        Section::Pages => pages(m),
        Section::Features => features(m),
        Section::System => system(m),
        Section::About => about(),
    }
}

fn height(item: &Item) -> f32 {
    match item {
        Item::Card { .. } => CARD_H,
        Item::Heading(_) => HEAD_H,
        Item::Switch { note: Some(_), .. } => ROW_NOTE_H,
        Item::Switch { .. } | Item::Choice { .. } | Item::Button { .. } => ROW_H,
        Item::Chip { .. } => CHIP_H,
    }
}

fn grid(item: &Item) -> Option<(u8, usize, f32, f32)> {
    match item {
        Item::Card { .. } => Some((1, 2, (CW - CARD_GAP) / 2.0, CARD_GAP)),
        Item::Chip { .. } => Some((2, 3, (CW - 2.0 * CHIP_GAP) / 3.0, CHIP_GAP)),
        _ => None,
    }
}

pub fn place(items: Vec<Item>) -> Vec<Placed> {
    let mut out = Vec::with_capacity(items.len());
    let mut y = TOP;
    let (mut kind, mut count, mut start) = (0u8, 0usize, TOP);
    for item in items {
        let h = height(&item);
        let rect = match grid(&item) {
            Some((k, cols, cell_w, gap)) => {
                if k != kind {
                    (kind, count, start) = (k, 0, y);
                }
                let (row, col) = (count / cols, count % cols);
                count += 1;
                let rect = Rect { x: CX + col as f32 * (cell_w + gap), y: start + row as f32 * (h + gap), w: cell_w, h };
                y = rect.y + h + gap;
                rect
            }
            None => {
                (kind, count) = (0, 0);
                let rect = Rect { x: CX, y, w: CW, h };
                y += h;
                rect
            }
        };
        let targets = match &item {
            Item::Card { go, .. } => vec![(rect, Action::Go(*go))],
            Item::Chip { action, .. } | Item::Switch { action, .. } => vec![(rect, *action)],
            Item::Heading(_) => Vec::new(),
            Item::Choice { options, .. } => {
                let total = options.len() as f32 * SEG_W + 4.0;
                let left = rect.x + rect.w - total + 2.0;
                let top = rect.y + (rect.h - SEG_H) / 2.0 + 2.0;
                options.iter().enumerate().map(|(i, (_, _, a))| (Rect { x: left + i as f32 * SEG_W, y: top, w: SEG_W, h: SEG_H - 4.0 }, *a)).collect()
            }
            Item::Button { action, .. } => vec![(Rect { x: rect.x + rect.w - BUTTON_W, y: rect.y + (rect.h - BUTTON_H) / 2.0, w: BUTTON_W, h: BUTTON_H }, *action)],
        };
        out.push(Placed { item, rect, targets });
    }
    out
}

pub fn hit_at(section: Section, m: &Model, x: f32, y: f32) -> Option<Action> {
    if CLOSE.contains(x, y) {
        return Some(Action::Close);
    }
    if let Some((s, _)) = nav_rects().into_iter().find(|(_, r)| r.contains(x, y)) {
        return Some(Action::Go(s));
    }
    place(section_items(section, m)).into_iter().flat_map(|p| p.targets).find(|(r, _)| r.contains(x, y)).map(|(_, a)| a)
}

pub fn caption_at(x: f32, y: f32) -> bool {
    (0.0..WIDTH).contains(&x) && (0.0..CAPTION_H).contains(&y) && !CLOSE.contains(x, y)
}

pub struct SettingsWin {
    pub hwnd: HWND,
    renderer: Renderer,
    size: (i32, i32),
    scale: f32,
    pub section: Section,
    pub hovered: Option<Action>,
    pub model: Model,
    tracking: bool,
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
            let mut win = SettingsWin { hwnd, renderer, size, scale, section: Section::Home, hovered: None, model, tracking: false };
            win.draw();
            Some(win)
        }
    }

    pub fn draw(&mut self) {
        let placed = place(section_items(self.section, &self.model));
        let version = env!("CARGO_PKG_VERSION");
        let (title, hint) = (self.section.title(), self.section.hint(version));
        let view = SettingsView { section: self.section, placed: &placed, hovered: self.hovered, title, hint: &hint, version };
        let _ = self.renderer.present_settings(self.hwnd, self.size.0, self.size.1, self.scale, &view);
    }

    pub fn hit(&self, px: f32, py: f32) -> Option<Action> {
        hit_at(self.section, &self.model, px / self.scale, py / self.scale)
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
    fn the_overview_cards_summarise_the_current_settings() {
        let items = section_items(Section::Home, &model());
        let cards: Vec<(&str, &str)> = items.iter().filter_map(|i| match i { Item::Card { value, note, .. } => Some((value.as_str(), note.as_str())), _ => None }).collect();
        assert_eq!(cards.len(), 4);
        assert_eq!(cards[0], ("Tai thỏ", "Kính nổi, đang bật"));
        assert_eq!(cards[1].0, "8 trên 9");
        assert!(cards[1].1.starts_with("Ẩn:") && cards[1].1.contains("Giới hạn"), "the hidden page is named: {}", cards[1].1);
        assert_eq!(cards[2], ("1 trên 3 đang bật", "Duyệt ngay trên notch"));
        assert_eq!(cards[3], ("Tiếng Việt", "Khởi động cùng Windows"));
    }

    #[test]
    fn a_click_on_a_switch_row_or_a_segment_gives_its_action() {
        let m = model();
        let placed = place(section_items(Section::Look, &m));
        let row = placed.iter().find(|p| matches!(&p.item, Item::Switch { action: Action::Glass, .. })).unwrap();
        assert_eq!(hit_at(Section::Look, &m, row.rect.x + 40.0, row.rect.y + 10.0), Some(Action::Glass));
        let (seg, action) = placed.iter().find_map(|p| matches!(&p.item, Item::Choice { label, .. } if *label == "Kiểu kính").then(|| p.targets[1])).unwrap();
        assert_eq!(action, Action::GlassFlat(true));
        assert_eq!(hit_at(Section::Look, &m, seg.x + 5.0, seg.y + 5.0), Some(Action::GlassFlat(true)));
        assert_eq!(hit_at(Section::Look, &m, 2.0, HEIGHT - 2.0), None, "empty corner of the window");
    }

    #[test]
    fn the_sidebar_and_the_close_button_have_their_own_actions() {
        let m = model();
        for (s, r) in nav_rects() {
            assert_eq!(hit_at(Section::Home, &m, r.x + r.w / 2.0, r.y + r.h / 2.0), Some(Action::Go(s)));
        }
        assert_eq!(hit_at(Section::Pages, &m, CLOSE.x + 4.0, CLOSE.y + 4.0), Some(Action::Close));
        assert!(caption_at(300.0, 20.0) && !caption_at(CLOSE.x + 4.0, CLOSE.y + 4.0) && !caption_at(300.0, 200.0), "only the top strip drags the window");
    }

    #[test]
    fn every_section_fits_inside_the_window() {
        let m = model();
        for s in SECTIONS {
            for p in place(section_items(s, &m)) {
                assert!(p.rect.x >= CX && p.rect.x + p.rect.w <= WIDTH - 18.0 + 0.01, "{s:?} item sticks out sideways: {:?}", p.rect);
                assert!(p.rect.y + p.rect.h <= HEIGHT - 12.0, "{s:?} item runs past the bottom: {:?}", p.rect);
            }
        }
    }

    #[test]
    fn the_nine_page_chips_form_a_grid_of_three_columns() {
        let placed = place(section_items(Section::Pages, &model()));
        assert_eq!(placed.len(), PAGES.len());
        let xs: Vec<f32> = placed.iter().map(|p| p.rect.x).collect();
        assert_eq!(xs[0], xs[3]);
        assert!(xs[0] < xs[1] && xs[1] < xs[2]);
        assert!(placed[3].rect.y > placed[0].rect.y && placed[0].rect.y == placed[2].rect.y);
    }

    #[test]
    #[ignore = "shows a real window for a moment"]
    fn the_settings_window_opens_draws_and_closes() {
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
            win.section = Section::Look;
            win.hovered = win.hit(CX + 20.0, TOP + 10.0);
            win.draw();
            assert!(IsWindow(Some(hwnd)).as_bool());
            assert!(win.caption(300.0, 20.0) && !win.caption(300.0, 200.0));
            drop(win);
            assert!(!IsWindow(Some(hwnd)).as_bool(), "dropping the window destroys it");
        }
    }

    #[test]
    fn the_last_page_cannot_be_switched_off() {
        assert_eq!(toggled_pages(Page::Music.bit(), Page::Music), Page::Music.bit());
        assert_eq!(toggled_pages(Page::Music.bit() | Page::Todo.bit(), Page::Todo), Page::Music.bit());
        assert_eq!(toggled_pages(Page::Music.bit(), Page::Todo), Page::Music.bit() | Page::Todo.bit());
    }
}

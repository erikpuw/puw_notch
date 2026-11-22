#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod install;
mod shortcut;

use install::{
    default_install_dir, is_elevated, launch_installed_app, needs_elevation, perform_install,
    perform_uninstall, relaunch_elevated, InstallOptions, APP_NAME, APP_VERSION,
};
use std::cell::RefCell;
use std::path::PathBuf;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateFontW, CreatePen,
    CreateSolidBrush, DeleteDC, DeleteObject, DrawTextW, EndPaint, FillRect, GetStockObject,
    InvalidateRect, LineTo, MoveToEx, RoundRect, SelectObject, SetBkMode, SetTextColor,
    CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_PITCH, DT_CENTER,
    DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX, DT_SINGLELINE, DT_TOP, DT_VCENTER, DT_WORDBREAK,
    FF_DONTCARE, FW_NORMAL, FW_SEMIBOLD, HDC, HFONT, NULL_BRUSH, NULL_PEN, OUT_DEFAULT_PRECIS,
    PAINTSTRUCT, PS_SOLID, SRCCOPY, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
use windows::Win32::UI::WindowsAndMessaging::*;

const WM_MOUSELEAVE: u32 = 0x02A3;
const WM_AUTO_RUN: u32 = WM_APP + 1;
const WIN_W: f32 = 540.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Install,
    Uninstall,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Status {
    Ready,
    Done,
    Failed,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Hit {
    Lang,
    Browse,
    ChkStartup,
    ChkStartMenu,
    ChkDesktop,
    ChkLaunch,
    Primary,
    Cancel,
}

struct CliArgs {
    mode: Mode,
    auto_run: bool,
    is_vi: bool,
    dir: Option<PathBuf>,
    startup: bool,
    start_menu: bool,
    desktop: bool,
    launch: bool,
}

impl CliArgs {
    fn parse() -> Self {
        let self_is_uninstaller = std::env::current_exe()
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()))
            .map(|n| n.contains("uninstall"))
            .unwrap_or(false);

        let mut out = CliArgs {
            mode: if self_is_uninstaller { Mode::Uninstall } else { Mode::Install },
            auto_run: false,
            is_vi: true,
            dir: None,
            startup: true,
            start_menu: true,
            desktop: true,
            launch: true,
        };
        let args: Vec<String> = std::env::args().skip(1).collect();
        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
                "--uninstall" => out.mode = Mode::Uninstall,
                "--auto-install" => {
                    out.mode = Mode::Install;
                    out.auto_run = true;
                }
                "--auto-uninstall" => {
                    out.mode = Mode::Uninstall;
                    out.auto_run = true;
                }
                "--en" => out.is_vi = false,
                "--vi" => out.is_vi = true,
                "--dir" if i + 1 < args.len() => {
                    i += 1;
                    out.dir = Some(PathBuf::from(&args[i]));
                }
                "--startup=0" => out.startup = false,
                "--startup=1" => out.startup = true,
                "--startmenu=0" => out.start_menu = false,
                "--startmenu=1" => out.start_menu = true,
                "--desktop=0" => out.desktop = false,
                "--desktop=1" => out.desktop = true,
                "--launch=0" => out.launch = false,
                "--launch=1" => out.launch = true,
                _ => {}
            }
            i += 1;
        }
        out
    }
}

fn is_windows_dark_mode() -> bool {
    let mut val = 1u32;
    let mut size = size_of::<u32>() as u32;
    let ok = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut val as *mut u32).cast()),
            Some(&mut size),
        )
    };
    ok.is_ok() && val == 0
}

const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF((r as u32) | ((g as u32) << 8) | ((b as u32) << 16))
}

#[derive(Clone, Copy)]
struct RectF {
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
}

const fn rect(l: f32, t: f32, r: f32, b: f32) -> RectF {
    RectF {
        left: l,
        top: t,
        right: r,
        bottom: b,
    }
}

fn contains(r: RectF, x: f32, y: f32) -> bool {
    x >= r.left && x <= r.right && y >= r.top && y <= r.bottom
}

#[derive(Clone, Copy)]
enum TextStyle {
    Title,
    Bold,
    Body,
    Small,
    Btn,
}

struct GdiFonts {
    title: HFONT,
    bold: HFONT,
    body: HFONT,
    small: HFONT,
    btn: HFONT,
}

impl GdiFonts {
    fn new(dpi: f32) -> Self {
        let mk = |dip_px: f32, bold: bool| -> HFONT {
            let h = -((dip_px * dpi / 96.0).round() as i32);
            unsafe {
                CreateFontW(
                    h,
                    0,
                    0,
                    0,
                    if bold { FW_SEMIBOLD.0 as i32 } else { FW_NORMAL.0 as i32 },
                    0,
                    0,
                    0,
                    DEFAULT_CHARSET,
                    OUT_DEFAULT_PRECIS,
                    CLIP_DEFAULT_PRECIS,
                    CLEARTYPE_QUALITY,
                    (DEFAULT_PITCH.0 | FF_DONTCARE.0) as u32,
                    w!("Segoe UI"),
                )
            }
        };
        Self {
            title: mk(20.0, true),
            bold: mk(12.5, true),
            body: mk(12.5, false),
            small: mk(11.5, false),
            btn: mk(12.5, true),
        }
    }

    fn get(&self, style: TextStyle) -> HFONT {
        match style {
            TextStyle::Title => self.title,
            TextStyle::Bold => self.bold,
            TextStyle::Body => self.body,
            TextStyle::Small => self.small,
            TextStyle::Btn => self.btn,
        }
    }
}

impl Drop for GdiFonts {
    fn drop(&mut self) {
        unsafe {
            for f in [self.title, self.bold, self.body, self.small, self.btn] {
                if !f.is_invalid() {
                    let _ = DeleteObject(f.into());
                }
            }
        }
    }
}

struct Canvas<'a> {
    hdc: HDC,
    scale: f32,
    fonts: &'a GdiFonts,
}

impl<'a> Canvas<'a> {
    fn px(&self, v: f32) -> i32 {
        (v * self.scale).round() as i32
    }

    fn to_rect(&self, r: RectF) -> RECT {
        RECT {
            left: self.px(r.left),
            top: self.px(r.top),
            right: self.px(r.right),
            bottom: self.px(r.bottom),
        }
    }

    fn text(&self, s: &str, style: TextStyle, r: RectF, c: COLORREF) {
        let mut wide: Vec<u16> = s.encode_utf16().collect();
        let mut rc = self.to_rect(r);
        let flags = match style {
            TextStyle::Btn => DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            TextStyle::Small => DT_LEFT | DT_TOP | DT_WORDBREAK | DT_NOPREFIX,
            _ => DT_LEFT | DT_TOP | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
        };
        unsafe {
            let old = SelectObject(self.hdc, self.fonts.get(style).into());
            SetTextColor(self.hdc, c);
            let _ = DrawTextW(self.hdc, &mut wide, &mut rc, flags);
            SelectObject(self.hdc, old);
        }
    }

    fn fill_rrect(&self, r: RectF, rad: f32, c: COLORREF) {
        let rc = self.to_rect(r);
        let d = self.px(rad * 2.0);
        unsafe {
            let br = CreateSolidBrush(c);
            let old_br = SelectObject(self.hdc, br.into());
            let old_pen = SelectObject(self.hdc, GetStockObject(NULL_PEN));
            let _ = RoundRect(self.hdc, rc.left, rc.top, rc.right + 1, rc.bottom + 1, d, d);
            SelectObject(self.hdc, old_pen);
            SelectObject(self.hdc, old_br);
            let _ = DeleteObject(br.into());
        }
    }

    fn stroke_rrect(&self, r: RectF, rad: f32, c: COLORREF, w: f32) {
        let rc = self.to_rect(r);
        let d = self.px(rad * 2.0);
        let pw = self.px(w).max(1);
        unsafe {
            let pen = CreatePen(PS_SOLID, pw, c);
            let old_pen = SelectObject(self.hdc, pen.into());
            let old_br = SelectObject(self.hdc, GetStockObject(NULL_BRUSH));
            let _ = RoundRect(self.hdc, rc.left, rc.top, rc.right, rc.bottom, d, d);
            SelectObject(self.hdc, old_br);
            SelectObject(self.hdc, old_pen);
            let _ = DeleteObject(pen.into());
        }
    }

    fn line(&self, a: (f32, f32), b: (f32, f32), c: COLORREF, w: f32) {
        let pw = self.px(w).max(1);
        unsafe {
            let pen = CreatePen(PS_SOLID, pw, c);
            let old_pen = SelectObject(self.hdc, pen.into());
            let _ = MoveToEx(self.hdc, self.px(a.0), self.px(a.1), None);
            let _ = LineTo(self.hdc, self.px(b.0), self.px(b.1));
            SelectObject(self.hdc, old_pen);
            let _ = DeleteObject(pen.into());
        }
    }
}

struct App {
    hwnd: HWND,
    dpi: f32,
    px_w: i32,
    px_h: i32,
    fonts: GdiFonts,
    mode: Mode,
    is_vi: bool,
    dark: bool,
    elevated: bool,
    install_dir: PathBuf,
    enable_startup_task: bool,
    create_start_menu: bool,
    create_desktop: bool,
    launch_after: bool,
    status: Status,
    message: String,
    installed_exe: Option<PathBuf>,
    hovered: Option<Hit>,
    tracking: bool,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

impl App {
    fn win_h(&self) -> f32 {
        if self.mode == Mode::Install { 468.0 } else { 338.0 }
    }

    fn options(&self) -> InstallOptions {
        InstallOptions {
            install_dir: self.install_dir.clone(),
            create_start_menu: self.create_start_menu,
            create_desktop: self.create_desktop,
            enable_startup_task: self.enable_startup_task,
            launch_after: self.launch_after,
        }
    }

    fn layout_hits(&self) -> Vec<(Hit, RectF)> {
        let mut out = vec![(Hit::Lang, rect(WIN_W - 22.0 - 44.0, 22.0, WIN_W - 22.0, 48.0))];
        if self.mode == Mode::Install {
            out.push((Hit::Browse, rect(36.0, 114.0, WIN_W - 36.0, 144.0)));
            out.push((Hit::ChkStartup, rect(36.0, 154.0, WIN_W - 36.0, 178.0)));
            out.push((Hit::ChkStartMenu, rect(36.0, 180.0, WIN_W - 36.0, 204.0)));
            out.push((Hit::ChkDesktop, rect(36.0, 206.0, WIN_W - 36.0, 230.0)));
            out.push((Hit::ChkLaunch, rect(36.0, 232.0, WIN_W - 36.0, 256.0)));
        }
        let by = self.win_h() - 22.0 - 34.0;
        if self.status == Status::Done {
            out.push((Hit::Primary, rect(WIN_W - 22.0 - 116.0, by, WIN_W - 22.0, by + 34.0)));
        } else {
            out.push((Hit::Cancel, rect(WIN_W - 22.0 - 92.0, by, WIN_W - 22.0, by + 34.0)));
            out.push((
                Hit::Primary,
                rect(WIN_W - 22.0 - 92.0 - 10.0 - 138.0, by, WIN_W - 22.0 - 92.0 - 10.0, by + 34.0),
            ));
        }
        out
    }

    fn hit_at(&self, px_x: i32, px_y: i32) -> Option<Hit> {
        let (x, y) = (px_x as f32 * 96.0 / self.dpi, px_y as f32 * 96.0 / self.dpi);
        self.layout_hits()
            .into_iter()
            .find(|(_, r)| contains(*r, x, y))
            .map(|(h, _)| h)
    }

    fn repaint(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    fn draw(&self, target_hdc: HDC) {
        let (bg, card, border, input_bg, btn_bg, btn_hov, text_pri, text_sec, accent, accent_hov, warn_col, ok_col, err_col) =
            if self.dark {
                (
                    rgb(24, 24, 24),
                    rgb(35, 35, 35),
                    rgb(58, 58, 58),
                    rgb(28, 28, 28),
                    rgb(48, 48, 48),
                    rgb(62, 62, 62),
                    rgb(243, 243, 243),
                    rgb(168, 168, 168),
                    rgb(76, 194, 255),
                    rgb(104, 206, 255),
                    rgb(252, 200, 90),
                    rgb(108, 214, 134),
                    rgb(255, 110, 110),
                )
            } else {
                (
                    rgb(243, 243, 243),
                    rgb(255, 255, 255),
                    rgb(220, 220, 220),
                    rgb(249, 249, 249),
                    rgb(240, 240, 240),
                    rgb(228, 228, 228),
                    rgb(24, 24, 24),
                    rgb(96, 96, 96),
                    rgb(0, 103, 192),
                    rgb(18, 118, 206),
                    rgb(176, 104, 0),
                    rgb(20, 130, 56),
                    rgb(196, 43, 28),
                )
            };
        let accent_ink = if self.dark { rgb(12, 18, 24) } else { rgb(255, 255, 255) };

        let (mem_dc, bmp, old_bmp) = unsafe {
            let mem_dc = CreateCompatibleDC(Some(target_hdc));
            let bmp = CreateCompatibleBitmap(target_hdc, self.px_w, self.px_h);
            let old_bmp = SelectObject(mem_dc, bmp.into());
            SetBkMode(mem_dc, TRANSPARENT);
            let bg_br = CreateSolidBrush(bg);
            let full = RECT { left: 0, top: 0, right: self.px_w, bottom: self.px_h };
            FillRect(mem_dc, &full, bg_br);
            let _ = DeleteObject(bg_br.into());
            (mem_dc, bmp, old_bmp)
        };

        let g = Canvas {
            hdc: mem_dc,
            scale: self.dpi / 96.0,
            fonts: &self.fonts,
        };

        // Header
        g.text(&format!("{APP_NAME} v{APP_VERSION} Setup"), TextStyle::Title, rect(22.0, 18.0, 460.0, 46.0), text_pri);
        let sub = match (self.mode, self.is_vi) {
            (Mode::Install, true) => "Trình cài đặt chiếc notch sống cho Windows",
            (Mode::Install, false) => "Installer for a living notch on Windows",
            (Mode::Uninstall, true) => "Gỡ cài đặt PUW Notch khỏi máy tính này",
            (Mode::Uninstall, false) => "Uninstall PUW Notch from this computer",
        };
        g.text(sub, TextStyle::Small, rect(22.0, 46.0, 460.0, 66.0), text_sec);

        // EN / VI button
        let lang_r = rect(WIN_W - 22.0 - 44.0, 22.0, WIN_W - 22.0, 48.0);
        g.fill_rrect(lang_r, 6.0, if self.hovered == Some(Hit::Lang) { btn_hov } else { btn_bg });
        g.stroke_rrect(lang_r, 6.0, border, 1.0);
        g.text(if self.is_vi { "EN" } else { "VI" }, TextStyle::Btn, lang_r, text_pri);

        // Card 1: Folder + options
        let card1_b = if self.mode == Mode::Install { 268.0 } else { 140.0 };
        let card1 = rect(22.0, 76.0, WIN_W - 22.0, card1_b);
        g.fill_rrect(card1, 8.0, card);
        g.stroke_rrect(card1, 8.0, border, 1.0);

        if self.mode == Mode::Install {
            g.text(
                if self.is_vi { "Thư mục cài đặt" } else { "Installation folder" },
                TextStyle::Bold,
                rect(36.0, 90.0, 360.0, 110.0),
                text_pri,
            );
            // Path display + Browse button
            let path_r = rect(36.0, 114.0, WIN_W - 150.0, 144.0);
            g.fill_rrect(path_r, 6.0, input_bg);
            g.stroke_rrect(path_r, 6.0, if self.hovered == Some(Hit::Browse) { accent } else { border }, 1.0);
            g.text(
                &self.install_dir.display().to_string(),
                TextStyle::Body,
                rect(path_r.left + 10.0, path_r.top + 7.0, path_r.right - 8.0, path_r.bottom - 4.0),
                text_pri,
            );
            let browse_r = rect(WIN_W - 142.0, 114.0, WIN_W - 36.0, 144.0);
            g.fill_rrect(browse_r, 6.0, if self.hovered == Some(Hit::Browse) { btn_hov } else { btn_bg });
            g.stroke_rrect(browse_r, 6.0, border, 1.0);
            g.text(if self.is_vi { "Chọn thư mục…" } else { "Browse…" }, TextStyle::Btn, browse_r, text_pri);

            let checkboxes = [
                (
                    Hit::ChkStartup,
                    154.0,
                    self.enable_startup_task,
                    if self.is_vi {
                        "Khởi động cùng Windows (Task Scheduler, không chớp cửa sổ)"
                    } else {
                        "Start with Windows (Task Scheduler, no console flash)"
                    },
                ),
                (
                    Hit::ChkStartMenu,
                    180.0,
                    self.create_start_menu,
                    if self.is_vi { "Tạo lối tắt trong Start Menu" } else { "Create Start Menu shortcut" },
                ),
                (
                    Hit::ChkDesktop,
                    206.0,
                    self.create_desktop,
                    if self.is_vi { "Tạo lối tắt ngoài Desktop" } else { "Create Desktop shortcut" },
                ),
                (
                    Hit::ChkLaunch,
                    232.0,
                    self.launch_after,
                    if self.is_vi { "Mở PUW Notch sau khi cài xong" } else { "Launch PUW Notch after installation" },
                ),
            ];
            for (hit, y, checked, label) in checkboxes {
                let box_r = rect(36.0, y + 3.0, 52.0, y + 19.0);
                if checked {
                    g.fill_rrect(box_r, 4.0, accent);
                    g.line((box_r.left + 3.5, box_r.top + 8.0), (box_r.left + 6.5, box_r.top + 11.5), accent_ink, 1.8);
                    g.line((box_r.left + 6.5, box_r.top + 11.5), (box_r.left + 12.5, box_r.top + 4.5), accent_ink, 1.8);
                } else {
                    g.fill_rrect(box_r, 4.0, input_bg);
                    g.stroke_rrect(box_r, 4.0, if self.hovered == Some(hit) { accent } else { border }, 1.2);
                }
                g.text(label, TextStyle::Body, rect(62.0, y + 3.0, WIN_W - 36.0, y + 22.0), text_pri);
            }
        } else {
            g.text(
                if self.is_vi { "Gỡ cài đặt PUW Notch khỏi máy tính này" } else { "Uninstall PUW Notch from this computer" },
                TextStyle::Bold,
                rect(36.0, 90.0, WIN_W - 36.0, 110.0),
                text_pri,
            );
            g.text(
                &format!("{}: {}", if self.is_vi { "Thư mục" } else { "Folder" }, self.install_dir.display()),
                TextStyle::Small,
                rect(36.0, 112.0, WIN_W - 36.0, 132.0),
                text_sec,
            );
        }

        // Card 2: Permissions & system changes
        let opts = self.options();
        let need_uac = match self.mode {
            Mode::Install => needs_elevation(&opts),
            Mode::Uninstall => true,
        };
        let c2_top = card1_b + 10.0;
        let c2_bot = c2_top + 96.0;
        let card2 = rect(22.0, c2_top, WIN_W - 22.0, c2_bot);
        g.fill_rrect(card2, 8.0, card);
        g.stroke_rrect(card2, 8.0, border, 1.0);

        g.text(
            if self.is_vi { "Quyền & thay đổi trên máy" } else { "Permissions & system changes" },
            TextStyle::Bold,
            rect(36.0, c2_top + 10.0, WIN_W - 36.0, c2_top + 28.0),
            text_pri,
        );
        let (uac_txt, uac_col) = if self.elevated {
            (if self.is_vi { "• Quyền quản trị (UAC): Đã cấp" } else { "• Administrator (UAC): Granted" }, ok_col)
        } else if need_uac {
            (
                if self.is_vi {
                    "• Quyền quản trị (UAC): Sẽ hỏi khi bấm để ghi thư mục hệ thống / tạo Task"
                } else {
                    "• Administrator (UAC): Will prompt to write system folder / startup task"
                },
                warn_col,
            )
        } else {
            (
                if self.is_vi {
                    "• Quyền quản trị (UAC): Không cần (cài trong thư mục người dùng)"
                } else {
                    "• Administrator (UAC): Not required (user folder install)"
                },
                ok_col,
            )
        };
        g.text(uac_txt, TextStyle::Small, rect(36.0, c2_top + 31.0, WIN_W - 36.0, c2_top + 48.0), uac_col);

        let detail_txt = match self.mode {
            Mode::Install => {
                let task = if self.enable_startup_task {
                    if self.is_vi {
                        "• Tạo Scheduled Task \"PuwNotch\" chạy thẳng puw_notch.exe (HighestAvailable)"
                    } else {
                        "• Creates Scheduled Task \"PuwNotch\" launching puw_notch.exe directly (HighestAvailable)"
                    }
                } else if self.is_vi {
                    "• Không tạo lịch tự khởi động cùng Windows"
                } else {
                    "• No Windows startup task will be created"
                };
                let reg = if self.is_vi {
                    "• Đăng ký gỡ cài đặt trong Windows Settings -> Apps"
                } else {
                    "• Registers uninstall entry in Windows Settings -> Apps"
                };
                format!("{task}\n{reg}")
            }
            Mode::Uninstall => {
                if self.is_vi {
                    "• Tắt PUW Notch, xoá Scheduled Task, lối tắt và mục Settings -> Apps\n• Giữ nguyên dữ liệu người dùng tại %APPDATA%\\PuwNotch".into()
                } else {
                    "• Stops PUW Notch, removes Scheduled Task, shortcuts and Settings -> Apps entry\n• Keeps user data in %APPDATA%\\PuwNotch".into()
                }
            }
        };
        g.text(&detail_txt, TextStyle::Small, rect(36.0, c2_top + 50.0, WIN_W - 36.0, c2_bot - 6.0), text_sec);

        // Footer note & status message
        g.text(
            if self.is_vi {
                "Bản cài đặt chưa có chữ ký số (Unsigned) · Chỉ tải từ github.com/erikpuw/puw_notch"
            } else {
                "Unsigned installer build · Download only from github.com/erikpuw/puw_notch"
            },
            TextStyle::Small,
            rect(22.0, c2_bot + 8.0, WIN_W - 22.0, c2_bot + 24.0),
            text_sec,
        );

        if !self.message.is_empty() {
            let col = match self.status {
                Status::Done => accent,
                Status::Failed => err_col,
                Status::Ready => text_sec,
            };
            g.text(&self.message, TextStyle::Small, rect(22.0, c2_bot + 26.0, WIN_W - 22.0, self.win_h() - 58.0), col);
        }

        // Bottom buttons
        let by = self.win_h() - 22.0 - 34.0;
        if self.status == Status::Done {
            let fin_r = rect(WIN_W - 22.0 - 116.0, by, WIN_W - 22.0, by + 34.0);
            g.fill_rrect(fin_r, 6.0, if self.hovered == Some(Hit::Primary) { accent_hov } else { accent });
            g.text(if self.is_vi { "Hoàn tất" } else { "Finish" }, TextStyle::Btn, fin_r, accent_ink);
        } else {
            let pri_r = rect(WIN_W - 22.0 - 92.0 - 10.0 - 138.0, by, WIN_W - 22.0 - 92.0 - 10.0, by + 34.0);
            g.fill_rrect(pri_r, 6.0, if self.hovered == Some(Hit::Primary) { accent_hov } else { accent });
            let pri_txt = match (self.mode, self.is_vi, need_uac && !self.elevated) {
                (Mode::Install, true, true) => "Cài đặt (UAC)",
                (Mode::Install, true, false) => "Cài đặt",
                (Mode::Install, false, true) => "Install (UAC)",
                (Mode::Install, false, false) => "Install",
                (Mode::Uninstall, true, true) => "Gỡ cài đặt (UAC)",
                (Mode::Uninstall, true, false) => "Gỡ cài đặt",
                (Mode::Uninstall, false, true) => "Uninstall (UAC)",
                (Mode::Uninstall, false, false) => "Uninstall",
            };
            g.text(pri_txt, TextStyle::Btn, pri_r, accent_ink);

            let can_r = rect(WIN_W - 22.0 - 92.0, by, WIN_W - 22.0, by + 34.0);
            g.fill_rrect(can_r, 6.0, if self.hovered == Some(Hit::Cancel) { btn_hov } else { btn_bg });
            g.stroke_rrect(can_r, 6.0, border, 1.0);
            g.text(if self.is_vi { "Đóng" } else { "Cancel" }, TextStyle::Btn, can_r, text_pri);
        }

        unsafe {
            let _ = BitBlt(target_hdc, 0, 0, self.px_w, self.px_h, Some(mem_dc), 0, 0, SRCCOPY);
            SelectObject(mem_dc, old_bmp);
            let _ = DeleteObject(bmp.into());
            let _ = DeleteDC(mem_dc);
        }
    }

    fn run_primary(&mut self) {
        if self.status == Status::Done {
            if self.mode == Mode::Install && self.launch_after {
                if let Some(exe) = &self.installed_exe {
                    launch_installed_app(exe);
                }
            }
            unsafe { let _ = PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)); }
            return;
        }
        let opts = self.options();
        let need_uac = match self.mode {
            Mode::Install => needs_elevation(&opts),
            Mode::Uninstall => true,
        };
        if need_uac && !self.elevated {
            let lang = if self.is_vi { "--vi" } else { "--en" };
            let dir_escaped = opts.install_dir.display().to_string().replace('"', "");
            let args = match self.mode {
                Mode::Install => format!(
                    "--auto-install {lang} --dir \"{dir_escaped}\" --startup={} --startmenu={} --desktop={} --launch={}",
                    opts.enable_startup_task as u8,
                    opts.create_start_menu as u8,
                    opts.create_desktop as u8,
                    opts.launch_after as u8
                ),
                Mode::Uninstall => format!("--auto-uninstall {lang} --dir \"{dir_escaped}\""),
            };
            match relaunch_elevated(&args) {
                Ok(()) => unsafe { let _ = PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)); },
                Err(err) => {
                    self.status = Status::Failed;
                    self.message = if self.is_vi {
                        format!("Đã huỷ xin quyền quản trị (UAC): {err}")
                    } else {
                        format!("UAC elevation cancelled: {err}")
                    };
                    self.repaint();
                }
            }
            return;
        }

        match self.mode {
            Mode::Install => match perform_install(&opts) {
                Ok(exe) => {
                    self.installed_exe = Some(exe);
                    self.status = Status::Done;
                    self.message = if self.is_vi {
                        format!("Đã cài đặt thành công {APP_NAME} v{APP_VERSION}!")
                    } else {
                        format!("{APP_NAME} v{APP_VERSION} installed successfully!")
                    };
                }
                Err(err) => {
                    self.status = Status::Failed;
                    self.message = err;
                }
            },
            Mode::Uninstall => match perform_uninstall(&opts.install_dir) {
                Ok(()) => {
                    self.status = Status::Done;
                    self.message = if self.is_vi {
                        format!("Đã gỡ cài đặt {APP_NAME} khỏi máy.")
                    } else {
                        format!("{APP_NAME} has been uninstalled.")
                    };
                }
                Err(err) => {
                    self.status = Status::Failed;
                    self.message = err;
                }
            },
        }
        self.repaint();
    }

    fn on_click(&mut self, hit: Hit) {
        match hit {
            Hit::Lang => self.is_vi = !self.is_vi,
            Hit::Browse => return,
            Hit::ChkStartup => self.enable_startup_task = !self.enable_startup_task,
            Hit::ChkStartMenu => self.create_start_menu = !self.create_start_menu,
            Hit::ChkDesktop => self.create_desktop = !self.create_desktop,
            Hit::ChkLaunch => self.launch_after = !self.launch_after,
            Hit::Primary => {
                self.run_primary();
                return;
            }
            Hit::Cancel => {
                unsafe { let _ = PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)); }
                return;
            }
        }
        self.repaint();
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            unsafe {
                let mut ps = PAINTSTRUCT::default();
                let hdc = BeginPaint(hwnd, &mut ps);
                APP.with(|a| {
                    if let Some(app) = a.borrow().as_ref() {
                        app.draw(hdc);
                    }
                });
                let _ = EndPaint(hwnd, &ps);
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_MOUSEMOVE => {
            let x = (lp.0 & 0xFFFF) as i16 as i32;
            let y = ((lp.0 >> 16) & 0xFFFF) as i16 as i32;
            APP.with(|a| {
                if let Some(app) = a.borrow_mut().as_mut() {
                    if !app.tracking {
                        let mut tme = TRACKMOUSEEVENT {
                            cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                            dwFlags: TME_LEAVE,
                            hwndTrack: hwnd,
                            dwHoverTime: 0,
                        };
                        app.tracking = unsafe { TrackMouseEvent(&mut tme) }.is_ok();
                    }
                    let h = app.hit_at(x, y);
                    if h != app.hovered {
                        app.hovered = h;
                        app.repaint();
                    }
                }
            });
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            APP.with(|a| {
                if let Some(app) = a.borrow_mut().as_mut() {
                    app.tracking = false;
                    if app.hovered.take().is_some() {
                        app.repaint();
                    }
                }
            });
            LRESULT(0)
        }
        WM_SETCURSOR => {
            let hand = APP.with(|a| a.borrow().as_ref().is_some_and(|app| app.hovered.is_some()));
            if hand && (lp.0 & 0xFFFF) as u32 == HTCLIENT {
                unsafe {
                    if let Ok(cur) = LoadCursorW(None, IDC_HAND) {
                        SetCursor(Some(cur));
                        return LRESULT(1);
                    }
                }
            }
            unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
        }
        WM_LBUTTONDOWN => {
            let x = (lp.0 & 0xFFFF) as i16 as i32;
            let y = ((lp.0 >> 16) & 0xFFFF) as i16 as i32;
            let hit = APP.with(|a| a.borrow().as_ref().and_then(|app| app.hit_at(x, y)));
            if let Some(h) = hit {
                if h == Hit::Browse {
                    let info = APP.with(|a| {
                        a.borrow().as_ref().map(|app| (app.hwnd, app.install_dir.clone(), app.is_vi))
                    });
                    if let Some((owner, cur_dir, is_vi)) = info {
                        let title = if is_vi {
                            "Chọn thư mục cài đặt PUW Notch"
                        } else {
                            "Choose PUW Notch installation folder"
                        };
                        if let Some(chosen) = shortcut::pick_folder(Some(owner), &cur_dir, title) {
                            APP.with(|a| {
                                if let Some(app) = a.borrow_mut().as_mut() {
                                    app.install_dir = if chosen
                                        .file_name()
                                        .and_then(|n| n.to_str())
                                        .is_some_and(|n| {
                                            n.eq_ignore_ascii_case(APP_NAME)
                                                || n.eq_ignore_ascii_case("PuwNotch")
                                        })
                                    {
                                        chosen
                                    } else {
                                        chosen.join(APP_NAME)
                                    };
                                    app.repaint();
                                }
                            });
                        }
                    }
                } else {
                    APP.with(|a| {
                        if let Some(app) = a.borrow_mut().as_mut() {
                            app.on_click(h);
                        }
                    });
                }
            }
            LRESULT(0)
        }
        WM_AUTO_RUN => {
            APP.with(|a| {
                if let Some(app) = a.borrow_mut().as_mut() {
                    app.run_primary();
                }
            });
            LRESULT(0)
        }
        WM_DESTROY => {
            APP.with(|a| {
                a.borrow_mut().take();
            });
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

fn main() {
    let cli = CliArgs::parse();
    let dark = is_windows_dark_mode();
    let win_h = if cli.mode == Mode::Install { 468.0 } else { 338.0 };

    unsafe {
        let hinst = GetModuleHandleW(None).unwrap_or_default();
        let class = w!("PuwNotchSetupGDI");
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hIcon: LoadIconW(Some(hinst.into()), PCWSTR(1 as *const u16)).unwrap_or_default(),
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassExW(&wc);

        let title = windows::core::HSTRING::from(format!("{APP_NAME} v{APP_VERSION} Setup"));
        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class,
            &title,
            style,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            WIN_W as i32,
            win_h as i32,
            None,
            None,
            Some(hinst.into()),
            None,
        )
        .expect("CreateWindowExW failed");

        let dark_flag = dark as i32;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            (&dark_flag as *const i32).cast(),
            size_of::<i32>() as u32,
        );

        let dpi = GetDpiForWindow(hwnd).max(96) as f32;
        let px_w = (WIN_W * dpi / 96.0).round() as i32;
        let px_h = (win_h * dpi / 96.0).round() as i32;
        let mut wr = RECT {
            left: 0,
            top: 0,
            right: px_w,
            bottom: px_h,
        };
        let _ = AdjustWindowRectEx(&mut wr, style, false, WINDOW_EX_STYLE::default());
        let ww = wr.right - wr.left;
        let wh = wr.bottom - wr.top;
        let sx = (GetSystemMetrics(SM_CXSCREEN) - ww) / 2;
        let sy = (GetSystemMetrics(SM_CYSCREEN) - wh) / 2;
        let _ = SetWindowPos(hwnd, None, sx.max(0), sy.max(0), ww, wh, SWP_NOZORDER);

        let fonts = GdiFonts::new(dpi);
        let initial_dir = cli.dir.unwrap_or_else(|| {
            if cli.mode == Mode::Uninstall {
                std::env::current_exe()
                    .ok()
                    .and_then(|p| p.parent().map(|d| d.to_path_buf()))
                    .unwrap_or_else(default_install_dir)
            } else {
                default_install_dir()
            }
        });

        let app = App {
            hwnd,
            dpi,
            px_w,
            px_h,
            fonts,
            mode: cli.mode,
            is_vi: cli.is_vi,
            dark,
            elevated: is_elevated(),
            install_dir: initial_dir,
            enable_startup_task: cli.startup,
            create_start_menu: cli.start_menu,
            create_desktop: cli.desktop,
            launch_after: cli.launch,
            status: Status::Ready,
            message: String::new(),
            installed_exe: None,
            hovered: None,
            tracking: false,
        };
        APP.with(|a| *a.borrow_mut() = Some(app));

        let _ = ShowWindow(hwnd, SW_SHOW);
        if cli.auto_run {
            let _ = PostMessageW(Some(hwnd), WM_AUTO_RUN, WPARAM(0), LPARAM(0));
        }

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

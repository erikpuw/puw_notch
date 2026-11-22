#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod install;
mod shortcut;

use install::{
    default_install_dir, is_elevated, launch_installed_app, needs_elevation, path_needs_admin,
    perform_install, perform_uninstall, relaunch_elevated, InstallOptions, APP_NAME, APP_VERSION,
};
use std::cell::RefCell;
use std::path::PathBuf;
use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateFontW, CreateSolidBrush, DeleteObject, GetStockObject, SetBkColor, SetBkMode,
    SetTextColor, CLEARTYPE_QUALITY, DEFAULT_CHARSET, DEFAULT_PITCH, FF_DONTCARE, FW_NORMAL,
    FW_SEMIBOLD, HBRUSH, HDC, HFONT, HGDIOBJ, OUT_DEFAULT_PRECIS, TRANSPARENT,
    WHITE_BRUSH,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{InitCommonControlsEx, ICC_STANDARD_CLASSES, INITCOMMONCONTROLSEX};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows::Win32::UI::WindowsAndMessaging::*;

const BST_UNCHECKED: usize = 0;
const BST_CHECKED: usize = 1;

const ID_LANG: isize = 101;
const ID_DIR_EDIT: isize = 102;
const ID_BROWSE: isize = 103;
const ID_CHK_STARTUP: isize = 104;
const ID_CHK_STARTMENU: isize = 105;
const ID_CHK_DESKTOP: isize = 106;
const ID_CHK_LAUNCH: isize = 107;
const ID_ACTION: isize = 108;
const ID_CLOSE: isize = 109;
const ID_TITLE: isize = 110;
const ID_SUBTITLE: isize = 111;
const ID_DIR_LABEL: isize = 112;
const ID_PERM_TITLE: isize = 113;
const ID_PERM_BODY: isize = 114;
const ID_STATUS: isize = 115;

const BCM_SETSHIELD: u32 = 0x160C;
const WM_AUTO_RUN: u32 = WM_APP + 1;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Install,
    Uninstall,
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
        let mut out = CliArgs {
            mode: Mode::Install,
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

struct UiState {
    hwnd: HWND,
    mode: Mode,
    is_vi: bool,
    elevated: bool,
    done: bool,
    failed: bool,
    status_text: String,
    dir_edit: HWND,
    browse_btn: HWND,
    lang_btn: HWND,
    chk_startup: HWND,
    chk_startmenu: HWND,
    chk_desktop: HWND,
    chk_launch: HWND,
    action_btn: HWND,
    close_btn: HWND,
    title_lbl: HWND,
    subtitle_lbl: HWND,
    dir_lbl: HWND,
    perm_title_lbl: HWND,
    perm_body_lbl: HWND,
    status_lbl: HWND,
    font_title: HFONT,
    font_bold: HFONT,
    font_body: HFONT,
    bg_brush: HBRUSH,
    card_brush: HBRUSH,
}

thread_local! {
    static STATE: RefCell<Option<UiState>> = const { RefCell::new(None) };
}

fn set_text(hwnd: HWND, text: &str) {
    let h = HSTRING::from(text);
    unsafe {
        let _ = SetWindowTextW(hwnd, &h);
    }
}

fn get_text(hwnd: HWND) -> String {
    unsafe {
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return String::new();
        }
        let mut buf = vec![0u16; len as usize + 1];
        let got = GetWindowTextW(hwnd, &mut buf);
        String::from_utf16_lossy(&buf[..got as usize])
    }
}

fn is_checked(hwnd: HWND) -> bool {
    unsafe { SendMessageW(hwnd, BM_GETCHECK, Some(WPARAM(0)), Some(LPARAM(0))).0 == BST_CHECKED as isize }
}

fn set_checked(hwnd: HWND, on: bool) {
    let v = if on { BST_CHECKED } else { BST_UNCHECKED };
    unsafe {
        let _ = SendMessageW(hwnd, BM_SETCHECK, Some(WPARAM(v)), Some(LPARAM(0)));
    }
}

fn set_shield(hwnd: HWND, on: bool) {
    unsafe {
        let _ = SendMessageW(hwnd, BCM_SETSHIELD, Some(WPARAM(0)), Some(LPARAM(on as isize)));
    }
}

fn set_font(hwnd: HWND, font: HFONT) {
    unsafe {
        let _ = SendMessageW(hwnd, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(1)));
    }
}

impl UiState {
    fn current_options(&self) -> InstallOptions {
        let raw = get_text(self.dir_edit).trim().to_string();
        let install_dir = if raw.is_empty() {
            default_install_dir()
        } else {
            PathBuf::from(raw)
        };
        InstallOptions {
            install_dir,
            create_start_menu: is_checked(self.chk_startmenu),
            create_desktop: is_checked(self.chk_desktop),
            enable_startup_task: is_checked(self.chk_startup),
            launch_after: is_checked(self.chk_launch),
        }
    }

    fn refresh_texts(&self) {
        let vi = self.is_vi;
        set_text(
            self.hwnd,
            &format!(
                "{APP_NAME} v{APP_VERSION} {}",
                match (self.mode, vi) {
                    (Mode::Install, true) => "Cài đặt",
                    (Mode::Install, false) => "Setup",
                    (Mode::Uninstall, true) => "Gỡ cài đặt",
                    (Mode::Uninstall, false) => "Uninstall",
                }
            ),
        );
        set_text(self.lang_btn, if vi { "EN" } else { "VI" });
        set_text(self.title_lbl, &format!("{APP_NAME} v{APP_VERSION}"));
        set_text(
            self.subtitle_lbl,
            match (self.mode, vi) {
                (Mode::Install, true) => "Trình cài đặt PUW Notch cho Windows",
                (Mode::Install, false) => "PUW Notch Installer for Windows",
                (Mode::Uninstall, true) => "Gỡ bỏ PUW Notch khỏi máy tính này",
                (Mode::Uninstall, false) => "Remove PUW Notch from this computer",
            },
        );
        set_text(
            self.dir_lbl,
            if vi { "Thư mục cài đặt:" } else { "Installation folder:" },
        );
        set_text(self.browse_btn, if vi { "Chọn thư mục…" } else { "Browse…" });
        set_text(
            self.chk_startup,
            if vi {
                "Khởi động cùng Windows (Task Scheduler, không chớp cửa sổ)"
            } else {
                "Start with Windows (Task Scheduler, no console flash)"
            },
        );
        set_text(
            self.chk_startmenu,
            if vi { "Tạo lối tắt trong Start Menu" } else { "Create Start Menu shortcut" },
        );
        set_text(
            self.chk_desktop,
            if vi { "Tạo lối tắt ngoài Desktop" } else { "Create Desktop shortcut" },
        );
        set_text(
            self.chk_launch,
            if vi { "Mở PUW Notch sau khi cài xong" } else { "Launch PUW Notch after installation" },
        );
        set_text(
            self.perm_title_lbl,
            if vi { "Quyền & thay đổi trên máy:" } else { "Permissions & system changes:" },
        );
        set_text(
            self.close_btn,
            if self.done {
                if vi { "Đóng" } else { "Close" }
            } else if vi {
                "Huỷ"
            } else {
                "Cancel"
            },
        );
        self.refresh_permissions();
    }

    fn refresh_permissions(&self) {
        let vi = self.is_vi;
        let opts = self.current_options();
        let need_uac = match self.mode {
            Mode::Install => needs_elevation(&opts),
            Mode::Uninstall => path_needs_admin(&opts.install_dir) || true,
        };
        let show_shield = !self.done && need_uac && !self.elevated;
        set_shield(self.action_btn, show_shield);

        let action_label = if self.done {
            if vi { "Đã xong ✓" } else { "Done ✓" }
        } else {
            match (self.mode, vi) {
                (Mode::Install, true) => "Cài đặt",
                (Mode::Install, false) => "Install",
                (Mode::Uninstall, true) => "Gỡ cài đặt",
                (Mode::Uninstall, false) => "Uninstall",
            }
        };
        set_text(self.action_btn, action_label);

        let body = match self.mode {
            Mode::Install => {
                let uac_line = if self.elevated {
                    if vi {
                        "• Quyền quản trị (UAC): Đã cấp ✓"
                    } else {
                        "• Administrator (UAC): Granted ✓"
                    }
                } else if need_uac {
                    if vi {
                        "• Quyền quản trị (UAC): Sẽ hỏi khi bấm Cài đặt (ghi thư mục hệ thống / tạo Task)"
                    } else {
                        "• Administrator (UAC): Will prompt on Install (system folder / startup task)"
                    }
                } else if vi {
                    "• Quyền quản trị (UAC): Không cần (cài trong thư mục người dùng)"
                } else {
                    "• Administrator (UAC): Not required (user folder install)"
                };
                let task_line = if opts.enable_startup_task {
                    if vi {
                        "• Tạo Scheduled Task \"PuwNotch\" chạy thẳng puw_notch.exe (HighestAvailable)"
                    } else {
                        "• Creates Scheduled Task \"PuwNotch\" launching puw_notch.exe directly (HighestAvailable)"
                    }
                } else if vi {
                    "• Không tạo lịch tự khởi động cùng Windows"
                } else {
                    "• No Windows startup task will be created"
                };
                let dir_line = if vi {
                    format!("• Ghi puw_notch.exe + puw_notch_uninstall.exe vào: {}", opts.install_dir.display())
                } else {
                    format!("• Writes puw_notch.exe + puw_notch_uninstall.exe to: {}", opts.install_dir.display())
                };
                let reg_line = if vi {
                    "• Đăng ký mục Gỡ cài đặt trong Windows Settings → Apps"
                } else {
                    "• Registers Uninstall entry in Windows Settings → Apps"
                };
                format!("{uac_line}\r\n{dir_line}\r\n{task_line}\r\n{reg_line}")
            }
            Mode::Uninstall => {
                let uac_line = if self.elevated {
                    if vi {
                        "• Quyền quản trị (UAC): Đã cấp ✓"
                    } else {
                        "• Administrator (UAC): Granted ✓"
                    }
                } else if vi {
                    "• Quyền quản trị (UAC): Sẽ hỏi khi bấm Gỡ cài đặt để xoá Scheduled Task & thư mục cài"
                } else {
                    "• Administrator (UAC): Will prompt on Uninstall to remove Scheduled Task & folder"
                };
                let detail = if vi {
                    format!(
                        "• Tắt PUW Notch đang chạy, xoá Scheduled Task \"PuwNotch\"\r\n• Xoá lối tắt Start Menu, Desktop và mục trong Settings → Apps\r\n• Xoá tệp chương trình tại: {} (giữ nguyên dữ liệu trong %APPDATA%\\PuwNotch)",
                        opts.install_dir.display()
                    )
                } else {
                    format!(
                        "• Stops running PUW Notch and removes Scheduled Task \"PuwNotch\"\r\n• Removes Start Menu / Desktop shortcuts and Settings → Apps entry\r\n• Removes program files at: {} (keeps user data in %APPDATA%\\PuwNotch)",
                        opts.install_dir.display()
                    )
                };
                format!("{uac_line}\r\n{detail}")
            }
        };
        set_text(self.perm_body_lbl, &body);
        set_text(self.status_lbl, &self.status_text);
    }

    fn run_action(&mut self) {
        if self.done {
            return;
        }
        let opts = self.current_options();
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
            match relaunch_elevated(self.hwnd, &args) {
                Ok(()) => unsafe {
                    PostQuitMessage(0);
                },
                Err(e) => {
                    self.failed = true;
                    self.status_text = if self.is_vi {
                        format!("Đã huỷ xin quyền quản trị (UAC): {e}")
                    } else {
                        format!("UAC elevation cancelled: {e}")
                    };
                    self.refresh_texts();
                }
            }
            return;
        }

        match self.mode {
            Mode::Install => match perform_install(&opts) {
                Ok(exe) => {
                    self.done = true;
                    self.failed = false;
                    self.status_text = if self.is_vi {
                        format!("Cài đặt thành công vào {}", opts.install_dir.display())
                    } else {
                        format!("Installed successfully to {}", opts.install_dir.display())
                    };
                    unsafe {
                        let _ = EnableWindow(self.action_btn, false);
                    }
                    if opts.launch_after {
                        launch_installed_app(&exe);
                    }
                }
                Err(e) => {
                    self.failed = true;
                    self.status_text = if self.is_vi {
                        format!("Lỗi cài đặt: {e}")
                    } else {
                        format!("Install failed: {e}")
                    };
                }
            },
            Mode::Uninstall => match perform_uninstall(&opts.install_dir) {
                Ok(()) => {
                    self.done = true;
                    self.failed = false;
                    self.status_text = if self.is_vi {
                        "Đã gỡ cài đặt PUW Notch thành công.".into()
                    } else {
                        "PUW Notch has been uninstalled.".into()
                    };
                    unsafe {
                        let _ = EnableWindow(self.action_btn, false);
                    }
                }
                Err(e) => {
                    self.failed = true;
                    self.status_text = if self.is_vi {
                        format!("Lỗi gỡ cài đặt: {e}")
                    } else {
                        format!("Uninstall failed: {e}")
                    };
                }
            },
        }
        self.refresh_texts();
    }
}

fn make_font(dpi: i32, pt: i32, bold: bool) -> HFONT {
    let height = -((pt * dpi) / 72);
    unsafe {
        CreateFontW(
            height,
            0,
            0,
            0,
            if bold { FW_SEMIBOLD.0 as i32 } else { FW_NORMAL.0 as i32 },
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            windows::Win32::Graphics::Gdi::CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            (DEFAULT_PITCH.0 | FF_DONTCARE.0) as u32,
            w!("Segoe UI"),
        )
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_COMMAND => {
            let id = (wp.0 & 0xFFFF) as isize;
            let code = ((wp.0 >> 16) & 0xFFFF) as u32;
            STATE.with(|s| {
                let mut g = s.borrow_mut();
                let Some(st) = g.as_mut() else { return };
                match id {
                    ID_LANG if code == BN_CLICKED => {
                        st.is_vi = !st.is_vi;
                        st.refresh_texts();
                    }
                    ID_BROWSE if code == BN_CLICKED => {
                        let cur = st.current_options().install_dir;
                        let title = if st.is_vi {
                            "Chọn thư mục cài đặt PUW Notch"
                        } else {
                            "Choose PUW Notch installation folder"
                        };
                        if let Some(chosen) = shortcut::pick_folder(hwnd, &cur, title) {
                            let final_dir = if chosen
                                .file_name()
                                .and_then(|n| n.to_str())
                                .is_some_and(|n| n.eq_ignore_ascii_case(APP_NAME) || n.eq_ignore_ascii_case("PuwNotch"))
                            {
                                chosen
                            } else {
                                chosen.join(APP_NAME)
                            };
                            set_text(st.dir_edit, &final_dir.display().to_string());
                            st.refresh_permissions();
                        }
                    }
                    ID_DIR_EDIT if code == EN_CHANGE => st.refresh_permissions(),
                    ID_CHK_STARTUP | ID_CHK_STARTMENU | ID_CHK_DESKTOP | ID_CHK_LAUNCH if code == BN_CLICKED => {
                        st.refresh_permissions();
                    }
                    ID_ACTION if code == BN_CLICKED => st.run_action(),
                    ID_CLOSE if code == BN_CLICKED => unsafe {
                        PostQuitMessage(0);
                    },
                    _ => {}
                }
            });
            LRESULT(0)
        }
        WM_AUTO_RUN => {
            STATE.with(|s| {
                if let Some(st) = s.borrow_mut().as_mut() {
                    st.run_action();
                }
            });
            LRESULT(0)
        }
        WM_CTLCOLORSTATIC => {
            let hdc = HDC(wp.0 as _);
            let ctl = HWND(lp.0 as _);
            STATE.with(|s| {
                let g = s.borrow();
                let Some(st) = g.as_ref() else {
                    return unsafe { LRESULT(GetStockObject(WHITE_BRUSH).0 as isize) };
                };
                unsafe {
                    SetBkMode(hdc, TRANSPARENT);
                    if ctl == st.status_lbl {
                        let c = if st.failed {
                            COLORREF(0x002B2BD6) // red
                        } else if st.done {
                            COLORREF(0x001E7E34) // green
                        } else {
                            COLORREF(0x00606060)
                        };
                        SetTextColor(hdc, c);
                        SetBkColor(hdc, COLORREF(0x00F5F5F5));
                        LRESULT(st.bg_brush.0 as isize)
                    } else if ctl == st.perm_title_lbl || ctl == st.perm_body_lbl {
                        SetTextColor(hdc, COLORREF(0x00202020));
                        SetBkColor(hdc, COLORREF(0x00FFFFFF));
                        LRESULT(st.card_brush.0 as isize)
                    } else if ctl == st.subtitle_lbl {
                        SetTextColor(hdc, COLORREF(0x00606060));
                        SetBkColor(hdc, COLORREF(0x00F5F5F5));
                        LRESULT(st.bg_brush.0 as isize)
                    } else {
                        SetTextColor(hdc, COLORREF(0x00181818));
                        SetBkColor(hdc, COLORREF(0x00F5F5F5));
                        LRESULT(st.bg_brush.0 as isize)
                    }
                }
            })
        }
        WM_DESTROY => {
            STATE.with(|s| {
                if let Some(st) = s.borrow_mut().take() {
                    unsafe {
                        let _ = DeleteObject(HGDIOBJ(st.font_title.0));
                        let _ = DeleteObject(HGDIOBJ(st.font_bold.0));
                        let _ = DeleteObject(HGDIOBJ(st.font_body.0));
                        let _ = DeleteObject(HGDIOBJ(st.bg_brush.0));
                        let _ = DeleteObject(HGDIOBJ(st.card_brush.0));
                    }
                }
            });
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

fn main() {
    let cli = CliArgs::parse();
    unsafe {
        let icc = INITCOMMONCONTROLSEX {
            dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_STANDARD_CLASSES,
        };
        let _ = InitCommonControlsEx(&icc);

        let hinst = GetModuleHandleW(None).unwrap_or_default();
        let bg_brush = CreateSolidBrush(COLORREF(0x00F5F5F5));
        let card_brush = CreateSolidBrush(COLORREF(0x00FFFFFF));
        let class = w!("PuwNotchSetupWindow");
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hIcon: LoadIconW(Some(hinst.into()), PCWSTR(1 as *const u16)).unwrap_or_default(),
            hbrBackground: bg_brush,
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassExW(&wc);

        let base_w = 540;
        let base_h = if cli.mode == Mode::Install { 460 } else { 330 };
        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class,
            w!("PUW Notch Setup"),
            style,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            base_w,
            base_h,
            None,
            None,
            Some(hinst.into()),
            None,
        )
        .expect("CreateWindowExW failed");

        let dpi = GetDpiForWindow(hwnd).max(96) as i32;
        let sc = |v: i32| (v * dpi + 48) / 96;
        let mut wr = RECT {
            left: 0,
            top: 0,
            right: sc(base_w),
            bottom: sc(base_h),
        };
        let _ = AdjustWindowRectEx(&mut wr, style, false, WINDOW_EX_STYLE::default());
        let ww = wr.right - wr.left;
        let wh = wr.bottom - wr.top;
        let sx = (GetSystemMetrics(SM_CXSCREEN) - ww) / 2;
        let sy = (GetSystemMetrics(SM_CYSCREEN) - wh) / 2;
        let _ = SetWindowPos(hwnd, None, sx.max(0), sy.max(0), ww, wh, SWP_NOZORDER);

        let font_title = make_font(dpi, 15, true);
        let font_bold = make_font(dpi, 10, true);
        let font_body = make_font(dpi, 9, false);

        let ctrl = |cls: PCWSTR, text: PCWSTR, st: WINDOW_STYLE, ex: WINDOW_EX_STYLE, x: i32, y: i32, w: i32, h: i32, id: isize| -> HWND {
            CreateWindowExW(
                ex,
                cls,
                text,
                WS_CHILD | WS_VISIBLE | st,
                sc(x),
                sc(y),
                sc(w),
                sc(h),
                Some(hwnd),
                Some(HMENU(id as _)),
                Some(hinst.into()),
                None,
            )
            .expect("create control")
        };

        let title_lbl = ctrl(w!("STATIC"), w!(""), WINDOW_STYLE(0), WINDOW_EX_STYLE(0), 22, 16, 420, 28, ID_TITLE);
        let subtitle_lbl = ctrl(w!("STATIC"), w!(""), WINDOW_STYLE(0), WINDOW_EX_STYLE(0), 22, 46, 420, 20, ID_SUBTITLE);
        let lang_btn = ctrl(w!("BUTTON"), w!("EN"), WINDOW_STYLE(BS_PUSHBUTTON as u32), WINDOW_EX_STYLE(0), 472, 18, 46, 26, ID_LANG);

        let dir_lbl = ctrl(w!("STATIC"), w!(""), WINDOW_STYLE(0), WINDOW_EX_STYLE(0), 22, 78, 496, 20, ID_DIR_LABEL);
        let edit_style = if cli.mode == Mode::Uninstall {
            WINDOW_STYLE(ES_AUTOHSCROLL as u32 | ES_READONLY as u32)
        } else {
            WINDOW_STYLE(ES_AUTOHSCROLL as u32)
        };
        let dir_edit = ctrl(w!("EDIT"), w!(""), edit_style, WS_EX_CLIENTEDGE, 22, 100, if cli.mode == Mode::Install { 378 } else { 496 }, 26, ID_DIR_EDIT);
        let browse_btn = ctrl(
            w!("BUTTON"),
            w!(""),
            WINDOW_STYLE(BS_PUSHBUTTON as u32),
            WINDOW_EX_STYLE(0),
            408,
            99,
            110,
            28,
            ID_BROWSE,
        );
        if cli.mode == Mode::Uninstall {
            let _ = ShowWindow(browse_btn, SW_HIDE);
        }

        let chk_startup = ctrl(w!("BUTTON"), w!(""), WINDOW_STYLE(BS_AUTOCHECKBOX as u32), WINDOW_EX_STYLE(0), 24, 138, 492, 22, ID_CHK_STARTUP);
        let chk_startmenu = ctrl(w!("BUTTON"), w!(""), WINDOW_STYLE(BS_AUTOCHECKBOX as u32), WINDOW_EX_STYLE(0), 24, 164, 492, 22, ID_CHK_STARTMENU);
        let chk_desktop = ctrl(w!("BUTTON"), w!(""), WINDOW_STYLE(BS_AUTOCHECKBOX as u32), WINDOW_EX_STYLE(0), 24, 190, 492, 22, ID_CHK_DESKTOP);
        let chk_launch = ctrl(w!("BUTTON"), w!(""), WINDOW_STYLE(BS_AUTOCHECKBOX as u32), WINDOW_EX_STYLE(0), 24, 216, 492, 22, ID_CHK_LAUNCH);
        if cli.mode == Mode::Uninstall {
            for c in [chk_startup, chk_startmenu, chk_desktop, chk_launch] {
                let _ = ShowWindow(c, SW_HIDE);
            }
        }

        let perm_y = if cli.mode == Mode::Install { 250 } else { 142 };
        let perm_title_lbl = ctrl(w!("STATIC"), w!(""), WINDOW_STYLE(0), WS_EX_STATICEDGE, 22, perm_y, 496, 22, ID_PERM_TITLE);
        let perm_body_lbl = ctrl(w!("STATIC"), w!(""), WINDOW_STYLE(0), WS_EX_STATICEDGE, 22, perm_y + 22, 496, 100, ID_PERM_BODY);

        let status_y = if cli.mode == Mode::Install { 382 } else { 270 };
        let status_lbl = ctrl(w!("STATIC"), w!(""), WINDOW_STYLE(0), WINDOW_EX_STYLE(0), 22, status_y, 496, 22, ID_STATUS);

        let btn_y = if cli.mode == Mode::Install { 412 } else { 292 };
        let action_btn = ctrl(w!("BUTTON"), w!(""), WINDOW_STYLE(BS_DEFPUSHBUTTON as u32), WINDOW_EX_STYLE(0), 300, btn_y, 120, 32, ID_ACTION);
        let close_btn = ctrl(w!("BUTTON"), w!(""), WINDOW_STYLE(BS_PUSHBUTTON as u32), WINDOW_EX_STYLE(0), 428, btn_y, 90, 32, ID_CLOSE);

        set_font(title_lbl, font_title);
        set_font(dir_lbl, font_bold);
        set_font(perm_title_lbl, font_bold);
        for c in [subtitle_lbl, lang_btn, dir_edit, browse_btn, chk_startup, chk_startmenu, chk_desktop, chk_launch, perm_body_lbl, status_lbl, action_btn, close_btn] {
            set_font(c, font_body);
        }

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
        set_text(dir_edit, &initial_dir.display().to_string());
        set_checked(chk_startup, cli.startup);
        set_checked(chk_startmenu, cli.start_menu);
        set_checked(chk_desktop, cli.desktop);
        set_checked(chk_launch, cli.launch);

        let state = UiState {
            hwnd,
            mode: cli.mode,
            is_vi: cli.is_vi,
            elevated: is_elevated(),
            done: false,
            failed: false,
            status_text: String::new(),
            dir_edit,
            browse_btn,
            lang_btn,
            chk_startup,
            chk_startmenu,
            chk_desktop,
            chk_launch,
            action_btn,
            close_btn,
            title_lbl,
            subtitle_lbl,
            dir_lbl,
            perm_title_lbl,
            perm_body_lbl,
            status_lbl,
            font_title,
            font_bold,
            font_body,
            bg_brush,
            card_brush,
        };
        state.refresh_texts();
        STATE.with(|s| *s.borrow_mut() = Some(state));

        let _ = ShowWindow(hwnd, SW_SHOW);
        if cli.auto_run {
            let _ = PostMessageW(Some(hwnd), WM_AUTO_RUN, WPARAM(0), LPARAM(0));
        }

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if !IsDialogMessageW(hwnd, &msg).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}

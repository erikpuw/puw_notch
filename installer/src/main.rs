#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod install;
mod shortcut;

use eframe::egui;
use egui::{
    Color32, CornerRadius, FontData, FontDefinitions, FontFamily, Frame, Margin, RichText, Stroke,
    Vec2,
};
use install::{
    default_install_dir, is_elevated, launch_installed_app, needs_elevation, perform_install,
    perform_uninstall, relaunch_elevated, InstallOptions, APP_NAME, APP_VERSION,
};
use std::path::PathBuf;
use std::sync::Arc;
use windows::core::w;
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};

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

struct InstallerApp {
    mode: Mode,
    is_vi: bool,
    dark: bool,
    elevated: bool,
    auto_run_pending: bool,
    install_dir_text: String,
    create_start_menu: bool,
    create_desktop: bool,
    enable_startup_task: bool,
    launch_after: bool,
    status: Status,
    message: String,
    installed_exe: Option<PathBuf>,
}

impl InstallerApp {
    fn new(cc: &eframe::CreationContext<'_>, cli: CliArgs) -> Self {
        configure_fonts(&cc.egui_ctx);
        let dark = is_windows_dark_mode();
        apply_visuals(&cc.egui_ctx, dark);
        let dir = cli.dir.unwrap_or_else(|| {
            if cli.mode == Mode::Uninstall {
                std::env::current_exe()
                    .ok()
                    .and_then(|p| p.parent().map(|d| d.to_path_buf()))
                    .unwrap_or_else(default_install_dir)
            } else {
                default_install_dir()
            }
        });
        Self {
            mode: cli.mode,
            is_vi: cli.is_vi,
            dark,
            elevated: is_elevated(),
            auto_run_pending: cli.auto_run,
            install_dir_text: dir.display().to_string(),
            create_start_menu: cli.start_menu,
            create_desktop: cli.desktop,
            enable_startup_task: cli.startup,
            launch_after: cli.launch,
            status: Status::Ready,
            message: String::new(),
            installed_exe: None,
        }
    }

    fn current_options(&self) -> InstallOptions {
        let raw = self.install_dir_text.trim();
        let install_dir = if raw.is_empty() {
            default_install_dir()
        } else {
            PathBuf::from(raw)
        };
        InstallOptions {
            install_dir,
            create_start_menu: self.create_start_menu,
            create_desktop: self.create_desktop,
            enable_startup_task: self.enable_startup_task,
            launch_after: self.launch_after,
        }
    }

    fn run_primary_action(&mut self, ctx: &egui::Context) {
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
            match relaunch_elevated(&args) {
                Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                Err(err) => {
                    self.status = Status::Failed;
                    self.message = if self.is_vi {
                        format!("Đã huỷ xin quyền quản trị (UAC): {err}")
                    } else {
                        format!("UAC elevation cancelled: {err}")
                    };
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
    }
}

impl eframe::App for InstallerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.auto_run_pending {
            self.auto_run_pending = false;
            self.run_primary_action(ctx);
        }

        let (bg, card, border, text_pri, text_sec, accent, warn_col, ok_col) = if self.dark {
            (
                Color32::from_rgb(24, 24, 24),
                Color32::from_rgb(35, 35, 35),
                Color32::from_rgb(56, 56, 56),
                Color32::from_rgb(243, 243, 243),
                Color32::from_rgb(168, 168, 168),
                Color32::from_rgb(76, 194, 255),
                Color32::from_rgb(252, 200, 90),
                Color32::from_rgb(108, 214, 134),
            )
        } else {
            (
                Color32::from_rgb(243, 243, 243),
                Color32::from_rgb(255, 255, 255),
                Color32::from_rgb(224, 224, 224),
                Color32::from_rgb(24, 24, 24),
                Color32::from_rgb(96, 96, 96),
                Color32::from_rgb(0, 103, 192),
                Color32::from_rgb(176, 104, 0),
                Color32::from_rgb(20, 130, 56),
            )
        };

        let opts = self.current_options();
        let need_uac = match self.mode {
            Mode::Install => needs_elevation(&opts),
            Mode::Uninstall => true,
        };

        egui::CentralPanel::default()
            .frame(Frame::new().fill(bg).inner_margin(Margin::same(22)))
            .show(ctx, |ui| {
                // Header
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new(format!("{APP_NAME} v{APP_VERSION} Setup"))
                                .size(20.0)
                                .strong()
                                .color(text_pri),
                        );
                        ui.label(
                            RichText::new(match (self.mode, self.is_vi) {
                                (Mode::Install, true) => "Trình cài đặt chiếc notch sống cho Windows",
                                (Mode::Install, false) => "Installer for a living notch on Windows",
                                (Mode::Uninstall, true) => "Gỡ cài đặt PUW Notch khỏi máy tính",
                                (Mode::Uninstall, false) => "Uninstall PUW Notch from this computer",
                            })
                            .size(12.0)
                            .color(text_sec),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                        let lang_btn = if self.is_vi { "EN" } else { "VI" };
                        if ui.button(lang_btn).clicked() {
                            self.is_vi = !self.is_vi;
                        }
                    });
                });

                ui.add_space(12.0);

                // Options card
                Frame::new()
                    .fill(card)
                    .stroke(Stroke::new(1.0_f32, border))
                    .corner_radius(CornerRadius::same(8))
                    .inner_margin(Margin::same(14))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        match self.mode {
                            Mode::Install => {
                                ui.label(
                                    RichText::new(if self.is_vi {
                                        "Thư mục cài đặt"
                                    } else {
                                        "Installation folder"
                                    })
                                    .size(12.5)
                                    .strong()
                                    .color(text_pri),
                                );
                                ui.add_space(4.0);
                                ui.horizontal(|ui| {
                                    let browse_w = 104.0;
                                    let edit_w = (ui.available_width() - browse_w - 8.0).max(120.0);
                                    ui.add_sized(
                                        [edit_w, 28.0],
                                        egui::TextEdit::singleline(&mut self.install_dir_text),
                                    );
                                    let browse_txt = if self.is_vi { "Chọn thư mục…" } else { "Browse…" };
                                    if ui.add_sized([browse_w, 28.0], egui::Button::new(browse_txt)).clicked() {
                                        let cur = PathBuf::from(self.install_dir_text.trim());
                                        let title = if self.is_vi {
                                            "Chọn thư mục cài đặt PUW Notch"
                                        } else {
                                            "Choose PUW Notch installation folder"
                                        };
                                        if let Some(chosen) = shortcut::pick_folder(None, &cur, title) {
                                            let final_dir = if chosen
                                                .file_name()
                                                .and_then(|n| n.to_str())
                                                .is_some_and(|n| {
                                                    n.eq_ignore_ascii_case(APP_NAME)
                                                        || n.eq_ignore_ascii_case("PuwNotch")
                                                }) {
                                                chosen
                                            } else {
                                                chosen.join(APP_NAME)
                                            };
                                            self.install_dir_text = final_dir.display().to_string();
                                        }
                                    }
                                });
                                ui.add_space(10.0);
                                ui.checkbox(
                                    &mut self.enable_startup_task,
                                    if self.is_vi {
                                        "Khởi động cùng Windows (Task Scheduler, không chớp cửa sổ)"
                                    } else {
                                        "Start with Windows (Task Scheduler, no console flash)"
                                    },
                                );
                                ui.checkbox(
                                    &mut self.create_start_menu,
                                    if self.is_vi {
                                        "Tạo lối tắt trong Start Menu"
                                    } else {
                                        "Create Start Menu shortcut"
                                    },
                                );
                                ui.checkbox(
                                    &mut self.create_desktop,
                                    if self.is_vi {
                                        "Tạo lối tắt ngoài Desktop"
                                    } else {
                                        "Create Desktop shortcut"
                                    },
                                );
                                ui.checkbox(
                                    &mut self.launch_after,
                                    if self.is_vi {
                                        "Mở PUW Notch sau khi cài xong"
                                    } else {
                                        "Launch PUW Notch after installation"
                                    },
                                );
                            }
                            Mode::Uninstall => {
                                ui.label(
                                    RichText::new(if self.is_vi {
                                        "Gỡ cài đặt PUW Notch khỏi máy tính này"
                                    } else {
                                        "Uninstall PUW Notch from this computer"
                                    })
                                    .size(13.0)
                                    .strong()
                                    .color(text_pri),
                                );
                                ui.add_space(6.0);
                                ui.label(
                                    RichText::new(format!(
                                        "{}: {}",
                                        if self.is_vi { "Thư mục" } else { "Folder" },
                                        self.install_dir_text
                                    ))
                                    .size(12.0)
                                    .color(text_sec),
                                );
                            }
                        }
                    });

                ui.add_space(10.0);

                // Permissions & system changes card (transparent UAC & actions)
                Frame::new()
                    .fill(card)
                    .stroke(Stroke::new(1.0_f32, border))
                    .corner_radius(CornerRadius::same(8))
                    .inner_margin(Margin::same(12))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.label(
                            RichText::new(if self.is_vi {
                                "Quyền & thay đổi trên máy"
                            } else {
                                "Permissions & system changes"
                            })
                            .size(12.0)
                            .strong()
                            .color(text_pri),
                        );
                        ui.add_space(4.0);
                        let (uac_txt, uac_col) = if self.elevated {
                            (
                                if self.is_vi {
                                    "• Quyền quản trị (UAC): Đã cấp"
                                } else {
                                    "• Administrator (UAC): Granted"
                                },
                                ok_col,
                            )
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
                        ui.label(RichText::new(uac_txt).size(11.5).color(uac_col));

                        let details = match self.mode {
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
                                    "• Tắt PUW Notch, xoá Scheduled Task, lối tắt và mục Settings -> Apps (giữ %APPDATA%\\PuwNotch)".into()
                                } else {
                                    "• Stops PUW Notch, removes Scheduled Task, shortcuts and Settings -> Apps entry (keeps %APPDATA%\\PuwNotch)".into()
                                }
                            }
                        };
                        ui.label(RichText::new(details).size(11.5).color(text_sec));
                    });

                ui.add_space(8.0);

                ui.label(
                    RichText::new(if self.is_vi {
                        "Bản cài đặt chưa có chữ ký số (Unsigned) · Chỉ tải từ github.com/erikpuw/puw_notch"
                    } else {
                        "Unsigned installer build · Download only from github.com/erikpuw/puw_notch"
                    })
                    .size(11.0)
                    .color(text_sec),
                );

                if !self.message.is_empty() {
                    ui.add_space(6.0);
                    let msg_col = match self.status {
                        Status::Done => accent,
                        Status::Failed => Color32::from_rgb(255, 110, 110),
                        Status::Ready => text_sec,
                    };
                    ui.label(RichText::new(&self.message).size(12.5).color(msg_col));
                }

                ui.with_layout(egui::Layout::bottom_up(egui::Align::RIGHT), |ui| {
                    ui.horizontal(|ui| {
                        if self.status == Status::Done {
                            let finish_txt = if self.is_vi { "Hoàn tất" } else { "Finish" };
                            if ui
                                .add_sized([110.0, 32.0], egui::Button::new(finish_txt))
                                .clicked()
                            {
                                if self.mode == Mode::Install && self.launch_after {
                                    if let Some(exe) = &self.installed_exe {
                                        launch_installed_app(exe);
                                    }
                                }
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                        } else {
                            let primary_txt = match (self.mode, self.is_vi, need_uac && !self.elevated) {
                                (Mode::Install, true, true) => "Cài đặt (UAC)",
                                (Mode::Install, true, false) => "Cài đặt",
                                (Mode::Install, false, true) => "Install (UAC)",
                                (Mode::Install, false, false) => "Install",
                                (Mode::Uninstall, true, true) => "Gỡ cài đặt (UAC)",
                                (Mode::Uninstall, true, false) => "Gỡ cài đặt",
                                (Mode::Uninstall, false, true) => "Uninstall (UAC)",
                                (Mode::Uninstall, false, false) => "Uninstall",
                            };
                            if ui
                                .add_sized([132.0, 32.0], egui::Button::new(primary_txt))
                                .clicked()
                            {
                                self.run_primary_action(ctx);
                            }

                            let cancel_txt = if self.is_vi { "Đóng" } else { "Cancel" };
                            if ui
                                .add_sized([90.0, 32.0], egui::Button::new(cancel_txt))
                                .clicked()
                            {
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                        }
                    });
                });
            });
    }
}

fn configure_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    if let Ok(bytes) = std::fs::read(r"C:\Windows\Fonts\segoeui.ttf") {
        fonts
            .font_data
            .insert("segoeui".to_owned(), Arc::new(FontData::from_owned(bytes)));
        fonts
            .families
            .entry(FontFamily::Proportional)
            .or_default()
            .insert(0, "segoeui".to_owned());
        ctx.set_fonts(fonts);
    }
}

fn apply_visuals(ctx: &egui::Context, dark: bool) {
    let mut visuals = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    visuals.widgets.inactive.corner_radius = CornerRadius::same(6);
    visuals.widgets.hovered.corner_radius = CornerRadius::same(6);
    visuals.widgets.active.corner_radius = CornerRadius::same(6);
    ctx.set_visuals(visuals);
}

fn main() -> eframe::Result<()> {
    let cli = CliArgs::parse();
    let height = if cli.mode == Mode::Install { 465.0 } else { 340.0 };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(format!("{APP_NAME} v{APP_VERSION} Setup"))
            .with_inner_size(Vec2::new(540.0, height))
            .with_resizable(false),
        centered: true,
        ..Default::default()
    };

    eframe::run_native(
        "PUW Notch Setup",
        options,
        Box::new(move |cc| Ok(Box::new(InstallerApp::new(cc, cli)))),
    )
}

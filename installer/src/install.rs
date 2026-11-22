use crate::shortcut::create_shortcut;
use flate2::read::GzDecoder;
use std::fs;
use std::io::Read;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND};
use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
    HKEY_LOCAL_MACHINE, KEY_WRITE, REG_DWORD, REG_OPTION_NON_VOLATILE, REG_SZ,
};
use windows::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, TerminateProcess, WaitForSingleObject,
    PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

pub const APP_NAME: &str = "PUW Notch";
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const APP_PUBLISHER: &str = "erikpuw";
pub const APP_URL: &str = "https://github.com/erikpuw/puw_notch";
pub const EXE_NAME: &str = "puw_notch.exe";
pub const UNINSTALLER_NAME: &str = "puw_notch_uninstall.exe";
pub const TASK_NAME: &str = "PuwNotch";

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const PAYLOAD_GZ: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/puw_notch_payload.gz"));
const APP_ICON_ICO: &[u8] = include_bytes!("../../assets/puw.ico");
const LICENSE_TXT: &str = include_str!("../../LICENSE");
const UNINSTALL_REG_SUBKEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\PuwNotch";

#[derive(Clone, Debug)]
pub struct InstallOptions {
    pub install_dir: PathBuf,
    pub create_start_menu: bool,
    pub create_desktop: bool,
    pub enable_startup_task: bool,
    pub launch_after: bool,
}

pub fn default_install_dir() -> PathBuf {
    if let Ok(pf) = std::env::var("ProgramFiles") {
        PathBuf::from(pf).join(APP_NAME)
    } else {
        PathBuf::from(r"C:\Program Files\PUW Notch")
    }
}

pub fn is_elevated() -> bool {
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut elev = TOKEN_ELEVATION::default();
        let mut len = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some((&mut elev as *mut TOKEN_ELEVATION).cast()),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        )
        .is_ok();
        let _ = CloseHandle(token);
        ok && elev.TokenIsElevated != 0
    }
}

pub fn path_needs_admin(dir: &Path) -> bool {
    let lower = dir.display().to_string().to_ascii_lowercase();
    for env_key in ["ProgramFiles", "ProgramFiles(x86)", "SystemRoot", "ProgramData"] {
        if let Ok(root) = std::env::var(env_key) {
            let root_l = root.to_ascii_lowercase();
            if !root_l.is_empty() && (lower == root_l || lower.starts_with(&(root_l + "\\"))) {
                return true;
            }
        }
    }
    false
}

pub fn needs_elevation(opts: &InstallOptions) -> bool {
    opts.enable_startup_task || path_needs_admin(&opts.install_dir)
}

pub fn relaunch_elevated(owner: HWND, args: &str) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("Cannot locate installer exe: {e}"))?;
    let exe_h = HSTRING::from(exe.as_os_str());
    let args_h = HSTRING::from(args);
    let r = unsafe {
        ShellExecuteW(
            Some(owner),
            w!("runas"),
            &exe_h,
            &args_h,
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    if (r.0 as isize) <= 32 {
        return Err("UAC elevation was cancelled or denied.".into());
    }
    Ok(())
}

pub fn start_menu_shortcut_path(all_users: bool) -> Option<PathBuf> {
    if all_users {
        std::env::var("ProgramData").ok().map(|pd| {
            PathBuf::from(pd)
                .join("Microsoft")
                .join("Windows")
                .join("Start Menu")
                .join("Programs")
                .join("PUW Notch.lnk")
        })
    } else {
        std::env::var("APPDATA").ok().map(|appdata| {
            PathBuf::from(appdata)
                .join("Microsoft")
                .join("Windows")
                .join("Start Menu")
                .join("Programs")
                .join("PUW Notch.lnk")
        })
    }
}

pub fn desktop_shortcut_path() -> Option<PathBuf> {
    std::env::var("USERPROFILE")
        .ok()
        .map(|home| PathBuf::from(home).join("Desktop").join("PUW Notch.lnk"))
}

pub fn decompress_payload() -> Result<Vec<u8>, String> {
    let mut decoder = GzDecoder::new(PAYLOAD_GZ);
    let mut out = Vec::new();
    decoder
        .read_to_end(&mut out)
        .map_err(|e| format!("Decompress payload failed: {e}"))?;
    if out.is_empty() {
        return Err(
            "Installer payload is empty. Build `cargo build --release` before building puw_notch_setup."
                .into(),
        );
    }
    Ok(out)
}

pub fn stop_running_app() {
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return;
        };
        let mut entry = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut more = Process32FirstW(snap, &mut entry).is_ok();
        while more {
            let len = entry
                .szExeFile
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(entry.szExeFile.len());
            let name = String::from_utf16_lossy(&entry.szExeFile[..len]);
            if name.eq_ignore_ascii_case(EXE_NAME) {
                if let Ok(h) = OpenProcess(PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, false, entry.th32ProcessID) {
                    let _ = TerminateProcess(h, 0);
                    let _ = WaitForSingleObject(h, 3_000);
                    let _ = CloseHandle(h);
                }
            }
            more = Process32NextW(snap, &mut entry).is_ok();
        }
        let _ = CloseHandle(snap);
    }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub fn task_xml(exe: &Path, user: &str) -> String {
    let user = xml_escape(user);
    let dir = xml_escape(&exe.parent().map_or(String::new(), |d| d.display().to_string()));
    let quoted = xml_escape(&format!("\"{}\"", exe.display()));
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <Triggers><LogonTrigger><Enabled>true</Enabled><UserId>{user}</UserId><Delay>PT15S</Delay></LogonTrigger></Triggers>
  <Principals><Principal id="Author"><UserId>{user}</UserId><LogonType>InteractiveToken</LogonType><RunLevel>HighestAvailable</RunLevel></Principal></Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Enabled>true</Enabled>
  </Settings>
  <Actions Context="Author"><Exec><Command>{quoted}</Command><WorkingDirectory>{dir}</WorkingDirectory></Exec></Actions>
</Task>
"#
    )
}

pub fn create_startup_task(exe: &Path) -> Result<(), String> {
    let user = format!(
        "{}\\{}",
        std::env::var("USERDOMAIN").unwrap_or_default(),
        std::env::var("USERNAME").unwrap_or_default()
    );
    let xml = task_xml(exe, &user);
    let file = std::env::temp_dir().join("puw_notch_setup_task.xml");
    let utf16: Vec<u8> = [0xFEFFu16]
        .into_iter()
        .chain(xml.encode_utf16())
        .flat_map(u16::to_le_bytes)
        .collect();
    fs::write(&file, utf16).map_err(|e| format!("Cannot write temp task XML: {e}"))?;
    let status = Command::new("schtasks.exe")
        .args(["/create", "/tn", TASK_NAME, "/xml"])
        .arg(&file)
        .arg("/f")
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .map_err(|e| format!("schtasks.exe failed: {e}"))?;
    let _ = fs::remove_file(&file);
    if status.success() {
        Ok(())
    } else {
        Err(format!("schtasks /create exited with {status}"))
    }
}

pub fn delete_startup_task() {
    let _ = Command::new("schtasks.exe")
        .args(["/delete", "/tn", TASK_NAME, "/f"])
        .creation_flags(CREATE_NO_WINDOW)
        .status();
}

pub fn perform_install(opts: &InstallOptions) -> Result<PathBuf, String> {
    let exe_bytes = decompress_payload()?;
    stop_running_app();

    fs::create_dir_all(&opts.install_dir)
        .map_err(|e| format!("Cannot create folder {}: {e}", opts.install_dir.display()))?;

    let target_exe = opts.install_dir.join(EXE_NAME);
    let target_ico = opts.install_dir.join("puw.ico");
    let target_license = opts.install_dir.join("LICENSE.txt");
    let uninstaller_exe = opts.install_dir.join(UNINSTALLER_NAME);

    fs::write(&target_exe, &exe_bytes)
        .map_err(|e| format!("Cannot write {EXE_NAME} (close PUW Notch first): {e}"))?;
    fs::write(&target_ico, APP_ICON_ICO).map_err(|e| format!("Cannot write puw.ico: {e}"))?;
    let _ = fs::write(&target_license, LICENSE_TXT);

    if let Ok(self_exe) = std::env::current_exe() {
        if self_exe != uninstaller_exe {
            let _ = fs::copy(&self_exe, &uninstaller_exe);
        }
    }

    let system_wide = is_elevated() && path_needs_admin(&opts.install_dir);
    if opts.create_start_menu {
        if let Some(lnk) = start_menu_shortcut_path(system_wide) {
            create_shortcut(
                &lnk,
                &target_exe,
                &opts.install_dir,
                &target_ico,
                "PUW Notch - A living notch for Windows",
            )?;
        }
    }
    if opts.create_desktop {
        if let Some(lnk) = desktop_shortcut_path() {
            create_shortcut(
                &lnk,
                &target_exe,
                &opts.install_dir,
                &target_ico,
                "PUW Notch - A living notch for Windows",
            )?;
        }
    }

    let estimated_kb = ((exe_bytes.len() as u64) / 1024).max(1) as u32;
    let reg_root = if system_wide { HKEY_LOCAL_MACHINE } else { HKEY_CURRENT_USER };
    register_uninstall_entry(reg_root, &opts.install_dir, &target_ico, &uninstaller_exe, estimated_kb)?;

    if opts.enable_startup_task {
        create_startup_task(&target_exe)?;
    }

    Ok(target_exe)
}

pub fn perform_uninstall(install_dir: &Path) -> Result<(), String> {
    stop_running_app();
    delete_startup_task();

    for all_users in [false, true] {
        if let Some(lnk) = start_menu_shortcut_path(all_users) {
            let _ = fs::remove_file(lnk);
        }
    }
    if let Some(lnk) = desktop_shortcut_path() {
        let _ = fs::remove_file(lnk);
    }
    unregister_uninstall_entry();

    let _ = fs::remove_file(install_dir.join(EXE_NAME));
    let _ = fs::remove_file(install_dir.join("puw.ico"));
    let _ = fs::remove_file(install_dir.join("LICENSE.txt"));

    // Schedule removal of self (puw_notch_uninstall.exe) and empty install_dir after exit
    let uninstaller = install_dir.join(UNINSTALLER_NAME);
    if uninstaller.exists() {
        let cmd = format!(
            "ping 127.0.0.1 -n 2 >nul & del /f /q \"{}\" >nul 2>&1 & rmdir \"{}\" >nul 2>&1",
            uninstaller.display(),
            install_dir.display()
        );
        let _ = Command::new("cmd.exe")
            .args(["/c", &cmd])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
    } else {
        let _ = fs::remove_dir(install_dir);
    }
    Ok(())
}

pub fn launch_installed_app(exe_path: &Path) {
    let file = HSTRING::from(exe_path.as_os_str());
    let dir = exe_path
        .parent()
        .map(|d| HSTRING::from(d.as_os_str()))
        .unwrap_or_default();
    unsafe {
        let _ = ShellExecuteW(
            None,
            w!("open"),
            &file,
            PCWSTR::null(),
            &dir,
            SW_SHOWNORMAL,
        );
    }
}

fn reg_set_sz(hkey: HKEY, name: &str, value: &str) -> Result<(), String> {
    let name_h = HSTRING::from(name);
    let wide: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = unsafe {
        std::slice::from_raw_parts(wide.as_ptr() as *const u8, wide.len() * size_of::<u16>())
    };
    let status = unsafe { RegSetValueExW(hkey, &name_h, None, REG_SZ, Some(bytes)) };
    if status.is_ok() {
        Ok(())
    } else {
        Err(format!("RegSetValueExW({name}) failed: {status:?}"))
    }
}

fn reg_set_dword(hkey: HKEY, name: &str, value: u32) -> Result<(), String> {
    let name_h = HSTRING::from(name);
    let bytes = value.to_le_bytes();
    let status = unsafe { RegSetValueExW(hkey, &name_h, None, REG_DWORD, Some(&bytes)) };
    if status.is_ok() {
        Ok(())
    } else {
        Err(format!("RegSetValueExW({name}) failed: {status:?}"))
    }
}

fn register_uninstall_entry(
    root: HKEY,
    install_dir: &Path,
    icon_path: &Path,
    uninstaller_exe: &Path,
    estimated_kb: u32,
) -> Result<(), String> {
    let subkey = HSTRING::from(UNINSTALL_REG_SUBKEY);
    let mut hkey = HKEY::default();
    let status = unsafe {
        RegCreateKeyExW(
            root,
            &subkey,
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut hkey,
            None,
        )
    };
    if status.is_err() {
        return Err(format!("Cannot register Uninstall entry: {status:?}"));
    }
    let uninstall_cmd = format!("\"{}\" --uninstall", uninstaller_exe.display());
    let res = (|| {
        reg_set_sz(hkey, "DisplayName", APP_NAME)?;
        reg_set_sz(hkey, "DisplayVersion", APP_VERSION)?;
        reg_set_sz(hkey, "Publisher", APP_PUBLISHER)?;
        reg_set_sz(hkey, "URLInfoAbout", APP_URL)?;
        reg_set_sz(hkey, "DisplayIcon", &icon_path.display().to_string())?;
        reg_set_sz(hkey, "InstallLocation", &install_dir.display().to_string())?;
        reg_set_sz(hkey, "UninstallString", &uninstall_cmd)?;
        reg_set_dword(hkey, "EstimatedSize", estimated_kb)?;
        reg_set_dword(hkey, "NoModify", 1)?;
        reg_set_dword(hkey, "NoRepair", 1)?;
        Ok(())
    })();
    unsafe {
        let _ = RegCloseKey(hkey);
    }
    res
}

fn unregister_uninstall_entry() {
    let subkey = HSTRING::from(UNINSTALL_REG_SUBKEY);
    unsafe {
        let _ = RegDeleteTreeW(HKEY_CURRENT_USER, &subkey);
        let _ = RegDeleteTreeW(HKEY_LOCAL_MACHINE, &subkey);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_xml_launches_exe_directly_with_highest_rights() {
        let xml = task_xml(Path::new(r"C:\Program Files\PUW Notch\puw_notch.exe"), r"PC\User");
        assert!(xml.contains("<RunLevel>HighestAvailable</RunLevel>"));
        assert!(xml.contains("<Delay>PT15S</Delay>"));
        assert!(xml.contains(r"<Command>&quot;C:\Program Files\PUW Notch\puw_notch.exe&quot;</Command>"));
        assert!(!xml.contains("powershell"));
    }

    #[test]
    fn path_needs_admin_detects_program_files_only() {
        assert!(path_needs_admin(Path::new(r"C:\Program Files\PUW Notch")));
        assert!(!path_needs_admin(Path::new(r"D:\Apps\PuwNotch")));
    }
}

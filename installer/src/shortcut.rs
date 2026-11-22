use std::fs;
use std::path::{Path, PathBuf};
use windows::core::{Interface, HSTRING, PCWSTR};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, IPersistFile,
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Shell::{
    FileOpenDialog, IFileOpenDialog, IShellItem, IShellLinkW, SHCreateItemFromParsingName,
    ShellLink, FOS_FORCEFILESYSTEM, FOS_PATHMUSTEXIST, FOS_PICKFOLDERS, SIGDN_FILESYSPATH,
};

struct ComGuard(bool);

impl ComGuard {
    fn init() -> Self {
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        Self(hr.is_ok())
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}

pub fn create_shortcut(
    lnk_path: &Path,
    target_exe: &Path,
    working_dir: &Path,
    icon_path: &Path,
    description: &str,
) -> Result<(), String> {
    if let Some(parent) = lnk_path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("Cannot create shortcut folder: {e}"))?;
    }
    let _com = ComGuard::init();
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)
            .map_err(|e| format!("IShellLinkW init failed: {e}"))?;
        let target_h = HSTRING::from(target_exe.as_os_str());
        let dir_h = HSTRING::from(working_dir.as_os_str());
        let icon_h = HSTRING::from(icon_path.as_os_str());
        let desc_h = HSTRING::from(description);
        link.SetPath(&target_h).map_err(|e| format!("SetPath failed: {e}"))?;
        link.SetWorkingDirectory(&dir_h)
            .map_err(|e| format!("SetWorkingDirectory failed: {e}"))?;
        link.SetIconLocation(&icon_h, 0)
            .map_err(|e| format!("SetIconLocation failed: {e}"))?;
        link.SetDescription(&desc_h)
            .map_err(|e| format!("SetDescription failed: {e}"))?;

        let persist: IPersistFile = link
            .cast()
            .map_err(|e| format!("IPersistFile cast failed: {e}"))?;
        let lnk_h = HSTRING::from(lnk_path.as_os_str());
        persist
            .Save(PCWSTR(lnk_h.as_ptr()), true)
            .map_err(|e| format!("Save shortcut failed: {e}"))?;
    }
    Ok(())
}

pub fn pick_folder(owner: Option<HWND>, current: &Path, title: &str) -> Option<PathBuf> {
    let _com = ComGuard::init();
    unsafe {
        let dialog: IFileOpenDialog =
            CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let opts = dialog.GetOptions().ok()?;
        dialog
            .SetOptions(opts | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST)
            .ok()?;
        let title_h = HSTRING::from(title);
        let _ = dialog.SetTitle(&title_h);
        if current.exists() {
            let cur_h = HSTRING::from(current.as_os_str());
            if let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(&cur_h, None) {
                let _ = dialog.SetFolder(&item);
            }
        }
        dialog.Show(owner).ok()?;
        let item = dialog.GetResult().ok()?;
        let pwstr = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let text = pwstr.to_string().ok();
        CoTaskMemFree(Some(pwstr.0.cast()));
        text.map(PathBuf::from)
    }
}

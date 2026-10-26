//! Suppress Windows toast banners without disabling notification delivery.
//! A separate process restores settings on pipe EOF, including when the UI crashes.

use std::collections::HashSet;
use std::io::{self, Read};
use std::os::windows::process::CommandExt;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::time::{Duration, Instant};
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::System::{Registry::*, Threading::*};

const SETTINGS: &str = r"Software\Microsoft\Windows\CurrentVersion\Notifications\Settings";
const BACKUP: &str = r"Software\PuwNotch\BannerBackup";
const ARG: &str = "--banner-guard";
const RESTORE_ARG: &str = "--restore-banners";

pub struct Guard {
    child: Child,
    pipe: Option<ChildStdin>,
}

impl Guard {
    pub fn start(suppress: bool) -> io::Result<Self> {
        let mut child = Command::new(std::env::current_exe()?)
            .arg(if suppress { ARG } else { RESTORE_ARG })
            .stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW.0).spawn()?;
        let pipe = child.stdin.take();
        Ok(Self { child, pipe })
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.pipe.take();
        let deadline = Instant::now() + Duration::from_secs(2);
        while matches!(self.child.try_wait(), Ok(None)) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        // Do not kill a helper that may still be restoring settings.
    }
}

/// Called before the UI's single-instance mutex; the helper has its own lock.
pub fn run_if_helper() -> bool {
    let suppress = match std::env::args().nth(1).as_deref() {
        Some(ARG) => true,
        Some(RESTORE_ARG) => false,
        _ => return false,
    };
    if let Err(error) = run_helper(SETTINGS, BACKUP, r"Local\PuwNotch.BannerGuard", suppress) {
        report(&error);
    }
    true
}

pub fn report(error: &io::Error) {
    if let Some(dir) = std::env::var_os("APPDATA") {
        let dir = std::path::PathBuf::from(dir).join("PuwNotch");
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join("banner-guard-error.log"), error.to_string());
    }
}

fn run_helper(settings: &str, backup: &str, mutex_name: &str, suppress: bool) -> io::Result<()> {
    let alive = Arc::new(AtomicBool::new(suppress));
    let reader_alive = alive.clone();
    std::thread::spawn(move || {
        let mut byte = [0];
        while matches!(io::stdin().read(&mut byte), Ok(n) if n > 0) {}
        reader_alive.store(false, Ordering::Release);
    });
    unsafe {
        let lock = CreateMutexW(None, false, PCWSTR(wide(mutex_name).as_ptr()))
            .map_err(io::Error::other)?;
        let result = (|| {
            loop {
                match WaitForSingleObject(lock, 250) {
                    WAIT_OBJECT_0 | WAIT_ABANDONED => break,
                    WAIT_TIMEOUT if alive.load(Ordering::Acquire) => continue,
                    WAIT_TIMEOUT => return Ok(()),
                    _ => return Err(io::Error::last_os_error()),
                }
            }
            let result = manage(settings, backup, &alive);
            let _ = ReleaseMutex(lock);
            result
        })();
        let _ = CloseHandle(lock);
        result
    }
}

fn manage(settings: &str, backup: &str, alive: &AtomicBool) -> io::Result<()> {
    let settings = Key::open(settings)?;
    let backup = Key::create(backup)?;
    // A journal survives a killed helper or a reboot. Never overwrite an unrestored journal.
    restore(&settings, &backup)?;
    let mut seen = HashSet::new();
    let result = (|| {
        while alive.load(Ordering::Acquire) {
            suppress(&settings, &backup, &mut seen)?;
            std::thread::sleep(Duration::from_millis(250));
        }
        Ok(())
    })();
    let restored = restore(&settings, &backup);
    result.and(restored)
}

fn suppress(settings: &Key, backup: &Key, seen: &mut HashSet<String>) -> io::Result<()> {
    for name in settings.children()? {
        if seen.contains(&name) { continue; }
        let app = match settings.child(&name, false) {
            Ok(app) => app,
            Err(e) if e.raw_os_error() == Some(ERROR_FILE_NOT_FOUND.0 as i32) => continue,
            Err(e) => return Err(e),
        };
        let original = app.dword("ShowBanner")?;
        if original != Some(0) {
            let journal = backup.child(&name, true)?;
            if let Some(value) = original { journal.set("Original", value)?; }
            journal.set("Present", u32::from(original.is_some()))?;
            // Durable original first: a failure must never leave an unrecorded change.
            unsafe { RegFlushKey(journal.0).ok().map_err(io::Error::other)?; }
            app.set("ShowBanner", 0)?;
        }
        seen.insert(name);
    }
    Ok(())
}

fn restore(settings: &Key, backup: &Key) -> io::Result<()> {
    let mut failure = None;
    for name in backup.children()? {
        let result = (|| {
            let journal = backup.child(&name, false)?;
            if let Some(present) = journal.dword("Present")? {
                let original = match present {
                    0 => None,
                    1 => Some(journal.dword("Original")?.ok_or_else(|| io::Error::other("Missing banner backup value"))?),
                    _ => return Err(io::Error::other("Invalid banner backup marker")),
                };
                match settings.child(&name, false) {
                    Ok(app) if app.dword("ShowBanner")? == Some(0) => {
                        match original {
                            Some(value) => app.set("ShowBanner", value)?,
                            None => app.delete_value("ShowBanner")?,
                        }
                        unsafe { RegFlushKey(app.0).ok().map_err(io::Error::other)?; }
                    }
                    Ok(_) => {} // Preserve a user's change made while Notch was running.
                    Err(e) if e.raw_os_error() == Some(ERROR_FILE_NOT_FOUND.0 as i32) => {}
                    Err(e) => return Err(e),
                }
            }
            drop(journal);
            unsafe { check(RegDeleteKeyW(backup.0, PCWSTR(wide(&name).as_ptr()))) }
        })();
        if let Err(error) = result { failure = Some(error); }
    }
    failure.map_or(Ok(()), Err)
}

struct Key(HKEY);
impl Drop for Key {
    fn drop(&mut self) { unsafe { let _ = RegCloseKey(self.0); } }
}

fn wide(s: &str) -> Vec<u16> { s.encode_utf16().chain(Some(0)).collect() }
fn check(status: WIN32_ERROR) -> io::Result<()> {
    if status == ERROR_SUCCESS { Ok(()) } else { Err(io::Error::from_raw_os_error(status.0 as i32)) }
}

impl Key {
    fn open(path: &str) -> io::Result<Self> { Self::at(HKEY_CURRENT_USER, path, false) }
    fn create(path: &str) -> io::Result<Self> { Self::at(HKEY_CURRENT_USER, path, true) }
    fn child(&self, name: &str, create: bool) -> io::Result<Self> { Self::at(self.0, name, create) }
    fn at(parent: HKEY, path: &str, create: bool) -> io::Result<Self> {
        let mut key = HKEY::default();
        let path = wide(path);
        unsafe {
            check(if create {
                RegCreateKeyExW(parent, PCWSTR(path.as_ptr()), None, PCWSTR::null(), REG_OPTION_NON_VOLATILE,
                    KEY_READ | KEY_WRITE, None, &mut key, None)
            } else {
                RegOpenKeyExW(parent, PCWSTR(path.as_ptr()), None, KEY_READ | KEY_WRITE, &mut key)
            })?;
        }
        Ok(Self(key))
    }
    fn children(&self) -> io::Result<Vec<String>> {
        let mut names = Vec::new();
        loop {
            let mut buf = [0u16; 256];
            let mut len = buf.len() as u32;
            let status = unsafe { RegEnumKeyExW(self.0, names.len() as u32, Some(PWSTR(buf.as_mut_ptr())),
                &mut len, None, None, None, None) };
            if status == ERROR_NO_MORE_ITEMS { break; }
            check(status)?;
            names.push(String::from_utf16(&buf[..len as usize]).map_err(io::Error::other)?);
        }
        Ok(names)
    }
    fn dword(&self, name: &str) -> io::Result<Option<u32>> {
        let mut bytes = [0u8; 4];
        let mut len = 4;
        let mut kind = REG_VALUE_TYPE::default();
        let status = unsafe { RegQueryValueExW(self.0, PCWSTR(wide(name).as_ptr()), None,
            Some(&mut kind), Some(bytes.as_mut_ptr()), Some(&mut len)) };
        if status == ERROR_FILE_NOT_FOUND { return Ok(None); }
        check(status)?;
        if kind != REG_DWORD || len != 4 { return Err(io::Error::other("Unexpected banner registry value type")); }
        Ok(Some(u32::from_le_bytes(bytes)))
    }
    fn set(&self, name: &str, value: u32) -> io::Result<()> {
        unsafe { check(RegSetValueExW(self.0, PCWSTR(wide(name).as_ptr()), None, REG_DWORD, Some(&value.to_le_bytes()))) }
    }
    fn delete_value(&self, name: &str) -> io::Result<()> {
        unsafe { check(RegDeleteValueW(self.0, PCWSTR(wide(name).as_ptr()))) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Invoked only by the process-lifetime test, against an isolated registry tree.
    #[test]
    #[ignore]
    fn helper_fixture() {
        let Ok(root) = std::env::var("PUW_BANNER_TEST_ROOT") else { return; };
        assert!(root.starts_with(r"Software\PuwNotchTest\"));
        run_helper(&format!(r"{root}\Settings"), &format!(r"{root}\Backup"),
            &format!(r"Local\{root}").replace('\\', "_"), true).unwrap();
    }

    #[test]
    #[ignore]
    fn parent_fixture() {
        if std::env::var_os("PUW_BANNER_TEST_ROOT").is_none() { return; }
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "banner_guard::tests::helper_fixture", "--ignored"])
            .stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW.0).spawn().unwrap();
        let _pipe = child.stdin.take();
        child.wait().unwrap();
    }

    #[test]
    fn helper_restores_when_parent_is_killed() {
        let root = format!(r"Software\PuwNotchTest\Lifetime{}", std::process::id());
        let settings = Key::create(&format!(r"{root}\Settings")).unwrap();
        let app = settings.child("app", true).unwrap();
        app.set("ShowBanner", 1).unwrap();
        let mut parent = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "banner_guard::tests::parent_fixture", "--ignored"])
            .env("PUW_BANNER_TEST_ROOT", &root)
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW.0).spawn().unwrap();
        let wait_value = |value| {
            let deadline = Instant::now() + Duration::from_secs(5);
            while app.dword("ShowBanner").unwrap() != Some(value) {
                if Instant::now() >= deadline { return false; }
                std::thread::sleep(Duration::from_millis(20));
            }
            true
        };
        let suppressed = wait_value(0);
        parent.kill().unwrap();
        parent.wait().unwrap();
        assert!(suppressed, "helper did not suppress fixture banners");
        assert!(wait_value(1), "helper must restore after parent termination");
        drop((app, settings));
        unsafe { check(RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(wide(&root).as_ptr()))).unwrap(); }
    }

    #[test]
    fn suppress_restore_and_recover_journal() {
        let root = format!(r"Software\PuwNotchTest\Banner{}", std::process::id());
        let settings = Key::create(&format!(r"{root}\Settings")).unwrap();
        let backup = Key::create(&format!(r"{root}\Backup")).unwrap();
        let on = settings.child("on", true).unwrap(); on.set("ShowBanner", 1).unwrap();
        on.set("Enabled", 1).unwrap(); on.set("ShowInActionCenter", 1).unwrap();
        let off = settings.child("off", true).unwrap(); off.set("ShowBanner", 0).unwrap();
        let default = settings.child("default", true).unwrap();
        let mut seen = HashSet::new();
        suppress(&settings, &backup, &mut seen).unwrap();
        assert_eq!(on.dword("ShowBanner").unwrap(), Some(0));
        assert_eq!(default.dword("ShowBanner").unwrap(), Some(0));
        assert_eq!(on.dword("Enabled").unwrap(), Some(1));
        assert_eq!(on.dword("ShowInActionCenter").unwrap(), Some(1));
        assert_eq!(backup.children().unwrap().len(), 2);
        // Discover apps registered after startup; don't undo manual banner changes.
        let late = settings.child("late", true).unwrap();
        on.set("ShowBanner", 2).unwrap();
        suppress(&settings, &backup, &mut seen).unwrap();
        assert_eq!(on.dword("ShowBanner").unwrap(), Some(2));
        assert_eq!(late.dword("ShowBanner").unwrap(), Some(0));
        // Restoration uses the persistent journal, not the process's memory.
        drop(seen);
        restore(&settings, &backup).unwrap();
        assert_eq!(on.dword("ShowBanner").unwrap(), Some(2));
        assert_eq!(off.dword("ShowBanner").unwrap(), Some(0));
        assert_eq!(default.dword("ShowBanner").unwrap(), None);
        assert_eq!(late.dword("ShowBanner").unwrap(), None);
        assert!(backup.children().unwrap().is_empty());
        on.set("ShowBanner", 1).unwrap();
        suppress(&settings, &backup, &mut HashSet::new()).unwrap();
        restore(&settings, &backup).unwrap();
        assert_eq!(on.dword("ShowBanner").unwrap(), Some(1));
        drop((on, off, default, late, settings, backup));
        unsafe { check(RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(wide(&root).as_ptr()))).unwrap(); }
    }
}

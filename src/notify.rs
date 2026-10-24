//! Windows notifications, read from the notification centre database
//! (%LOCALAPPDATA%\Microsoft\Windows\Notifications\wpndatabase.db) with the SQLite that ships
//! in System32 (winsqlite3.dll). No package identity or extra dependency needed.
//! Only toasts Windows keeps in the notification centre show up: notifications must be on.

use std::ffi::{c_char, c_int, c_void, CString};
use windows::core::{s, w};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

#[derive(Clone, Debug, PartialEq)]
pub struct Toast {
    pub app: String,
    pub title: String,
    pub body: String,
    /// 0..1 bar for status banners (volume, brightness); None for notifications.
    pub level: Option<f32>,
    /// Fluent icon glyph for the banner; None = notification bell.
    pub icon: Option<&'static str>,
}

type Open = unsafe extern "system" fn(*const c_char, *mut *mut c_void, c_int, *const c_char) -> c_int;
type Prepare = unsafe extern "system" fn(*mut c_void, *const c_char, c_int, *mut *mut c_void, *mut *const c_char) -> c_int;
type Step = unsafe extern "system" fn(*mut c_void) -> c_int;
type ColInt = unsafe extern "system" fn(*mut c_void, c_int) -> i64;
type ColBlob = unsafe extern "system" fn(*mut c_void, c_int) -> *const c_void;
type ColBytes = unsafe extern "system" fn(*mut c_void, c_int) -> c_int;
type Finalize = unsafe extern "system" fn(*mut c_void) -> c_int;

const SQLITE_OPEN_READONLY: c_int = 1;
const SQLITE_ROW: c_int = 100;

struct Sqlite {
    prepare: Prepare,
    step: Step,
    col_int: ColInt,
    col_blob: ColBlob,
    col_bytes: ColBytes,
    finalize: Finalize,
}

pub struct Reader {
    sql: Sqlite,
    db: *mut c_void,
    last_id: i64,
}

macro_rules! proc {
    ($lib:expr, $name:expr) => {
        std::mem::transmute(GetProcAddress($lib, $name)?)
    };
}

impl Reader {
    /// Opens the database; what is already there is skipped, only newer toasts are reported.
    pub fn open() -> Option<Self> {
        let path = std::path::PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("Microsoft\\Windows\\Notifications\\wpndatabase.db");
        Self::open_path(path.to_str()?)
    }

    fn open_path(path: &str) -> Option<Self> {
        unsafe {
            let lib = LoadLibraryW(w!("winsqlite3.dll")).ok()?;
            let open: Open = proc!(lib, s!("sqlite3_open_v2"));
            let sql = Sqlite {
                prepare: proc!(lib, s!("sqlite3_prepare_v2")),
                step: proc!(lib, s!("sqlite3_step")),
                col_int: proc!(lib, s!("sqlite3_column_int64")),
                col_blob: proc!(lib, s!("sqlite3_column_blob")),
                col_bytes: proc!(lib, s!("sqlite3_column_bytes")),
                finalize: proc!(lib, s!("sqlite3_finalize")),
            };
            let cpath = CString::new(path).ok()?;
            let mut db = std::ptr::null_mut();
            if open(cpath.as_ptr(), &mut db, SQLITE_OPEN_READONLY, std::ptr::null()) != 0 {
                return None;
            }
            let mut r = Reader { sql, db, last_id: -1 };
            r.poll();
            Some(r)
        }
    }

    /// Toasts that arrived since the last call, oldest first.
    pub fn poll(&mut self) -> Vec<Toast> {
        let q = format!(
            "SELECT n.Id, h.PrimaryId, n.Payload FROM Notification n JOIN NotificationHandler h ON h.RecordId = n.HandlerId \
             WHERE n.Type = 'toast' AND n.Id > {} ORDER BY n.Id",
            self.last_id
        );
        let mut out = Vec::new();
        unsafe {
            let Ok(cq) = CString::new(q) else { return out };
            let mut stmt = std::ptr::null_mut();
            if (self.sql.prepare)(self.db, cq.as_ptr(), -1, &mut stmt, std::ptr::null_mut()) != 0 {
                return out;
            }
            while (self.sql.step)(stmt) == SQLITE_ROW {
                self.last_id = self.last_id.max((self.sql.col_int)(stmt, 0));
                let aumid = self.text(stmt, 1);
                let (title, body) = parse_payload(&self.text(stmt, 2));
                if !title.is_empty() {
                    out.push(Toast { app: app_name(&aumid), title, body, level: None, icon: None });
                }
            }
            (self.sql.finalize)(stmt);
        }
        out
    }

    unsafe fn text(&self, stmt: *mut c_void, col: c_int) -> String {
        let p = (self.sql.col_blob)(stmt, col) as *const u8;
        let n = (self.sql.col_bytes)(stmt, col).max(0) as usize;
        if p.is_null() {
            return String::new();
        }
        String::from_utf8_lossy(std::slice::from_raw_parts(p, n)).into_owned()
    }
}

/// Toast XML -> (first <text>, the other <text>s joined).
pub fn parse_payload(xml: &str) -> (String, String) {
    let mut texts = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find("<text") {
        rest = &rest[start + 5..];
        // skip "<texts>" or similar tags that only start with "text"
        if !rest.starts_with(['>', ' ', '/']) {
            continue;
        }
        let Some(gt) = rest.find('>') else { break };
        if rest[..gt].ends_with('/') {
            rest = &rest[gt + 1..];
            continue;
        }
        rest = &rest[gt + 1..];
        let Some(end) = rest.find("</text>") else { break };
        let t = unescape(rest[..end].trim());
        if !t.is_empty() {
            texts.push(t);
        }
        rest = &rest[end..];
    }
    let mut it = texts.into_iter();
    let title = it.next().unwrap_or_default();
    (title, it.collect::<Vec<_>>().join(" "))
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

/// App id -> short display name.
/// "{guid}\...\chrome.exe" -> "chrome", "MSTeams_8wekyb3d8bbwe!MSTeams" -> "MSTeams", "Microsoft.Windows.Explorer" -> "Explorer".
pub fn app_name(aumid: &str) -> String {
    let base = if let Some(i) = aumid.rfind(['\\', '/']) {
        let file = &aumid[i + 1..];
        file.rsplit_once('.').map_or(file, |(stem, _)| stem)
    } else if let Some((_, app)) = aumid.split_once('!') {
        app
    } else {
        aumid.rsplit('.').next().unwrap_or(aumid)
    };
    // Packaged apps often use the generic "App": take the package name instead.
    let name = if base.eq_ignore_ascii_case("App") {
        let family = aumid.split('_').next().unwrap_or(aumid);
        family.rsplit('.').next().unwrap_or(family)
    } else {
        base
    };
    name.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_texts() {
        let xml = r#"<toast launch="x"><visual><binding template="ToastGeneric"><text hint-maxLines="1">Anh A</text><text>Tối nay đi &amp; ăn &lt;3</text><text>thêm</text><text/></binding></visual></toast>"#;
        assert_eq!(parse_payload(xml), ("Anh A".into(), "Tối nay đi & ăn <3 thêm".into()));
        assert_eq!(parse_payload("<toast/>"), (String::new(), String::new()));
    }

    #[test]
    fn app_names() {
        assert_eq!(app_name(r"{6D809377-6AF0-444B-8957-A3773F02200E}\Google\Chrome\Application\chrome.exe"), "chrome");
        assert_eq!(app_name("MSTeams_8wekyb3d8bbwe!MSTeams"), "MSTeams");
        assert_eq!(app_name("Microsoft.Windows.Explorer"), "Explorer");
        assert_eq!(app_name("5319275A.WhatsAppDesktop_cv1g1gvanyjgm!App"), "WhatsAppDesktop");
    }

    /// Real winsqlite3 against a throwaway database with the same two tables.
    #[test]
    fn reads_new_rows_only() {
        let dir = std::env::temp_dir().join(format!("puw_wpn_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("wpn.db");
        let _ = std::fs::remove_file(&path);
        let sql = "CREATE TABLE NotificationHandler(RecordId INTEGER PRIMARY KEY, PrimaryId TEXT);\
                   CREATE TABLE Notification(Id INTEGER, HandlerId INTEGER, Type TEXT, Payload BLOB);\
                   INSERT INTO NotificationHandler VALUES(1,'MSTeams_8wekyb3d8bbwe!MSTeams');\
                   INSERT INTO Notification VALUES(5,1,'toast','<toast><text>Cũ</text></toast>');";
        exec(&path, sql);
        let mut r = Reader::open_path(path.to_str().unwrap()).expect("open");
        assert!(r.poll().is_empty(), "what was there at start is not new");
        exec(&path, "INSERT INTO Notification VALUES(6,1,'toast','<toast><text>Mới</text><text>nội dung</text></toast>');\
                     INSERT INTO Notification VALUES(7,1,'tile','<tile><text>bỏ qua</text></tile>');");
        let new = r.poll();
        assert_eq!(new.len(), 1);
        assert_eq!(new[0].title, "Mới");
        assert_eq!(new[0].body, "nội dung");
    }

    fn exec(path: &std::path::Path, sql: &str) {
        type Exec = unsafe extern "system" fn(*mut c_void, *const c_char, *const c_void, *mut c_void, *mut *mut c_char) -> c_int;
        type Close = unsafe extern "system" fn(*mut c_void) -> c_int;
        unsafe {
            let lib = LoadLibraryW(w!("winsqlite3.dll")).unwrap();
            let open: Open = std::mem::transmute(GetProcAddress(lib, s!("sqlite3_open_v2")).unwrap());
            let exec: Exec = std::mem::transmute(GetProcAddress(lib, s!("sqlite3_exec")).unwrap());
            let close: Close = std::mem::transmute(GetProcAddress(lib, s!("sqlite3_close")).unwrap());
            let p = CString::new(path.to_str().unwrap()).unwrap();
            let mut db = std::ptr::null_mut();
            assert_eq!(open(p.as_ptr(), &mut db, 2 | 4, std::ptr::null()), 0);
            let q = CString::new(sql).unwrap();
            assert_eq!(exec(db, q.as_ptr(), std::ptr::null(), std::ptr::null_mut(), std::ptr::null_mut()), 0);
            close(db);
        }
    }
}

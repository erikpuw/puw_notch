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
    /// A picture file the notification carries (a screenshot's preview): click the banner to open it.
    pub image: Option<String>,
    /// What clicking the banner opens (see `parse_open`); None for status banners.
    pub open: Option<String>,
    /// Short text at the right end of the title row: a Bluetooth device's battery.
    pub badge: Option<String>,
}

/// One notification on the panel's Notifications page.
#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    /// App user model id: where Windows takes the app's icon from.
    pub id: String,
    pub app: String,
    pub title: String,
    pub body: String,
    /// Arrival as local yyyymmddHHMM.
    pub at: u64,
    /// What clicking the row opens: its picture, else its link or app (as for the banner).
    pub open: Option<String>,
}

/// Windows FILETIME (100 ns ticks since 1601-01-01 UTC) -> local yyyymmddHHMM, given the local
/// offset from UTC in minutes.
pub fn local_stamp(filetime: i64, utc_offset_min: i64) -> u64 {
    let total = filetime / 600_000_000 + utc_offset_min; // minutes since 1601-01-01
    let (days, minute) = (total.div_euclid(1440), total.rem_euclid(1440));
    // civil from days (Howard Hinnant), counted from 1970-01-01
    let z = days - 134_774 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (((y * 100 + m) * 100 + d) * 10_000 + minute / 60 * 100 + minute % 60) as u64
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
    /// Notifications with an id up to this are left out of `recent` (cleared on the page).
    pub cleared: i64,
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
            let mut r = Reader { sql, db, last_id: -1, cleared: 0 };
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
                let payload = self.text(stmt, 2);
                let (title, body) = parse_payload(&payload);
                if !title.is_empty() {
                    let open = parse_open(&aumid, &payload);
                    out.push(Toast { app: app_name(&aumid), title, body, level: None, icon: None, image: parse_image(&payload), open, badge: None });
                }
            }
            (self.sql.finalize)(stmt);
        }
        out
    }

    /// Hide everything that has arrived so far from `recent`; returns the id to keep (settings.txt).
    pub fn clear(&mut self) -> i64 {
        self.cleared = self.last_id.max(self.cleared);
        self.cleared
    }

    /// The newest `limit` notifications still in the notification centre, newest first (what the
    /// user cleared there is gone here too). `utc_offset_min` turns arrival times into local time.
    pub fn recent(&self, limit: usize, utc_offset_min: i64) -> Vec<Note> {
        let q = format!(
            "SELECT h.PrimaryId, n.Payload, n.ArrivalTime FROM Notification n JOIN NotificationHandler h ON h.RecordId = n.HandlerId \
             WHERE n.Type = 'toast' AND n.Id > {} ORDER BY n.ArrivalTime DESC, n.Id DESC LIMIT {limit}",
            self.cleared
        );
        let mut out = Vec::new();
        unsafe {
            let Ok(cq) = CString::new(q) else { return out };
            let mut stmt = std::ptr::null_mut();
            if (self.sql.prepare)(self.db, cq.as_ptr(), -1, &mut stmt, std::ptr::null_mut()) != 0 {
                return out;
            }
            while (self.sql.step)(stmt) == SQLITE_ROW {
                let payload = self.text(stmt, 1);
                let (title, body) = parse_payload(&payload);
                if !title.is_empty() {
                    let at = local_stamp((self.sql.col_int)(stmt, 2), utc_offset_min);
                    let id = self.text(stmt, 0);
                    let open = parse_image(&payload).or_else(|| parse_open(&id, &payload));
                    out.push(Note { app: app_name(&id), id, title, body, at, open });
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

/// The picture file a toast carries: its hero image if it has one, else the first image that is not an app logo, when that
/// is a file on disk (`file:///C%3A%5C...` or a plain path). Snipping Tool's "Screenshot copied"
/// toast carries the saved screenshot this way.
pub fn parse_image(xml: &str) -> Option<String> {
    let tags: Vec<&str> = xml.split("<image").skip(1).filter_map(|t| t.split('>').next()).collect();
    let src_of = |tag: &str| {
        let rest = &tag[tag.find(" src=\"")? + 6..];
        Some(unescape(&rest[..rest.find('"')?]))
    };
    let hero = tags.iter().find(|t| t.contains("placement=\"hero\""));
    // An app's own logo / the sender's avatar (Discord) is not what the toast is about: clicking
    // it must open the app, so it is never taken as the picture.
    let plain = tags.iter().find(|t| !t.contains("placement=\"appLogoOverride\""));
    let src = hero.or(plain).and_then(|t| src_of(t))?;
    let path = percent_decode(src.strip_prefix("file:///").unwrap_or(&src)).replace('/', "\\");
    // A drive path (C:\...) or a share (\\server\...), nothing from the web.
    let drive = path.as_bytes().get(1) == Some(&b':') && path.as_bytes()[0].is_ascii_alphabetic();
    (drive || path.starts_with("\\\\")).then_some(path)
}

/// What clicking a notification opens, for ShellExecute: the toast's own link when it is a
/// protocol toast (`windowsdefender://update`), else the app that sent it, started through
/// `shell:AppsFolder` by its app id (works for packaged and desktop apps alike).
/// ponytail: opens the app, not the exact message; that needs the app's COM toast activator.
pub fn parse_open(aumid: &str, xml: &str) -> Option<String> {
    let tag = &xml[xml.find("<toast")?..];
    let tag = &tag[..tag.find('>').unwrap_or(tag.len())];
    let attr = |name: &str| {
        let rest = &tag[tag.find(&format!(" {name}=\""))? + name.len() + 3..];
        Some(unescape(&rest[..rest.find('"')?]))
    };
    // A real URI scheme only: no drive paths ("C:\..") and no local files.
    let uri = attr("launch").filter(|_| attr("activationType").as_deref() == Some("protocol")).filter(|l| {
        l.split_once(':').is_some_and(|(scheme, _)| scheme.len() > 1 && scheme.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c)) && !scheme.eq_ignore_ascii_case("file"))
    });
    uri.or_else(|| (!aumid.is_empty()).then(|| format!("shell:AppsFolder\\{aumid}")))
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
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
    fn screenshot_toast_carries_the_saved_file() {
        // The payload Snipping Tool writes for "Screenshot copied to clipboard".
        let xml = r#"<toast launch="ms-screensketch:edit?&amp;filePath=C%3A%5CUsers%5Cerikpuw%5CPictures%5CScreenshots%5CScreenshot%202026-10-28%20223805.png&amp;saved=true" activationType="protocol"><visual><binding template="ToastGeneric"><image placement="hero" src="file:///C%3A%5CUsers%5Cerikpuw%5CPictures%5CScreenshots%5CScreenshot%202026-10-28%20223805.png" alt="Snipped screen area"></image><text>Screenshot copied to clipboard</text><text>Automatically saved to screenshots folder.</text></binding></visual></toast>"#;
        assert_eq!(parse_image(xml).as_deref(), Some(r"C:\Users\erikpuw\Pictures\Screenshots\Screenshot 2026-10-28 223805.png"));
        assert_eq!(parse_payload(xml).0, "Screenshot copied to clipboard");
        // The hero wins over a logo; web pictures and toasts without one give nothing.
        let two = r#"<image placement="appLogoOverride" src="file:///C:/a.png"/><image placement="hero" src="file:///C:/b.png"/>"#;
        assert_eq!(parse_image(two).as_deref(), Some(r"C:\b.png"));
        assert_eq!(parse_image(r#"<image src="https://example.com/x.png"/>"#), None);
        assert_eq!(parse_image("<toast><text>hi</text></toast>"), None);
    }

    #[test]
    fn click_opens_link_or_app() {
        let defender = r#"<toast launch="windowsdefender://update" activationType="protocol"><visual/></toast>"#;
        assert_eq!(parse_open("Windows.Defender.SecurityCenter", defender).as_deref(), Some("windowsdefender://update"));
        // Foreground toasts carry app arguments, not a link: start the app.
        let teams = r#"<toast launch="{&quot;chat&quot;:1}"><visual/></toast>"#;
        assert_eq!(parse_open("MSTeams_8wekyb3d8bbwe!MSTeams", teams).as_deref(), Some(r"shell:AppsFolder\MSTeams_8wekyb3d8bbwe!MSTeams"));
        assert_eq!(parse_open("Claude_pzs8sxrjxfjjc!Claude", "<toast><visual/></toast>").as_deref(), Some(r"shell:AppsFolder\Claude_pzs8sxrjxfjjc!Claude"));
        // A protocol "link" that is really a file is not followed.
        for bad in [r"C:\evil.exe", "file:///C:/evil.exe"] {
            let xml = format!(r#"<toast launch="{bad}" activationType="protocol"/>"#);
            assert_eq!(parse_open("App!A", &xml).as_deref(), Some(r"shell:AppsFolder\App!A"));
        }
        assert_eq!(parse_open("", "<toast/>"), None);
        assert_eq!(parse_open("App!A", "<tile/>"), None);
    }

    #[test]
    fn payload_texts() {
        let xml = r#"<toast launch="x"><visual><binding template="ToastGeneric"><text hint-maxLines="1">Anh A</text><text>Tối nay đi &amp; ăn &lt;3</text><text>thêm</text><text/></binding></visual></toast>"#;
        assert_eq!(parse_payload(xml), ("Anh A".into(), "Tối nay đi & ăn <3 thêm".into()));
        assert_eq!(parse_payload("<toast/>"), (String::new(), String::new()));
    }

    #[test]
    fn filetime_to_local_stamp() {
        // 2026-10-26 07:51 UTC
        let ft = 134_374_746_600_000_000;
        assert_eq!(local_stamp(ft, 0), 202610260751);
        assert_eq!(local_stamp(ft, 7 * 60), 202610261451, "UTC+7");
        assert_eq!(local_stamp(ft, -8 * 60), 202610252351, "back across midnight");
        assert_eq!(local_stamp(0, 0), 160101010000);
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
                   CREATE TABLE Notification(Id INTEGER, HandlerId INTEGER, Type TEXT, Payload BLOB, ArrivalTime INTEGER);\
                   INSERT INTO NotificationHandler VALUES(1,'MSTeams_8wekyb3d8bbwe!MSTeams');\
                   INSERT INTO Notification VALUES(5,1,'toast','<toast><text>Cũ</text></toast>',134374746600000000);";
        exec(&path, sql);
        let mut r = Reader::open_path(path.to_str().unwrap()).expect("open");
        assert!(r.poll().is_empty(), "what was there at start is not new");
        exec(&path, "INSERT INTO Notification VALUES(6,1,'toast','<toast><text>Mới</text><text>nội dung</text></toast>',134374747200000000);\
                     INSERT INTO Notification VALUES(7,1,'tile','<tile><text>bỏ qua</text></tile>',134374747800000000);");
        let new = r.poll();
        assert_eq!(new.len(), 1);
        assert_eq!(new[0].title, "Mới");
        assert_eq!(new[0].body, "nội dung");
        // the Notifications page: every toast still there, newest first, local time
        let list = r.recent(30, 7 * 60);
        assert_eq!(list.iter().map(|n| (n.title.as_str(), n.at)).collect::<Vec<_>>(), [("Mới", 202610261452), ("Cũ", 202610261451)]);
        assert_eq!(list[0].app, "MSTeams");
        assert_eq!(list[0].open.as_deref(), Some(r"shell:AppsFolder\MSTeams_8wekyb3d8bbwe!MSTeams"), "a row opens its app");
        assert_eq!(r.recent(1, 0).len(), 1);
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

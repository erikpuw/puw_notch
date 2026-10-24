//! Now playing (any app that reports to Windows media controls: Spotify, browsers, ...)
//! plus synced lyrics from LRCLIB. WinRT calls block, so a worker thread polls and the UI reads
//! a shared snapshot after a WM_MUSIC nudge.

use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows::core::HSTRING;
use windows::Data::Json::{JsonArray, JsonObject, JsonValueType};
use windows::Foundation::Uri;
use windows::Media::Control::*;
use windows::Storage::Streams::{DataReader, IRandomAccessStreamReference};
use windows::Web::Http::HttpClient;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

/// Posted when the snapshot changed (new track, play/pause, lyrics arrived).
pub const WM_MUSIC: u32 = WM_APP + 5;
const POLL: Duration = Duration::from_millis(500);

#[derive(Clone, Default)]
pub struct Track {
    pub title: String,
    pub artist: String,
    pub playing: bool,
    pub duration_ms: u64,
    position_ms: u64,
    /// When `position_ms` was read; the UI extrapolates from here while playing.
    at: Option<Instant>,
    /// Encoded cover image (PNG/JPEG), decoded by the renderer; `art_key` changes with the track.
    pub art: Option<Arc<Vec<u8>>>,
    pub art_key: u64,
    /// Synced lyric lines (ms, text), sorted. Empty = none found (yet).
    pub lyrics: Arc<Vec<(u64, String)>>,
}

impl Track {
    /// 0 when the player reports no timeline (e.g. foobar2000): nothing to extrapolate from.
    pub fn position_ms(&self) -> u64 {
        if self.duration_ms == 0 {
            return 0;
        }
        let extra = match (self.playing, self.at) {
            (true, Some(at)) => at.elapsed().as_millis() as u64,
            _ => 0,
        };
        (self.position_ms + extra).min(self.duration_ms)
    }

    pub fn lyric_now(&self) -> Option<&str> {
        current_line(&self.lyrics, self.position_ms())
    }
}

pub enum Cmd {
    PlayPause,
    Next,
    Previous,
}

pub struct Music {
    pub now: Arc<Mutex<Option<Track>>>,
    cmds: Sender<Cmd>,
}

impl Music {
    pub fn start(hwnd: HWND) -> Self {
        let now = Arc::new(Mutex::new(None));
        let (cmds, rx) = channel();
        let shared = now.clone();
        let target = hwnd.0 as isize;
        std::thread::spawn(move || worker(shared, rx, target));
        Music { now, cmds }
    }

    pub fn send(&self, c: Cmd) {
        let _ = self.cmds.send(c);
    }

    pub fn snapshot(&self) -> Option<Track> {
        self.now.lock().ok()?.clone()
    }
}

fn worker(shared: Arc<Mutex<Option<Track>>>, rx: Receiver<Cmd>, target: isize) {
    unsafe { let _ = RoInitialize(RO_INIT_MULTITHREADED); }
    let notify = || unsafe { let _ = PostMessageW(Some(HWND(target as _)), WM_MUSIC, WPARAM(0), LPARAM(0)); };
    let Ok(manager) = GlobalSystemMediaTransportControlsSessionManager::RequestAsync().and_then(|op| op.get()) else { return };
    let mut key = String::new();
    let mut art_key = 0u64;
    loop {
        let cmd = match rx.recv_timeout(POLL) {
            Ok(c) => Some(c),
            Err(RecvTimeoutError::Disconnected) => return,
            Err(RecvTimeoutError::Timeout) => None,
        };
        let session = manager.GetCurrentSession().ok();
        if let (Some(cmd), Some(s)) = (cmd, &session) {
            let op = match cmd {
                Cmd::PlayPause => s.TryTogglePlayPauseAsync(),
                Cmd::Next => s.TrySkipNextAsync(),
                Cmd::Previous => s.TrySkipPreviousAsync(),
            };
            let _ = op.and_then(|o| o.get());
        }
        let Some(session) = session else {
            let had = shared.lock().map(|mut g| g.take().is_some()).unwrap_or(false);
            if had {
                key.clear();
                notify();
            }
            continue;
        };
        let Some(mut t) = read(&session) else { continue };
        let new_key = format!("{}\u{1}{}", t.title, t.artist);
        let changed_track = new_key != key;
        let prev = shared.lock().ok().and_then(|g| g.clone());
        if changed_track {
            key = new_key;
            art_key += 1;
            t.art = session.TryGetMediaPropertiesAsync().and_then(|o| o.get()).and_then(|p| p.Thumbnail()).ok().and_then(|r| read_stream(&r)).map(Arc::new);
        } else if let Some(p) = &prev {
            t.art = p.art.clone();
            t.lyrics = p.lyrics.clone();
        }
        t.art_key = art_key;
        let visible_change = prev.as_ref().map_or(true, |p| changed_track || p.playing != t.playing || p.duration_ms != t.duration_ms);
        if let Ok(mut g) = shared.lock() {
            *g = Some(t.clone());
        }
        if visible_change {
            notify();
        }
        if changed_track && !t.title.is_empty() {
            let lines = fetch_lyrics(&t.title, &t.artist, t.duration_ms / 1000);
            if !lines.is_empty() {
                if let Ok(mut g) = shared.lock() {
                    if let Some(cur) = g.as_mut().filter(|c| c.art_key == art_key) {
                        cur.lyrics = Arc::new(lines);
                    }
                }
                notify();
            }
        }
    }
}

fn read(s: &GlobalSystemMediaTransportControlsSession) -> Option<Track> {
    let props = s.TryGetMediaPropertiesAsync().ok()?.get().ok()?;
    let playing = s.GetPlaybackInfo().and_then(|i| i.PlaybackStatus()).ok() == Some(GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing);
    let tl = s.GetTimelineProperties().ok();
    // TimeSpan / DateTime are 100 ns ticks.
    let ms = |ticks: i64| (ticks.max(0) / 10_000) as u64;
    let duration_ms = tl.as_ref().and_then(|t| Some(ms(t.EndTime().ok()?.Duration - t.StartTime().ok()?.Duration))).unwrap_or(0);
    let mut position_ms = tl.as_ref().and_then(|t| t.Position().ok()).map_or(0, |p| ms(p.Duration));
    // Position is as of LastUpdatedTime; players only refresh it now and then.
    if playing {
        if let Some(updated) = tl.as_ref().and_then(|t| t.LastUpdatedTime().ok()) {
            position_ms += ms(now_ticks() - updated.UniversalTime);
        }
    }
    Some(Track {
        title: props.Title().ok()?.to_string(),
        artist: props.Artist().map(|a| a.to_string()).unwrap_or_default(),
        playing,
        duration_ms,
        position_ms,
        at: Some(Instant::now()),
        ..Default::default()
    })
}

fn now_ticks() -> i64 {
    let ft = unsafe { windows::Win32::System::SystemInformation::GetSystemTimePreciseAsFileTime() };
    ((ft.dwHighDateTime as i64) << 32) | ft.dwLowDateTime as i64
}

fn read_stream(r: &IRandomAccessStreamReference) -> Option<Vec<u8>> {
    let stream = r.OpenReadAsync().ok()?.get().ok()?;
    let size = u32::try_from(stream.Size().ok()?).ok()?;
    if size == 0 || size > 8 << 20 {
        return None;
    }
    let reader = DataReader::CreateDataReader(&stream).ok()?;
    reader.LoadAsync(size).ok()?.get().ok()?;
    let mut buf = vec![0u8; size as usize];
    reader.ReadBytes(&mut buf).ok()?;
    Some(buf)
}

/// LRCLIB: exact match first (title + artist + duration), then a search.
fn fetch_lyrics(title: &str, artist: &str, duration_s: u64) -> Vec<(u64, String)> {
    let base = "https://lrclib.net/api";
    let q = format!("track_name={}&artist_name={}", url_encode(title), url_encode(artist));
    if duration_s > 0 {
        if let Some(body) = http_get(&format!("{base}/get?{q}&duration={duration_s}")) {
            if let Some(lines) = JsonObject::Parse(&HSTRING::from(body)).ok().and_then(|o| synced(&o)) {
                return lines;
            }
        }
    }
    let Some(body) = http_get(&format!("{base}/search?{q}")) else { return Vec::new() };
    let Ok(items) = JsonArray::Parse(&HSTRING::from(body)) else { return Vec::new() };
    let n = items.Size().unwrap_or(0);
    (0..n).filter_map(|i| items.GetObjectAt(i).ok()).find_map(|o| synced(&o)).unwrap_or_default()
}

fn synced(o: &JsonObject) -> Option<Vec<(u64, String)>> {
    let v = o.GetNamedValue(&HSTRING::from("syncedLyrics")).ok()?;
    if v.ValueType().ok()? != JsonValueType::String {
        return None;
    }
    let lines = parse_lrc(&v.GetString().ok()?.to_string());
    (!lines.is_empty()).then_some(lines)
}

fn http_get(url: &str) -> Option<String> {
    let client = HttpClient::new().ok()?;
    let _ = client.DefaultRequestHeaders().ok()?.UserAgent().ok()?.TryParseAdd(&HSTRING::from("PUW-Notch/0.1 (Windows)"));
    let uri = Uri::CreateUri(&HSTRING::from(url)).ok()?;
    let resp = client.GetAsync(&uri).ok()?.get().ok()?;
    if !resp.IsSuccessStatusCode().ok()? {
        return None;
    }
    Some(resp.Content().ok()?.ReadAsStringAsync().ok()?.get().ok()?.to_string())
}

/// Percent-encode for a query value (UTF-8, unreserved chars kept).
pub fn url_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// "[01:02.50]text" lines -> sorted (ms, text). Lines with several stamps repeat; metadata tags are skipped.
pub fn parse_lrc(lrc: &str) -> Vec<(u64, String)> {
    let mut out = Vec::new();
    for line in lrc.lines() {
        let mut rest = line.trim();
        let mut stamps = Vec::new();
        while let Some(end) = rest.strip_prefix('[').and_then(|r| r.find(']')) {
            let tag = &rest[1..end + 1];
            rest = &rest[end + 2..];
            if let Some(ms) = parse_stamp(tag) {
                stamps.push(ms);
            }
        }
        let text = rest.trim();
        for ms in stamps {
            out.push((ms, text.to_string()));
        }
    }
    out.sort_by_key(|l| l.0);
    out
}

fn parse_stamp(tag: &str) -> Option<u64> {
    let (m, s) = tag.split_once(':')?;
    let m: u64 = m.parse().ok()?;
    let secs: f64 = s.parse().ok()?;
    Some(m * 60_000 + (secs * 1000.0).round() as u64)
}

/// Line being sung at `pos_ms` (the last one that started), skipping empty spacer lines.
pub fn current_line(lines: &[(u64, String)], pos_ms: u64) -> Option<&str> {
    let i = lines.partition_point(|l| l.0 <= pos_ms);
    let (_, text) = lines.get(i.checked_sub(1)?)?;
    (!text.is_empty()).then_some(text.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lrc_lines() {
        let lrc = "[ar:Ca sĩ]\n[00:01.00]Câu một\n[00:03.50][00:10.00]Điệp khúc\n[00:05.00]\n";
        let l = parse_lrc(lrc);
        assert_eq!(l, vec![(1000, "Câu một".into()), (3500, "Điệp khúc".into()), (5000, String::new()), (10000, "Điệp khúc".into())]);
        assert_eq!(current_line(&l, 500), None);
        assert_eq!(current_line(&l, 1000), Some("Câu một"));
        assert_eq!(current_line(&l, 4000), Some("Điệp khúc"));
        assert_eq!(current_line(&l, 6000), None);
        assert_eq!(current_line(&l, 60000), Some("Điệp khúc"));
    }

    /// Machine state: `cargo test list_sessions -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn list_sessions() {
        unsafe { let _ = RoInitialize(RO_INIT_MULTITHREADED); }
        let m = GlobalSystemMediaTransportControlsSessionManager::RequestAsync().unwrap().get().unwrap();
        let cur = m.GetCurrentSession().ok().and_then(|s| s.SourceAppUserModelId().ok());
        eprintln!("current: {cur:?}");
        for s in m.GetSessions().unwrap() {
            let t = read(&s);
            eprintln!("{:?} title={:?} playing={:?} dur={:?}", s.SourceAppUserModelId().ok(), t.as_ref().map(|t| &t.title), t.as_ref().map(|t| t.playing), t.as_ref().map(|t| t.duration_ms));
        }
    }

    /// Network: `cargo test lrclib_live -- --ignored`
    #[test]
    #[ignore]
    fn lrclib_live() {
        unsafe { let _ = RoInitialize(RO_INIT_MULTITHREADED); }
        let lines = fetch_lyrics("Lạc Trôi", "Sơn Tùng M-TP", 0);
        assert!(lines.len() > 10, "got {} lines", lines.len());
        eprintln!("first: {:?}", &lines[..3]);
    }

    #[test]
    fn encode() {
        assert_eq!(url_encode("Lạc Trôi & co"), "L%E1%BA%A1c%20Tr%C3%B4i%20%26%20co");
    }
}

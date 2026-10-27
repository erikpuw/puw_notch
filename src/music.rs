//! Now playing (any app that reports to Windows media controls: Spotify, browsers, ...)
//! plus synced lyrics from LRCLIB. WinRT calls block, so a worker thread polls and the UI reads
//! a shared snapshot after a WM_MUSIC nudge.

use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows::core::{w, HSTRING};
use windows::Data::Json::{JsonArray, JsonObject, JsonValueType};
use windows::Foundation::Uri;
use windows::Media::Control::*;
use windows::Storage::Streams::{DataReader, IRandomAccessStreamReference};
use windows::Web::Http::HttpClient;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};
use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, GetForegroundWindow, PostMessageW, WM_APP};

/// Posted when the snapshot changed (new track, play/pause, lyrics arrived).
pub const WM_MUSIC: u32 = WM_APP + 5;
const POLL: Duration = Duration::from_millis(500);
/// A paused player keeps the music pill this long, then the notch goes back to the clock.
const PAUSE_KEEP: Duration = Duration::from_secs(10);

/// Music layout in the resting pill: while playing, or for `PAUSE_KEEP` after a pause so a quick
/// resume is still one hover away. `paused_for` = None when nothing is paused.
pub fn shows_pill(playing: bool, paused_for: Option<Duration>) -> bool {
    playing || paused_for.is_some_and(|d| d < PAUSE_KEEP)
}

#[derive(Clone, Default)]
pub struct Track {
    pub source_id: String,
    pub title: String,
    pub artist: String,
    pub playing: bool,
    pub duration_ms: u64,
    pub(crate) position_ms: u64,
    pub(crate) start_ticks: i64,
    pub(crate) rate: f64,
    pub(crate) reported_position: i64,
    pub(crate) updated_ticks: i64,
    pub can_toggle: bool,
    pub can_next: bool,
    pub can_previous: bool,
    pub can_seek: bool,
    pub command_error: Option<&'static str>,
    /// When `position_ms` was read; the UI extrapolates from here while playing.
    pub(crate) at: Option<Instant>,
    /// Encoded cover image (PNG/JPEG), decoded by the renderer; `art_key` changes with the track.
    pub art: Option<Arc<Vec<u8>>>,
    pub art_key: u64,
    /// Synced lyric lines (ms, text), sorted. Empty = none found (yet).
    pub lyrics: Arc<Vec<(u64, String)>>,
}

impl Track {
    /// 0 when the player reports no timeline (e.g. foobar2000): nothing to extrapolate from.
    pub fn position_ms(&self) -> u64 {
        self.position_after(self.at.map_or(Duration::ZERO, |at| at.elapsed()))
    }

    fn position_after(&self, elapsed: Duration) -> u64 {
        if self.duration_ms == 0 {
            return 0;
        }
        let extra = if self.playing { (elapsed.as_millis() as f64 * self.rate) as u64 } else { 0 };
        self.position_ms.saturating_add(extra).min(self.duration_ms)
    }

    fn seek_ticks(&self, position_ms: u64) -> i64 {
        self.start_ticks.saturating_add((position_ms.min(self.duration_ms) as i64).saturating_mul(10_000))
    }

    fn inherit_cached(&mut self, previous: &Track) {
        if self.updated_ticks <= 0 && previous.updated_ticks <= 0 && self.reported_position == previous.reported_position {
            self.position_ms = previous.position_ms();
        }
        self.art = previous.art.clone();
        self.lyrics = previous.lyrics.clone();
        self.command_error = previous.command_error;
    }

    pub fn lyric_now(&self) -> Option<&str> {
        current_line(&self.lyrics, self.position_ms())
    }
}

pub enum Cmd {
    PlayPause,
    Next,
    Previous,
    Seek(u64),
}

pub struct Music {
    pub now: Arc<Mutex<Option<Track>>>,
    cmds: Sender<(Cmd, Track)>,
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
        if let Some(track) = self.snapshot() { let _ = self.cmds.send((c, track)); }
    }

    pub fn snapshot(&self) -> Option<Track> {
        self.now.lock().ok()?.clone()
    }
}

fn worker(shared: Arc<Mutex<Option<Track>>>, rx: Receiver<(Cmd, Track)>, target: isize) {
    unsafe { let _ = RoInitialize(RO_INIT_MULTITHREADED); }
    let notify = || unsafe { let _ = PostMessageW(Some(HWND(target as _)), WM_MUSIC, WPARAM(0), LPARAM(0)); };
    let manager = GlobalSystemMediaTransportControlsSessionManager::RequestAsync().and_then(|op| op.get()).ok();
    let mut key = String::new();
    let mut art_key = 0u64;
    let (assets, jobs) = channel();
    let asset_shared = shared.clone();
    std::thread::spawn(move || asset_worker(asset_shared, jobs, target));
    loop {
        let cmd = match rx.recv_timeout(POLL) {
            Ok(c) => Some(c),
            Err(RecvTimeoutError::Disconnected) => return,
            Err(RecvTimeoutError::Timeout) => None,
        };
        let native_target = cmd.as_ref().is_some_and(|(_, t)| t.source_id == crate::itunes::SOURCE_ID);
        let mut session = match &cmd {
            Some(_) if native_target => None,
            Some((_, track)) => manager.as_ref().and_then(|m| m.GetSessions().ok()).and_then(|sessions|
                sessions.into_iter().find(|s| s.SourceAppUserModelId().is_ok_and(|id| id == track.source_id.as_str()))),
            None => manager.as_ref().and_then(|m| m.GetCurrentSession().ok()),
        };
        let had_command = cmd.is_some();
        let mut command_error = None;
        if let Some((cmd, track)) = &cmd {
            let accepted = if native_target { crate::itunes::command(cmd) }
            else if let Some(s) = &session {
            let op = match cmd {
                Cmd::PlayPause => {
                    let info = s.GetPlaybackInfo().ok();
                    if info.as_ref().and_then(|i| i.Controls().ok()).is_some_and(|c| c.IsPlayPauseToggleEnabled().unwrap_or(false)) {
                        s.TryTogglePlayPauseAsync()
                    } else if info.and_then(|i| i.PlaybackStatus().ok()) == Some(GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing) {
                        s.TryPauseAsync()
                    } else { s.TryPlayAsync() }
                }
                Cmd::Next => s.TrySkipNextAsync(),
                Cmd::Previous => s.TrySkipPreviousAsync(),
                Cmd::Seek(position) => {
                    s.TryChangePlaybackPositionAsync(track.seek_ticks(*position))
                }
            };
            op.and_then(|o| o.get()) == Ok(true)
            } else { false };
            if !accepted { command_error = Some(crate::i18n::tr("Trình phát chưa nhận lệnh. Hãy thử lại.", "The player did not take the command. Try again.")); }
        }
        let standard = session.as_ref().and_then(read);
        let native = if cmd.is_none() || native_target { crate::itunes::read() } else { None };
        let native_foreground = unsafe { FindWindowW(None, w!("iTunes")).is_ok_and(|h| h == GetForegroundWindow()) };
        let selected = if native_target || (cmd.is_none() && prefer_itunes(standard.as_ref(), native.as_ref(), native_foreground)) {
            session = None;
            native
        } else { standard };
        let Some(mut t) = selected else {
            let had = shared.lock().map(|mut g| g.take().is_some()).unwrap_or(false);
            if had {
                key.clear();
                notify();
            }
            continue;
        };
        // Duration stays out: some players report it only on some polls, and a flapping key
        // would drop the lyrics and fetch them again every time.
        let new_key = format!("{}\u{1}{}\u{1}{}", t.source_id, t.title, t.artist);
        let changed_track = new_key != key;
        let prev = shared.lock().ok().and_then(|g| g.clone());
        if changed_track {
            key = new_key;
            art_key += 1;
        }
        t.art_key = art_key;
        let visible_change = had_command || prev.as_ref().is_none_or(|p| changed_track || p.playing != t.playing || p.duration_ms != t.duration_ms);
        if let Ok(mut g) = shared.lock() {
            if let Some(p) = g.as_ref().filter(|p| !changed_track && p.art_key == art_key) {
                t.inherit_cached(p);
            }
            if had_command { t.command_error = command_error; }
            *g = Some(t.clone());
        }
        if visible_change {
            notify();
        }
        if changed_track { let _ = assets.send((t, session)); }
    }
}

fn asset_worker(shared: Arc<Mutex<Option<Track>>>, rx: Receiver<(Track, Option<GlobalSystemMediaTransportControlsSession>)>, target: isize) {
    unsafe { let _ = RoInitialize(RO_INIT_MULTITHREADED); }
    while let Ok(mut job) = rx.recv() {
        for newer in rx.try_iter() { job = newer; }
        let (t, session) = job;
        if !shared.lock().is_ok_and(|g| g.as_ref().is_some_and(|c| c.art_key == t.art_key)) { continue; }
        let art = match session {
            Some(s) => s.TryGetMediaPropertiesAsync().and_then(|o| o.get())
                .and_then(|p| p.Thumbnail()).ok().and_then(|r| read_stream(&r)),
            None if t.source_id == crate::itunes::SOURCE_ID => crate::itunes::artwork(&t.title, &t.artist),
            None => None,
        }.map(Arc::new);
        if let Ok(mut g) = shared.lock() {
            if let Some(cur) = g.as_mut().filter(|c| c.art_key == t.art_key) { cur.art = art; }
        }
        unsafe { let _ = PostMessageW(Some(HWND(target as _)), WM_MUSIC, WPARAM(0), LPARAM(0)); }
        if t.title.is_empty() { continue; }
        let lines = fetch_lyrics(&t.title, &t.artist, t.duration_ms / 1000);
        if let Ok(mut g) = shared.lock() {
            if let Some(cur) = g.as_mut().filter(|c| c.art_key == t.art_key) { cur.lyrics = Arc::new(lines); }
        }
        unsafe { let _ = PostMessageW(Some(HWND(target as _)), WM_MUSIC, WPARAM(0), LPARAM(0)); }
    }
}

fn prefer_itunes(standard: Option<&Track>, native: Option<&Track>, foreground: bool) -> bool {
    let Some(native) = native else { return false };
    let Some(standard) = standard else { return true };
    match (native.playing, standard.playing) {
        (true, false) => true,
        (false, true) => false,
        _ => foreground,
    }
}

fn read(s: &GlobalSystemMediaTransportControlsSession) -> Option<Track> {
    let props = s.TryGetMediaPropertiesAsync().ok()?.get().ok()?;
    let info = s.GetPlaybackInfo().ok()?;
    let playing = info.PlaybackStatus().ok() == Some(GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing);
    let controls = info.Controls().ok();
    let rate = info.PlaybackRate().and_then(|r| r.Value()).ok().filter(|r| r.is_finite() && *r >= 0.0).unwrap_or(1.0);
    let tl = s.GetTimelineProperties().ok();
    let start_ticks = tl.as_ref().and_then(|t| t.StartTime().ok()).map_or(0, |t| t.Duration);
    let reported_position = tl.as_ref().and_then(|t| t.Position().ok()).map_or(0, |t| t.Duration);
    let updated_ticks = tl.as_ref().and_then(|t| t.LastUpdatedTime().ok()).map_or(0, |t| t.UniversalTime);
    let (duration_ms, position_ms) = tl.as_ref().and_then(|t| Some(timeline_position(start_ticks,
        t.EndTime().ok()?.Duration, reported_position, updated_ticks,
        now_ticks(), playing, rate))).unwrap_or((0, 0));
    let mut track = Track {
        source_id: s.SourceAppUserModelId().ok()?.to_string(),
        title: props.Title().ok()?.to_string(),
        artist: props.Artist().map(|a| a.to_string()).unwrap_or_default(),
        playing,
        duration_ms,
        position_ms,
        start_ticks,
        rate,
        reported_position,
        updated_ticks,
        can_toggle: controls.as_ref().is_some_and(|c| c.IsPlayPauseToggleEnabled().unwrap_or(false)
            || if playing { c.IsPauseEnabled().unwrap_or(false) } else { c.IsPlayEnabled().unwrap_or(false) }),
        can_next: controls.as_ref().is_some_and(|c| c.IsNextEnabled().unwrap_or(false)),
        can_previous: controls.as_ref().is_some_and(|c| c.IsPreviousEnabled().unwrap_or(false)),
        can_seek: duration_ms > 0 && controls.as_ref().is_some_and(|c| c.IsPlaybackPositionEnabled().unwrap_or(false)),
        at: Some(Instant::now()),
        ..Default::default()
    };
    if track.duration_ms == 0 && track.source_id.to_ascii_lowercase().contains("foobar2000") {
        if let Some(timing) = crate::foobar::timeline() {
            track.duration_ms = timing.duration_ms;
            track.position_ms = timing.position_ms;
            track.start_ticks = 0;
            track.reported_position = (timing.position_ms as i64).saturating_mul(10_000);
            track.updated_ticks = 0;
        }
    }
    Some(track)
}

fn timeline_position(start: i64, end: i64, position: i64, updated: i64, now: i64, playing: bool, rate: f64) -> (u64, u64) {
    let duration = end.saturating_sub(start).max(0);
    let mut position = position.saturating_sub(start).max(0) as u64 / 10_000;
    let age = now.saturating_sub(updated);
    // Empty, future or implausibly old timestamps must not send the bar straight to the end.
    if playing && updated > 0 && now >= updated && age <= duration {
        position = position.saturating_add((age as f64 / 10_000.0 * rate) as u64);
    }
    let duration = duration as u64 / 10_000;
    (duration, position.min(duration))
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

/// LRCLIB, most precise first: exact (title + artist + duration), field search, then free text.
/// Names are cleaned first: video titles carry "| Official MV", channel names "Official" etc.
fn fetch_lyrics(title: &str, artist: &str, duration_s: u64) -> Vec<(u64, String)> {
    let base = "https://lrclib.net/api";
    let (title, artist) = clean_names(title, artist);
    if title.is_empty() {
        return Vec::new();
    }
    let fields = format!("track_name={}&artist_name={}", url_encode(&title), url_encode(&artist));
    if duration_s > 0 && !artist.is_empty() {
        if let Some(body) = http_get(&format!("{base}/get?{fields}&duration={duration_s}")) {
            if let Some(lines) = JsonObject::Parse(&HSTRING::from(body)).ok().and_then(|o| synced(&o)) {
                return lines;
            }
        }
    }
    let mut searches = Vec::new();
    if !artist.is_empty() {
        searches.push(format!("{base}/search?{fields}"));
        searches.push(format!("{base}/search?q={}", url_encode(&format!("{artist} {title}"))));
    }
    searches.push(format!("{base}/search?q={}", url_encode(&title)));
    for url in searches {
        let Some(body) = http_get(&url) else { continue };
        let Ok(items) = JsonArray::Parse(&HSTRING::from(body)) else { continue };
        let n = items.Size().unwrap_or(0);
        let found = (0..n).filter_map(|i| items.GetObjectAt(i).ok()).filter_map(|o| {
            let lines = synced(&o)?;
            let length = o.GetNamedNumber(&HSTRING::from("duration")).unwrap_or(0.0);
            Some((lines, length))
        });
        if let Some(lines) = closest_length(found.collect(), duration_s) {
            return lines;
        }
    }
    Vec::new()
}

/// Clean player names for a lyrics lookup.
/// "SƠN TÙNG M-TP | LẠC TRÔI | OFFICIAL MUSIC VIDEO" by "Sơn Tùng M-TP Official" -> ("LẠC TRÔI", "Sơn Tùng M-TP").
pub fn clean_names(title: &str, artist: &str) -> (String, String) {
    let lower_has = |s: &str, words: &[&str]| { let l = s.to_lowercase(); words.iter().any(|w| l.contains(w)) };
    const NOISE: [&str; 14] = ["official", "music video", " mv", "mv ", "lyric", "audio", "video", "visualizer", "karaoke", "live", "remaster", "4k", "hd", "m/v"];
    // Drop bracketed parts: (Official MV), [Lyrics], 【...】
    let mut t = String::new();
    let mut depth = 0;
    for c in title.chars() {
        match c {
            '(' | '[' | '【' | '「' => depth += 1,
            ')' | ']' | '】' | '」' => depth = (depth - 1).max(0),
            _ if depth == 0 => t.push(c),
            _ => {}
        }
    }
    let mut artist = artist.trim().to_string();
    for suffix in [" - Topic", "VEVO", " Official", " official", " OFFICIAL"] {
        if let Some(stripped) = artist.strip_suffix(suffix) {
            artist = stripped.trim().to_string();
        }
    }
    // "A | B | Official MV": keep the parts that are not noise; the part equal to the artist goes.
    let parts: Vec<String> = t.split(['|', '｜', '•']).map(|p| p.trim().to_string()).filter(|p| !p.is_empty() && !lower_has(p, &NOISE)).collect();
    let mut t = match parts.as_slice() {
        [] => t.trim().to_string(),
        [one] => one.clone(),
        // A part that merely mentions the artist ("PHƯƠNG MỸ CHI x DTAP") is not the song either.
        many => {
            let artist_l = artist.to_lowercase();
            let names_artist = |p: &&String| !artist_l.is_empty() && p.to_lowercase().contains(&artist_l);
            many.iter().find(|p| !names_artist(p)).or_else(|| many.iter().find(|p| p.to_lowercase() != artist_l)).unwrap_or(&many[0]).clone()
        }
    };
    // Quotes around the song name: 'THIÊN ANH HÙNG CA'.
    let mut t = t.trim_matches(|c: char| matches!(c, '\'' | '"' | '‘' | '’' | '“' | '”')).to_string();
    // "Artist - Title": take the artist from the title when the channel is not a person's name.
    if let Some((a, rest)) = t.split_once(" - ") {
        if artist.is_empty() || a.to_lowercase().contains(&artist.to_lowercase()) || artist.to_lowercase().contains(&a.to_lowercase()) {
            artist = a.trim().to_string();
        }
        t = rest.trim().to_string();
    }
    // Featured artists confuse the lookup.
    for sep in [" ft.", " ft ", " feat.", " feat ", " x "] {
        if let Some(i) = t.to_lowercase().find(sep) {
            t.truncate(i);
        }
    }
    (t.trim().to_string(), artist)
}

/// A version of the song is wrong when its length is far from the playing one (extended mixes, loops):
/// its time stamps would run ahead or behind. Pick the closest length; none within this many seconds
/// means no lyrics. Unknown playing length: the first.
const LENGTH_TOLERANCE_S: f64 = 20.0;

fn closest_length(mut found: Vec<(Vec<(u64, String)>, f64)>, duration_s: u64) -> Option<Vec<(u64, String)>> {
    if duration_s == 0 {
        return found.into_iter().next().map(|(lines, _)| lines);
    }
    let gap = |length: f64| (length - duration_s as f64).abs();
    found.sort_by(|a, b| gap(a.1).total_cmp(&gap(b.1)));
    found.into_iter().next().filter(|(_, length)| gap(*length) <= LENGTH_TOLERANCE_S).map(|(lines, _)| lines)
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
    fn pill_stays_ten_seconds_after_pausing() {
        assert!(shows_pill(true, None), "playing");
        assert!(shows_pill(false, Some(Duration::from_secs(9))), "just paused: still there to resume");
        assert!(!shows_pill(false, Some(Duration::from_secs(10))), "paused 10 s: back to the clock");
        assert!(!shows_pill(false, None), "no player");
    }

    #[test]
    fn automatic_source_selection_prefers_playing_then_foreground() {
        let mut standard = Track::default();
        let mut native = Track::default();
        assert!(!prefer_itunes(Some(&standard), None, true));
        assert!(prefer_itunes(None, Some(&native), false));
        native.playing = true;
        assert!(prefer_itunes(Some(&standard), Some(&native), false));
        standard.playing = true;
        assert!(!prefer_itunes(Some(&standard), Some(&native), false));
        assert!(prefer_itunes(Some(&standard), Some(&native), true));
        native.playing = false;
        assert!(!prefer_itunes(Some(&standard), Some(&native), true));
    }

    #[test]
    #[ignore = "read-only live test of the complete source-selection worker"]
    fn live_selected_source() {
        let music = Music::start(HWND::default());
        let end = Instant::now() + Duration::from_secs(5);
        let mut result = None;
        while Instant::now() < end {
            if let Some(track) = music.snapshot() { result = Some(track); break; }
            std::thread::sleep(Duration::from_millis(50));
        }
        let track = result.expect("a supported player must be open with a current track");
        eprintln!("selected={} title={} playing={} time={}/{}ms", track.source_id, track.title, track.playing, track.position_ms(), track.duration_ms);
        assert!(!track.title.is_empty());
        assert!(track.position_ms() <= track.duration_ms);
    }

    #[test]
    fn timeline_uses_relative_position_and_valid_update_time() {
        let now = 1_340_000_000_000_000i64;
        assert_eq!(timeline_position(100_000_000, 700_000_000, 250_000_000, now - 20_000_000, now, true, 1.0), (60_000, 17_000));
        assert_eq!(timeline_position(0, 600_000_000, 150_000_000, 0, now, true, 1.0), (60_000, 15_000));
        assert_eq!(timeline_position(0, 600_000_000, 150_000_000, now + 10, now, true, 1.0), (60_000, 15_000));
        assert_eq!(timeline_position(0, 600_000_000, 150_000_000, now - 20_000_000, now, false, 1.0), (60_000, 15_000));
    }

    #[test]
    fn timeline_and_ui_follow_playback_rate_and_clamp() {
        assert_eq!(timeline_position(0, 600_000_000, 150_000_000, 100_000_000, 120_000_000, true, 2.0), (60_000, 19_000));
        let mut t = Track { playing: true, duration_ms: 60_000, position_ms: 15_000, rate: 2.0, ..Default::default() };
        assert_eq!(t.position_after(Duration::from_secs(2)), 19_000);
        assert_eq!(t.position_after(Duration::from_secs(100)), 60_000);
        t.playing = false;
        assert_eq!(t.position_after(Duration::from_secs(2)), 15_000);
        t.duration_ms = 0;
        assert_eq!(t.position_after(Duration::from_secs(2)), 0);
    }

    #[test]
    fn seek_time_includes_timeline_start_and_stays_in_bounds() {
        let t = Track { duration_ms: 60_000, start_ticks: 100_000_000, ..Default::default() };
        assert_eq!(t.seek_ticks(15_000), 250_000_000);
        assert_eq!(t.seek_ticks(u64::MAX), 700_000_000);
    }

    #[test]
    fn missing_update_time_does_not_restart_the_clock_each_poll() {
        let previous = Track { playing: false, rate: 1.0, duration_ms: 60_000,
            position_ms: 12_000, reported_position: 100_000_000, ..Default::default() };
        let mut next = Track { duration_ms: 60_000, reported_position: 100_000_000,
            position_ms: 10_000, ..Default::default() };
        next.inherit_cached(&previous);
        assert_eq!(next.position_ms(), 12_000, "pause must freeze the extrapolated position");
        next.playing = true;
        next.rate = 1.0;
        assert_eq!(next.position_after(Duration::from_secs(1)), 13_000);
    }

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
            eprintln!("{:?} title={:?} playing={:?} dur={:?} pos={:?} seek={:?}", s.SourceAppUserModelId().ok(), t.as_ref().map(|t| &t.title), t.as_ref().map(|t| t.playing), t.as_ref().map(|t| t.duration_ms), t.as_ref().map(|t| t.position_ms()), t.as_ref().map(|t| t.can_seek));
            if let Ok(tl) = s.GetTimelineProperties() {
                eprintln!("timeline start={:?} end={:?} min={:?} max={:?} pos={:?} updated={:?}",
                    tl.StartTime().map(|t| t.Duration), tl.EndTime().map(|t| t.Duration),
                    tl.MinSeekTime().map(|t| t.Duration), tl.MaxSeekTime().map(|t| t.Duration),
                    tl.Position().map(|t| t.Duration), tl.LastUpdatedTime().map(|t| t.UniversalTime));
            }
            if let Ok(info) = s.GetPlaybackInfo() {
                eprintln!("status={:?} rate={:?}", info.PlaybackStatus(), info.PlaybackRate().and_then(|r| r.Value()));
                if let Ok(c) = info.Controls() { eprintln!("controls play={:?} pause={:?} toggle={:?} prev={:?} next={:?} seek={:?}",
                    c.IsPlayEnabled(), c.IsPauseEnabled(), c.IsPlayPauseToggleEnabled(), c.IsPreviousEnabled(), c.IsNextEnabled(), c.IsPlaybackPositionEnabled()); }
            }
            let art = s.TryGetMediaPropertiesAsync().and_then(|op| op.get()).and_then(|p| p.Thumbnail())
                .and_then(|r| r.OpenReadAsync()).and_then(|op| op.get()).and_then(|s| s.Size());
            eprintln!("thumbnail bytes={art:?}");
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
    fn lyrics_version_is_the_one_closest_in_length() {
        let v = |name: &str, length: f64| (vec![(0, name.to_string())], length);
        let found = vec![v("extended", 890.0), v("album", 302.0), v("mv", 289.0)];
        assert_eq!(closest_length(found.clone(), 275).unwrap()[0].1, "mv");
        assert_eq!(closest_length(found.clone(), 301).unwrap()[0].1, "album");
        assert_eq!(closest_length(found.clone(), 0).unwrap()[0].1, "extended", "unknown length: first");
        assert!(closest_length(found, 120).is_none(), "nothing near 120 s");
    }

    #[test]
    fn names_are_cleaned() {
        assert_eq!(clean_names("SƠN TÙNG M-TP | LẠC TRÔI | OFFICIAL MUSIC VIDEO", "Sơn Tùng M-TP Official"), ("LẠC TRÔI".into(), "Sơn Tùng M-TP".into()));
        assert_eq!(clean_names("Đen - Mang Tiền Về Cho Mẹ ft. Nguyên Thảo (M/V)", "Đen Vâu Official"), ("Mang Tiền Về Cho Mẹ".into(), "Đen".into()));
        assert_eq!(clean_names("Shape of You [Official Video]", "Ed Sheeran"), ("Shape of You".into(), "Ed Sheeran".into()));
        assert_eq!(clean_names("Blinding Lights", "The Weeknd - Topic"), ("Blinding Lights".into(), "The Weeknd".into()));
        assert_eq!(clean_names("Lạc Trôi", "Sơn Tùng M-TP"), ("Lạc Trôi".into(), "Sơn Tùng M-TP".into()));
        // The part that only mentions the artist and a collaborator is not the song; quotes go.
        assert_eq!(clean_names("PHƯƠNG MỸ CHI x DTAP | 'THIÊN ANH HÙNG CA' | Official MV", "Phương Mỹ Chi"), ("THIÊN ANH HÙNG CA".into(), "Phương Mỹ Chi".into()));
    }

    /// Network: `cargo test lrclib_video_title -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn lrclib_video_title() {
        unsafe { let _ = RoInitialize(RO_INIT_MULTITHREADED); }
        let lines = fetch_lyrics("SƠN TÙNG M-TP | LẠC TRÔI | OFFICIAL MUSIC VIDEO", "Sơn Tùng M-TP Official", 0);
        assert!(lines.len() > 10, "got {} lines", lines.len());
    }

    #[test]
    fn encode() {
        assert_eq!(url_encode("Lạc Trôi & co"), "L%E1%BA%A1c%20Tr%C3%B4i%20%26%20co");
    }
}

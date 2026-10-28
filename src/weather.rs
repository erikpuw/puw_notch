//! Weather from Open-Meteo: free, no key, under 10,000 calls a day per IP for non-commercial use,
//! data under CC BY 4.0 (credited on the Weather page and in the README). The city is typed once
//! and looked up to coordinates (unaccented: the geocoder matches "Da Nang" and answers "Đà Nẵng");
//! the current conditions are fetched on a worker thread every REFRESH, and again when the network
//! comes back or the PC wakes.

use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use windows::core::HSTRING;
use windows::Data::Json::JsonObject;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

use crate::i18n::tr;
use crate::music::{http_get, url_encode};

/// Posted when the place or the conditions change.
pub const WM_WEATHER: u32 = WM_APP + 9;

/// Open-Meteo's data moves by the hour here (its 15-minute models cover North America and
/// Central Europe only): every half hour is fresh enough, ~48 calls a day.
const REFRESH: Duration = Duration::from_secs(30 * 60);
/// A network or wake-up refresh sooner than this after the last fetch is skipped.
const MIN_GAP: Duration = Duration::from_secs(2 * 60);
/// Try again after this when a fetch failed.
const RETRY: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, PartialEq)]
pub struct Place {
    /// As the geocoder spells it, with its marks ("Đà Nẵng").
    pub name: String,
    pub lat: f64,
    pub lon: f64,
}

/// How temperatures are shown. The data is always in °C.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Unit {
    C,
    F,
}

impl Unit {
    /// What the PC's region setting says: Fahrenheit where Windows counts in US units (the US and
    /// a few others), Celsius everywhere else.
    pub fn automatic() -> Unit {
        if crate::system::us_measurement() { Unit::F } else { Unit::C }
    }

    pub fn symbol(self) -> &'static str {
        match self {
            Unit::C => "°C",
            Unit::F => "°F",
        }
    }

    /// `celsius` as a whole number in this unit ("82"). Rounded before it is written, so -0.4 is
    /// "0", never "-0"; below zero it starts with a real minus sign (U+2212: as wide as a plus and
    /// level with the digits, which a hyphen is not).
    pub fn number(self, celsius: f32) -> String {
        let v = match self {
            Unit::C => celsius,
            Unit::F => celsius * 1.8 + 32.0,
        };
        let n = v.round() as i32;
        if n < 0 { format!("\u{2212}{}", -n) } else { n.to_string() }
    }

    /// The settings file's word for it.
    pub fn tag(self) -> &'static str {
        match self {
            Unit::C => "c",
            Unit::F => "f",
        }
    }

    pub fn from_tag(tag: &str) -> Option<Unit> {
        match tag.trim() {
            "c" => Some(Unit::C),
            "f" => Some(Unit::F),
            _ => None,
        }
    }
}

/// What the sky is doing, from the WMO weather code.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Sky {
    Clear,
    PartlyCloudy,
    Cloudy,
    Fog,
    Rain,
    Snow,
    Storm,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Now {
    /// °C.
    pub temp: f32,
    pub feels: f32,
    /// %.
    pub humidity: f32,
    /// km/h.
    pub wind: f32,
    pub sky: Sky,
    pub day: bool,
}

/// Conditions older than this (minutes) are shown as old: a failed fetch keeps the last ones.
pub const STALE_MIN: i64 = 90;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    pub place: Option<Place>,
    pub now: Option<Now>,
    /// When `now` was fetched, local yyyymmddHHMM.
    pub updated: Option<u64>,
    /// A lookup is running.
    pub looking: bool,
    /// The last city typed that the geocoder did not know.
    pub not_found: Option<String>,
}

impl State {
    /// The conditions are older than STALE_MIN at `now` (local yyyymmddHHMM).
    pub fn is_stale(&self, now: u64) -> bool {
        self.updated.is_some_and(|u| crate::todo::minutes(now) - crate::todo::minutes(u) > STALE_MIN)
    }
}

enum Job {
    Find(String),
    Refresh,
}

pub struct Weather {
    state: Arc<Mutex<State>>,
    jobs: Sender<Job>,
}

impl Weather {
    pub fn start(hwnd: HWND, place: Option<Place>) -> Self {
        let state = Arc::new(Mutex::new(State { place, ..State::default() }));
        let (jobs, rx) = channel();
        let (shared, target) = (state.clone(), hwnd.0 as isize);
        std::thread::spawn(move || {
            unsafe { let _ = RoInitialize(RO_INIT_MULTITHREADED); }
            let notify = || unsafe { let _ = PostMessageW(Some(HWND(target as _)), WM_WEATHER, WPARAM(0), LPARAM(0)); };
            let mut last: Option<Instant> = None;
            let mut job = Some(Job::Refresh);
            // How long to wait for the next job: a failed fetch (no network yet) tries again sooner.
            let mut wait = REFRESH;
            loop {
                match job.take() {
                    Some(Job::Find(city)) => {
                        let found = find_place(&city);
                        if let Ok(mut s) = shared.lock() {
                            s.looking = false;
                            match found.clone() {
                                Some(p) => (s.place, s.now, s.updated, s.not_found) = (Some(p), None, None, None),
                                None => s.not_found = Some(city),
                            }
                        }
                        notify();
                        if found.is_some() {
                            last = None;
                            job = Some(Job::Refresh);
                            continue;
                        }
                    }
                    Some(Job::Refresh) if last.is_none_or(|t| t.elapsed() >= MIN_GAP) => {
                        let place = shared.lock().ok().and_then(|s| s.place.clone());
                        wait = REFRESH;
                        if let Some(p) = place {
                            match current(&p) {
                                Some(now) => {
                                    last = Some(Instant::now());
                                    if let Ok(mut s) = shared.lock() {
                                        // The city may have been switched or switched off while this was fetched.
                                        if s.place.as_ref() == Some(&p) {
                                            (s.now, s.updated) = (Some(now), Some(crate::system::now_stamp()));
                                        }
                                    }
                                    notify();
                                }
                                None => wait = RETRY,
                            }
                        }
                    }
                    _ => {}
                }
                job = match rx.recv_timeout(wait) {
                    Ok(j) => Some(j),
                    Err(RecvTimeoutError::Timeout) => {
                        last = None;
                        Some(Job::Refresh)
                    }
                    Err(RecvTimeoutError::Disconnected) => break,
                };
            }
        });
        Weather { state, jobs }
    }

    /// Look `city` up (as typed: marks and all) and switch to it when found.
    pub fn find(&self, city: &str) {
        let city = city.trim();
        if city.is_empty() {
            return;
        }
        if let Ok(mut s) = self.state.lock() {
            (s.looking, s.not_found) = (true, None);
        }
        let _ = self.jobs.send(Job::Find(city.to_string()));
    }

    /// Switch the weather off: no city, nothing shown.
    pub fn clear(&self) {
        if let Ok(mut s) = self.state.lock() {
            *s = State::default();
        }
    }

    /// Fetch again now (network back, PC awake), unless it was just done.
    pub fn refresh(&self) {
        let _ = self.jobs.send(Job::Refresh);
    }

    pub fn snapshot(&self) -> State {
        self.state.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

/// Vietnamese letters without their marks: "Đà Nẵng" -> "Da Nang". Other text is kept.
pub fn strip_marks(s: &str) -> String {
    const GROUPS: [(&str, char); 12] = [
        ("àáạảãâầấậẩẫăằắặẳẵ", 'a'), ("ÀÁẠẢÃÂẦẤẬẨẪĂẰẮẶẲẴ", 'A'),
        ("èéẹẻẽêềếệểễ", 'e'), ("ÈÉẸẺẼÊỀẾỆỂỄ", 'E'),
        ("ìíịỉĩ", 'i'), ("ÌÍỊỈĨ", 'I'),
        ("òóọỏõôồốộổỗơờớợởỡ", 'o'), ("ÒÓỌỎÕÔỒỐỘỔỖƠỜỚỢỞỠ", 'O'),
        ("ùúụủũưừứựửữ", 'u'), ("ÙÚỤỦŨƯỪỨỰỬỮ", 'U'),
        ("ỳýỵỷỹ", 'y'), ("ỲÝỴỶỸ", 'Y'),
    ];
    s.chars()
        .map(|c| match c {
            'đ' => 'd',
            'Đ' | 'Ð' => 'D',
            _ => GROUPS.iter().find(|(set, _)| set.contains(c)).map_or(c, |&(_, plain)| plain),
        })
        .collect()
}

/// What to ask the geocoder for a typed city: unaccented, without "TP" / "Thành phố" / "Tỉnh" in
/// front or "Việt Nam" behind, and the usual short names spelled out (it does not know "HCM").
pub fn query_name(typed: &str) -> String {
    let plain = strip_marks(typed).replace(['.', ','], " ");
    let mut words: Vec<&str> = plain.split_whitespace().collect();
    let lower = |w: &str| w.to_lowercase();
    loop {
        match words.iter().map(|w| lower(w)).collect::<Vec<_>>().as_slice() {
            [a, b, ..] if a == "thanh" && b == "pho" => drop(words.drain(..2)),
            [a, ..] if a == "tp" || a == "tinh" => drop(words.remove(0)),
            [.., a, b] if a == "viet" && b == "nam" => drop(words.drain(words.len() - 2..)),
            [.., a] if a == "vietnam" => drop(words.pop()),
            _ => break,
        }
    }
    let name = words.join(" ");
    match name.to_lowercase().replace(' ', "").as_str() {
        "hcm" | "tphcm" | "hcmc" | "saigon" | "hochiminhcity" => "Ho Chi Minh".to_string(),
        "hn" => "Ha Noi".to_string(),
        _ => name,
    }
}

/// GET `url` and return its body. WinINet first: it builds certificate chains the way the rest of
/// Windows does (curl, .NET, the browsers), and Open-Meteo's API hosts, on Let's Encrypt's newest
/// roots, make WinHTTP (behind the WinRT HttpClient) fail with 0x80072F8F on a PC whose root list is
/// not up to date, measured on this one. Certificate checks stay fully on. The WinRT client is the
/// fallback (it follows the WinHTTP proxy setting, WinINet the user's).
fn fetch(url: &str) -> Option<String> {
    wininet_get(url).or_else(|| http_get(url))
}

fn wininet_get(url: &str) -> Option<String> {
    use std::ffi::c_void;
    use windows::core::{w, PCWSTR};
    use windows::Win32::Networking::WinInet::*;
    /// Biggest answer read (these are a few hundred bytes).
    const MAX: usize = 256 * 1024;
    /// A WinINet handle, closed when it goes out of scope.
    struct Handle(*mut c_void);
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe { let _ = InternetCloseHandle(self.0); }
        }
    }
    unsafe {
        let session = InternetOpenW(w!("PUW-Notch/1.0 (Windows)"), INTERNET_OPEN_TYPE_PRECONFIG.0, PCWSTR::null(), PCWSTR::null(), 0);
        if session.is_null() {
            return None;
        }
        let session = Handle(session);
        // 15 s to connect and to receive, so a dead network does not hold the worker for minutes.
        let timeout = 15_000u32.to_ne_bytes();
        for option in [INTERNET_OPTION_CONNECT_TIMEOUT, INTERNET_OPTION_RECEIVE_TIMEOUT] {
            let _ = InternetSetOptionW(Some(session.0), option, Some(timeout.as_ptr() as *const c_void), timeout.len() as u32);
        }
        let flags = INTERNET_FLAG_SECURE | INTERNET_FLAG_RELOAD | INTERNET_FLAG_NO_UI | INTERNET_FLAG_NO_COOKIES;
        let wide: Vec<u16> = url.encode_utf16().chain([0]).collect();
        let request = InternetOpenUrlW(session.0, PCWSTR(wide.as_ptr()), None, flags, None);
        if request.is_null() {
            return None;
        }
        let request = Handle(request);
        let (mut status, mut size) = (0u32, 4u32);
        HttpQueryInfoW(request.0, HTTP_QUERY_STATUS_CODE | HTTP_QUERY_FLAG_NUMBER, Some(&mut status as *mut u32 as *mut c_void), &mut size, None).ok()?;
        if status != 200 {
            return None;
        }
        let mut body = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            let mut read = 0u32;
            InternetReadFile(request.0, chunk.as_mut_ptr() as *mut c_void, chunk.len() as u32, &mut read).ok()?;
            if read == 0 || body.len() + read as usize > MAX {
                break;
            }
            body.extend_from_slice(&chunk[..read as usize]);
        }
        String::from_utf8(body).ok()
    }
}

/// The sky for a WMO weather code (Open-Meteo's `weather_code`).
pub fn sky(code: u32) -> Sky {
    match code {
        0 | 1 => Sky::Clear,
        2 => Sky::PartlyCloudy,
        3 => Sky::Cloudy,
        45 | 48 => Sky::Fog,
        71..=77 | 85 | 86 => Sky::Snow,
        95..=99 => Sky::Storm,
        _ if code >= 51 => Sky::Rain,
        _ => Sky::Cloudy,
    }
}

/// The sky in words.
pub fn describe(sky: Sky) -> &'static str {
    match sky {
        Sky::Clear => tr("Trời quang", "Clear"),
        Sky::PartlyCloudy => tr("Ít mây", "Partly cloudy"),
        Sky::Cloudy => tr("Nhiều mây", "Cloudy"),
        Sky::Fog => tr("Sương mù", "Fog"),
        Sky::Rain => tr("Có mưa", "Rain"),
        Sky::Snow => tr("Tuyết", "Snow"),
        Sky::Storm => tr("Giông sấm chớp", "Thunderstorm"),
    }
}

/// Colours of the big icon on the Weather page: (the cloud, moon or sun itself, what comes with
/// it: rays, drops, snow, the bolt).
pub fn tints(sky: Sky, day: bool) -> ((f32, f32, f32), (f32, f32, f32)) {
    const SUN: (f32, f32, f32) = (1.0, 0.8, 0.25);
    const MOON: (f32, f32, f32) = (0.78, 0.84, 1.0);
    const CLOUD: (f32, f32, f32) = (0.82, 0.85, 0.92);
    match sky {
        Sky::Clear if day => (SUN, SUN),
        Sky::Clear => (MOON, MOON),
        Sky::PartlyCloudy if day => (CLOUD, SUN),
        Sky::PartlyCloudy => (CLOUD, MOON),
        Sky::Cloudy => (CLOUD, CLOUD),
        Sky::Fog => (CLOUD, (0.62, 0.66, 0.74)),
        Sky::Rain => (CLOUD, (0.4, 0.7, 1.0)),
        Sky::Snow => (CLOUD, (0.88, 0.95, 1.0)),
        // A darker slate cloud under the bolt, not purple.
        Sky::Storm => ((0.58, 0.64, 0.76), (1.0, 0.85, 0.2)),
    }
}

/// The geocoder's best place for a typed city: one in Vietnam if there is any, else its first.
fn find_place(typed: &str) -> Option<Place> {
    let name = query_name(typed);
    if name.is_empty() {
        return None;
    }
    let url = format!("https://geocoding-api.open-meteo.com/v1/search?name={}&count=10&language=vi&format=json", url_encode(&name));
    let body = JsonObject::Parse(&HSTRING::from(fetch(&url)?)).ok()?;
    let results = body.GetNamedArray(&HSTRING::from("results")).ok()?;
    let places: Vec<(Place, bool)> = (0..results.Size().unwrap_or(0))
        .filter_map(|i| results.GetObjectAt(i).ok())
        .filter_map(|o| {
            let text = |k: &str| o.GetNamedString(&HSTRING::from(k)).ok().map(|s| s.to_string());
            let num = |k: &str| o.GetNamedNumber(&HSTRING::from(k)).ok();
            let place = Place { name: text("name")?, lat: num("latitude")?, lon: num("longitude")? };
            Some((place, text("country_code").as_deref() == Some("VN")))
        })
        .collect();
    let pick = places.iter().position(|(_, vn)| *vn).unwrap_or(0);
    places.into_iter().nth(pick).map(|(p, _)| p)
}

fn current(p: &Place) -> Option<Now> {
    let url = format!(
        "https://api.open-meteo.com/v1/forecast?latitude={:.4}&longitude={:.4}&current=temperature_2m,apparent_temperature,relative_humidity_2m,wind_speed_10m,weather_code,is_day&timezone=auto",
        p.lat, p.lon
    );
    let body = JsonObject::Parse(&HSTRING::from(fetch(&url)?)).ok()?;
    let c = body.GetNamedObject(&HSTRING::from("current")).ok()?;
    let num = |k: &str| c.GetNamedNumber(&HSTRING::from(k)).ok();
    Some(Now {
        temp: num("temperature_2m")? as f32,
        feels: num("apparent_temperature")? as f32,
        humidity: num("relative_humidity_2m")? as f32,
        wind: num("wind_speed_10m")? as f32,
        sky: sky(num("weather_code")? as u32),
        day: num("is_day")? != 0.0,
    })
}

/// The place as one settings line: "lat,lon,name".
pub fn place_line(p: &Place) -> String {
    format!("{:.5},{:.5},{}", p.lat, p.lon, p.name)
}

pub fn parse_place(line: &str) -> Option<Place> {
    let (lat, rest) = line.split_once(',')?;
    let (lon, name) = rest.split_once(',')?;
    let name = name.trim();
    (!name.is_empty()).then(|| Some(Place { name: name.to_string(), lat: lat.trim().parse().ok()?, lon: lon.trim().parse().ok()? }))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_come_off() {
        assert_eq!(strip_marks("Đà Nẵng"), "Da Nang");
        assert_eq!(strip_marks("Hồ Chí Minh"), "Ho Chi Minh");
        assert_eq!(strip_marks("Cần Thơ, Huế, Vũng Tàu, Biên Hòa"), "Can Tho, Hue, Vung Tau, Bien Hoa");
        assert_eq!(strip_marks("ăâđêôơư ẮẦẼỎỰỸ Ðà Lạt"), "aadeoou AAEOUY Da Lat");
        assert_eq!(strip_marks("Paris 75"), "Paris 75", "other text stays");
    }

    #[test]
    fn typed_cities_become_what_the_geocoder_knows() {
        assert_eq!(query_name("  Hà Nội "), "Ha Noi");
        assert_eq!(query_name("TP. Hồ Chí Minh"), "Ho Chi Minh");
        assert_eq!(query_name("Thành phố Đà Nẵng"), "Da Nang");
        assert_eq!(query_name("tỉnh Lâm Đồng"), "Lam Dong");
        assert_eq!(query_name("Hà Nội, Việt Nam"), "Ha Noi");
        assert_eq!(query_name("Hue Vietnam"), "Hue");
        for short in ["HCM", "TP HCM", "TPHCM", "Sài Gòn", "Saigon"] {
            assert_eq!(query_name(short), "Ho Chi Minh", "{short}");
        }
        assert_eq!(query_name("HN"), "Ha Noi");
        assert_eq!(query_name("Tokyo"), "Tokyo");
        assert_eq!(query_name("TP"), "");
    }

    #[test]
    fn weather_codes() {
        assert_eq!(sky(0), Sky::Clear);
        assert_eq!(sky(2), Sky::PartlyCloudy);
        assert_eq!(sky(3), Sky::Cloudy);
        assert_eq!(sky(48), Sky::Fog);
        for code in [51, 55, 61, 65, 67, 80, 82] {
            assert_eq!(sky(code), Sky::Rain, "{code}");
        }
        assert_eq!(sky(73), Sky::Snow);
        assert_eq!(sky(86), Sky::Snow);
        for code in [95, 96, 99] {
            assert_eq!(sky(code), Sky::Storm, "{code}");
        }
    }

    #[test]
    fn temperatures_in_both_units() {
        assert_eq!(Unit::C.number(28.4), "28");
        assert_eq!(Unit::C.number(28.5), "29");
        assert_eq!(Unit::F.number(0.0), "32");
        assert_eq!(Unit::F.number(100.0), "212");
        assert_eq!(Unit::F.number(28.0), "82");
        assert_eq!(Unit::F.number(-40.0), "\u{2212}40", "the two scales meet at -40");
        assert_eq!(Unit::C.number(-0.4), "0", "not -0");
        assert_eq!(Unit::C.number(-0.5), "\u{2212}1", "-0.5 rounds away from zero");
        assert_eq!(Unit::F.number(-17.8), "0", "-17.8 C is 0.04 F: not -0 either");
        assert_eq!(Unit::C.number(-3.2), "\u{2212}3");
        assert_eq!(Unit::C.number(-12.6), "\u{2212}13");
        assert_eq!(Unit::F.number(-89.0), "\u{2212}128", "the coldest places on Earth: three digits");
        assert_eq!(Unit::F.number(-89.2), "\u{2212}129", "-128.56 F rounds to -129");
        assert_eq!(Unit::F.number(54.0), "129", "and the hottest");
        assert!(!Unit::C.number(-3.2).contains('-'), "no hyphen-minus");
        assert_eq!((Unit::C.symbol(), Unit::F.symbol()), ("°C", "°F"));
        for u in [Unit::C, Unit::F] {
            assert_eq!(Unit::from_tag(u.tag()), Some(u));
        }
        assert_eq!(Unit::from_tag("k"), None);
    }

    #[test]
    fn old_conditions_are_stale_after_ninety_minutes() {
        let at = |h: u32, m: u32| crate::todo::stamp(20261029, h, m);
        let st = |updated| State { updated, ..State::default() };
        assert!(!st(Some(at(1, 0))).is_stale(at(2, 30)), "exactly 90 minutes is still fresh");
        assert!(st(Some(at(1, 0))).is_stale(at(2, 31)));
        assert!(!st(None).is_stale(at(23, 0)), "never fetched: not 'old', just absent");
        // Across midnight.
        assert!(st(Some(crate::todo::stamp(20261028, 23, 0))).is_stale(at(1, 0)));
        assert!(!st(Some(crate::todo::stamp(20261028, 23, 30))).is_stale(at(0, 40)));
    }

    #[test]
    fn every_sky_has_words_and_colours() {
        for sky in [Sky::Clear, Sky::PartlyCloudy, Sky::Cloudy, Sky::Fog, Sky::Rain, Sky::Snow, Sky::Storm] {
            assert!(!describe(sky).is_empty());
            for day in [true, false] {
                let (a, b) = tints(sky, day);
                assert!([a.0, a.1, a.2, b.0, b.1, b.2].iter().all(|c| (0.0..=1.0).contains(c)), "{sky:?}");
            }
        }
        assert_ne!(tints(Sky::Clear, true), tints(Sky::Clear, false), "sun by day, moon by night");
    }

    #[test]
    #[ignore = "uses the network: Open-Meteo's geocoder and forecast"]
    fn live_lookup_and_conditions() {
        unsafe { let _ = RoInitialize(RO_INIT_MULTITHREADED); }
        for (typed, expect) in [("Đà Nẵng", "Đà Nẵng"), ("ha noi", "Hà Nội"), ("TP. Hồ Chí Minh", "Hồ Chí Minh"), ("HCM", "Hồ Chí Minh"), ("Huế", "Huế")] {
            let p = find_place(typed).unwrap_or_else(|| panic!("{typed} not found"));
            assert!(p.name.contains(expect), "{typed} -> {}", p.name);
            assert!((8.0..24.0).contains(&p.lat) && (102.0..110.0).contains(&p.lon), "{typed} is in Vietnam: {p:?}");
        }
        assert!(find_place("zzzxqwv").is_none(), "nonsense is not found");
        let now = current(&find_place("Đà Nẵng").unwrap()).expect("conditions");
        assert!((-5.0..50.0).contains(&now.temp) && (0.0..=100.0).contains(&now.humidity), "{now:?}");
        println!("Da Nang now: {now:?}");
    }


    #[test]
    fn place_round_trips_through_settings() {
        let p = Place { name: "Đà Nẵng".into(), lat: 16.06778, lon: 108.22083 };
        assert_eq!(parse_place(&place_line(&p)), Some(p));
        assert_eq!(parse_place("1.5,2.5,Tên, có phẩy"), Some(Place { name: "Tên, có phẩy".into(), lat: 1.5, lon: 2.5 }));
        assert_eq!(parse_place("x,2,Hue"), None);
        assert_eq!(parse_place("1,2,"), None);
    }
}

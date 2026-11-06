//! The Calendar page's logic: the month as weeks that begin on Monday (ISO 8601, the same on every PC), the
//! weekday of a date, and the lunar date for the regions that keep a lunar calendar. No holidays: each country
//! has its own, and the page tells none.
//!
//! The lunar date is the astronomical method of Ho Ngoc Duc (new moons and the sun's longitude, in the time zone
//! of the region), the one Vietnamese lunar calendars follow.

use std::f64::consts::PI;

pub fn days_in_month(year: i32, month: u32) -> u32 {
    crate::todo::days_in_month(year as u32, month)
}

/// Julian day number of a Gregorian date.
fn jd(year: i32, month: u32, day: u32) -> i64 {
    let (y, m, d) = (year as i64, month as i64, day as i64);
    let a = (14 - m) / 12;
    let y = y + 4800 - a;
    let m = m + 12 * a - 3;
    d + (153 * m + 2) / 5 + 365 * y + y.div_euclid(4) - y.div_euclid(100) + y.div_euclid(400) - 32045
}

/// 0 = Monday ... 6 = Sunday.
pub fn weekday(year: i32, month: u32, day: u32) -> u32 {
    jd(year, month, day).rem_euclid(7) as u32
}

/// The column (0 = Monday) the 1st of the month falls in.
pub fn first_column(year: i32, month: u32) -> u32 {
    weekday(year, month, 1)
}

/// The month `delta` months from `(year, month)`.
pub fn shift_month(year: i32, month: u32, delta: i32) -> (i32, u32) {
    let n = year * 12 + month as i32 - 1 + delta;
    (n.div_euclid(12), n.rem_euclid(12) as u32 + 1)
}

/// What the page shows: the month, the day picked, today, and whether this region keeps a lunar calendar (`zone`,
/// then the choice between the two is on show) and the lunar one is chosen.
#[derive(Clone, Copy, Default, Debug)]
pub struct CalView {
    pub year: i32,
    pub month: u32,
    pub sel: u32,
    pub today: (i32, u32, u32),
    pub zone: Option<f64>,
    pub lunar_on: bool,
}

/// A lunar date: day, month, whether the month is the leap one.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Lunar {
    pub day: u32,
    pub month: u32,
    pub leap: bool,
}

fn new_moon(k: i64) -> f64 {
    let k = k as f64;
    let t = k / 1236.85;
    let (t2, t3) = (t * t, t * t * t);
    let dr = PI / 180.0;
    let s = |deg: f64| (deg * dr).sin();
    let mut jd1 = 2415020.75933 + 29.53058868 * k + 0.0001178 * t2 - 0.000000155 * t3;
    jd1 += 0.00033 * s(166.56 + 132.87 * t - 0.009173 * t2);
    let m = 359.2242 + 29.10535608 * k - 0.0000333 * t2 - 0.00000347 * t3;
    let mp = 306.0253 + 385.81691806 * k + 0.0107306 * t2 + 0.00001236 * t3;
    let f = 21.2964 + 390.67050646 * k - 0.0016528 * t2 - 0.00000239 * t3;
    let mut c = (0.1734 - 0.000393 * t) * s(m) + 0.0021 * s(2.0 * m);
    c -= 0.4068 * s(mp);
    c += 0.0161 * s(2.0 * mp);
    c -= 0.0004 * s(3.0 * mp);
    c += 0.0104 * s(2.0 * f);
    c -= 0.0051 * s(m + mp);
    c -= 0.0074 * s(m - mp);
    c += 0.0004 * s(2.0 * f + m);
    c -= 0.0004 * s(2.0 * f - m);
    c -= 0.0006 * s(2.0 * f + mp);
    c += 0.0010 * s(2.0 * f - mp);
    c += 0.0005 * s(2.0 * mp + m);
    let delta_t = if t < -11.0 {
        0.001 + 0.000839 * t + 0.0002261 * t2 - 0.00000845 * t3 - 0.000000081 * t * t3
    } else {
        -0.000278 + 0.000265 * t + 0.000262 * t2
    };
    jd1 + c - delta_t
}

fn new_moon_day(k: i64, zone: f64) -> i64 {
    (new_moon(k) + 0.5 + zone / 24.0).floor() as i64
}

/// The sun's longitude at day `jdn`, in 30-degree steps (0..=11).
fn sun_longitude(jdn: f64, zone: f64) -> i64 {
    let jdn = jdn - 0.5 - zone / 24.0;
    let t = (jdn - 2451545.0) / 36525.0;
    let t2 = t * t;
    let dr = PI / 180.0;
    let m = 357.52910 + 35999.05030 * t - 0.0001559 * t2 - 0.00000048 * t * t2;
    let l0 = 280.46645 + 36000.76983 * t + 0.0003032 * t2;
    let mut dl = (1.914600 - 0.004817 * t - 0.000014 * t2) * (dr * m).sin();
    dl += (0.019993 - 0.000101 * t) * (dr * 2.0 * m).sin() + 0.000290 * (dr * 3.0 * m).sin();
    let l = (l0 + dl) * dr;
    let l = l - PI * 2.0 * (l / (PI * 2.0)).floor();
    (l / PI * 6.0).floor() as i64
}

fn lunar_month_11(year: i32, zone: f64) -> i64 {
    let off = jd(year, 12, 31) - 2415021;
    let k = (off as f64 / 29.530588853).floor() as i64;
    let nm = new_moon_day(k, zone);
    if sun_longitude(nm as f64, zone) >= 9 { new_moon_day(k - 1, zone) } else { nm }
}

fn leap_month_offset(a11: i64, zone: f64) -> i64 {
    let k = ((a11 as f64 - 2415021.076998695) / 29.530588853 + 0.5).floor() as i64;
    let mut i = 1;
    let mut arc = sun_longitude(new_moon_day(k + i, zone) as f64, zone);
    loop {
        let last = arc;
        i += 1;
        arc = sun_longitude(new_moon_day(k + i, zone) as f64, zone);
        if arc == last || i >= 14 {
            break;
        }
    }
    i - 1
}

/// The lunar date of a Gregorian date, for a region `zone` hours from UTC (Vietnam 7, China 8, Korea 9).
pub fn lunar(year: i32, month: u32, day: u32, zone: f64) -> Lunar {
    let day_number = jd(year, month, day);
    let k = ((day_number as f64 - 2415021.076998695) / 29.530588853).floor() as i64;
    let mut month_start = new_moon_day(k + 1, zone);
    if month_start > day_number {
        month_start = new_moon_day(k, zone);
    }
    let mut a11 = lunar_month_11(year, zone);
    let mut b11 = a11;
    if a11 >= month_start {
        a11 = lunar_month_11(year - 1, zone);
    } else {
        b11 = lunar_month_11(year + 1, zone);
    }
    let diff = (month_start - a11) / 29;
    let mut leap = false;
    let mut m = diff + 11;
    if b11 - a11 > 365 {
        let leap_off = leap_month_offset(a11, zone);
        if diff >= leap_off {
            m = diff + 10;
            leap = diff == leap_off;
        }
    }
    if m > 12 {
        m -= 12;
    }
    Lunar { day: (day_number - month_start + 1) as u32, month: m as u32, leap }
}

/// The time zone (hours from UTC) of a region that keeps a lunar calendar, by its two-letter ISO code; None for
/// the regions that do not, which then never see the lunar choice.
pub fn lunar_zone(region: &str) -> Option<f64> {
    match region.to_ascii_uppercase().as_str() {
        "VN" => Some(7.0),
        "CN" | "HK" | "MO" | "TW" => Some(8.0),
        "KR" | "KP" => Some(9.0),
        _ => None,
    }
}

/// The two-letter region of this PC's Windows settings ("VN", "US"...), read when asked so a change in Windows
/// shows at once.
pub fn region() -> String {
    let mut buf = [0u16; 16];
    let n = unsafe { windows::Win32::Globalization::GetUserDefaultGeoName(&mut buf) };
    if n <= 1 { String::new() } else { String::from_utf16_lossy(&buf[..(n as usize - 1).min(buf.len())]) }
}

const LONG_VI: [&str; 7] = ["Thứ Hai", "Thứ Ba", "Thứ Tư", "Thứ Năm", "Thứ Sáu", "Thứ Bảy", "Chủ nhật"];
const LONG_EN: [&str; 7] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];
const SHORT_VI: [&str; 7] = ["T2", "T3", "T4", "T5", "T6", "T7", "CN"];
const SHORT_EN: [&str; 7] = ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"];
const MONTH_EN: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

/// Weekday names in the interface language, Monday first: long for the day shown, short for the column heads.
pub fn weekday_long(col: u32, english: bool) -> &'static str {
    (if english { LONG_EN } else { LONG_VI })[col as usize % 7]
}

pub fn weekday_short(col: u32, english: bool) -> &'static str {
    (if english { SHORT_EN } else { SHORT_VI })[col as usize % 7]
}

/// "Tháng 11, 2026" / "November 2026".
pub fn month_title(year: i32, month: u32, english: bool) -> String {
    if english { format!("{} {year}", MONTH_EN[(month as usize).saturating_sub(1) % 12]) } else { format!("Tháng {month}, {year}") }
}

/// The lunar months a Gregorian month touches: "Tháng 9 – 10 âm lịch" (a month has one or two of them).
pub fn lunar_title(year: i32, month: u32, zone: f64, english: bool) -> String {
    let (a, b) = (lunar(year, month, 1, zone), lunar(year, month, days_in_month(year, month), zone));
    let name = |l: Lunar| if l.leap { if english { format!("{} (leap)", l.month) } else { format!("{} nhuận", l.month) } } else { l.month.to_string() };
    let months = if (a.month, a.leap) == (b.month, b.leap) { name(a) } else { format!("{} – {}", name(a), name(b)) };
    if english { format!("Lunar month {months}") } else { format!("Tháng {months} âm lịch") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weeks_begin_on_monday() {
        assert_eq!(weekday(2026, 11, 7), 5, "Saturday");
        assert_eq!(first_column(2026, 11), 6, "Nov 1 2026 is a Sunday");
        assert_eq!(first_column(2024, 1), 0, "Jan 1 2024 is a Monday");
        assert_eq!(weekday(2000, 2, 29), 1, "a leap day, a Tuesday");
    }

    #[test]
    fn month_lengths_and_steps() {
        assert_eq!((days_in_month(2024, 2), days_in_month(2100, 2), days_in_month(2000, 2), days_in_month(2026, 11)), (29, 28, 29, 30));
        assert_eq!(shift_month(2026, 12, 1), (2027, 1));
        assert_eq!(shift_month(2026, 1, -1), (2025, 12));
        assert_eq!(shift_month(2026, 3, -15), (2024, 12));
    }

    #[test]
    fn lunar_new_year_is_the_first_of_the_first_month() {
        assert_eq!(lunar(2024, 2, 10, 7.0), Lunar { day: 1, month: 1, leap: false });
        assert_eq!(lunar(2025, 1, 29, 7.0), Lunar { day: 1, month: 1, leap: false });
        assert_eq!(lunar(2026, 2, 17, 7.0), Lunar { day: 1, month: 1, leap: false });
        assert_eq!(lunar(2026, 2, 16, 7.0).month, 12, "the eve is in the twelfth month");
    }

    #[test]
    fn lunar_dates_that_are_known() {
        // Mid-Autumn is the 15th of the 8th month: 17 Sep 2024, 6 Oct 2025.
        assert_eq!(lunar(2024, 9, 17, 7.0), Lunar { day: 15, month: 8, leap: false });
        assert_eq!(lunar(2025, 10, 6, 7.0), Lunar { day: 15, month: 8, leap: false });
        // 2025 has a leap 6th month, from 25 Jul.
        assert_eq!(lunar(2025, 7, 25, 7.0), Lunar { day: 1, month: 6, leap: true });
        // A lunar month ends and the next one starts on the 9th.
        let (a, b) = (lunar(2026, 11, 8, 7.0), lunar(2026, 11, 9, 7.0));
        assert_eq!((b.day, a.day > 28), (1, true));
    }

    #[test]
    fn regions_with_a_lunar_calendar() {
        assert_eq!(lunar_zone("vn"), Some(7.0));
        assert_eq!(lunar_zone("CN"), Some(8.0));
        assert_eq!(lunar_zone("KR"), Some(9.0));
        assert_eq!(lunar_zone("US"), None);
        assert_eq!(lunar_zone(""), None);
    }

    #[test]
    fn titles() {
        assert_eq!(month_title(2026, 11, false), "Tháng 11, 2026");
        assert_eq!(month_title(2026, 11, true), "November 2026");
        assert_eq!(lunar_title(2026, 11, 7.0, false), "Tháng 9 – 10 âm lịch");
        assert_eq!(weekday_long(5, false), "Thứ Bảy");
        assert_eq!(weekday_short(6, true), "Su");
    }
}

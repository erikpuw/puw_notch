//! Appointments typed on the notch: "Khám răng 28/10 14:30", "Họp mai 9h", "Gọi điện 16:00".
//! Date: dd/mm[/yyyy], "hôm nay", "mai" / "ngày mai"; time: 24 h as in to-dos (default 08:00).
//! Reminded 15 minutes before and at the time. Saved in %APPDATA%\PuwNotch\calendar.txt,
//! one per line: "@<yyyymmddHHMM>\t<text>". Appointments of past days are dropped.

use std::path::PathBuf;

use crate::todo::{next_day, parse_time, stamp, MAX_LEN};

/// Minutes before the time for the early reminder.
pub const SOON_MIN: u64 = 15;

#[derive(Clone, Debug, PartialEq)]
pub struct Appointment {
    pub text: String,
    /// yyyymmddHHMM
    pub at: u64,
    /// Early (15 min) and on-time reminders already shown (this run only).
    pub told_soon: bool,
    pub told_now: bool,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Ring {
    /// 15 minutes to go.
    Soon,
    /// It is time.
    Now,
}

#[derive(Default)]
pub struct Calendar {
    pub items: Vec<Appointment>,
    path: Option<PathBuf>,
}

fn days_in_month(year: u32, month: u32) -> u32 {
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        1..=12 => 31,
        _ => 0,
    }
}

/// Minutes since an arbitrary epoch, for differences between two stamps.
pub fn minutes(t: u64) -> i64 {
    let (date, hm) = ((t / 10_000) as u32, t % 10_000);
    let (y, m, d) = (date / 10_000, date / 100 % 100, date % 100);
    // days from civil (Howard Hinnant), good for any Gregorian date
    let (y, m) = if m <= 2 { (y as i64 - 1, m as i64 + 12) } else { (y as i64, m as i64) };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m - 3) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe;
    days * 1440 + (hm / 100) as i64 * 60 + (hm % 100) as i64
}

/// Read typed text into (text, yyyymmddHHMM) relative to `now`. None when nothing is left to say.
pub fn read(text: &str, now: u64) -> Option<(String, u64)> {
    let text: String = text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(MAX_LEN).collect();
    let today = (now / 10_000) as u32;
    let (year_now, _) = (today / 10_000, 0);
    let mut words: Vec<String> = text.split(' ').map(str::to_string).collect();
    let mut date: Option<(u32, bool)> = None; // (yyyymmdd, year given)

    // "ngày mai", "mai", "hôm nay"
    let lower: Vec<String> = words.iter().map(|w| w.to_lowercase()).collect();
    let mut drop = Vec::new();
    for (i, w) in lower.iter().enumerate() {
        match w.as_str() {
            "mai" => {
                date = Some((next_day(today), true));
                drop.push(i);
                if i > 0 && lower[i - 1] == "ngày" {
                    drop.push(i - 1);
                }
            }
            "nay" if i > 0 && lower[i - 1] == "hôm" => {
                date = Some((today, true));
                drop.extend([i - 1, i]);
            }
            _ => {}
        }
        if date.is_some() {
            break;
        }
    }
    // dd/mm or dd/mm/yyyy (also with '-')
    if date.is_none() {
        for (i, w) in words.iter().enumerate() {
            let t = w.trim_end_matches([',', '.']);
            let parts: Vec<&str> = t.split(['/', '-']).collect();
            if !(2..=3).contains(&parts.len()) || parts.iter().any(|p| p.is_empty() || !p.chars().all(|c| c.is_ascii_digit())) {
                continue;
            }
            let (d, m): (u32, u32) = (parts[0].parse().ok()?, parts[1].parse().ok()?);
            let (y, given) = match parts.get(2) {
                Some(y) => {
                    let y: u32 = y.parse().ok()?;
                    (if y < 100 { 2000 + y } else { y }, true)
                }
                None => (year_now, false),
            };
            if d == 0 || d > days_in_month(y, m) {
                continue;
            }
            date = Some((y * 10_000 + m * 100 + d, given));
            drop.push(i);
            break;
        }
    }
    drop.sort_unstable();
    for i in drop.into_iter().rev() {
        words.remove(i);
    }
    let rest = words.join(" ");
    let (h, m, rest) = parse_time(&rest).unwrap_or((8, 0, rest));
    let rest = rest.trim().to_string();
    if rest.is_empty() {
        return None;
    }
    let at = match date {
        Some((day, _)) if stamp(day, h, m) > now || day != today => {
            let at = stamp(day, h, m);
            // dd/mm already behind us this year: next year
            match date {
                Some((_, false)) if at <= now => stamp(day + 10_000, h, m),
                _ => at,
            }
        }
        Some((day, _)) => stamp(day, h, m),
        None => {
            let at = stamp(today, h, m);
            if at > now { at } else { stamp(next_day(today), h, m) }
        }
    };
    Some((rest, at))
}

impl Calendar {
    /// Load and drop past days; reminders already behind `now` are not replayed.
    pub fn load(now: u64) -> Self {
        let path = std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("PuwNotch").join("calendar.txt"));
        let text = path.as_ref().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
        let mut c = Calendar { items: parse(&text), path };
        for a in &mut c.items {
            a.told_now = a.at <= now;
            a.told_soon = minutes(a.at) - minutes(now) <= SOON_MIN as i64;
        }
        if c.new_day((now / 10_000) as u32) {
            c.save();
        }
        c
    }

    fn save(&self) {
        let Some(p) = &self.path else { return };
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(p, serialize(&self.items));
    }

    fn sort(&mut self) {
        self.items.sort_by_key(|a| a.at);
    }

    pub fn add(&mut self, text: &str, now: u64) -> bool {
        let Some((text, at)) = read(text, now) else { return false };
        // Set inside the 15-minute window: only the on-time reminder is left.
        let told_soon = minutes(at) - minutes(now) <= SOON_MIN as i64;
        self.items.push(Appointment { text, at, told_soon, told_now: false });
        self.sort();
        self.save();
        true
    }

    pub fn update(&mut self, i: usize, text: &str, now: u64) -> bool {
        let Some((text, at)) = read(text, now) else { return false };
        let Some(a) = self.items.get_mut(i) else { return false };
        if a.at != at {
            a.told_soon = minutes(at) - minutes(now) <= SOON_MIN as i64;
            a.told_now = false;
        }
        (a.text, a.at) = (text, at);
        self.sort();
        self.save();
        true
    }

    pub fn remove(&mut self, i: usize) {
        if i < self.items.len() {
            self.items.remove(i);
            self.save();
        }
    }

    /// What the edit box shows: the text with date and time back in.
    pub fn edit_text(&self, i: usize) -> String {
        let Some(a) = self.items.get(i) else { return String::new() };
        // yyyymmddHHMM
        let (d, mo, h, m) = (a.at / 10_000 % 100, a.at / 1_000_000 % 100, a.at / 100 % 100, a.at % 100);
        format!("{} {d:02}/{mo:02} {h:02}:{m:02}", a.text)
    }

    /// Drop appointments of days before `today`. Returns whether anything went.
    pub fn new_day(&mut self, today: u32) -> bool {
        let before = self.items.len();
        self.items.retain(|a| (a.at / 10_000) as u32 >= today);
        self.items.len() != before
    }

    /// Reminders to show now, earliest first.
    pub fn rings(&self, now: u64) -> Vec<(usize, Ring)> {
        let mut out = Vec::new();
        for (i, a) in self.items.iter().enumerate() {
            if !a.told_now && a.at <= now {
                out.push((i, Ring::Now));
            } else if !a.told_soon && minutes(a.at) - minutes(now) <= SOON_MIN as i64 {
                out.push((i, Ring::Soon));
            }
        }
        out
    }

    pub fn mark(&mut self, i: usize, ring: Ring) {
        if let Some(a) = self.items.get_mut(i) {
            match ring {
                Ring::Soon => a.told_soon = true,
                Ring::Now => (a.told_soon, a.told_now) = (true, true),
            }
        }
    }

    pub fn upcoming(&self, now: u64) -> usize {
        self.items.iter().filter(|a| a.at > now).count()
    }

    pub fn today_count(&self, today: u32) -> usize {
        self.items.iter().filter(|a| (a.at / 10_000) as u32 == today).count()
    }
}

fn parse(text: &str) -> Vec<Appointment> {
    let mut items: Vec<Appointment> = text
        .lines()
        .filter_map(|line| {
            let (head, body) = line.split_once('\t')?;
            let at = head.trim().strip_prefix('@')?.parse().ok()?;
            let body = body.trim();
            (!body.is_empty()).then(|| Appointment { text: body.to_string(), at, told_soon: false, told_now: false })
        })
        .collect();
    items.sort_by_key(|a| a.at);
    items
}

fn serialize(items: &[Appointment]) -> String {
    items.iter().map(|a| format!("@{}\t{}\n", a.at, a.text.replace(['\t', '\r', '\n'], " "))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sun 25/10/2026 14:30
    const NOW: u64 = 202610251430;

    #[test]
    fn reads_dates_and_times() {
        assert_eq!(read("Khám răng 28/10 14:30", NOW), Some(("Khám răng".into(), 202610281430)));
        assert_eq!(read("Họp mai 9h", NOW), Some(("Họp".into(), 202610260900)));
        assert_eq!(read("Họp ngày mai 9h", NOW), Some(("Họp".into(), 202610260900)));
        assert_eq!(read("Gọi điện 16:00", NOW), Some(("Gọi điện".into(), 202610251600)));
        assert_eq!(read("Gọi điện 9:00", NOW), Some(("Gọi điện".into(), 202610260900)), "passed today -> tomorrow");
        assert_eq!(read("Sinh nhật 5/1", NOW), Some(("Sinh nhật".into(), 202701050800)), "no time -> 08:00, past date -> next year");
        assert_eq!(read("Bảo vệ 3/11/2026 7h30", NOW), Some(("Bảo vệ".into(), 202611030730)));
        assert_eq!(read("Cuộc hẹn hôm nay 20:00", NOW), Some(("Cuộc hẹn".into(), 202610252000)));
        assert_eq!(read("Lạ 31/2 9:00", NOW).map(|r| r.1), Some(202610260900), "invalid date ignored");
        assert_eq!(read("  28/10 9:00 ", NOW), None, "needs some text");
    }

    #[test]
    fn minutes_across_days() {
        assert_eq!(minutes(202610260005) - minutes(202610252355), 10);
        assert_eq!(minutes(202701010000) - minutes(202612312359), 1);
    }

    #[test]
    fn rings_soon_then_now_once() {
        let mut c = Calendar::default();
        c.add("Họp 15:30", NOW);
        c.add("Gặp khách 14:40", NOW);
        assert_eq!(c.items[0].text, "Gặp khách", "sorted by time");
        assert!(c.items[0].told_soon, "set 10 min ahead: no early reminder");
        assert!(c.rings(NOW).is_empty());
        assert_eq!(c.rings(202610251440), vec![(0, Ring::Now)]);
        c.mark(0, Ring::Now);
        let r = c.rings(202610251515);
        assert_eq!(r, vec![(1, Ring::Soon)]);
        c.mark(1, Ring::Soon);
        assert!(c.rings(202610251516).iter().all(|(i, _)| *i != 1));
        assert_eq!(c.rings(202610251530), vec![(1, Ring::Now)]);
        c.mark(1, Ring::Now);
        assert!(c.rings(202610251600).is_empty());
    }

    #[test]
    fn edit_and_new_day() {
        let mut c = Calendar::default();
        c.add("Khám răng 28/10 14:30", NOW);
        assert_eq!(c.edit_text(0), "Khám răng 28/10 14:30");
        assert!(c.update(0, "Khám răng 29/10 9h", NOW));
        assert_eq!(c.items[0].at, 202610290900);
        c.add("Hôm qua 9:00", NOW);
        assert!(!c.new_day(20261026), "tomorrow 09:00 is not in the past");
        assert!(c.new_day(20261030), "after 29/10 everything before is dropped");
        assert!(c.items.is_empty());
    }

    #[test]
    fn file_round_trip() {
        let items = vec![Appointment { text: "a\tb".into(), at: 202610281430, told_soon: true, told_now: false }];
        let text = serialize(&items);
        assert_eq!(text, "@202610281430\ta b\n");
        assert_eq!(parse(&text)[0].at, 202610281430);
        assert!(parse("rác\n@x\tsai\n").is_empty());
    }
}

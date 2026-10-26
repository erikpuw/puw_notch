//! Tasks and appointments in one list, typed on the notch: "Mua sữa", "Họp nhóm 20:00",
//! "Khám răng 28/10 14:30", "Họp mai 9h". A time alone is today (tomorrow once it has passed);
//! a date (dd/mm[/yyyy], "hôm nay", "mai" / "ngày mai", "today", "tomorrow") without a time is
//! 08:00. Timed tasks ring `remind_before` minutes early (0 = only on time) and on time; open
//! tasks past their time stay, marked overdue, until ticked or deleted.
//! Saved in %APPDATA%\PuwNotch\todo.txt, one task per line: "-[ @<at>]\t<text>" open,
//! "x <yyyymmdd>[ @<at>]\t<text>" done that day (<at> = yyyymmddHHMM), plus "# remind <min>".
//! calendar.txt ("@<at>\t<text>") from the old Appointments page is folded in once.

use std::path::{Path, PathBuf};

/// Longest task text kept (characters).
pub const MAX_LEN: usize = 120;
/// Choices for the early reminder, minutes before the time (0 = off).
pub const REMIND_STEPS: [u32; 6] = [0, 5, 10, 15, 30, 60];
const REMIND_DEFAULT: u32 = 15;
/// A date typed without a time.
const DEFAULT_HOUR: u32 = 8;

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub text: String,
    /// Day it was ticked (yyyymmdd), None while open.
    pub done: Option<u32>,
    /// When, yyyymmddHHMM; None for a plain task.
    pub at: Option<u64>,
    /// Early and on-time reminders already shown (this run only; past ones are not replayed).
    pub told_soon: bool,
    pub told_now: bool,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Ring {
    /// `remind_before` minutes to go.
    Soon,
    /// It is time.
    Now,
}

pub struct Todo {
    pub items: Vec<Item>,
    /// Early reminder, one of `REMIND_STEPS`.
    pub remind_before: u32,
    path: Option<PathBuf>,
}

impl Default for Todo {
    fn default() -> Self {
        Todo { items: Vec::new(), remind_before: REMIND_DEFAULT, path: None }
    }
}

/// Local time as yyyymmddHHMM.
pub fn stamp(date: u32, hour: u32, minute: u32) -> u64 {
    date as u64 * 10_000 + hour as u64 * 100 + minute as u64
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

/// Day after a yyyymmdd date.
pub fn next_day(date: u32) -> u32 {
    let (y, m, d) = (date / 10_000, date / 100 % 100, date % 100);
    match (d < days_in_month(y, m), m < 12) {
        (true, _) => date + 1,
        (false, true) => y * 10_000 + (m + 1) * 100 + 1,
        (false, false) => (y + 1) * 10_000 + 101,
    }
}

/// Minutes since an arbitrary epoch of a yyyymmddHHMM stamp, for differences between two stamps.
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

/// Find a 24 h time in the text ("20:00", "8:05", "20h", "20h30", "8H"). Returns
/// (hour, minute, text without it). Only whole tokens count: "2024" or "abc20h" are left alone.
pub fn parse_time(text: &str) -> Option<(u32, u32, String)> {
    let words: Vec<&str> = text.split_whitespace().collect();
    for (i, w) in words.iter().enumerate() {
        let token = w.trim_end_matches([',', '.', ';', '!']);
        let parsed = if let Some((h, m)) = token.split_once(':') {
            (m.len() == 2).then(|| (h.parse::<u32>().ok(), m.parse::<u32>().ok())).and_then(|(h, m)| Some((h?, m?)))
        } else if let Some((h, m)) = token.split_once(['h', 'H']) {
            let minute = if m.is_empty() { Some(0) } else if m.len() == 2 { m.parse().ok() } else { None };
            h.parse::<u32>().ok().zip(minute)
        } else {
            None
        };
        let Some((h, m)) = parsed else { continue };
        if !(1..=2).contains(&token.split([':', 'h', 'H']).next().unwrap_or("").len()) || h > 23 || m > 59 {
            continue;
        }
        let rest: Vec<&str> = words.iter().enumerate().filter(|(j, _)| *j != i).map(|(_, w)| *w).collect();
        let mut rest = rest.join(" ");
        // "Họp lúc 20:00" -> "Họp"
        for tail in [" lúc", " vào", " at"] {
            if rest.to_lowercase().ends_with(tail) {
                rest.truncate(rest.len() - tail.len());
            }
        }
        return Some((h, m, rest.trim().to_string()));
    }
    None
}

/// Find a date in the words: "hôm nay", "mai" / "ngày mai", "today", "tomorrow", dd/mm[/yyyy]
/// (also with '-'). Returns (yyyymmdd, year given) and the words without it.
fn parse_date(words: &[&str], today: u32) -> Option<((u32, bool), Vec<String>)> {
    let lower: Vec<String> = words.iter().map(|w| w.to_lowercase()).collect();
    let rest = |drop: &[usize]| words.iter().enumerate().filter(|(i, _)| !drop.contains(i)).map(|(_, w)| w.to_string()).collect();
    for (i, w) in lower.iter().enumerate() {
        match w.as_str() {
            "mai" | "tomorrow" => {
                let drop = if i > 0 && lower[i - 1] == "ngày" { vec![i - 1, i] } else { vec![i] };
                return Some(((next_day(today), true), rest(&drop)));
            }
            "nay" if i > 0 && lower[i - 1] == "hôm" => return Some(((today, true), rest(&[i - 1, i]))),
            "today" => return Some(((today, true), rest(&[i]))),
            _ => {}
        }
    }
    for (i, w) in words.iter().enumerate() {
        let t = w.trim_end_matches([',', '.']);
        let parts: Vec<&str> = t.split(['/', '-']).collect();
        if !(2..=3).contains(&parts.len()) || parts.iter().any(|p| p.is_empty() || !p.chars().all(|c| c.is_ascii_digit())) {
            continue;
        }
        let (Ok(d), Ok(m)) = (parts[0].parse::<u32>(), parts[1].parse::<u32>()) else { continue };
        let (y, given) = match parts.get(2).map(|y| y.parse::<u32>()) {
            Some(Ok(y)) => (if y < 100 { 2000 + y } else { y }, true),
            Some(Err(_)) => continue,
            None => (today / 10_000, false),
        };
        if d == 0 || d > days_in_month(y, m) {
            continue;
        }
        return Some(((y * 10_000 + m * 100 + d, given), rest(&[i])));
    }
    None
}

/// Typed text -> (task text, when). Spaces squeezed, length capped; None when nothing is left.
/// No date and no time: a plain task. A time alone: today, or tomorrow once passed. A date:
/// that day (08:00 without a time); dd/mm already behind us this year means next year.
pub fn read(text: &str, now: u64) -> Option<(String, Option<u64>)> {
    let text: String = text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(MAX_LEN).collect();
    let today = (now / 10_000) as u32;
    let words: Vec<&str> = text.split(' ').filter(|w| !w.is_empty()).collect();
    let (date, rest) = match parse_date(&words, today) {
        Some((date, rest)) => (Some(date), rest.join(" ")),
        None => (None, text.clone()),
    };
    let time = parse_time(&rest);
    let (h, m, rest) = match (&time, date) {
        (Some((h, m, rest)), _) => (*h, *m, rest.clone()),
        (None, Some(_)) => (DEFAULT_HOUR, 0, rest),
        (None, None) => return (!text.is_empty()).then_some((text, None)),
    };
    let at = match date {
        Some((day, given)) => {
            let at = stamp(day, h, m);
            if !given && at <= now && day != today { stamp(day + 10_000, h, m) } else { at }
        }
        None => {
            let at = stamp(today, h, m);
            if at > now { at } else { stamp(next_day(today), h, m) }
        }
    };
    // A bare time ("20h") still needs a readable title.
    let rest = if rest.is_empty() && time.is_some() { format!("{h:02}:{m:02}") } else { rest };
    (!rest.is_empty()).then_some((rest, Some(at)))
}

impl Todo {
    /// Load todo.txt (folding in an old calendar.txt once) and drop tasks finished before today.
    /// Reminders already behind `now` are not replayed.
    pub fn load(now: u64) -> Self {
        let dir = std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("PuwNotch"));
        let mut t = dir.as_deref().map(Self::load_from).unwrap_or_default();
        t.settle_reminders(now);
        t.new_day((now / 10_000) as u32);
        t.save();
        t
    }

    fn load_from(dir: &Path) -> Self {
        let path = dir.join("todo.txt");
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let text = strip_bom(&text);
        let mut t = Todo { items: parse(text), remind_before: parse_remind(text), path: Some(path) };
        let old = dir.join("calendar.txt");
        if let Ok(cal) = std::fs::read_to_string(&old) {
            t.items.extend(parse_calendar(strip_bom(&cal)));
            let _ = std::fs::rename(&old, dir.join("calendar.txt.bak"));
        }
        t
    }

    fn save(&self) {
        let Some(p) = &self.path else { return };
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(p, format!("# remind {}\n{}", self.remind_before, serialize(&self.items)));
    }

    /// Reminders whose moment is already behind `now` count as shown.
    fn settle_reminders(&mut self, now: u64) {
        let before = self.remind_before as i64;
        for it in &mut self.items {
            if let Some(at) = it.at {
                it.told_now = at <= now;
                it.told_soon = minutes(at) - minutes(now) <= before;
            }
        }
    }

    /// Open first; among those today's (also overdue and plain tasks) before later days, by time.
    /// Equal keys keep their order, so a new plain task stays on top.
    fn sort(&mut self, today: u32) {
        self.items.sort_by_key(|it| {
            let later = it.at.is_some_and(|a| (a / 10_000) as u32 > today);
            (it.done.is_some(), later, it.at.unwrap_or(0))
        });
    }

    pub fn add(&mut self, text: &str, now: u64) -> bool {
        let Some((text, at)) = read(text, now) else { return false };
        // Set inside the early window: only the on-time reminder is left.
        let told_soon = at.is_some_and(|a| minutes(a) - minutes(now) <= self.remind_before as i64);
        self.items.insert(0, Item { text, done: None, at, told_soon, told_now: false });
        self.sort((now / 10_000) as u32);
        self.save();
        true
    }

    /// Replace a task's text and time (read the same way as `add`); done state is kept.
    /// Blank text leaves the task as it was.
    pub fn update(&mut self, i: usize, text: &str, now: u64) -> bool {
        let Some((text, at)) = read(text, now) else { return false };
        let before = self.remind_before as i64;
        let Some(it) = self.items.get_mut(i) else { return false };
        if it.at != at {
            it.told_soon = at.is_some_and(|a| minutes(a) - minutes(now) <= before);
            it.told_now = false;
        }
        (it.text, it.at) = (text, at);
        self.sort((now / 10_000) as u32);
        self.save();
        true
    }

    /// What the edit box shows: the text with its time (and date when not today) back in.
    pub fn edit_text(&self, i: usize, today: u32) -> String {
        let Some(it) = self.items.get(i) else { return String::new() };
        match it.at {
            Some(a) => {
                let (day, h, m) = ((a / 10_000) as u32, a / 100 % 100, a % 100);
                if day == today {
                    format!("{} {h:02}:{m:02}", it.text)
                } else {
                    format!("{} {:02}/{:02} {h:02}:{m:02}", it.text, day % 100, day / 100 % 100)
                }
            }
            None => it.text.clone(),
        }
    }

    /// Tick / untick. Done tasks sink below the open ones.
    pub fn toggle(&mut self, i: usize, today: u32) {
        let Some(it) = self.items.get_mut(i) else { return };
        it.done = if it.done.is_some() { None } else { Some(today) };
        self.sort(today);
        self.save();
    }

    pub fn remove(&mut self, i: usize) {
        if i < self.items.len() {
            self.items.remove(i);
            self.save();
        }
    }

    /// Forget tasks finished on an earlier day and re-sort (tomorrow's tasks become today's).
    /// Open tasks stay, even past their day. Returns whether anything went.
    pub fn new_day(&mut self, today: u32) -> bool {
        let before = self.items.len();
        self.items.retain(|it| it.done.is_none_or(|d| d >= today));
        self.sort(today);
        let gone = self.items.len() != before;
        if gone {
            self.save();
        }
        gone
    }

    /// Step the early reminder through `REMIND_STEPS` (-1 / +1).
    pub fn step_remind(&mut self, dir: i32) {
        let i = REMIND_STEPS.iter().position(|&m| m == self.remind_before).unwrap_or(3) as i32;
        self.remind_before = REMIND_STEPS[(i + dir).clamp(0, REMIND_STEPS.len() as i32 - 1) as usize];
        self.save();
    }

    /// (open for today: plain, overdue and today's; open on later days).
    pub fn counts(&self, today: u32) -> (usize, usize) {
        let open = self.items.iter().filter(|it| it.done.is_none());
        let later = open.clone().filter(|it| it.at.is_some_and(|a| (a / 10_000) as u32 > today)).count();
        (open.count() - later, later)
    }

    /// Reminders to show now: open timed tasks reaching their early or on-time moment.
    pub fn rings(&self, now: u64) -> Vec<(usize, Ring)> {
        let before = self.remind_before as i64;
        let mut out = Vec::new();
        for (i, it) in self.items.iter().enumerate() {
            let (Some(at), None) = (it.at, it.done) else { continue };
            if !it.told_now && at <= now {
                out.push((i, Ring::Now));
            } else if before > 0 && !it.told_soon && minutes(at) - minutes(now) <= before {
                out.push((i, Ring::Soon));
            }
        }
        out
    }

    pub fn mark(&mut self, i: usize, ring: Ring) {
        if let Some(it) = self.items.get_mut(i) {
            match ring {
                Ring::Soon => it.told_soon = true,
                Ring::Now => (it.told_soon, it.told_now) = (true, true),
            }
        }
    }
}

/// Files saved by Notepad or PowerShell can start with a byte-order mark.
fn strip_bom(text: &str) -> &str {
    text.strip_prefix('\u{feff}').unwrap_or(text)
}

fn parse_remind(text: &str) -> u32 {
    text.lines()
        .find_map(|l| l.strip_prefix("# remind ")?.trim().parse().ok())
        .filter(|m| REMIND_STEPS.contains(m))
        .unwrap_or(REMIND_DEFAULT)
}

fn parse(text: &str) -> Vec<Item> {
    text.lines()
        .filter_map(|line| {
            let (head, body) = line.split_once('\t')?;
            let mut parts = head.split_whitespace();
            let done = match parts.next()? {
                "-" => None,
                "x" => Some(parts.next()?.parse().ok()?),
                _ => return None,
            };
            let at = match parts.next() {
                Some(d) => Some(d.strip_prefix('@')?.parse().ok()?),
                None => None,
            };
            let body = body.trim();
            (!body.is_empty()).then(|| Item { text: body.to_string(), done, at, told_soon: false, told_now: false })
        })
        .collect()
}

/// Old Appointments page file: "@<at>\t<text>" per line, all open.
fn parse_calendar(text: &str) -> Vec<Item> {
    text.lines()
        .filter_map(|line| {
            let (head, body) = line.split_once('\t')?;
            let at = head.trim().strip_prefix('@')?.parse().ok()?;
            let body = body.trim();
            (!body.is_empty()).then(|| Item { text: body.to_string(), done: None, at: Some(at), told_soon: false, told_now: false })
        })
        .collect()
}

fn serialize(items: &[Item]) -> String {
    items
        .iter()
        .map(|it| {
            // Tabs and line breaks would break the format.
            let text = it.text.replace(['\t', '\r', '\n'], " ");
            let at = it.at.map(|d| format!(" @{d}")).unwrap_or_default();
            match it.done {
                None => format!("-{at}\t{text}\n"),
                Some(d) => format!("x {d}{at}\t{text}\n"),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: u32 = 20261025;
    /// Sun 25/10/2026 14:30
    const NOW: u64 = 202610251430;

    fn texts(t: &Todo) -> Vec<&str> {
        t.items.iter().map(|i| i.text.as_str()).collect()
    }

    #[test]
    fn reads_plain_timed_and_dated() {
        assert_eq!(read("  Mua   sữa ", NOW), Some(("Mua sữa".into(), None)), "no date, no time: plain task");
        assert_eq!(read("Họp nhóm 20:00", NOW), Some(("Họp nhóm".into(), Some(202610252000))));
        assert_eq!(read("Gửi báo cáo 8h30", NOW), Some(("Gửi báo cáo".into(), Some(202610260830))), "time passed -> tomorrow");
        assert_eq!(read("Khám răng 28/10 14:30", NOW), Some(("Khám răng".into(), Some(202610281430))));
        assert_eq!(read("Họp mai 9h", NOW), Some(("Họp".into(), Some(202610260900))));
        assert_eq!(read("Họp ngày mai 9h", NOW), Some(("Họp".into(), Some(202610260900))));
        assert_eq!(read("Sinh nhật 5/1", NOW), Some(("Sinh nhật".into(), Some(202701050800))), "date alone -> 08:00, past date -> next year");
        assert_eq!(read("Bảo vệ 3/11/2026 7h30", NOW), Some(("Bảo vệ".into(), Some(202611030730))));
        assert_eq!(read("Cuộc hẹn hôm nay 20:00", NOW), Some(("Cuộc hẹn".into(), Some(202610252000))));
        assert_eq!(read("Dentist tomorrow 9:30", NOW), Some(("Dentist".into(), Some(202610260930))));
        assert_eq!(read("Call today at 16:00", NOW), Some(("Call".into(), Some(202610251600))));
        assert_eq!(read("20h", NOW), Some(("20:00".into(), Some(202610252000))), "bare time keeps a title");
        assert_eq!(read("Lạ 31/2 9:00", NOW).map(|r| r.1), Some(Some(202610260900)), "invalid date is just text");
        assert_eq!(read("Báo cáo 2024", NOW), Some(("Báo cáo 2024".into(), None)));
        assert_eq!(read("   ", NOW), None);
        assert_eq!(read("mai", NOW), None, "a date with nothing to do");
        assert_eq!(read(&"a".repeat(500), NOW).unwrap().0.chars().count(), MAX_LEN);
    }

    #[test]
    fn times_24h() {
        assert_eq!(parse_time("Họp lúc 9:05."), Some((9, 5, "Họp".into())));
        assert_eq!(parse_time("Uống thuốc 0:00"), Some((0, 0, "Uống thuốc".into())));
        assert_eq!(parse_time("Nộp 25:00"), None);
        assert_eq!(parse_time("Xem 9:5"), None);
        assert_eq!(parse_time("Mua 3h30m sạc"), None);
        assert_eq!(next_day(20261031), 20261101);
        assert_eq!(next_day(20261231), 20270101);
        assert_eq!(next_day(20280228), 20280229);
        assert_eq!(minutes(202610260005) - minutes(202610252355), 10);
        assert_eq!(minutes(202701010000) - minutes(202612312359), 1);
    }

    #[test]
    fn one_list_today_first_then_later_days() {
        let mut t = Todo::default();
        t.add("Khám răng 28/10 14:30", NOW);
        t.add("Mua sữa", NOW);
        t.add("Họp 16:00", NOW);
        t.add("Gọi mẹ mai 9h", NOW);
        assert_eq!(texts(&t), ["Mua sữa", "Họp", "Gọi mẹ", "Khám răng"]);
        assert_eq!(t.counts(DAY), (2, 2), "today: plain + 16:00; later: tomorrow + 28/10");
        t.toggle(0, DAY);
        assert_eq!(texts(&t)[3], "Mua sữa", "done sinks");
        assert_eq!(t.counts(DAY), (1, 2));
        t.toggle(3, DAY);
        assert_eq!(t.items[0].done, None, "untick");
        t.remove(0);
        t.remove(9);
        assert_eq!(t.items.len(), 3);
    }

    #[test]
    fn rings_early_by_the_chosen_minutes_then_on_time() {
        let mut t = Todo::default();
        assert_eq!(t.remind_before, 15);
        t.add("Họp 15:30", NOW);
        t.add("Gặp khách 14:40", NOW);
        t.add("Mua sữa", NOW);
        let i = |t: &Todo, s: &str| t.items.iter().position(|x| x.text == s).unwrap();
        assert!(t.items[i(&t, "Gặp khách")].told_soon, "set 10 min ahead: no early reminder");
        assert!(t.rings(NOW).is_empty());
        assert_eq!(t.rings(202610251440), vec![(i(&t, "Gặp khách"), Ring::Now)]);
        let g = i(&t, "Gặp khách");
        t.mark(g, Ring::Now);
        assert_eq!(t.rings(202610251515), vec![(i(&t, "Họp"), Ring::Soon)]);
        let h = i(&t, "Họp");
        t.mark(h, Ring::Soon);
        assert!(t.rings(202610251516).is_empty());
        assert_eq!(t.rings(202610251530), vec![(h, Ring::Now)]);
        t.mark(h, Ring::Now);
        assert!(t.rings(202610251600).is_empty(), "each reminder once; plain tasks never ring");
        // 0 = only on time; done tasks never ring
        let mut t = Todo::default();
        t.remind_before = 0;
        t.add("Họp 15:30", NOW);
        assert!(t.rings(202610251525).is_empty());
        t.toggle(0, DAY);
        assert!(t.rings(202610251530).is_empty());
    }

    #[test]
    fn remind_steps_through_the_choices() {
        let mut t = Todo::default();
        t.step_remind(1);
        assert_eq!(t.remind_before, 30);
        t.step_remind(1);
        t.step_remind(1);
        assert_eq!(t.remind_before, 60, "stops at the last choice");
        for _ in 0..9 {
            t.step_remind(-1);
        }
        assert_eq!(t.remind_before, 0);
    }

    #[test]
    fn overdue_stays_done_goes_next_day() {
        let mut t = Todo::default();
        t.add("Nộp thuế 25/10 15:00", NOW);
        t.add("Đã xong", NOW);
        t.toggle(0, DAY);
        assert!(t.new_day(DAY + 1));
        assert_eq!(texts(&t), ["Nộp thuế"], "finished yesterday goes, overdue stays");
        assert!(!t.new_day(DAY + 1));
    }

    #[test]
    fn edit_text_puts_time_and_date_back() {
        let mut t = Todo::default();
        t.add("Họp nhóm 20:00", NOW);
        t.add("Khám răng 28/10 14:30", NOW);
        t.add("Mua sữa", NOW);
        assert_eq!(t.edit_text(0, DAY), "Mua sữa");
        assert_eq!(t.edit_text(1, DAY), "Họp nhóm 20:00", "today: time only");
        assert_eq!(t.edit_text(2, DAY), "Khám răng 28/10 14:30");
        assert!(t.update(1, "Họp cả công ty 29/10 9h", NOW));
        assert_eq!(t.items.iter().find(|x| x.text == "Họp cả công ty").unwrap().at, Some(202610290900));
        assert!(t.update(0, "Mua sữa tươi", NOW));
        assert!(!t.update(0, "   ", NOW), "blank keeps the task");
        assert!(!t.update(9, "x", NOW));
    }

    #[test]
    fn file_round_trip_and_old_calendar() {
        let items = vec![
            Item { text: "mở\tcó tab".into(), done: None, at: Some(202610252000), told_soon: true, told_now: false },
            Item { text: "xong".into(), done: Some(DAY), at: None, told_soon: false, told_now: false },
        ];
        let text = serialize(&items);
        assert_eq!(text, "- @202610252000\tmở có tab\nx 20261025\txong\n");
        let back = parse(&format!("# remind 30\n{text}"));
        assert_eq!((back[0].at, back[1].done), (Some(202610252000), Some(DAY)));
        assert_eq!(parse_remind("# remind 30\n"), 30);
        assert_eq!(parse_remind("# remind 7\n"), 15, "unknown step -> default");
        assert_eq!(parse_remind("-\tcũ\n"), 15);
        assert!(parse("rác\n\t\nx abc\tsai ngày\n- @xx\tsai giờ\n").is_empty(), "bad lines are skipped");
        assert_eq!(parse(strip_bom("\u{feff}-\tMua sữa\n")).len(), 1, "a BOM (Notepad, PowerShell) does not eat the first line");
        assert_eq!(parse_calendar(strip_bom("\u{feff}@202610281430\tKhám răng\n")).len(), 1);
        let cal = parse_calendar("@202610281430\tKhám răng\nrác\n");
        assert_eq!((cal.len(), cal[0].at, cal[0].done), (1, Some(202610281430), None));
    }

    #[test]
    fn folds_old_calendar_file_once() {
        let dir = std::env::temp_dir().join(format!("puw_todo_test_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("todo.txt"), "-\tMua sữa\n").unwrap();
        std::fs::write(dir.join("calendar.txt"), "@202610281430\tKhám răng\n").unwrap();
        let t = Todo::load_from(&dir);
        assert_eq!(texts(&t), ["Mua sữa", "Khám răng"]);
        assert!(!dir.join("calendar.txt").exists() && dir.join("calendar.txt.bak").exists(), "old file kept as .bak");
        t.save();
        assert_eq!(texts(&Todo::load_from(&dir)), ["Mua sữa", "Khám răng"], "not folded twice");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

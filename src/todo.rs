//! To-do list: add, tick done, delete, optional reminder time (24 h, written in the task text:
//! "Họp 20:00", "8h30 gửi báo cáo", "20h"). Saved as plain text in %APPDATA%\PuwNotch\todo.txt,
//! one task per line: "-[ @<due>]\t<text>" open, "x <yyyymmdd>[ @<due>]\t<text>" done that day,
//! where <due> is yyyymmddHHMM.

use std::path::PathBuf;

/// Longest task text kept (characters).
pub const MAX_LEN: usize = 120;

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub text: String,
    /// Day it was ticked (yyyymmdd), None while open.
    pub done: Option<u32>,
    /// Reminder as yyyymmddHHMM.
    pub due: Option<u64>,
    /// Reminder banner already shown (this run only; past reminders are not replayed at start).
    pub notified: bool,
}

#[derive(Default)]
pub struct Todo {
    pub items: Vec<Item>,
    path: Option<PathBuf>,
}

/// Local time as yyyymmddHHMM.
pub fn stamp(date: u32, hour: u32, minute: u32) -> u64 {
    date as u64 * 10_000 + hour as u64 * 100 + minute as u64
}

/// Day after a yyyymmdd date.
pub fn next_day(date: u32) -> u32 {
    let (y, m, d) = (date / 10_000, date / 100 % 100, date % 100);
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let days = match m {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    match (d < days, m < 12) {
        (true, _) => date + 1,
        (false, true) => y * 10_000 + (m + 1) * 100 + 1,
        (false, false) => (y + 1) * 10_000 + 101,
    }
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

impl Todo {
    /// Load from disk, drop tasks finished before `today`; reminders already past are not replayed.
    pub fn load(today: u32, now: u64) -> Self {
        let path = std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("PuwNotch").join("todo.txt"));
        let text = path.as_ref().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
        let mut t = Todo { items: parse(&text), path };
        for it in &mut t.items {
            it.notified = it.due.is_some_and(|d| d <= now);
        }
        if t.new_day(today) {
            t.save();
        }
        t
    }

    fn save(&self) {
        let Some(p) = &self.path else { return };
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(p, serialize(&self.items));
    }

    /// New open task on top. A 24 h time in the text becomes the reminder: today if it is still
    /// ahead of `now` (yyyymmddHHMM), otherwise tomorrow. Blank text is ignored.
    pub fn add(&mut self, text: &str, now: u64) -> bool {
        let Some((text, due)) = read_input(text, now) else { return false };
        self.items.insert(0, Item { text, done: None, due, notified: false });
        self.save();
        true
    }

    /// Replace a task's text and reminder (read the same way as `add`); done state is kept.
    /// Blank text leaves the task as it was.
    pub fn update(&mut self, i: usize, text: &str, now: u64) -> bool {
        let Some((text, due)) = read_input(text, now) else { return false };
        let Some(item) = self.items.get_mut(i) else { return false };
        item.notified = item.due == due && item.notified;
        (item.text, item.due) = (text, due);
        self.save();
        true
    }

    /// What the edit box shows for a task: its text with the reminder time back in.
    pub fn edit_text(&self, i: usize) -> String {
        let Some(it) = self.items.get(i) else { return String::new() };
        match it.due {
            Some(d) => format!("{} {:02}:{:02}", it.text, d / 100 % 100, d % 100),
            None => it.text.clone(),
        }
    }

    /// Tick / untick. Done tasks sink below the open ones (order otherwise kept).
    pub fn toggle(&mut self, i: usize, today: u32) {
        let Some(item) = self.items.get_mut(i) else { return };
        item.done = if item.done.is_some() { None } else { Some(today) };
        self.items.sort_by_key(|it| it.done.is_some());
        self.save();
    }

    pub fn remove(&mut self, i: usize) {
        if i < self.items.len() {
            self.items.remove(i);
            self.save();
        }
    }

    /// Forget tasks finished on an earlier day. Returns whether anything went.
    pub fn new_day(&mut self, today: u32) -> bool {
        let before = self.items.len();
        self.items.retain(|it| it.done.is_none_or(|d| d >= today));
        self.items.len() != before
    }

    pub fn open_count(&self) -> usize {
        self.items.iter().filter(|it| it.done.is_none()).count()
    }

    pub fn done_today(&self, today: u32) -> usize {
        self.items.iter().filter(|it| it.done == Some(today)).count()
    }

    /// Open tasks whose reminder time has come and that were not announced yet.
    pub fn due(&self, now: u64) -> Vec<usize> {
        (0..self.items.len()).filter(|&i| {
            let it = &self.items[i];
            it.done.is_none() && !it.notified && it.due.is_some_and(|d| d <= now)
        }).collect()
    }

    pub fn mark_notified(&mut self, i: usize) {
        if let Some(it) = self.items.get_mut(i) {
            it.notified = true;
        }
    }
}

/// Typed text -> (task text, reminder). Spaces squeezed, length capped; None when blank.
fn read_input(text: &str, now: u64) -> Option<(String, Option<u64>)> {
    let text: String = text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(MAX_LEN).collect();
    let (text, due) = match parse_time(&text) {
        Some((h, m, rest)) => {
            let today = (now / 10_000) as u32;
            let at = stamp(today, h, m);
            let due = if at > now { at } else { stamp(next_day(today), h, m) };
            // A bare time ("20:00") stays as the task text.
            (if rest.is_empty() { format!("{h:02}:{m:02}") } else { rest }, Some(due))
        }
        None => (text, None),
    };
    (!text.is_empty()).then_some((text, due))
}

fn parse(text: &str) -> Vec<Item> {
    let mut items: Vec<Item> = text
        .lines()
        .filter_map(|line| {
            let (head, body) = line.split_once('\t')?;
            let mut parts = head.split_whitespace();
            let done = match parts.next()? {
                "-" => None,
                "x" => Some(parts.next()?.parse().ok()?),
                _ => return None,
            };
            let due = match parts.next() {
                Some(d) => Some(d.strip_prefix('@')?.parse().ok()?),
                None => None,
            };
            let body = body.trim();
            (!body.is_empty()).then(|| Item { text: body.to_string(), done, due, notified: false })
        })
        .collect();
    items.sort_by_key(|it| it.done.is_some());
    items
}

fn serialize(items: &[Item]) -> String {
    items
        .iter()
        .map(|it| {
            // Tabs and line breaks would break the format.
            let text = it.text.replace(['\t', '\r', '\n'], " ");
            let due = it.due.map(|d| format!(" @{d}")).unwrap_or_default();
            match it.done {
                None => format!("-{due}\t{text}\n"),
                Some(d) => format!("x {d}{due}\t{text}\n"),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: u32 = 20261025;
    /// 25/10/2026 14:30
    const NOW: u64 = 202610251430;

    fn list(items: &[(&str, Option<u32>)]) -> Todo {
        Todo { items: items.iter().map(|(t, d)| Item { text: t.to_string(), done: *d, due: None, notified: false }).collect(), path: None }
    }

    fn texts(t: &Todo) -> Vec<&str> {
        t.items.iter().map(|i| i.text.as_str()).collect()
    }

    #[test]
    fn add_tick_remove() {
        let mut t = list(&[]);
        assert!(!t.add("   ", NOW));
        assert!(t.add("  Gửi   báo cáo  ", NOW));
        assert!(t.add("Họp nhóm", NOW));
        assert_eq!(texts(&t), ["Họp nhóm", "Gửi báo cáo"]);
        t.toggle(0, DAY);
        assert_eq!(texts(&t), ["Gửi báo cáo", "Họp nhóm"], "done sinks");
        assert_eq!((t.open_count(), t.done_today(DAY)), (1, 1));
        t.toggle(1, DAY);
        assert_eq!(t.items[1].done, None, "untick");
        t.remove(0);
        assert_eq!(texts(&t), ["Họp nhóm"]);
        t.remove(5);
        assert_eq!(t.items.len(), 1);
    }

    #[test]
    fn long_text_is_cut() {
        let mut t = list(&[]);
        t.add(&"a".repeat(500), NOW);
        assert_eq!(t.items[0].text.chars().count(), MAX_LEN);
    }

    #[test]
    fn next_day_clears_finished() {
        let mut t = list(&[("cũ xong", Some(DAY - 1)), ("hôm nay xong", Some(DAY)), ("còn mở", None)]);
        assert!(t.new_day(DAY));
        assert_eq!(texts(&t), ["hôm nay xong", "còn mở"]);
        assert!(!t.new_day(DAY));
    }

    #[test]
    fn times_24h() {
        assert_eq!(parse_time("Họp nhóm 20:00"), Some((20, 0, "Họp nhóm".into())));
        assert_eq!(parse_time("8h30 gửi báo cáo"), Some((8, 30, "gửi báo cáo".into())));
        assert_eq!(parse_time("Đi đá bóng 17h"), Some((17, 0, "Đi đá bóng".into())));
        assert_eq!(parse_time("Họp lúc 9:05."), Some((9, 5, "Họp".into())));
        assert_eq!(parse_time("Uống thuốc 0:00"), Some((0, 0, "Uống thuốc".into())));
        assert_eq!(parse_time("Nộp 25:00"), None, "not a 24 h time");
        assert_eq!(parse_time("Xem 9:5"), None);
        assert_eq!(parse_time("Báo cáo 2024"), None);
        assert_eq!(parse_time("Mua 3h30m sạc"), None);
        assert_eq!(parse_time("Mua sữa"), None);
    }

    #[test]
    fn reminder_today_or_tomorrow() {
        let mut t = list(&[]);
        t.add("Họp nhóm 20:00", NOW);
        assert_eq!((t.items[0].text.as_str(), t.items[0].due), ("Họp nhóm", Some(202610252000)));
        t.add("Gửi báo cáo 8h30", NOW);
        assert_eq!(t.items[0].due, Some(202610260830), "already past today -> tomorrow");
        t.add("20h", NOW);
        assert_eq!(t.items[0].text, "20:00", "bare time keeps a readable title");
        assert_eq!(next_day(20261031), 20261101);
        assert_eq!(next_day(20261231), 20270101);
        assert_eq!(next_day(20280228), 20280229);
    }

    #[test]
    fn due_once_and_not_when_done() {
        let mut t = list(&[]);
        t.add("Họp 20:00", NOW);
        t.add("Gọi mẹ 15:00", NOW);
        assert!(t.due(NOW).is_empty());
        let later = 202610252001;
        let due = t.due(later);
        assert_eq!(due.len(), 2);
        t.mark_notified(due[0]);
        assert_eq!(t.due(later).len(), 1, "announced once");
        let left = t.due(later)[0];
        t.toggle(left, DAY);
        assert!(t.due(later).is_empty(), "done tasks do not ring");
    }

    #[test]
    fn edit_text_and_time() {
        let mut t = list(&[]);
        t.add("Họp nhóm 20:00", NOW);
        assert_eq!(t.edit_text(0), "Họp nhóm 20:00", "the box gets the time back");
        assert!(t.update(0, "Họp cả công ty 21h", NOW));
        assert_eq!((t.items[0].text.as_str(), t.items[0].due), ("Họp cả công ty", Some(202610252100)));
        assert!(t.update(0, "Họp cả công ty", NOW));
        assert_eq!(t.items[0].due, None, "time removed -> no reminder");
        assert!(!t.update(0, "   ", NOW), "blank keeps the task");
        assert_eq!(t.items[0].text, "Họp cả công ty");
        t.toggle(0, DAY);
        t.update(0, "Đã họp", NOW);
        assert_eq!(t.items[0].done, Some(DAY), "editing keeps done");
        assert!(!t.update(9, "x", NOW));
    }

    #[test]
    fn file_round_trip() {
        let items = vec![
            Item { text: "mở\tcó tab".into(), done: None, due: Some(202610252000), notified: true },
            Item { text: "xong".into(), done: Some(DAY), due: None, notified: false },
        ];
        let text = serialize(&items);
        assert_eq!(text, "- @202610252000\tmở có tab\nx 20261025\txong\n");
        let back = parse(&text);
        assert_eq!(back[0].text, "mở có tab");
        assert_eq!(back[0].due, Some(202610252000));
        assert_eq!(back[1].done, Some(DAY));
        // lines written before reminders existed still load
        assert_eq!(parse("-\tcũ\nx 20261024\tcũ xong\n").len(), 2);
        assert!(parse("rác\n\t\nx abc\tsai ngày\n- @xx\tsai giờ\n").is_empty(), "bad lines are skipped");
    }
}

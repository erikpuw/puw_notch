//! To-do list: add, tick done, delete. Saved as plain text in %APPDATA%\PuwNotch\todo.txt,
//! one task per line: "-\t<text>" open, "x <yyyymmdd>\t<text>" done on that day.

use std::path::PathBuf;

/// Longest task text kept (characters).
pub const MAX_LEN: usize = 120;

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub text: String,
    /// Day it was ticked (yyyymmdd), None while open.
    pub done: Option<u32>,
}

#[derive(Default)]
pub struct Todo {
    pub items: Vec<Item>,
    path: Option<PathBuf>,
}

impl Todo {
    /// Load from disk and drop tasks finished before `today`.
    pub fn load(today: u32) -> Self {
        let path = std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("PuwNotch").join("todo.txt"));
        let text = path.as_ref().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
        let mut t = Todo { items: parse(&text), path };
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

    /// New open task on top. Blank text is ignored; returns whether something was added.
    pub fn add(&mut self, text: &str) -> bool {
        let text: String = text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(MAX_LEN).collect();
        if text.is_empty() {
            return false;
        }
        self.items.insert(0, Item { text, done: None });
        self.save();
        true
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
}

fn parse(text: &str) -> Vec<Item> {
    let mut items: Vec<Item> = text
        .lines()
        .filter_map(|line| {
            let (head, body) = line.split_once('\t')?;
            let done = match head.trim() {
                "-" => None,
                h => Some(h.strip_prefix("x ")?.trim().parse().ok()?),
            };
            let body = body.trim();
            (!body.is_empty()).then(|| Item { text: body.to_string(), done })
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
            match it.done {
                None => format!("-\t{text}\n"),
                Some(d) => format!("x {d}\t{text}\n"),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: u32 = 20261025;

    fn list(items: &[(&str, Option<u32>)]) -> Todo {
        Todo { items: items.iter().map(|(t, d)| Item { text: t.to_string(), done: *d }).collect(), path: None }
    }

    fn texts(t: &Todo) -> Vec<&str> {
        t.items.iter().map(|i| i.text.as_str()).collect()
    }

    #[test]
    fn add_tick_remove() {
        let mut t = list(&[]);
        assert!(!t.add("   "));
        assert!(t.add("  Gửi   báo cáo  "));
        assert!(t.add("Họp 8 giờ"));
        assert_eq!(texts(&t), ["Họp 8 giờ", "Gửi báo cáo"]);
        t.toggle(0, DAY);
        assert_eq!(texts(&t), ["Gửi báo cáo", "Họp 8 giờ"], "done sinks");
        assert_eq!((t.open_count(), t.done_today(DAY)), (1, 1));
        t.toggle(1, DAY);
        assert_eq!(t.items[1].done, None, "untick");
        t.remove(0);
        assert_eq!(texts(&t), ["Họp 8 giờ"]);
        t.remove(5);
        assert_eq!(t.items.len(), 1);
    }

    #[test]
    fn long_text_is_cut() {
        let mut t = list(&[]);
        t.add(&"a".repeat(500));
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
    fn file_round_trip() {
        let items = vec![Item { text: "mở\tcó tab".into(), done: None }, Item { text: "xong".into(), done: Some(DAY) }];
        let text = serialize(&items);
        assert_eq!(text, "-\tmở có tab\nx 20261025\txong\n");
        let back = parse(&text);
        assert_eq!(back[0].text, "mở có tab");
        assert_eq!(back[1].done, Some(DAY));
        assert!(parse("rác\n\t\nx abc\tsai ngày\n").is_empty(), "bad lines are skipped");
    }
}

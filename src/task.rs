use std::path::PathBuf;

use crate::calendar::{jd, shift_month};
use crate::todo::{days_in_month, next_day, stamp};

pub const SLOT_BOOT: u8 = 1;
pub const SLOT_NOON: u8 = 2;
pub const SLOT_FOUR: u8 = 4;
const ALL_SLOTS: u8 = SLOT_BOOT | SLOT_NOON | SLOT_FOUR;
pub const WINDOW_DAYS: i64 = 7;
pub const SNOOZE_STEPS: [u32; 7] = [10, 15, 30, 45, 60, 90, 120];
const SNOOZE_DEFAULT: u32 = 30;
const NEW_TASK_DAYS: u32 = 7;
const NEW_TASK_HOUR: u32 = 9;
const KEEP_DONE: usize = 12;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Repeat {
    Once,
    Weekly,
    Monthly,
    Quarterly,
}

impl Repeat {
    fn tag(self) -> &'static str {
        match self {
            Repeat::Once => "once",
            Repeat::Weekly => "week",
            Repeat::Monthly => "month",
            Repeat::Quarterly => "quarter",
        }
    }

    fn from_tag(tag: &str) -> Option<Self> {
        [Repeat::Once, Repeat::Weekly, Repeat::Monthly, Repeat::Quarterly].into_iter().find(|r| r.tag() == tag)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Task {
    pub title: String,
    pub repeat: Repeat,
    pub day: u32,
    pub due: u64,
    pub slots: u8,
    pub snooze: u32,
    pub done: Vec<u32>,
    pub nag_at: Option<u64>,
    pub told: u32,
    pub finished: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    Done,
    Waiting,
    Today,
    Late(u32),
}

pub fn date_of(t: u64) -> u32 {
    (t / 10_000) as u32
}

fn day_number(date: u32) -> i64 {
    jd((date / 10_000) as i32, date / 100 % 100, date % 100)
}

pub fn days_left(due: u64, now: u64) -> i64 {
    day_number(date_of(due)) - day_number(date_of(now))
}

fn days_after(date: u32, n: u32) -> u32 {
    (0..n).fold(date, |d, _| next_day(d))
}

fn later(t: u64, mins: u32) -> u64 {
    let (mut date, mut m) = (date_of(t), (t % 10_000 / 100 * 60 + t % 100) as u32 + mins);
    while m >= 24 * 60 {
        m -= 24 * 60;
        date = next_day(date);
    }
    stamp(date, m / 60, m % 60)
}

fn on_day(year: i32, month: u32, months: i32, day: u32) -> u32 {
    let (y, m) = shift_month(year, month, months);
    y as u32 * 10_000 + m * 100 + day.min(days_in_month(y as u32, m))
}

pub fn next_due(t: &Task) -> u64 {
    let date = date_of(t.due);
    let (y, m) = ((date / 10_000) as i32, date / 100 % 100);
    let next = match t.repeat {
        Repeat::Once => return t.due,
        Repeat::Weekly => days_after(date, 7),
        Repeat::Monthly => on_day(y, m, 1, t.day),
        Repeat::Quarterly => on_day(y, m, 3, t.day),
    };
    next as u64 * 10_000 + t.due % 10_000
}

fn clean(title: &str) -> String {
    title.replace(['\t', '\n', '\r'], " ").trim().to_string()
}

#[derive(Clone, Debug)]
pub struct Tasks {
    pub items: Vec<Task>,
    pub enabled: bool,
    pub fired_day: u32,
    pub fired: u8,
    path: Option<PathBuf>,
}

impl Default for Tasks {
    fn default() -> Self {
        Tasks { items: Vec::new(), enabled: true, fired_day: 0, fired: 0, path: None }
    }
}

impl Tasks {
    pub fn load(now: u64) -> Self {
        let path = std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("PuwNotch").join("tasks.txt"));
        let text = path.as_deref().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
        let mut t = parse(text.trim_start_matches('\u{feff}'));
        t.path = path;
        if t.fired_day != date_of(now) {
            (t.fired_day, t.fired) = (date_of(now), 0);
        }
        t
    }

    pub fn save(&self) {
        let Some(p) = &self.path else { return };
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(p, serialize(self));
    }

    pub fn add(&mut self, title: &str, now: u64) -> Option<usize> {
        let title = clean(title);
        if title.is_empty() {
            return None;
        }
        let date = days_after(date_of(now), NEW_TASK_DAYS);
        self.items.push(Task { title, repeat: Repeat::Once, day: date % 100, due: stamp(date, NEW_TASK_HOUR, 0), slots: ALL_SLOTS, snooze: SNOOZE_DEFAULT, done: Vec::new(), nag_at: None, told: 0, finished: false });
        self.save();
        Some(self.items.len() - 1)
    }

    pub fn remove(&mut self, i: usize) {
        if i < self.items.len() {
            self.items.remove(i);
            self.save();
        }
    }

    pub fn mark_done(&mut self, i: usize, _now: u64) {
        let Some(t) = self.items.get_mut(i) else { return };
        t.done.push(date_of(t.due));
        if t.done.len() > KEEP_DONE {
            t.done.remove(0);
        }
        (t.told, t.nag_at) = (0, None);
        match t.repeat {
            Repeat::Once => t.finished = true,
            _ => t.due = next_due(t),
        }
        self.save();
    }

    pub fn not_yet(&mut self, i: usize, now: u64) {
        let Some(t) = self.items.get_mut(i) else { return };
        t.nag_at = Some(later(now, t.snooze));
        self.save();
    }

    pub fn due_now(&mut self, now: u64, starting: bool) -> Vec<usize> {
        let today = date_of(now);
        if self.fired_day != today {
            (self.fired_day, self.fired) = (today, 0);
        }
        let hm = now % 10_000;
        let passed = [(SLOT_BOOT, starting), (SLOT_NOON, hm >= 1200), (SLOT_FOUR, hm >= 1600)];
        let fresh = passed.iter().filter(|(slot, on)| *on && self.fired & slot == 0).fold(0, |m, (slot, _)| m | slot);
        self.fired |= fresh;
        let picks: Vec<usize> = if self.enabled {
            (0..self.items.len())
                .filter(|&i| {
                    let t = &self.items[i];
                    !t.finished && days_left(t.due, now) <= WINDOW_DAYS && (t.slots & fresh != 0 || t.nag_at.is_some_and(|a| a <= now))
                })
                .collect()
        } else {
            Vec::new()
        };
        for &i in &picks {
            let t = &mut self.items[i];
            t.told += 1;
            t.nag_at = None;
        }
        if fresh != 0 || !picks.is_empty() {
            self.save();
        }
        picks
    }

    pub fn status(&self, i: usize, now: u64) -> Status {
        let t = &self.items[i];
        if t.finished {
            return Status::Done;
        }
        match days_left(t.due, now) {
            d if d < 0 => Status::Late((-d) as u32),
            0 => Status::Today,
            _ => Status::Waiting,
        }
    }

    pub fn order(&self, now: u64) -> Vec<usize> {
        let mut order: Vec<usize> = (0..self.items.len()).collect();
        order.sort_by_key(|&i| (self.status(i, now) == Status::Done, self.items[i].due));
        order
    }

    pub fn week(&self, monday: u32, now: u64) -> [Option<Status>; 7] {
        let rank = |s: Status| match s {
            Status::Done => 0,
            Status::Waiting => 1,
            Status::Today => 2,
            Status::Late(_) => 3,
        };
        let mut out = [None; 7];
        for (k, slot) in out.iter_mut().enumerate() {
            let date = days_after(monday, k as u32);
            for (i, t) in self.items.iter().enumerate() {
                let seen = if !t.finished && date_of(t.due) == date {
                    Some(self.status(i, now))
                } else if t.done.contains(&date) {
                    Some(Status::Done)
                } else {
                    None
                };
                if let Some(s) = seen {
                    if slot.is_none_or(|old| rank(s) > rank(old)) {
                        *slot = Some(s);
                    }
                }
            }
        }
        out
    }
}

fn parse(text: &str) -> Tasks {
    let mut t = Tasks::default();
    for line in text.lines() {
        if let Some(head) = line.strip_prefix("# ") {
            let w: Vec<&str> = head.split_whitespace().collect();
            if let ["on", on, "fired", day, mask] = w[..] {
                t.enabled = on != "0";
                t.fired_day = day.parse().unwrap_or(0);
                t.fired = mask.parse().unwrap_or(0);
            }
            continue;
        }
        if let Some(task) = parse_task(line) {
            t.items.push(task);
        }
    }
    t
}

fn parse_task(line: &str) -> Option<Task> {
    let f: Vec<&str> = line.split('\t').collect();
    let [title, repeat, day, due, slots, snooze, done, nag, told, finished] = f[..] else { return None };
    let title = clean(title);
    let done = done.split(',').filter(|d| !d.is_empty()).map(str::parse).collect::<Result<Vec<u32>, _>>().ok()?;
    let nag_at = if nag == "-" { None } else { Some(nag.parse().ok()?) };
    (!title.is_empty()).then_some(())?;
    Some(Task {
        title,
        repeat: Repeat::from_tag(repeat)?,
        day: day.parse().ok().filter(|d| (1..=31).contains(d))?,
        due: due.parse().ok()?,
        slots: slots.parse::<u8>().ok()? & ALL_SLOTS,
        snooze: snooze.parse().ok().filter(|n| SNOOZE_STEPS.contains(n))?,
        done,
        nag_at,
        told: told.parse().ok()?,
        finished: finished == "1",
    })
}

fn serialize(t: &Tasks) -> String {
    let mut out = format!("# on {} fired {} {}\n", t.enabled as u8, t.fired_day, t.fired);
    for it in &t.items {
        let done: Vec<String> = it.done.iter().map(u32::to_string).collect();
        let nag = it.nag_at.map_or("-".to_string(), |n| n.to_string());
        out += &format!("{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n", clean(&it.title), it.repeat.tag(), it.day, it.due, it.slots, it.snooze, done.join(","), nag, it.told, it.finished as u8);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(title: &str, repeat: Repeat, due: u64) -> Task {
        Task { title: title.into(), repeat, day: (due / 10_000 % 100) as u32, due, slots: SLOT_BOOT | SLOT_NOON | SLOT_FOUR, snooze: 30, done: Vec::new(), nag_at: None, told: 0, finished: false }
    }

    fn tasks_with(t: Task) -> Tasks {
        Tasks { items: vec![t], ..Tasks::default() }
    }

    #[test]
    fn monthly_keeps_the_chosen_day() {
        let mut t = task("Lương", Repeat::Monthly, 202601310900);
        t.day = 31;
        t.due = next_due(&t);
        assert_eq!(t.due, 202602280900);
        t.due = next_due(&t);
        assert_eq!(t.due, 202603310900);
    }

    #[test]
    fn quarterly_and_weekly_steps() {
        assert_eq!(next_due(&task("Thuế", Repeat::Quarterly, 202610300900)), 202701300900);
        assert_eq!(next_due(&task("Họp", Repeat::Weekly, 202611131100)), 202611201100);
    }

    #[test]
    fn window_opens_seven_days_before() {
        let t = tasks_with(task("Thuế", Repeat::Once, 202611201600));
        assert!(t.clone().due_now(202611121200, false).is_empty());
        assert_eq!(t.clone().due_now(202611131200, false), vec![0]);
    }

    #[test]
    fn boot_slot_only_on_start() {
        let mut t = tasks_with(task("Thuế", Repeat::Once, 202611151600));
        assert_eq!(t.due_now(202611130800, true), vec![0]);
        assert!(t.due_now(202611130900, false).is_empty());
        assert!(t.due_now(202611140001, false).is_empty(), "midnight while running is not a boot");
        assert_eq!(t.due_now(202611141200, false), vec![0]);
    }

    #[test]
    fn missed_slots_fire_once() {
        let mut t = tasks_with(task("Thuế", Repeat::Once, 202611151600));
        t.fired_day = 20261113;
        t.fired = SLOT_BOOT;
        assert_eq!(t.due_now(202611131700, false), vec![0]);
        assert!(t.due_now(202611131701, false).is_empty());
    }

    #[test]
    fn not_yet_nags_every_n_minutes_until_done() {
        let mut t = tasks_with(task("Thuế", Repeat::Once, 202611151600));
        t.fired_day = 20261113;
        t.fired = SLOT_BOOT | SLOT_NOON;
        t.not_yet(0, 202611131200);
        assert!(t.due_now(202611131229, false).is_empty());
        assert_eq!(t.due_now(202611131230, false), vec![0]);
        t.mark_done(0, 202611131231);
        assert!(t.due_now(202611131600, false).is_empty());
        assert_eq!(t.status(0, 202611131600), Status::Done);
    }

    #[test]
    fn done_on_a_late_recurring_task_moves_one_period() {
        let mut t = tasks_with(task("Thuế", Repeat::Quarterly, 202604300900));
        t.mark_done(0, 202611131000);
        assert_eq!(t.items[0].due, 202607300900);
        assert!(matches!(t.status(0, 202611131000), Status::Late(_)));
    }

    #[test]
    fn switch_off_silences_everything() {
        let mut t = tasks_with(task("Thuế", Repeat::Once, 202611131600));
        t.enabled = false;
        assert!(t.due_now(202611131600, true).is_empty());
    }

    #[test]
    fn round_trip_and_broken_line_is_skipped() {
        let t = tasks_with(task("Báo cáo\tthuế", Repeat::Monthly, 202611200900));
        assert_eq!(parse(&serialize(&t)).items[0].title, "Báo cáo thuế");
        assert_eq!(parse(&format!("{}rác\n", serialize(&t))).items.len(), 1);
    }

    #[test]
    fn new_task_defaults() {
        let mut t = Tasks::default();
        let i = t.add("Nộp BHXH", 202611131000).unwrap();
        let it = &t.items[i];
        assert_eq!((it.repeat, it.due, it.slots, it.snooze), (Repeat::Once, 202611200900, SLOT_BOOT | SLOT_NOON | SLOT_FOUR, 30));
        assert!(t.add("   ", 202611131000).is_none());
    }

    #[test]
    fn week_dots() {
        let mut t = tasks_with(task("Họp", Repeat::Once, 202611111000));
        t.items.push(task("Lương", Repeat::Once, 202611151000));
        let w = t.week(20261109, 202611131000);
        assert_eq!((w[2], w[6], w[0]), (Some(Status::Late(2)), Some(Status::Waiting), None));
    }
}

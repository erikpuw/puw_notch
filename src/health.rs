//! Water and "stand up" reminders. Times are minutes: of the day for the water window, and
//! `calendar::minutes` totals for clocks that span days. Settings and today's water are kept in
//! %APPDATA%\PuwNotch\health.txt as key=value lines.

use std::path::PathBuf;

/// Away from the keyboard and mouse this long = the sitting clock starts over.
pub const AWAY_SEC: u32 = 5 * 60;
/// "Để sau" on a water reminder.
pub const SNOOZE_MIN: i64 = 10;

/// A setting changed with the −/+ buttons (or a switch).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Setting {
    WaterOn,
    Cup,
    Goal,
    From,
    To,
    SitOn,
    SitEvery,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Health {
    pub water_on: bool,
    pub cup_ml: u32,
    pub goal_ml: u32,
    /// Reminder window, minutes of the day.
    pub from_min: u32,
    pub to_min: u32,
    /// Today's water.
    pub taken_ml: u32,
    pub day: u32,
    /// Last drink or reminder (total minutes): the next reminder counts from here.
    pub water_mark: Option<i64>,
    pub snooze_until: Option<i64>,
    pub goal_told: bool,
    pub sit_on: bool,
    pub sit_every_min: u32,
    /// Sitting since (total minutes); None while away.
    pub sitting_since: Option<i64>,
    path: Option<PathBuf>,
}

impl Default for Health {
    fn default() -> Self {
        Health {
            water_on: true,
            cup_ml: 250,
            goal_ml: 2000,
            from_min: 8 * 60,
            to_min: 17 * 60,
            taken_ml: 0,
            day: 0,
            water_mark: None,
            snooze_until: None,
            goal_told: false,
            sit_on: true,
            sit_every_min: 45,
            sitting_since: None,
            path: None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Ring {
    Drink,
    GoalReached,
    StandUp,
}

impl Health {
    pub fn load(today: u32) -> Self {
        let path = std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("PuwNotch").join("health.txt"));
        let text = path.as_ref().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
        let mut h = parse(&text);
        h.path = path;
        h.new_day(today);
        h
    }

    fn save(&self) {
        let Some(p) = &self.path else { return };
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(p, serialize(self));
    }

    /// New day: today's water starts from zero.
    pub fn new_day(&mut self, today: u32) {
        if self.day != today {
            self.day = today;
            self.taken_ml = 0;
            self.water_mark = None;
            self.snooze_until = None;
            self.goal_told = false;
            self.save();
        }
    }

    /// Minutes between water reminders: the window split by the cups still to drink.
    pub fn water_every(&self) -> i64 {
        let cups = self.goal_ml.div_ceil(self.cup_ml.max(1)).max(1) as i64;
        ((self.to_min.saturating_sub(self.from_min)) as i64 / cups).max(15)
    }

    /// When the next water reminder rings (total minutes), None when done or off for today.
    pub fn next_drink(&self, now: i64, minute_of_day: u32) -> Option<i64> {
        if !self.water_on || self.taken_ml >= self.goal_ml || minute_of_day >= self.to_min {
            return None;
        }
        let day_start = now - minute_of_day as i64;
        let from = day_start + self.from_min as i64;
        let base = self.water_mark.unwrap_or(from).max(from);
        let at = self.snooze_until.unwrap_or(base + self.water_every());
        Some(at.min(day_start + self.to_min as i64))
    }

    pub fn drink(&mut self, now: i64) {
        self.taken_ml += self.cup_ml;
        self.water_mark = Some(now);
        self.snooze_until = None;
        self.save();
    }

    pub fn snooze(&mut self, now: i64) {
        self.snooze_until = Some(now + SNOOZE_MIN);
    }

    /// Sitting clock. `idle_sec` = time since the last keyboard / mouse input.
    pub fn sitting_minutes(&self, now: i64) -> i64 {
        self.sitting_since.map_or(0, |s| (now - s).max(0))
    }

    pub fn stood_up(&mut self, now: i64) {
        self.sitting_since = Some(now);
    }

    /// Advance the clocks; reminders due now. `now` total minutes, `minute_of_day` local.
    pub fn tick(&mut self, now: i64, minute_of_day: u32, idle_sec: u32) -> Vec<Ring> {
        let mut out = Vec::new();
        if self.water_on && self.taken_ml >= self.goal_ml && !self.goal_told {
            self.goal_told = true;
            out.push(Ring::GoalReached);
        }
        if minute_of_day >= self.from_min && self.next_drink(now, minute_of_day).is_some_and(|at| now >= at) {
            self.water_mark = Some(now);
            self.snooze_until = None;
            out.push(Ring::Drink);
        }
        if !self.sit_on || idle_sec >= AWAY_SEC {
            self.sitting_since = None;
        } else {
            let since = *self.sitting_since.get_or_insert(now);
            if now - since >= self.sit_every_min as i64 {
                self.sitting_since = Some(now);
                out.push(Ring::StandUp);
            }
        }
        out
    }

    /// One −/+ step (`dir` = -1 / +1) or a switch flip (any `dir`).
    pub fn change(&mut self, s: Setting, dir: i32) {
        let step = |v: u32, by: u32, lo: u32, hi: u32| (v as i64 + dir as i64 * by as i64).clamp(lo as i64, hi as i64) as u32;
        match s {
            Setting::WaterOn => self.water_on = !self.water_on,
            Setting::Cup => self.cup_ml = step(self.cup_ml, 50, 100, 600),
            Setting::Goal => self.goal_ml = step(self.goal_ml, 250, 1000, 4000),
            Setting::From => self.from_min = step(self.from_min, 30, 5 * 60, self.to_min - 60),
            Setting::To => self.to_min = step(self.to_min, 30, self.from_min + 60, 23 * 60 + 30),
            Setting::SitOn => self.sit_on = !self.sit_on,
            Setting::SitEvery => self.sit_every_min = step(self.sit_every_min, 15, 15, 120),
        }
        if matches!(s, Setting::Goal | Setting::Cup) {
            self.goal_told = self.taken_ml >= self.goal_ml;
        }
        self.save();
    }
}

fn parse(text: &str) -> Health {
    let mut h = Health::default();
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else { continue };
        let num = |d: u32| v.trim().parse().unwrap_or(d);
        match k.trim() {
            "water_on" => h.water_on = v.trim() != "0",
            "cup" => h.cup_ml = num(h.cup_ml).clamp(100, 600),
            "goal" => h.goal_ml = num(h.goal_ml).clamp(1000, 4000),
            "from" => h.from_min = num(h.from_min).min(23 * 60),
            "to" => h.to_min = num(h.to_min).min(24 * 60),
            "taken" => h.taken_ml = num(0),
            "day" => h.day = num(0),
            "sit_on" => h.sit_on = v.trim() != "0",
            "sit_every" => h.sit_every_min = num(h.sit_every_min).clamp(15, 120),
            _ => {}
        }
    }
    if h.to_min <= h.from_min {
        (h.from_min, h.to_min) = (8 * 60, 17 * 60);
    }
    h.goal_told = h.taken_ml >= h.goal_ml;
    h
}

fn serialize(h: &Health) -> String {
    format!(
        "water_on={}\ncup={}\ngoal={}\nfrom={}\nto={}\ntaken={}\nday={}\nsit_on={}\nsit_every={}\n",
        h.water_on as u8, h.cup_ml, h.goal_ml, h.from_min, h.to_min, h.taken_ml, h.day, h.sit_on as u8, h.sit_every_min
    )
}

/// 480 -> "08:00"
pub fn hhmm(minute_of_day: u32) -> String {
    format!("{:02}:{:02}", minute_of_day / 60, minute_of_day % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Total minutes at midnight of some day; tests add the minute of the day.
    const DAY0: i64 = 1_000_000 * 1440;

    fn at(h: u32, m: u32) -> (i64, u32) {
        let mod_ = h * 60 + m;
        (DAY0 + mod_ as i64, mod_)
    }

    #[test]
    fn water_rings_on_schedule() {
        let mut h = Health::default();
        assert_eq!(h.water_every(), 67, "540 min / 8 cups");
        let (n, m) = at(7, 30);
        assert!(h.tick(n, m, 0).iter().all(|r| *r != Ring::Drink), "before the window");
        let (n, m) = at(9, 6);
        assert!(!h.tick(n, m, 0).contains(&Ring::Drink), "one minute early");
        let (n, m) = at(9, 7);
        assert!(h.tick(n, m, 0).contains(&Ring::Drink), "08:00 + 67 min");
        let (n, m) = at(9, 30);
        assert!(!h.tick(n, m, 0).contains(&Ring::Drink), "counted from the last reminder");
        h.drink(n);
        assert_eq!(h.taken_ml, 250);
        assert_eq!(h.next_drink(n, m), Some(n + 67));
        h.snooze(n);
        assert_eq!(h.next_drink(n, m), Some(n + SNOOZE_MIN));
        let (n, m) = at(17, 0);
        assert_eq!(h.next_drink(n, m), None, "window closed");
    }

    #[test]
    fn goal_reached_once_then_quiet() {
        let mut h = Health::default();
        let (n, m) = at(10, 0);
        for _ in 0..8 {
            h.drink(n);
        }
        let rings = h.tick(n, m, 0);
        assert!(rings.contains(&Ring::GoalReached));
        assert!(!rings.contains(&Ring::Drink));
        assert!(h.tick(n + 200, m + 200, 0).iter().all(|r| *r != Ring::GoalReached && *r != Ring::Drink));
        h.new_day(20261026);
        assert_eq!(h.taken_ml, 0);
    }

    #[test]
    fn stand_up_after_sitting_resets_when_away() {
        let mut h = Health { water_on: false, ..Health::default() };
        let (n, m) = at(10, 0);
        assert!(h.tick(n, m, 0).is_empty());
        assert!(!h.tick(n + 44, m + 44, 0).contains(&Ring::StandUp));
        assert!(h.tick(n + 45, m + 45, 0).contains(&Ring::StandUp));
        assert_eq!(h.sitting_minutes(n + 45), 0, "clock restarts after the reminder");
        h.tick(n + 60, m + 60, AWAY_SEC);
        assert_eq!(h.sitting_since, None, "away from the PC");
        h.tick(n + 61, m + 61, 0);
        assert!(!h.tick(n + 100, m + 100, 0).contains(&Ring::StandUp), "counted again from 61");
        h.change(Setting::SitOn, 1);
        assert!(h.tick(n + 200, m + 200, 0).is_empty(), "off");
    }

    #[test]
    fn settings_steps_and_limits() {
        let mut h = Health::default();
        h.change(Setting::Cup, 1);
        assert_eq!(h.cup_ml, 300);
        h.change(Setting::Goal, -1);
        assert_eq!(h.goal_ml, 1750);
        for _ in 0..20 {
            h.change(Setting::From, 1);
        }
        assert_eq!(h.from_min, h.to_min - 60, "window keeps an hour");
        h.change(Setting::SitEvery, -1);
        h.change(Setting::SitEvery, -1);
        h.change(Setting::SitEvery, -1);
        assert_eq!(h.sit_every_min, 15);
        h.change(Setting::WaterOn, 1);
        assert!(!h.water_on);
    }

    #[test]
    fn file_round_trip() {
        let mut h = Health::default();
        h.cup_ml = 300;
        h.taken_ml = 900;
        h.day = 20261025;
        h.sit_on = false;
        let back = parse(&serialize(&h));
        assert_eq!((back.cup_ml, back.taken_ml, back.day, back.sit_on), (300, 900, 20261025, false));
        // Nusa-style junk and bad values fall back to defaults
        let d = parse("bottle=500\ncup=99999\nfrom=900\nto=600\n");
        assert_eq!((d.cup_ml, d.from_min, d.to_min), (600, 480, 1020));
        assert_eq!(hhmm(495), "08:15");
    }
}

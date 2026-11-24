// Water: today's drinks against a daily goal, and whether a gentle nudge is due. Stored in %APPDATA%\notch\water.json.
use std::path::PathBuf;

use chrono::{DateTime, Local, NaiveDate, NaiveTime, Timelike};
use serde::{Deserialize, Serialize};

use crate::config::Water as Cfg;

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
pub struct Day {
    pub date: Option<NaiveDate>,
    /// (unix seconds, ml)
    pub drinks: Vec<(i64, u32)>,
    /// When the last nudge showed (unix seconds)
    #[serde(default)]
    pub nudged: Option<i64>,
}

impl Day {
    /// The same day, or a fresh one after midnight.
    pub fn today(self, today: NaiveDate) -> Self {
        if self.date == Some(today) { self } else { Day { date: Some(today), ..Default::default() } }
    }

    pub fn total(&self) -> u32 {
        self.drinks.iter().map(|d| d.1).sum()
    }

    pub fn drink(&mut self, at: DateTime<Local>, ml: u32) {
        self.drinks.push((at.timestamp(), ml));
    }

    pub fn undo(&mut self) {
        self.drinks.pop();
    }

    /// Millilitres drunk in each hour of the day.
    pub fn by_hour(&self) -> [u32; 24] {
        let mut h = [0; 24];
        for (t, ml) in &self.drinks {
            if let Some(at) = DateTime::from_timestamp(*t, 0) {
                h[at.with_timezone(&Local).hour() as usize] += ml;
            }
        }
        h
    }
}

fn hours(cfg: &Cfg) -> (NaiveTime, NaiveTime) {
    let t = |s: &str, d| NaiveTime::parse_from_str(s, "%H:%M").unwrap_or(d);
    (t(&cfg.start, NaiveTime::MIN), t(&cfg.end, NaiveTime::MIN))
}

/// What you'd have had by `now` drinking evenly through your hours.
pub fn pace(cfg: &Cfg, now: NaiveTime) -> u32 {
    let (start, end) = hours(cfg);
    if now <= start || end <= start {
        return 0;
    }
    let frac = ((now - start).num_seconds() as f64 / (end - start).num_seconds() as f64).min(1.0);
    (cfg.goal_ml as f64 * frac) as u32
}

/// A nudge is due inside your hours, when you're more than half a glass behind the pace and the last one was a
/// while ago.
pub fn nudge_due(cfg: &Cfg, day: &Day, now: DateTime<Local>) -> bool {
    let (start, end) = hours(cfg);
    let t = now.time();
    let gap_ok = day.nudged.is_none_or(|n| now.timestamp() - n >= cfg.remind_every as i64 * 60);
    cfg.enabled
        && cfg.remind_every > 0
        && t >= start
        && t < end
        && day.total() < cfg.goal_ml
        && pace(cfg, t) > day.total() + cfg.glass_ml / 2
        && gap_ok
}

fn file() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("APPDATA")?).join("notch").join("water.json"))
}

pub fn load(today: NaiveDate) -> Day {
    let day: Day = file().and_then(|f| std::fs::read_to_string(f).ok()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    day.today(today)
}

/// Temp file + rename, so a crash mid-write can't lose the day.
pub fn save(day: &Day) {
    let Some(f) = file() else { return };
    let _ = std::fs::create_dir_all(f.parent().unwrap());
    let tmp = f.with_extension("json.tmp");
    if let Ok(json) = serde_json::to_string(day)
        && std::fs::write(&tmp, json).is_ok()
    {
        let _ = std::fs::rename(tmp, f);
    }
}

/// "1.25" litres, without trailing zeros.
pub fn litres(ml: u32) -> String {
    let s = format!("{:.2}", ml as f64 / 1000.0);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn cfg() -> Cfg {
        Cfg { enabled: true, goal_ml: 2000, glass_ml: 250, start: "08:00".into(), end: "22:00".into(), remind_every: 60 }
    }

    fn at(h: u32, m: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 11, 24, h, m, 0).unwrap()
    }

    #[test]
    fn pace_through_the_day() {
        let t = |h, m| NaiveTime::from_hms_opt(h, m, 0).unwrap();
        assert_eq!(pace(&cfg(), t(7, 0)), 0);
        assert_eq!(pace(&cfg(), t(15, 0)), 1000, "half way through 08-22");
        assert_eq!(pace(&cfg(), t(23, 0)), 2000);
    }

    #[test]
    fn nudges() {
        let mut d = Day::default().today(at(15, 0).date_naive());
        assert!(nudge_due(&cfg(), &d, at(15, 0)), "behind at 15:00 with nothing drunk");
        assert!(!nudge_due(&cfg(), &d, at(7, 30)), "before your hours");
        d.nudged = Some(at(14, 30).timestamp());
        assert!(!nudge_due(&cfg(), &d, at(15, 0)), "nudged half an hour ago");
        d.nudged = None;
        for _ in 0..4 {
            d.drink(at(14, 0), 250);
        }
        assert!(!nudge_due(&cfg(), &d, at(15, 0)), "1 L by 15:00 is on pace");
        assert!(!nudge_due(&Cfg { remind_every: 0, ..cfg() }, &Day::default(), at(15, 0)), "nudges off");
    }

    #[test]
    fn day_rolls_over_and_counts_by_hour() {
        let mut d = Day::default().today(at(9, 0).date_naive());
        d.drink(at(9, 10), 250);
        d.drink(at(9, 50), 300);
        d.drink(at(13, 0), 250);
        assert_eq!(d.total(), 800);
        assert_eq!((d.by_hour()[9], d.by_hour()[13]), (550, 250));
        d.undo();
        assert_eq!(d.total(), 550);
        let next = d.clone().today(at(9, 0).date_naive().succ_opt().unwrap());
        assert_eq!(next.total(), 0, "a new day starts empty");
        assert_eq!(d.clone().today(at(9, 0).date_naive()), d, "the same day is kept");
    }

    #[test]
    fn litre_text() {
        assert_eq!(litres(1250), "1.25");
        assert_eq!(litres(1500), "1.5");
        assert_eq!(litres(2000), "2");
        assert_eq!(litres(0), "0");
    }
}

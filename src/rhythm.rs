// The daily rhythm: focus timer, prayer times, to-do, Jot. Logic and small files only; the window does the timing.
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chrono::{DateTime, Local, NaiveDate, NaiveTime, TimeZone, Utc};
use salah::prelude::{Configuration, Coordinates, Madhab, Prayer, PrayerSchedule, TimeAdjustment};
use serde::{Deserialize, Serialize};

use crate::config::{Focus as FocusCfg, Prayer as PrayerCfg};

// ---------- focus timer

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Phase {
    Focus,
    Short,
    Long,
}

#[derive(Clone, Debug)]
pub struct Focus {
    pub phase: Phase,
    /// 1-based focus round
    pub round: u32,
    ends: Option<Instant>,
    left: Duration,
    pub started: bool,
}

pub fn length(cfg: &FocusCfg, phase: Phase) -> Duration {
    let min = match phase {
        Phase::Focus => cfg.focus,
        Phase::Short => cfg.short_break,
        Phase::Long => cfg.long_break,
    };
    Duration::from_secs_f32(min.max(0.1) * 60.0)
}

impl Focus {
    pub fn new(cfg: &FocusCfg) -> Self {
        Self { phase: Phase::Focus, round: 1, ends: None, left: length(cfg, Phase::Focus), started: false }
    }

    pub fn running(&self) -> bool {
        self.ends.is_some()
    }

    pub fn remaining(&self, now: Instant) -> Duration {
        self.ends.map_or(self.left, |e| e.saturating_duration_since(now))
    }

    pub fn toggle(&mut self, now: Instant) {
        match self.ends.take() {
            Some(e) => self.left = e.saturating_duration_since(now),
            None => {
                self.ends = Some(now + self.left);
                self.started = true;
            }
        }
    }

    /// Next phase (time ran out, or skip), running. After the long break the cycle ends: back to the start, stopped.
    pub fn advance(&mut self, cfg: &FocusCfg, now: Instant) -> Option<Phase> {
        let next = match self.phase {
            Phase::Focus if self.round >= cfg.rounds.max(1) => Phase::Long,
            Phase::Focus => Phase::Short,
            Phase::Short => {
                self.round += 1;
                Phase::Focus
            }
            Phase::Long => {
                *self = Self::new(cfg);
                return None;
            }
        };
        self.phase = next;
        self.left = length(cfg, next);
        self.ends = Some(now + self.left);
        Some(next)
    }

    pub fn label(&self, cfg: &FocusCfg) -> String {
        match self.phase {
            Phase::Focus if !self.started => "Ready to focus".into(),
            Phase::Focus => format!("Focus · {} of {}", self.round, cfg.rounds.max(1)),
            Phase::Short => "Short break".into(),
            Phase::Long => "Long break".into(),
        }
    }
}

/// "18 min", or "45 s" in the last minute: for the closed pill, which redraws once a minute.
pub fn minutes_left(d: Duration) -> String {
    let s = d.as_secs();
    if s < 60 { format!("{s} s") } else { format!("{} min", s.div_ceil(60)) }
}

// ---------- prayer times

/// Today's five prayers (Fajr, Dhuhr, Asr, Maghrib, Isha) for a local date, or None when no location is set.
pub fn prayer_day(cfg: &PrayerCfg, date: NaiveDate) -> Option<[DateTime<Utc>; 5]> {
    if cfg.latitude == 0.0 && cfg.longitude == 0.0 {
        return None;
    }
    let madhab = if cfg.madhab == "hanafi" { Madhab::Hanafi } else { Madhab::Shafi };
    let mut params = Configuration::new(cfg.fajr_angle, cfg.isha_angle).madhab(madhab).done();
    let m = cfg.ihtiyat as i64;
    params.adjustments = TimeAdjustment::new(m, 0, m, m, m, m);
    let t = PrayerSchedule::new().on(date).for_location(Coordinates::new(cfg.latitude, cfg.longitude)).with_configuration(params).calculate().ok()?;
    Some([Prayer::Fajr, Prayer::Dhuhr, Prayer::Asr, Prayer::Maghrib, Prayer::Isha].map(|p| t.time(p)))
}

/// The next prayer after `now`: (index 0..5, time). Looks at today, then tomorrow.
pub fn next_prayer(cfg: &PrayerCfg, now: DateTime<Utc>) -> Option<(usize, DateTime<Utc>)> {
    let today = now.with_timezone(&Local).date_naive();
    for date in [today, today.succ_opt()?] {
        let times = prayer_day(cfg, date)?;
        if let Some(i) = times.iter().position(|t| *t > now) {
            return Some((i, times[i]));
        }
    }
    None
}

/// "1 h 8 min", "12 min", "now"
pub fn until(d: chrono::TimeDelta) -> String {
    let m = (d.num_seconds() + 59) / 60;
    match m {
        ..=0 => "now".into(),
        1..60 => format!("{m} min"),
        _ if m % 60 == 0 => format!("{} h", m / 60),
        _ => format!("{} h {} min", m / 60, m % 60),
    }
}

// ---------- to-do

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Task {
    pub text: String,
    /// Unix seconds
    #[serde(default)]
    pub due: Option<i64>,
    #[serde(default)]
    pub done: bool,
    #[serde(default)]
    pub reminded: bool,
}

fn data_dir() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("APPDATA")?).join("notch"))
}

pub fn load_tasks() -> Vec<Task> {
    data_dir().and_then(|d| std::fs::read_to_string(d.join("todo.json")).ok()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// Write to a temp file, then rename over the old one, so a crash mid-write can't lose the list.
pub fn save_tasks(tasks: &[Task]) {
    let Some(dir) = data_dir() else { return };
    let _ = std::fs::create_dir_all(&dir);
    let (tmp, file) = (dir.join("todo.json.tmp"), dir.join("todo.json"));
    if let Ok(json) = serde_json::to_string_pretty(tasks)
        && std::fs::write(&tmp, json).is_ok()
    {
        let _ = std::fs::rename(&tmp, &file);
    }
}

fn parse_clock(word: &str) -> Option<NaiveTime> {
    let (h, m) = word.split_once([':', '.'])?;
    if h.is_empty() || h.len() > 2 || m.len() != 2 {
        return None;
    }
    NaiveTime::from_hms_opt(h.parse().ok()?, m.parse().ok()?, 0)
}

/// "Reply to Dina 15:30" → ("Reply to Dina", 15:30). A time (H:MM, HH:MM or HH.MM) is taken from the start or the end.
pub fn split_time(s: &str) -> (String, Option<NaiveTime>) {
    let s = s.trim();
    if let Some((rest, last)) = s.rsplit_once(' ')
        && let Some(t) = parse_clock(last)
    {
        return (rest.trim().to_string(), Some(t));
    }
    if let Some((first, rest)) = s.split_once(' ')
        && let Some(t) = parse_clock(first)
    {
        return (rest.trim().to_string(), Some(t));
    }
    (s.to_string(), None)
}

/// The next time the clock shows `t`: later today, else tomorrow.
pub fn next_at(t: NaiveTime, now: DateTime<Local>) -> DateTime<Local> {
    let today = now.date_naive().and_time(t);
    let at = |d: chrono::NaiveDateTime| Local.from_local_datetime(&d).earliest().unwrap_or(now);
    let first = at(today);
    if first > now { first } else { at(today + chrono::Days::new(1)) }
}

// ---------- Jot

/// Appends "- 2026-11-23 14:02 the idea" to the jot file, creating it (and its folder) the first time.
pub fn jot(file: &Path, text: &str, now: DateTime<Local>) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(file)?;
    writeln!(f, "- {} {}", now.format("%Y-%m-%d %H:%M"), text.trim())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{FixedOffset, Timelike};

    fn focus_cfg() -> FocusCfg {
        FocusCfg { focus: 25.0, short_break: 5.0, long_break: 15.0, rounds: 2 }
    }

    #[test]
    fn focus_cycle() {
        let cfg = focus_cfg();
        let t0 = Instant::now();
        let mut f = Focus::new(&cfg);
        assert_eq!(f.label(&cfg), "Ready to focus");
        f.toggle(t0);
        assert!(f.running());
        assert_eq!(f.remaining(t0 + Duration::from_secs(60)), Duration::from_secs(24 * 60));
        assert_eq!(f.advance(&cfg, t0), Some(Phase::Short));
        assert_eq!(f.advance(&cfg, t0), Some(Phase::Focus));
        assert_eq!(f.label(&cfg), "Focus · 2 of 2");
        assert_eq!(f.advance(&cfg, t0), Some(Phase::Long), "after the last round");
        assert_eq!(f.advance(&cfg, t0), None, "cycle done");
        assert!(!f.running() && f.round == 1 && !f.started);
    }

    #[test]
    fn pause_keeps_time() {
        let cfg = focus_cfg();
        let t0 = Instant::now();
        let mut f = Focus::new(&cfg);
        f.toggle(t0);
        f.toggle(t0 + Duration::from_secs(300));
        assert!(!f.running());
        assert_eq!(f.remaining(t0 + Duration::from_secs(9999)), Duration::from_secs(20 * 60));
    }

    #[test]
    fn minute_texts() {
        assert_eq!(minutes_left(Duration::from_secs(45)), "45 s");
        assert_eq!(minutes_left(Duration::from_secs(61)), "2 min");
        assert_eq!(until(chrono::TimeDelta::minutes(68)), "1 h 8 min");
        assert_eq!(until(chrono::TimeDelta::minutes(120)), "2 h");
        assert_eq!(until(chrono::TimeDelta::seconds(30)), "1 min");
        assert_eq!(until(chrono::TimeDelta::seconds(-5)), "now");
    }

    fn jakarta(ihtiyat: i32) -> PrayerCfg {
        PrayerCfg {
            latitude: -6.1754,
            longitude: 106.8272,
            fajr_angle: 20.0,
            isha_angle: 18.0,
            ihtiyat,
            madhab: "shafi".into(),
            names: vec![],
            pause_music: false,
        }
    }

    /// Against api.aladhan.com method 20 (Kemenag angles, no ihtiyat) for central Jakarta, 23 Nov 2026, WIB.
    #[test]
    fn prayer_times_match_kemenag() {
        let wib = FixedOffset::east_opt(7 * 3600).unwrap();
        let times = prayer_day(&jakarta(0), NaiveDate::from_ymd_opt(2026, 11, 23).unwrap()).unwrap();
        let expect = [(4, 3), (11, 39), (15, 3), (17, 52), (19, 6)];
        for (t, (h, m)) in times.iter().zip(expect) {
            let local = t.with_timezone(&wib);
            let got = local.hour() as i32 * 60 + local.minute() as i32;
            assert!((got - (h * 60 + m)).abs() <= 2, "got {local}, want {h:02}:{m:02}");
        }
        let with = prayer_day(&jakarta(2), NaiveDate::from_ymd_opt(2026, 11, 23).unwrap()).unwrap();
        assert_eq!(with[2] - times[2], chrono::TimeDelta::minutes(2), "ihtiyat adds minutes");
    }

    #[test]
    fn no_location_no_prayers() {
        assert!(prayer_day(&PrayerCfg { latitude: 0.0, longitude: 0.0, ..jakarta(0) }, NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()).is_none());
    }

    #[test]
    fn next_prayer_rolls_to_tomorrow() {
        let cfg = jakarta(0);
        let late = Utc.with_ymd_and_hms(2026, 11, 23, 16, 0, 0).unwrap(); // 23:00 WIB, after Isha
        let (i, t) = next_prayer(&cfg, late).unwrap();
        assert_eq!(i, 0, "Fajr");
        assert!(t > late && t - late < chrono::TimeDelta::hours(8));
    }

    #[test]
    fn task_times() {
        let t = |h, m| NaiveTime::from_hms_opt(h, m, 0);
        assert_eq!(split_time("Reply to Dina 15:30"), ("Reply to Dina".into(), t(15, 30)));
        assert_eq!(split_time("9.05 standup"), ("standup".into(), t(9, 5)));
        assert_eq!(split_time("buy milk"), ("buy milk".into(), None));
        assert_eq!(split_time("read chapter 12"), ("read chapter 12".into(), None));
        assert_eq!(split_time("call at 25:00"), ("call at 25:00".into(), None), "not a real time");
    }

    #[test]
    fn next_at_rolls_over() {
        let now = Local.with_ymd_and_hms(2026, 11, 23, 16, 0, 0).unwrap();
        assert_eq!(next_at(NaiveTime::from_hms_opt(17, 0, 0).unwrap(), now).hour(), 17);
        assert_eq!(next_at(NaiveTime::from_hms_opt(9, 0, 0).unwrap(), now).date_naive(), now.date_naive().succ_opt().unwrap());
    }

    #[test]
    fn jot_appends_lines() {
        let dir = std::env::temp_dir().join(format!("notch-jot-test-{}", std::process::id()));
        let file = dir.join("sub").join("Jot.md");
        let now = Local.with_ymd_and_hms(2026, 11, 23, 14, 2, 0).unwrap();
        jot(&file, " an idea ", now).unwrap();
        jot(&file, "another", now).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "- 2026-11-23 14:02 an idea\n- 2026-11-23 14:02 another\n");
        let _ = std::fs::remove_dir_all(dir);
    }
}

// The resting eyes: when nothing is happening they take the clock's place, blink now and then, doze off when you've
// been away a while, and wake up when you're back. Pure timing here; drawing and the lid spring live in the app.
use std::time::{Duration, Instant};

/// How long the PC sits untouched before the eyes doze off.
pub const DOZE_AFTER: Duration = Duration::from_secs(5 * 60);
/// While dozing, how often to look whether you're back.
const WAKE_CHECK: Duration = Duration::from_secs(5);

#[derive(Debug, PartialEq)]
pub enum Event {
    Blink,
    Doze,
    /// You're back: open up with a little bounce
    Wake,
}

pub struct Eyes {
    after: Duration,
    quiet_since: Instant,
    pub dozing: bool,
    next_blink: Instant,
    seed: u32,
}

impl Eyes {
    pub fn new(after_secs: u32, now: Instant) -> Self {
        let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(7, |d| d.subsec_nanos()) | 1;
        let mut e = Self { after: Duration::from_secs(after_secs as u64), quiet_since: now, dozing: false, next_blink: now, seed };
        e.schedule_blink(now);
        e
    }

    pub fn set_after(&mut self, secs: u32) {
        self.after = Duration::from_secs(secs as u64);
    }

    /// Something happened on the notch (music, a timer, a pop-up, your pointer): the eyes step aside.
    pub fn poke(&mut self, now: Instant) {
        self.quiet_since = now;
        self.dozing = false;
    }

    /// Whether the eyes are out instead of the clock.
    pub fn showing(&self, now: Instant) -> bool {
        !self.after.is_zero() && now.duration_since(self.quiet_since) >= self.after
    }

    /// When they come out, from a quiet notch.
    pub fn due_in(&self, now: Instant) -> Option<Duration> {
        (!self.after.is_zero()).then(|| (self.quiet_since + self.after).saturating_duration_since(now))
    }

    /// 4 to 9 seconds between blinks, never in step with anything (a tiny LCG; no rand crate for this).
    fn schedule_blink(&mut self, now: Instant) {
        self.seed = self.seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        self.next_blink = now + Duration::from_millis(4000 + (self.seed >> 8) as u64 % 5000);
    }

    /// Called from the eyes' timer. `pc_idle` is how long since any keyboard or mouse input on the PC.
    pub fn tick(&mut self, now: Instant, pc_idle: Duration) -> Option<Event> {
        if !self.showing(now) {
            return None;
        }
        if self.dozing {
            if pc_idle < WAKE_CHECK {
                self.dozing = false;
                self.schedule_blink(now);
                return Some(Event::Wake);
            }
            return None;
        }
        if pc_idle >= DOZE_AFTER {
            self.dozing = true;
            return Some(Event::Doze);
        }
        if now >= self.next_blink {
            self.schedule_blink(now);
            return Some(Event::Blink);
        }
        None
    }

    /// When the timer should fire next, while the eyes are out.
    pub fn next_tick(&self, now: Instant) -> Duration {
        if self.dozing { WAKE_CHECK } else { self.next_blink.saturating_duration_since(now).max(Duration::from_millis(50)) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn come_out_after_a_quiet_while() {
        let t0 = Instant::now();
        let mut e = Eyes::new(60, t0);
        assert!(!e.showing(t0 + Duration::from_secs(59)));
        assert!(e.showing(t0 + Duration::from_secs(60)));
        e.poke(t0 + Duration::from_secs(61));
        assert!(!e.showing(t0 + Duration::from_secs(90)), "something happened");
        assert!(!Eyes::new(0, t0).showing(t0 + Duration::from_secs(9999)), "turned off");
    }

    #[test]
    fn blink_doze_wake() {
        let t0 = Instant::now();
        let mut e = Eyes::new(1, t0);
        let out = t0 + Duration::from_secs(2);
        let mut blinks = 0;
        let mut t = out;
        while t < out + Duration::from_secs(30) {
            if e.tick(t, Duration::ZERO) == Some(Event::Blink) {
                blinks += 1;
            }
            t += Duration::from_millis(100);
        }
        assert!((3..=8).contains(&blinks), "a blink every 4-9 s, got {blinks} in 30 s");
        assert_eq!(e.tick(t, DOZE_AFTER), Some(Event::Doze));
        assert_eq!(e.tick(t, DOZE_AFTER + Duration::from_secs(60)), None, "stays asleep");
        assert_eq!(e.next_tick(t), Duration::from_secs(5));
        assert_eq!(e.tick(t, Duration::from_secs(1)), Some(Event::Wake), "you're back");
        assert!(!e.dozing);
    }
}

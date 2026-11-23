// Pointer gestures: speed from recent samples, rubber-band resistance, and where a release should land.
use std::time::{Duration, Instant};

/// Tracks a value over the last 100 ms so a release can hand its speed to a spring.
#[derive(Default)]
pub struct Velocity {
    samples: Vec<(Instant, f32)>,
}

impl Velocity {
    pub fn reset(&mut self) {
        self.samples.clear();
    }

    pub fn push(&mut self, t: Instant, v: f32) {
        self.samples.retain(|(s, _)| t.duration_since(*s) <= Duration::from_millis(100));
        self.samples.push((t, v));
    }

    /// Units per second over the recent window; 0 if the pointer stopped before letting go.
    pub fn per_second(&self, now: Instant) -> f32 {
        match (self.samples.first(), self.samples.last()) {
            (Some(a), Some(b)) if now.duration_since(b.0) < Duration::from_millis(80) && b.0 > a.0 => (b.1 - a.1) / (b.0 - a.0).as_secs_f32(),
            _ => 0.0,
        }
    }
}

/// Resistance past an edge: grows with `d` but never beyond `limit`.
pub fn rubber(d: f32, limit: f32) -> f32 {
    d.signum() * limit * (1.0 - 1.0 / (d.abs() / limit + 1.0))
}

/// Open/closed after a drag on a 0 (closed) .. 1 (open) axis: project where the flick would coast to.
pub fn lands_open(progress: f32, velocity: f32) -> bool {
    progress + velocity * 0.12 > 0.5
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rubber_is_bounded_and_monotonic() {
        let mut last = 0.0;
        for i in 1..1000 {
            let r = rubber(i as f32 * 0.01, 0.1);
            assert!(r > last && r < 0.1);
            last = r;
        }
        assert_eq!(rubber(-0.5, 0.1), -rubber(0.5, 0.1));
        assert_eq!(rubber(0.0, 0.1), 0.0);
    }

    #[test]
    fn release_decisions() {
        assert!(!lands_open(0.2, 0.0), "small slow pull snaps back");
        assert!(lands_open(0.6, 0.0), "past halfway opens");
        assert!(lands_open(0.15, 4.0), "flick down opens");
        assert!(!lands_open(0.9, -5.0), "flick up closes");
    }

    #[test]
    fn velocity_from_recent_samples() {
        let t0 = Instant::now();
        let mut v = Velocity::default();
        v.push(t0, 0.0);
        v.push(t0 + Duration::from_millis(500), 10.0); // the old sample falls out of the window
        v.push(t0 + Duration::from_millis(550), 15.0);
        let s = v.per_second(t0 + Duration::from_millis(560));
        assert!((s - 100.0).abs() < 0.5, "{s}");
        assert_eq!(v.per_second(t0 + Duration::from_millis(800)), 0.0, "held still before release");
    }
}

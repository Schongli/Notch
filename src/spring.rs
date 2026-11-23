/// A damped spring that pulls `value` toward `target`. Time-based, so uneven frame timing doesn't change the motion.
#[derive(Clone, Copy, Debug)]
pub struct Spring {
    pub value: f32,
    pub velocity: f32,
    pub target: f32,
    pub stiffness: f32,
    pub damping: f32,
    /// How close counts as "there", in the value's own units. Every frame past it is invisible work.
    pub eps: f32,
}

impl Spring {
    /// `eps` defaults to a quarter DIP, right for sizes and positions; set it smaller for 0..1 values like opacity.
    pub fn new(value: f32, stiffness: f32, damping: f32) -> Self {
        Self { value, velocity: 0.0, target: value, stiffness, damping, eps: 0.25 }
    }

    pub fn step(&mut self, dt: f32) {
        // Small substeps keep semi-implicit Euler stable after a long frame (waking from sleep, a stalled thread).
        let dt = dt.clamp(0.0, 0.1);
        let n = (dt * 240.0).ceil().max(1.0) as u32;
        let h = dt / n as f32;
        for _ in 0..n {
            let force = -self.stiffness * (self.value - self.target) - self.damping * self.velocity;
            self.velocity += force * h;
            self.value += self.velocity * h;
        }
        if self.is_settled() {
            self.value = self.target;
            self.velocity = 0.0;
        }
    }

    pub fn is_settled(&self) -> bool {
        (self.value - self.target).abs() < self.eps && self.velocity.abs() < self.eps * 8.0
    }
}

#[cfg(test)]
mod tests {
    use super::Spring;

    fn run(s: &mut Spring, seconds: f32, dt: f32) -> f32 {
        let mut peak = s.value;
        for _ in 0..(seconds / dt) as usize {
            s.step(dt);
            peak = peak.max(s.value);
        }
        peak
    }

    #[test]
    fn settles_on_target() {
        let mut s = Spring::new(0.0, 380.0, 26.0);
        s.target = 100.0;
        run(&mut s, 2.0, 1.0 / 60.0);
        assert!(s.is_settled());
        assert_eq!(s.value, 100.0);
    }

    #[test]
    fn springy_overshoots_a_little() {
        let mut s = Spring::new(0.0, 380.0, 26.0);
        s.target = 100.0;
        let peak = run(&mut s, 1.0, 1.0 / 60.0);
        assert!(peak > 101.0 && peak < 115.0, "peak {peak}");
    }

    #[test]
    fn critically_damped_does_not_overshoot() {
        let k: f32 = 600.0;
        let mut s = Spring::new(0.0, k, 2.0 * k.sqrt());
        s.target = 100.0;
        let peak = run(&mut s, 1.0, 1.0 / 60.0);
        assert!(peak <= 100.0 + 0.01, "peak {peak}");
    }

    #[test]
    fn huge_frame_gap_stays_finite() {
        let mut s = Spring::new(0.0, 380.0, 26.0);
        s.target = 100.0;
        for _ in 0..50 {
            s.step(5.0);
        }
        assert!(s.value.is_finite());
        assert!(s.is_settled());
    }

    #[test]
    fn uneven_frames_land_in_the_same_place() {
        let (mut a, mut b) = (Spring::new(0.0, 380.0, 26.0), Spring::new(0.0, 380.0, 26.0));
        a.target = 100.0;
        b.target = 100.0;
        run(&mut a, 0.2, 1.0 / 60.0);
        for dt in [0.05, 0.01, 0.07, 0.02, 0.05] {
            b.step(dt);
        }
        assert!((a.value - b.value).abs() < 1.5, "{} vs {}", a.value, b.value);
    }
}

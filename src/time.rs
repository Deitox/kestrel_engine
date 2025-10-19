use std::time::{Duration, Instant};

pub struct Time {
    start: Instant,
    last: Instant,
    pub delta: Duration,
}
impl Time {
    pub fn new() -> Self {
        let now = Instant::now();
        Self { start: now, last: now, delta: Duration::from_secs_f32(0.0) }
    }
    pub fn tick(&mut self) {
        let now = Instant::now();
        self.delta = now - self.last;
        self.last = now;
    }
    pub fn delta_seconds(&self) -> f32 {
        self.delta.as_secs_f32()
    }
    pub fn elapsed_seconds(&self) -> f32 {
        self.last.duration_since(self.start).as_secs_f32()
    }
}

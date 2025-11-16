use crate::time::Time;

pub(crate) struct RuntimeLoop {
    time: Time,
    accumulator: f32,
    fixed_dt: f32,
}

pub(crate) struct RuntimeTick {
    pub dt: f32,
    pub dropped_backlog: Option<f32>,
}

impl RuntimeLoop {
    pub(crate) fn new(time: Time, fixed_dt: f32) -> Self {
        Self { time, accumulator: 0.0, fixed_dt }
    }

    pub(crate) fn time(&self) -> &Time {
        &self.time
    }

    pub(crate) fn tick(&mut self, max_backlog: f32) -> RuntimeTick {
        self.time.tick();
        let dt = self.time.delta_seconds();
        self.accumulator += dt;
        let mut dropped_backlog = None;
        if self.accumulator > max_backlog {
            dropped_backlog = Some(self.accumulator - max_backlog);
            self.accumulator = max_backlog;
        }
        RuntimeTick { dt, dropped_backlog }
    }

    pub(crate) fn pop_fixed_step(&mut self) -> Option<f32> {
        if self.accumulator >= self.fixed_dt {
            self.accumulator -= self.fixed_dt;
            Some(self.fixed_dt)
        } else {
            None
        }
    }
}

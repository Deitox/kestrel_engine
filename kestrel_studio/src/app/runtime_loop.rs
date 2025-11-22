use crate::time::Time;

pub(crate) struct RuntimeLoop {
    time: Time,
    accumulator: f32,
    fixed_dt: f32,
    max_backlog: f32,
    smoothed_dt: f32,
    smoothing_half_life: f32,
}

pub(crate) struct RuntimeTick {
    pub dt: f32,
    #[allow(dead_code)]
    pub raw_dt: f32,
    pub dropped_backlog: Option<f32>,
    #[allow(dead_code)]
    pub interpolation: f32,
}

impl RuntimeLoop {
    pub(crate) fn new(time: Time, fixed_dt: f32, max_backlog: f32, smoothing_half_life: f32) -> Self {
        let fixed_dt = fixed_dt.max(f32::EPSILON);
        let max_backlog = max_backlog.max(fixed_dt);
        Self {
            time,
            accumulator: 0.0,
            fixed_dt,
            max_backlog,
            smoothed_dt: fixed_dt,
            smoothing_half_life: smoothing_half_life.max(0.0),
        }
    }

    pub(crate) fn time(&self) -> &Time {
        &self.time
    }

    pub(crate) fn tick(&mut self) -> RuntimeTick {
        self.time.tick();
        let dt_raw = self.time.delta_seconds();
        self.accumulator += dt_raw;
        let mut dropped_backlog = None;
        if self.accumulator > self.max_backlog {
            dropped_backlog = Some(self.accumulator - self.max_backlog);
            self.accumulator = self.max_backlog;
        }
        let smoothing_half_life = self.smoothing_half_life;
        let alpha = if smoothing_half_life > 0.0 {
            let decay = (-dt_raw / smoothing_half_life).exp();
            1.0 - decay
        } else {
            1.0
        };
        let target_dt = dt_raw.clamp(0.0, self.max_backlog);
        self.smoothed_dt = self.smoothed_dt + (target_dt - self.smoothed_dt) * alpha.clamp(0.0, 1.0);
        let interpolation = (self.accumulator / self.fixed_dt).clamp(0.0, 1.0);
        RuntimeTick { dt: self.smoothed_dt, raw_dt: dt_raw, dropped_backlog, interpolation }
    }

    pub(crate) fn tick_paused(&mut self) -> RuntimeTick {
        // Keep wall-clock aligned so resuming does not accumulate a huge delta, but do not
        // advance the accumulator or smoothed dt while paused.
        self.time.tick();
        RuntimeTick { dt: 0.0, raw_dt: 0.0, dropped_backlog: None, interpolation: 0.0 }
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

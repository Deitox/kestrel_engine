use bevy_ecs::prelude::Resource;
use std::collections::HashMap;
use std::time::Instant;

#[derive(Clone, Copy, Debug)]
pub struct SystemTimingSummary {
    pub name: &'static str,
    pub last_ms: f32,
    pub average_ms: f32,
    pub max_ms: f32,
    pub samples: u64,
}

#[derive(Default)]
struct SystemTiming {
    last_ms: f32,
    total_ms: f32,
    max_ms: f32,
    samples: u64,
}

#[derive(Resource)]
pub struct SystemProfiler {
    timings: HashMap<&'static str, SystemTiming>,
}

impl SystemProfiler {
    pub fn new() -> Self {
        Self { timings: HashMap::new() }
    }

    pub fn begin_frame(&mut self) {}

    pub fn scope(&mut self, name: &'static str) -> SystemProfileScope<'_> {
        SystemProfileScope { name, profiler: self, start: Instant::now() }
    }

    fn record(&mut self, name: &'static str, duration: f32) {
        let entry = self.timings.entry(name).or_default();
        entry.last_ms = duration;
        entry.max_ms = entry.max_ms.max(duration);
        entry.total_ms += duration;
        entry.samples += 1;
    }

    pub fn summaries(&self) -> Vec<SystemTimingSummary> {
        let mut out = Vec::with_capacity(self.timings.len());
        for (&name, timing) in &self.timings {
            let avg = if timing.samples == 0 { 0.0 } else { timing.total_ms / timing.samples as f32 };
            out.push(SystemTimingSummary {
                name,
                last_ms: timing.last_ms,
                average_ms: avg,
                max_ms: timing.max_ms,
                samples: timing.samples,
            });
        }
        out.sort_by(|a, b| b.last_ms.partial_cmp(&a.last_ms).unwrap_or(std::cmp::Ordering::Equal));
        out
    }
}

impl Default for SystemProfiler {
    fn default() -> Self {
        Self::new()
    }
}

pub struct SystemProfileScope<'a> {
    name: &'static str,
    profiler: &'a mut SystemProfiler,
    start: Instant,
}

impl<'a> Drop for SystemProfileScope<'a> {
    fn drop(&mut self) {
        let duration_ms = self.start.elapsed().as_secs_f32() * 1000.0;
        self.profiler.record(self.name, duration_ms);
    }
}

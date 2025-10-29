use bevy_ecs::prelude::Resource;
use std::collections::HashMap;

mod animation;
mod particles;
mod physics;
mod picking;

pub use animation::*;
pub use particles::*;
pub use physics::*;
pub use picking::*;

#[derive(Resource, Clone, Copy)]
pub struct TimeDelta(pub f32);

#[derive(Resource, Clone)]
pub struct AnimationTime {
    pub scale: f32,
    pub paused: bool,
    pub fixed_step: Option<f32>,
    pub remainder: f32,
    pub group_scales: HashMap<String, f32>,
}

impl Default for AnimationTime {
    fn default() -> Self {
        Self { scale: 1.0, paused: false, fixed_step: None, remainder: 0.0, group_scales: HashMap::new() }
    }
}

impl AnimationTime {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_group_scale(&mut self, group: &str, scale: f32) {
        if (scale - 1.0).abs() < f32::EPSILON {
            self.group_scales.remove(group);
        } else {
            self.group_scales.insert(group.to_string(), scale);
        }
    }

    pub fn group_scale(&self, group: Option<&str>) -> f32 {
        group.and_then(|name| self.group_scales.get(name).copied()).unwrap_or(1.0).max(0.0)
    }

    pub fn set_fixed_step(&mut self, value: Option<f32>) {
        self.fixed_step = value.map(|step| step.max(std::f32::EPSILON));
        if self.fixed_step.is_none() {
            self.remainder = 0.0;
        }
    }

    pub fn consume(&mut self, raw_dt: f32) -> AnimationDelta {
        if raw_dt <= 0.0 || self.paused {
            return AnimationDelta::None;
        }
        let scaled = (raw_dt * self.scale.max(0.0)).max(0.0);
        if scaled <= 0.0 {
            return AnimationDelta::None;
        }
        if let Some(step) = self.fixed_step {
            let step = step.max(std::f32::EPSILON);
            self.remainder += scaled;
            let steps = (self.remainder / step).floor() as u32;
            if steps == 0 {
                return AnimationDelta::None;
            }
            self.remainder -= step * steps as f32;
            AnimationDelta::Fixed { step, steps }
        } else {
            self.remainder = 0.0;
            AnimationDelta::Single(scaled)
        }
    }
}

#[derive(Clone, Copy)]
pub enum AnimationDelta {
    None,
    Single(f32),
    Fixed { step: f32, steps: u32 },
}

impl AnimationDelta {
    pub fn has_steps(&self) -> bool {
        !matches!(self, AnimationDelta::None)
    }
}

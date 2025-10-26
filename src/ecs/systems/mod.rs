use bevy_ecs::prelude::Resource;

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

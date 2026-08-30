use bevy::app::{App, PreUpdate};

use crate::physics::bvh::{SceneBvh, update_bvh};

pub mod bvh;

#[allow(non_snake_case)]
pub fn PhysicsPlugin(app: &mut App) {
    app.insert_resource(SceneBvh {
        bvh: Vec::new(),
        root: 0,
    })
    .add_systems(PreUpdate, update_bvh);
}

use bevy::app::{App, PreUpdate, Update};

use crate::physics::bvh::{SceneBvh, debug_draw_scene_bvh, update_bvh};

pub mod bvh;

pub fn PhysicsPlugin(app: &mut App) {
    app.insert_resource(SceneBvh {
        bvh: Vec::new(),
        root: 0,
    })
    .add_systems(PreUpdate, update_bvh)
    .add_systems(Update, debug_draw_scene_bvh);
}

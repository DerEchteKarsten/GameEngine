use bevy::{
    app::{App, PostUpdate, Update},
    asset::{Assets, Handle},
    ecs::{
        component::Component,
        entity::Entity,
        resource::Resource,
        system::{Commands, Query, Res},
    },
    reflect::{self, Reflect},
    transform::components::Transform,
};

use lava::image::{Image, format, usage};

use crate::{
    assets::mesh::{GpuMesh, Scene},
    render::world::InstanceFlags,
    scene::camera::{Camera, update_camera},
};
use bevy::prelude::ReflectComponent;
pub mod camera;

#[derive(Component, Clone, Reflect)]
#[reflect(Component, Clone)]
pub struct SpawnScene {
    pub scene: Handle<Scene>,
}

#[derive(Component, Reflect)]
#[reflect(Component)]
pub struct Instance {
    pub mesh: Handle<GpuMesh>,
    pub flags: InstanceFlags,
}

fn add_sub_instances(
    mut commands: Commands,
    query: Query<(Entity, &SpawnScene)>,
    scenes: Res<Assets<Scene>>,
) {
    for (entity, instance) in &query {
        let Some(scene) = scenes.get(&instance.scene) else {
            continue;
        };

        commands
            .entity(entity)
            .with_children(|parent| {
                for instance in 0..scene.instance_transforms.len() {
                    let mesh = scene.meshes[scene.instance_mesh[instance] as usize].clone();
                    parent.spawn((
                        Instance {
                            mesh,
                            flags: InstanceFlags::empty(),
                        },
                        Transform::from_matrix(scene.instance_transforms[instance]),
                    ));
                }
            })
            .remove::<SpawnScene>();
    }
}

pub fn ScenePlugin(app: &mut App) {
    app.add_systems(PostUpdate, update_camera)
        .add_systems(Update, add_sub_instances)
        .register_type::<Instance>()
        .register_type::<InstanceFlags>()
        .register_type::<SpawnScene>()
        .register_type::<Camera>();
}

#[derive(Resource)]
pub struct Skybox {
    image: Image<format::R8G8B8A8Srgb, usage::Sampled>,
}

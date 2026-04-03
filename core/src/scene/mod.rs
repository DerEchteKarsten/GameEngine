use bevy::{
    app::{App, PostUpdate, Update},
    asset::{Assets, Handle},
    ecs::{
        component::Component,
        entity::Entity,
        system::{Commands, Query, Res},
    },
    reflect::{self, Reflect},
    transform::components::Transform,
};

use crate::{
    assets::{GpuMeshletMesh, Scene},
    render::world::InstanceFlags,
    scene::camera::update_camera,
};

pub mod camera;

#[derive(Component, Clone)]
pub struct SpawnScene {
    pub scene: Handle<Scene>,
}

use bevy::ecs::reflect::ReflectComponent;

#[derive(Component)]
pub struct Instance {
    pub mesh: Handle<GpuMeshletMesh>,
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
        .add_systems(Update, add_sub_instances);
}

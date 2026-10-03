//! Scene entities: mesh instances with material settings and spawning of imported scenes.
use bevy::{
    app::{App, PostUpdate, Update},
    asset::{Assets, Handle},
    ecs::{
        component::Component,
        entity::Entity,
        resource::Resource,
        system::{Commands, Query, Res},
    },
    reflect::{Reflect, TypeRegistry},
    transform::components::Transform,
};

use glam::{Vec3, Vec4};
use lava::{
    bindless::NULL_HANDLE,
    image::{Image, format, usage},
};

use crate::{
    assets::{
        material::{Material, TEXTURE_SLOTS, texture_indices},
        mesh::{GpuMesh, Scene},
        texture::GpuTexture,
    },
    editor::selected::EditorView,
    render::world::InstanceFlags,
    scene::camera::{Camera, update_camera},
    ui::builder::UiWindowBuilder,
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
pub struct MaterialSettings {
    pub color: Vec4,
    pub emissive: Vec3,
    pub metalic_factor: f32,
    pub roughness_factor: f32,
    pub normal_scale: f32,
    pub occlusion_strength: f32,
    pub color_texture: Option<Handle<GpuTexture>>,
    pub metallic_roughness_texture: Option<Handle<GpuTexture>>,
    pub normal_texture: Option<Handle<GpuTexture>>,
    pub occlusion_texture: Option<Handle<GpuTexture>>,
    pub emissive_texture: Option<Handle<GpuTexture>>,
}

impl MaterialSettings {
    pub(crate) fn into_material(&self, textures: &Assets<GpuTexture>) -> Material {
        Material {
            color: self.color,
            emissive: self.emissive,
            metalic_factor: self.metalic_factor,
            roughness_factor: self.roughness_factor,
            normal_scale: self.normal_scale,
            occlusion_strength: self.occlusion_strength,
            color_texture: self
                .color_texture
                .as_ref()
                .map(|t| textures.get(t).unwrap().descriptor_index())
                .unwrap_or(NULL_HANDLE),
            metallic_roughness_texture: self
                .metallic_roughness_texture
                .as_ref()
                .map(|t| textures.get(t).unwrap().descriptor_index())
                .unwrap_or(NULL_HANDLE),
            normal_texture: self
                .normal_texture
                .as_ref()
                .map(|t| textures.get(t).unwrap().descriptor_index())
                .unwrap_or(NULL_HANDLE),
            occlusion_texture: self
                .occlusion_texture
                .as_ref()
                .map(|t| textures.get(t).unwrap().descriptor_index())
                .unwrap_or(NULL_HANDLE),
            emissive_texture: self
                .emissive_texture
                .as_ref()
                .map(|t| textures.get(t).unwrap().descriptor_index())
                .unwrap_or(NULL_HANDLE),
        }
    }
}

#[derive(Component, Reflect)]
#[reflect(Component)]
pub struct Instance {
    pub mesh: Handle<GpuMesh>,
    pub material: MaterialSettings,
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
                    parent.spawn((
                        scene.get_instance(instance, InstanceFlags::empty()),
                        scene.get_transform(instance),
                    ));
                }
            })
            .remove::<SpawnScene>();
    }
}

#[allow(non_snake_case)]
pub fn ScenePlugin(app: &mut App) {
    app.add_systems(PostUpdate, update_camera)
        .add_systems(Update, add_sub_instances)
        .register_type::<Instance>()
        .register_type::<MaterialSettings>()
        .register_type::<InstanceFlags>()
        .register_type::<SpawnScene>()
        .register_type::<Camera>();
}

#[derive(Resource)]
pub struct Skybox {
    image: Handle<GpuTexture>,
}

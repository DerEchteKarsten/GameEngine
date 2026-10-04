//! Scene entities: mesh instances with material settings and spawning of imported scenes.
use bevy::{
    app::{App, PostUpdate, Update},
    asset::{Assets, Handle},
    ecs::{
        component::Component,
        entity::Entity,
        name::Name,
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
    /// Fragments with less alpha are not drawn. 0 draws everything.
    pub alpha_cutoff: f32,
    pub color_texture: Option<Handle<GpuTexture>>,
    pub metallic_roughness_texture: Option<Handle<GpuTexture>>,
    pub normal_texture: Option<Handle<GpuTexture>>,
    pub occlusion_texture: Option<Handle<GpuTexture>>,
    pub emissive_texture: Option<Handle<GpuTexture>>,
}

impl Default for MaterialSettings {
    fn default() -> Self {
        Self {
            color: Vec4::ONE,
            emissive: Vec3::ZERO,
            metalic_factor: 0.0,
            roughness_factor: 1.0,
            normal_scale: 1.0,
            occlusion_strength: 1.0,
            alpha_cutoff: 0.0,
            color_texture: None,
            metallic_roughness_texture: None,
            normal_texture: None,
            occlusion_texture: None,
            emissive_texture: None,
        }
    }
}

impl MaterialSettings {
    pub(crate) fn into_material(&self, textures: &Assets<GpuTexture>) -> Material {
        // A texture that is unset or still loading falls back to the null handle.
        let index = |texture: &Option<Handle<GpuTexture>>| {
            texture
                .as_ref()
                .and_then(|t| textures.get(t))
                .map(|t| t.descriptor_index())
                .unwrap_or(NULL_HANDLE)
        };
        Material {
            color: self.color,
            emissive: self.emissive,
            metalic_factor: self.metalic_factor,
            roughness_factor: self.roughness_factor,
            normal_scale: self.normal_scale,
            occlusion_strength: self.occlusion_strength,
            alpha_cutoff: self.alpha_cutoff,
            color_texture: index(&self.color_texture),
            metallic_roughness_texture: index(&self.metallic_roughness_texture),
            normal_texture: index(&self.normal_texture),
            occlusion_texture: index(&self.occlusion_texture),
            emissive_texture: index(&self.emissive_texture),
            pad: Vec3::ZERO,
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
                    let mut child = parent.spawn((
                        scene.get_instance(instance, InstanceFlags::empty()),
                        scene.get_transform(instance),
                    ));
                    let name = &scene.instance_names[instance];
                    if !name.is_empty() {
                        child.insert(Name::new(name.clone()));
                    }
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

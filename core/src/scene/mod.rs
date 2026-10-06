//! Scene entities: mesh instances, the selected instance's editable material settings and spawning of imported scenes.
use bevy::ecs::reflect::ReflectResource;
use bevy::{
    app::{App, Last, PostUpdate, Update},
    asset::{AssetEvent, Assets, Handle},
    ecs::{
        component::Component,
        entity::Entity,
        message::MessageReader,
        name::Name,
        query::{Changed, With, Without},
        resource::Resource,
        schedule::IntoScheduleConfigs,
        system::{Commands, Query, Res, ResMut},
    },
    reflect::{Reflect, TypeRegistry},
    transform::components::Transform,
};

use glam::{Vec3, Vec4};
use lava::{
    bindings::Material,
    image::{Image, format, usage},
};

use crate::{
    assets::{
        mesh::{GpuMesh, MaterialSet, MaterialTextures, Scene},
        texture::GpuTexture,
    },
    editor::{picking::Selected, selected::EditorView},
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

/// The editable view of an instance's material. Only selected instances have one; it is read
/// from the instance's [`MaterialSet`] and edits are written back into that GPU memory.
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
    fn from_material(material: &Material, textures: &MaterialTextures) -> Self {
        let [
            color_texture,
            metallic_roughness_texture,
            normal_texture,
            occlusion_texture,
            emissive_texture,
        ] = textures.clone();
        Self {
            color: material.color,
            emissive: material.emissive,
            metalic_factor: material.metalic_factor,
            roughness_factor: material.roughness_factor,
            normal_scale: material.normal_scale,
            occlusion_strength: material.occlusion_strength,
            alpha_cutoff: material.alpha_cutoff,
            color_texture,
            metallic_roughness_texture,
            normal_texture,
            occlusion_texture,
            emissive_texture,
        }
    }

    fn textures(&self) -> MaterialTextures {
        [
            self.color_texture.clone(),
            self.metallic_roughness_texture.clone(),
            self.normal_texture.clone(),
            self.occlusion_texture.clone(),
            self.emissive_texture.clone(),
        ]
    }

    /// `base` with the settings written over it, which leaves its texture indices.
    pub(crate) fn into_material(&self, base: Material) -> Material {
        Material {
            color: self.color,
            emissive: self.emissive,
            metalic_factor: self.metalic_factor,
            roughness_factor: self.roughness_factor,
            normal_scale: self.normal_scale,
            occlusion_strength: self.occlusion_strength,
            alpha_cutoff: self.alpha_cutoff,
            ..base
        }
    }
}

#[derive(Component, Reflect)]
#[reflect(Component)]
pub struct Instance {
    pub mesh: Handle<GpuMesh>,
    pub material_set: Handle<MaterialSet>,
    pub material_index: u32,
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

/// Textures load on their own, after the material sets that use them. Whenever one arrives
/// or is replaced, the materials get its bindless index.
fn resolve_material_textures(
    mut texture_events: MessageReader<AssetEvent<GpuTexture>>,
    mut set_events: MessageReader<AssetEvent<MaterialSet>>,
    material_sets: Res<Assets<MaterialSet>>,
    textures: Res<Assets<GpuTexture>>,
) {
    let arrived = |id| textures.get(id).is_some();
    let texture_arrived = texture_events.read().any(|event| match event {
        AssetEvent::Added { id } | AssetEvent::Modified { id } => arrived(*id),
        _ => false,
    });
    let new_sets: Vec<_> = set_events
        .read()
        .filter_map(|event| match event {
            AssetEvent::Added { id } | AssetEvent::Modified { id } => Some(*id),
            _ => None,
        })
        .collect();
    if texture_arrived {
        for (_, set) in material_sets.iter() {
            set.resolve_textures(&textures);
        }
    } else {
        for set in new_sets.into_iter().filter_map(|id| material_sets.get(id)) {
            set.resolve_textures(&textures);
        }
    }
}

/// Writes edited settings into the material set, which is what the renderer reads.
fn write_material_settings(
    query: Query<(&Instance, &MaterialSettings), Changed<MaterialSettings>>,
    mut material_sets: ResMut<Assets<MaterialSet>>,
) {
    for (instance, settings) in &query {
        // This marks the set as modified, so `resolve_material_textures` writes the indices
        // of its new textures.
        let Some(mut set) = material_sets.get_mut(&instance.material_set) else {
            continue;
        };
        let index = instance.material_index as usize;
        set.textures[index] = settings.textures();
        let mut materials = set.buffer.range(..);
        materials[index] = settings.into_material(materials[index]);
    }
}

fn remove_material_settings(
    mut commands: Commands,
    query: Query<Entity, (With<MaterialSettings>, Without<Selected>)>,
) {
    for entity in &query {
        commands.entity(entity).remove::<MaterialSettings>();
    }
}

fn add_material_settings(
    mut commands: Commands,
    query: Query<(Entity, &Instance), (With<Selected>, Without<MaterialSettings>)>,
    material_sets: Res<Assets<MaterialSet>>,
) {
    for (entity, instance) in &query {
        let Some(set) = material_sets.get(&instance.material_set) else {
            continue;
        };
        let material = &set.buffer[instance.material_index as usize];
        commands
            .entity(entity)
            .insert(MaterialSettings::from_material(
                material,
                &set.textures[instance.material_index as usize],
            ));
    }
}

#[allow(non_snake_case)]
pub fn ScenePlugin(app: &mut App) {
    app.add_systems(PostUpdate, update_camera)
        .add_systems(Update, add_sub_instances)
        // After the asset events of the frame are sent.
        .add_systems(Last, resolve_material_textures)
        .add_systems(
            Update,
            (
                write_material_settings,
                remove_material_settings,
                add_material_settings,
            )
                .chain(),
        )
        .register_type::<Instance>()
        .register_type::<MaterialSettings>()
        .register_type::<InstanceFlags>()
        .register_type::<SpawnScene>()
        .register_type::<Camera>();
}

#[derive(Resource, Reflect)]
#[reflect(Resource)]
pub struct Skybox {
    image: Handle<GpuTexture>,
}

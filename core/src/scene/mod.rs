//! Scene entities: mesh instances and how scene files save them, the selected instance's editable material settings, spawning scenes, and the skybox.
use anyhow::Result;
use bevy::ecs::reflect::ReflectResource;
use bevy::{
    app::{App, PostUpdate, Update},
    asset::{Assets, Handle},
    ecs::{
        component::Component,
        entity::Entity,
        name::Name,
        query::{Changed, With, Without},
        resource::Resource,
        schedule::IntoScheduleConfigs,
        system::{Commands, Query, Res, ResMut},
        world::{Mut, World},
    },
    reflect::Reflect,
    transform::components::Transform,
};
use glam::{Vec3, Vec4};
use lava::bindings::Material;
use serde::{Deserialize, Serialize};

use crate::{
    assets::{material::GpuMaterial, mesh::GpuMesh, texture::GpuTexture},
    editor::picking::Selected,
    scene::{
        camera::{Camera, update_camera},
        file::{LoadCtx, ReflectSceneComponent, SaveCtx, Scene, SceneComponent, spawn_scene},
    },
};
use bevy::prelude::ReflectComponent;
pub mod camera;
pub mod file;

/// Spawns the entities of `scene` as children of this entity, then is removed. Scene files
/// save it as an asset index, so a scene can place other scenes.
#[derive(Component, Clone, Reflect)]
#[require(Transform)]
#[reflect(Component, Clone, SceneComponent)]
pub struct SpawnScene {
    pub scene: Handle<Scene>,
}

impl SceneComponent for SpawnScene {
    type Saved = u32;
    fn save(&self, ctx: &mut SaveCtx) -> Option<u32> {
        ctx.asset(&self.scene)
    }
    fn load(saved: u32, ctx: &mut LoadCtx) -> Result<Self> {
        Ok(Self {
            scene: ctx.asset(saved)?,
        })
    }
}

/// The editable view of an instance's material. Only selected instances have one; it is read
/// from the instance's [`GpuMaterial`] and edits are written back into its GPU memory.
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
    fn from_material(material: &Material, textures: &[Option<Handle<GpuTexture>>; 5]) -> Self {
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

    fn textures(&self) -> [Option<Handle<GpuTexture>>; 5] {
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

#[derive(Component, Reflect, Clone)]
#[reflect(Component, SceneComponent)]
pub struct Instance {
    pub mesh: Handle<GpuMesh>,
    pub material: Handle<GpuMaterial>,
}

/// An [`Instance`] in a scene file: indices into the file's assets.
#[derive(Serialize, Deserialize)]
pub struct SavedInstance {
    pub mesh: u32,
    pub material: u32,
}

impl SceneComponent for Instance {
    type Saved = SavedInstance;
    fn save(&self, ctx: &mut SaveCtx) -> Option<SavedInstance> {
        Some(SavedInstance {
            mesh: ctx.asset(&self.mesh)?,
            material: ctx.asset(&self.material)?,
        })
    }
    fn load(saved: SavedInstance, ctx: &mut LoadCtx) -> Result<Self> {
        Ok(Self {
            mesh: ctx.asset(saved.mesh)?,
            material: ctx.asset(saved.material)?,
        })
    }
}

fn spawn_scenes(world: &mut World) {
    let pending: Vec<(Entity, Handle<Scene>)> = world
        .query::<(Entity, &SpawnScene)>()
        .iter(world)
        .map(|(entity, spawn)| (entity, spawn.scene.clone()))
        .collect();
    for (root, handle) in pending {
        world.resource_scope(|world, scenes: Mut<Assets<Scene>>| {
            if let Some(scene) = scenes.get(&handle) {
                spawn_scene(world, root, scene);
                world.entity_mut(root).remove::<SpawnScene>();
            }
        });
    }
}

/// Writes edited settings into the material, which is what the renderer reads.
fn write_material_settings(
    query: Query<(&Instance, &MaterialSettings), Changed<MaterialSettings>>,
    mut materials: ResMut<Assets<GpuMaterial>>,
) {
    for (instance, settings) in &query {
        let Some(mut material) = materials.get_mut(&instance.material) else {
            continue;
        };
        material.textures = settings.textures();
        material.write(settings.into_material(material.read()));
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
    materials: Res<Assets<GpuMaterial>>,
) {
    for (entity, instance) in &query {
        let Some(material) = materials.get(&instance.material) else {
            continue;
        };
        commands
            .entity(entity)
            .insert(MaterialSettings::from_material(
                &material.read(),
                &material.textures,
            ));
    }
}

#[allow(non_snake_case)]
pub fn ScenePlugin(app: &mut App) {
    app.add_systems(PostUpdate, update_camera)
        .add_systems(Update, spawn_scenes)
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
        .register_type::<SpawnScene>()
        .register_type::<Transform>()
        .register_type::<Name>()
        .register_type_data::<Transform, ReflectSceneComponent>()
        .register_type_data::<Name, ReflectSceneComponent>()
        .register_type::<Camera>()
        .init_resource::<Skybox>();
}

/// The sky drawn behind the scene: an equirectangular texture, usually HDR (baked from an
/// OpenEXR file). Until it is loaded, or without one, the sky is a flat colour.
#[derive(Resource, Reflect, Default)]
#[reflect(Resource)]
pub struct Skybox {
    pub image: Handle<GpuTexture>,
}

//! Material assets: the `.mat`/`.mat.ron` file, its loader, and the one host-mapped GPU buffer every loaded material has a slot in.
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{Context, Result, bail};
use bevy::{
    asset::{Asset, AssetLoader, Handle, LoadContext, io::Reader},
    ecs::{resource::Resource, world::FromWorld, world::World},
    reflect::TypePath,
};
use glam::{Vec3, Vec4};
use lava::{bindings::Material, bindless::NULL_HANDLE, buffer::Buffer};
use serde::{Deserialize, Serialize};

use crate::assets::texture::{GpuTexture, texture_index};

pub const MATERIAL_EXTENSION: &str = "mat";
pub const MAX_MATERIALS: u32 = 65536;

/// A `.mat` (postcard) or `.mat.ron` file. Texture paths are relative to the file.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct MaterialFile {
    pub color: Vec4,
    pub emissive: Vec3,
    pub metalic_factor: f32,
    pub roughness_factor: f32,
    pub normal_scale: f32,
    pub occlusion_strength: f32,
    pub alpha_cutoff: f32,
    pub color_texture: Option<String>,
    pub metallic_roughness_texture: Option<String>,
    pub normal_texture: Option<String>,
    pub occlusion_texture: Option<String>,
    pub emissive_texture: Option<String>,
}

impl MaterialFile {
    pub fn write(&self, ron: bool) -> Result<Vec<u8>> {
        Ok(if ron {
            ron::ser::to_string_pretty(self, Default::default())?.into_bytes()
        } else {
            postcard::to_stdvec(self)?
        })
    }

    pub fn read(bytes: &[u8], ron: bool) -> Result<Self> {
        Ok(if ron {
            ron::de::from_bytes(bytes)?
        } else {
            postcard::from_bytes(bytes)?
        })
    }

    pub fn textures(&self) -> [&Option<String>; 5] {
        [
            &self.color_texture,
            &self.metallic_roughness_texture,
            &self.normal_texture,
            &self.occlusion_texture,
            &self.emissive_texture,
        ]
    }

    /// The material with the given texture indices.
    pub fn material(&self, textures: [u32; 5]) -> Material {
        let [
            color_texture,
            metallic_roughness_texture,
            normal_texture,
            occlusion_texture,
            emissive_texture,
        ] = textures;
        Material {
            color: self.color,
            emissive: self.emissive,
            metalic_factor: self.metalic_factor,
            roughness_factor: self.roughness_factor,
            normal_scale: self.normal_scale,
            occlusion_strength: self.occlusion_strength,
            alpha_cutoff: self.alpha_cutoff,
            color_texture,
            metallic_roughness_texture,
            normal_texture,
            occlusion_texture,
            emissive_texture,
            pad: Vec3::ZERO,
        }
    }
}

/// The material buffer and its free slots, shared by the loader and the systems.
#[derive(Default)]
pub struct MaterialState {
    /// Created on the first allocation: the loader is built before lava is initialised.
    buffer: OnceLock<Buffer<Material>>,
    /// Freed slots, and how many slots were handed out so far.
    slots: Mutex<(Vec<u32>, u32)>,
}

fn take_slot((free, count): &mut (Vec<u32>, u32)) -> Result<u32> {
    if let Some(slot) = free.pop() {
        return Ok(slot);
    }
    if *count == MAX_MATERIALS {
        bail!("all {MAX_MATERIALS} material slots are in use");
    }
    *count += 1;
    Ok(*count - 1)
}

impl MaterialState {
    /// Writes `material` into a free slot. Texture indices come from `textures`.
    pub fn alloc(
        self: &Arc<Self>,
        material: Material,
        textures: [Option<Handle<GpuTexture>>; 5],
    ) -> Result<GpuMaterial> {
        let slot = take_slot(&mut self.slots.lock().unwrap())?;
        let buffer = self
            .buffer
            .get_or_init(|| Buffer::new(MAX_MATERIALS as usize, true).unwrap());
        let gpu_material = GpuMaterial {
            gpu: buffer.address + slot as u64 * size_of::<Material>() as u64,
            cpu: unsafe { buffer.range(..).ptr().add(slot as usize) },
            textures,
            state: self.clone(),
        };
        gpu_material.write(material);
        Ok(gpu_material)
    }
}

/// The state that `MaterialLoader` shares with systems that create materials outside a load.
#[derive(Resource, Clone)]
pub struct Materials(pub Arc<MaterialState>);

#[derive(Asset, TypePath)]
pub struct GpuMaterial {
    /// Address of the material, what the renderer writes per instance.
    pub gpu: u64,
    /// The same slot through the host mapping.
    pub cpu: *mut Material,
    /// The color, metallic-roughness, normal, occlusion and emissive texture. Keeps them alive.
    pub textures: [Option<Handle<GpuTexture>>; 5],
    state: Arc<MaterialState>,
}

// `cpu` points into the buffer `state` keeps alive, which never moves.
unsafe impl Send for GpuMaterial {}
unsafe impl Sync for GpuMaterial {}

impl GpuMaterial {
    pub fn read(&self) -> Material {
        unsafe { *self.cpu }
    }

    /// Writes `material` with the indices of `self.textures`.
    pub fn write(&self, material: Material) {
        let [
            color_texture,
            metallic_roughness_texture,
            normal_texture,
            occlusion_texture,
            emissive_texture,
        ] = self.textures.each_ref().map(|texture| {
            texture
                .as_ref()
                .map_or(NULL_HANDLE, |texture| texture_index(texture.id()))
        });
        unsafe {
            *self.cpu = Material {
                color_texture,
                metallic_roughness_texture,
                normal_texture,
                occlusion_texture,
                emissive_texture,
                ..material
            }
        };
    }

    /// The file of the material, with texture paths relative to the asset root.
    pub fn file(&self) -> MaterialFile {
        let material = self.read();
        let [
            color_texture,
            metallic_roughness_texture,
            normal_texture,
            occlusion_texture,
            emissive_texture,
        ] = self
            .textures
            .each_ref()
            .map(|texture| Some(format!("/{}", texture.as_ref()?.path()?)));
        MaterialFile {
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
}

impl Drop for GpuMaterial {
    fn drop(&mut self) {
        let base = self.state.buffer.get().map_or(0, |buffer| buffer.address);
        let slot = ((self.gpu - base) / size_of::<Material>() as u64) as u32;
        self.state.slots.lock().unwrap().0.push(slot);
    }
}

#[derive(TypePath)]
pub struct MaterialLoader(Arc<MaterialState>);

impl FromWorld for MaterialLoader {
    fn from_world(world: &mut World) -> Self {
        let state = Arc::new(MaterialState::default());
        world.insert_resource(Materials(state.clone()));
        Self(state)
    }
}

impl AssetLoader for MaterialLoader {
    type Asset = GpuMaterial;
    type Error = anyhow::Error;
    type Settings = ();
    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &(),
        load_context: &mut LoadContext<'_>,
    ) -> Result<GpuMaterial> {
        let ron = load_context.path().to_string().ends_with(".ron");
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        let file = MaterialFile::read(&bytes, ron)?;
        // The handles of the deferred loads already decide the bindless index of each texture.
        let mut textures: [Option<Handle<GpuTexture>>; 5] = Default::default();
        for (texture, path) in textures.iter_mut().zip(file.textures()) {
            if let Some(path) = path {
                let path = load_context
                    .path()
                    .resolve_embed_str(path)
                    .with_context(|| format!("texture {path}"))?;
                *texture = Some(load_context.load(path));
            }
        }
        self.0.alloc(file.material([NULL_HANDLE; 5]), textures)
    }

    fn extensions(&self) -> &[&str] {
        &[MATERIAL_EXTENSION, "mat.ron"]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_come_from_the_free_list_first() {
        let mut slots = (Vec::new(), 0);
        assert_eq!(take_slot(&mut slots).unwrap(), 0);
        assert_eq!(take_slot(&mut slots).unwrap(), 1);
        slots.0.push(0);
        assert_eq!(take_slot(&mut slots).unwrap(), 0);
        assert_eq!(take_slot(&mut slots).unwrap(), 2);

        let mut full = (Vec::new(), MAX_MATERIALS);
        assert!(take_slot(&mut full).is_err());
        full.0.push(7);
        assert_eq!(take_slot(&mut full).unwrap(), 7);
    }

    #[test]
    fn material_files_round_trip() {
        let file = MaterialFile {
            color: Vec4::new(1.0, 0.5, 0.25, 1.0),
            emissive: Vec3::ZERO,
            metalic_factor: 0.0,
            roughness_factor: 0.8,
            normal_scale: 1.0,
            occlusion_strength: 1.0,
            alpha_cutoff: 0.5,
            color_texture: Some("../textures/bricks.tex".into()),
            metallic_roughness_texture: None,
            normal_texture: Some("/sponza/textures/n.tex".into()),
            occlusion_texture: None,
            emissive_texture: None,
        };
        for ron in [false, true] {
            let bytes = file.write(ron).unwrap();
            assert_eq!(MaterialFile::read(&bytes, ron).unwrap(), file);
        }
    }
}

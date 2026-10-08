//! Scene files: entities with a parent and opted-in components, in a binary (`.scene`) and a RON (`.scene.ron`) layout; their loader, spawning and saving.
use std::{
    collections::{BTreeMap, HashMap},
    io::Write,
    path::Path,
};

use anyhow::{Context, Result, bail, ensure};
use bevy::{
    asset::{
        Asset, AssetLoader, Assets, AsyncReadExt, Handle, LoadContext, UntypedAssetId,
        UntypedHandle, io::Reader,
    },
    ecs::{
        component::Component,
        entity::Entity,
        hierarchy::{ChildOf, Children},
        reflect::AppTypeRegistry,
        world::{FromWorld, World},
    },
    log::warn,
    reflect::{FromType, TypePath},
};
use ron::value::RawValue;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::assets::{
    material::{GpuMaterial, MATERIAL_EXTENSION},
    util::{read_slice, write_slice},
};

/// A component that scene files can hold. Plain components only need serde and `Clone`;
/// ones that hold handles save them as indices into the file's assets.
pub trait SceneComponent: Component + Clone {
    type Saved: Serialize + DeserializeOwned;
    /// `None` skips the component, with a warning.
    fn save(&self, ctx: &mut SaveCtx) -> Option<Self::Saved>;
    fn load(saved: Self::Saved, ctx: &mut LoadCtx) -> Result<Self>;
}

impl<T: Component + Clone + Serialize + DeserializeOwned> SceneComponent for T {
    type Saved = T;
    fn save(&self, _: &mut SaveCtx) -> Option<T> {
        Some(self.clone())
    }
    fn load(saved: T, _: &mut LoadCtx) -> Result<T> {
        Ok(saved)
    }
}

/// Collects the assets that saved components refer to.
#[derive(Default)]
pub struct SaveCtx {
    assets: Vec<String>,
    indices: HashMap<String, u32>,
    /// Paths, relative to the scene file, of assets that had none and were written for it.
    written: HashMap<UntypedAssetId, String>,
}

impl SaveCtx {
    pub fn asset<A: Asset>(&mut self, handle: &Handle<A>) -> Option<u32> {
        let path = match handle.path() {
            Some(path) => format!("/{path}"),
            None => self.written.get(&handle.id().untyped())?.clone(),
        };
        let next = self.assets.len() as u32;
        Some(*self.indices.entry(path).or_insert_with_key(|path| {
            self.assets.push(path.clone());
            next
        }))
    }
}

/// Starts the loads of the assets that loaded components refer to.
pub struct LoadCtx<'a, 'ctx> {
    load_context: &'a mut LoadContext<'ctx>,
    assets: Vec<String>,
    handles: Vec<Option<UntypedHandle>>,
}

impl LoadCtx<'_, '_> {
    /// The handle of asset `index`. Its (deferred) load starts on first use.
    pub fn asset<A: Asset>(&mut self, index: u32) -> Result<Handle<A>> {
        let i = index as usize;
        let path = self.assets.get(i).context("asset index out of range")?;
        if let Some(handle) = &self.handles[i] {
            return Ok(handle.clone().try_typed()?);
        }
        let path = self.load_context.path().resolve_embed_str(path)?;
        let handle: Handle<A> = self.load_context.load(path);
        self.handles[i] = Some(handle.clone().untyped());
        Ok(handle)
    }
}

/// Inserts one column's components into the spawned entities of a scene.
type SpawnColumn = Box<dyn Fn(&mut World, &[Entity]) + Send + Sync>;

#[derive(Asset, TypePath)]
pub struct Scene {
    /// One per entity: the index of its parent, or `u32::MAX` for the spawn root.
    pub parents: Vec<u32>,
    pub columns: Vec<SpawnColumn>,
}

/// The values of one column as read from either layout.
enum Values<'a> {
    Postcard(&'a [u8]),
    Ron(Vec<Box<RawValue>>),
}

/// Lets scene files save and load a component, by its `short_type_path`.
#[derive(Clone)]
pub struct ReflectSceneComponent {
    load: fn(Values, Vec<u32>, &mut LoadCtx) -> Result<SpawnColumn>,
    save: fn(&World, &[Entity], &mut SaveCtx) -> Option<Column>,
}

impl<T: SceneComponent + TypePath> FromType<T> for ReflectSceneComponent {
    fn from_type() -> Self {
        Self {
            load: |values, entities, ctx| {
                let saved: Vec<T::Saved> = match values {
                    Values::Postcard(bytes) => postcard::from_bytes(bytes)?,
                    Values::Ron(values) => values
                        .iter()
                        .map(|value| value.into_rust())
                        .collect::<Result<_, _>>()?,
                };
                ensure!(
                    saved.len() == entities.len(),
                    "{}: one value per entity",
                    T::short_type_path()
                );
                let components: Vec<T> = saved
                    .into_iter()
                    .map(|saved| T::load(saved, ctx))
                    .collect::<Result<_>>()?;
                Ok(Box::new(move |world, spawned| {
                    world.insert_batch(
                        entities
                            .iter()
                            .zip(&components)
                            .map(|(&i, component)| (spawned[i as usize], component.clone())),
                    );
                }))
            },
            save: |world, entities, ctx| {
                let mut column = (Vec::new(), Vec::new());
                for (i, &entity) in entities.iter().enumerate() {
                    let Some(component) = world.get::<T>(entity) else {
                        continue;
                    };
                    match component.save(ctx) {
                        Some(saved) => {
                            column.0.push(i as u32);
                            column.1.push(saved);
                        }
                        None => warn!("{entity}: its {} can't be saved", T::short_type_path()),
                    }
                }
                (!column.0.is_empty()).then(|| Column::new::<T>(column.0, column.1))
            },
        }
    }
}

/// The values of a column, serialisable for either layout.
pub trait ColumnValues {
    fn postcard(&self) -> Result<Vec<u8>>;
    fn ron(&self, index: usize) -> Result<Box<RawValue>>;
}

impl<S: Serialize> ColumnValues for Vec<S> {
    fn postcard(&self) -> Result<Vec<u8>> {
        Ok(postcard::to_stdvec(self)?)
    }
    fn ron(&self, index: usize) -> Result<Box<RawValue>> {
        Ok(RawValue::from_rust(&self[index])?)
    }
}

pub struct Column {
    pub name: String,
    /// Indices of the entities that have the component.
    pub entities: Vec<u32>,
    pub values: Box<dyn ColumnValues>,
}

impl Column {
    pub fn new<T: SceneComponent + TypePath>(entities: Vec<u32>, values: Vec<T::Saved>) -> Self
    where
        T::Saved: 'static,
    {
        assert_eq!(entities.len(), values.len());
        Self {
            name: T::short_type_path().to_string(),
            entities,
            values: Box::new(values),
        }
    }
}

/// The contents of a scene file, in either layout.
#[derive(Default)]
pub struct SceneData {
    /// Paths relative to the scene file; a leading `/` is relative to the asset root.
    pub assets: Vec<String>,
    pub parents: Vec<u32>,
    pub columns: Vec<Column>,
}

/// The RON layout: the same data, listed entity by entity so it can be edited by hand.
#[derive(Serialize, Deserialize)]
pub(crate) struct RonScene {
    pub(crate) assets: Vec<String>,
    pub(crate) entities: Vec<RonEntity>,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct RonEntity {
    pub(crate) parent: Option<u32>,
    pub(crate) components: BTreeMap<String, Box<RawValue>>,
}

impl SceneData {
    pub fn write(&self, writer: &mut impl Write, ron: bool) -> Result<()> {
        if ron {
            self.write_ron(writer)
        } else {
            self.write_binary(writer)
        }
    }

    fn write_binary(&self, writer: &mut impl Write) -> Result<()> {
        let head = postcard::to_stdvec(&(&self.assets, &self.parents, self.columns.len() as u32))?;
        write_slice(&head, writer)?;
        for column in &self.columns {
            // The postcard of a tuple is that of its fields one after the other.
            let mut frame = postcard::to_stdvec(&(&column.name, &column.entities))?;
            frame.extend(column.values.postcard()?);
            write_slice(&frame, writer)?;
        }
        Ok(())
    }

    fn write_ron(&self, writer: &mut impl Write) -> Result<()> {
        let mut entities: Vec<RonEntity> = self
            .parents
            .iter()
            .map(|&parent| RonEntity {
                parent: (parent != u32::MAX).then_some(parent),
                components: BTreeMap::new(),
            })
            .collect();
        for column in &self.columns {
            for (i, &entity) in column.entities.iter().enumerate() {
                entities[entity as usize]
                    .components
                    .insert(column.name.clone(), column.values.ron(i)?);
            }
        }
        let scene = RonScene {
            assets: self.assets.clone(),
            entities,
        };
        let config = ron::ser::PrettyConfig::default().struct_names(false);
        writer.write_all(ron::ser::to_string_pretty(&scene, config)?.as_bytes())?;
        Ok(())
    }
}

#[derive(TypePath)]
pub struct SceneLoader(AppTypeRegistry);

impl FromWorld for SceneLoader {
    fn from_world(world: &mut World) -> Self {
        Self(world.resource::<AppTypeRegistry>().clone())
    }
}

impl SceneLoader {
    fn column_type(&self, name: &str) -> Result<ReflectSceneComponent> {
        let registry = self.0.read();
        let mut matches = registry.iter().filter(|registration| {
            registration.type_info().type_path_table().short_path() == name
                && registration.data::<ReflectSceneComponent>().is_some()
        });
        let Some(registration) = matches.next() else {
            bail!("no scene component is called {name}");
        };
        ensure!(
            matches.next().is_none(),
            "several scene components are called {name}"
        );
        Ok(registration
            .data::<ReflectSceneComponent>()
            .unwrap()
            .clone())
    }

    fn column(
        &self,
        name: &str,
        values: Values,
        entities: Vec<u32>,
        ctx: &mut LoadCtx,
        entity_count: usize,
    ) -> Result<SpawnColumn> {
        ensure!(
            entities.iter().all(|&e| (e as usize) < entity_count),
            "{name}: entity index out of range"
        );
        (self.column_type(name)?.load)(values, entities, ctx).with_context(|| name.to_string())
    }
}

impl AssetLoader for SceneLoader {
    type Asset = Scene;
    type Error = anyhow::Error;
    type Settings = ();
    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &(),
        load_context: &mut LoadContext<'_>,
    ) -> Result<Scene> {
        let mut columns = Vec::new();
        let parents;
        if load_context.path().to_string().ends_with(".ron") {
            // The RON parser needs the whole text.
            let mut text = String::new();
            reader.read_to_string(&mut text).await?;
            let scene: RonScene = ron::from_str(&text)?;
            parents = scene
                .entities
                .iter()
                .map(|entity| entity.parent.unwrap_or(u32::MAX))
                .collect::<Vec<_>>();
            let mut ctx = load_context_for(load_context, scene.assets);
            let mut grouped: BTreeMap<String, (Vec<u32>, Vec<Box<RawValue>>)> = BTreeMap::new();
            for (i, entity) in scene.entities.into_iter().enumerate() {
                for (name, value) in entity.components {
                    let column = grouped.entry(name).or_default();
                    column.0.push(i as u32);
                    column.1.push(value);
                }
            }
            for (name, (entities, values)) in grouped {
                columns.push(self.column(
                    &name,
                    Values::Ron(values),
                    entities,
                    &mut ctx,
                    parents.len(),
                )?);
            }
        } else {
            let head: Vec<u8> = read_slice(reader).await?;
            let (assets, parents_read, column_count): (Vec<String>, Vec<u32>, u32) =
                postcard::from_bytes(&head)?;
            parents = parents_read;
            let mut ctx = load_context_for(load_context, assets);
            for _ in 0..column_count {
                let frame: Vec<u8> = read_slice(reader).await?;
                let ((name, entities), values): ((String, Vec<u32>), _) =
                    postcard::take_from_bytes(&frame)?;
                columns.push(self.column(
                    &name,
                    Values::Postcard(values),
                    entities,
                    &mut ctx,
                    parents.len(),
                )?);
            }
        }
        ensure!(
            parents
                .iter()
                .all(|&p| p == u32::MAX || (p as usize) < parents.len()),
            "parent index out of range"
        );
        Ok(Scene { parents, columns })
    }

    fn extensions(&self) -> &[&str] {
        &["scene", "scene.ron"]
    }
}

fn load_context_for<'a, 'ctx>(
    load_context: &'a mut LoadContext<'ctx>,
    assets: Vec<String>,
) -> LoadCtx<'a, 'ctx> {
    LoadCtx {
        load_context,
        handles: vec![None; assets.len()],
        assets,
    }
}

/// Spawns the entities of `scene` as descendants of `root`.
pub fn spawn_scene(world: &mut World, root: Entity, scene: &Scene) {
    let entities: Vec<Entity> = scene
        .parents
        .iter()
        .map(|_| world.spawn_empty().id())
        .collect();
    world.insert_batch(
        scene
            .parents
            .iter()
            .zip(&entities)
            .map(|(&parent, &entity)| {
                let parent = if parent == u32::MAX {
                    root
                } else {
                    entities[parent as usize]
                };
                (entity, ChildOf(parent))
            }),
    );
    for column in &scene.columns {
        column(world, &entities);
    }
}

/// Saves the descendants of `root` to `path`, relative to `ASSET_DIR`. A `.ron` extension
/// picks the RON layout. Materials without a file are written next to the scene first.
pub fn save_scene(world: &mut World, root: Entity, path: &str) -> Result<()> {
    let ron = path.ends_with(".ron");
    let file = Path::new(crate::ASSET_DIR).join(path);
    let stem = path
        .rsplit('/')
        .next()
        .and_then(|name| name.split('.').next())
        .filter(|stem| !stem.is_empty())
        .context("the scene file has no name")?;

    // Parents before their children.
    let mut entities = Vec::new();
    let mut parents = Vec::new();
    let mut stack = vec![(root, u32::MAX)];
    while let Some((entity, parent)) = stack.pop() {
        let index = if entity == root {
            u32::MAX
        } else {
            entities.push(entity);
            parents.push(parent);
            entities.len() as u32 - 1
        };
        if let Some(children) = world.get::<Children>(entity) {
            stack.extend(children.iter().rev().map(|&child| (child, index)));
        }
    }

    let mut ctx = SaveCtx::default();
    let mut n = 0;
    for &entity in &entities {
        let Some(instance) = world.get::<crate::scene::Instance>(entity) else {
            continue;
        };
        let id = instance.material.id().untyped();
        if instance.material.path().is_some() || ctx.written.contains_key(&id) {
            continue;
        }
        let Some(material) = world
            .resource::<Assets<GpuMaterial>>()
            .get(&instance.material)
        else {
            continue;
        };
        let relative = format!("{stem}/materials/material_{n}.{MATERIAL_EXTENSION}");
        let material_file = file.parent().unwrap_or(Path::new("")).join(&relative);
        std::fs::create_dir_all(material_file.parent().unwrap())?;
        std::fs::write(&material_file, material.file().write(false)?)?;
        ctx.written.insert(id, relative);
        n += 1;
    }

    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry = registry.read();
    let mut columns: Vec<(bool, Column)> = Vec::new();
    for registration in registry.iter() {
        if let Some(data) = registration.data::<ReflectSceneComponent>() {
            let assets = ctx.assets.len();
            if let Some(column) = (data.save)(world, &entities, &mut ctx) {
                columns.push((ctx.assets.len() == assets, column));
            }
        }
    }
    // Columns that refer to assets go first, so their loads start early.
    columns.sort_by_key(|(no_assets, column)| (*no_assets, column.name.clone()));

    let scene = SceneData {
        assets: ctx.assets,
        parents,
        columns: columns.into_iter().map(|(_, column)| column).collect(),
    };
    let mut bytes = Vec::new();
    scene.write(&mut bytes, ron)?;
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&file, bytes)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::{
        app::App,
        asset::{AssetApp, AssetPlugin, AssetServer, LoadState},
        ecs::name::Name,
        reflect::Reflect,
        tasks::{IoTaskPool, TaskPool},
        transform::components::Transform,
    };
    use glam::Vec3;

    #[derive(Component, Clone, Serialize, Deserialize, Reflect, PartialEq, Debug)]
    #[reflect(SceneComponent)]
    struct Health(u32);

    fn scene() -> SceneData {
        let transforms = vec![
            Transform::from_xyz(0.0, 1.0, 0.0),
            Transform::from_scale(Vec3::splat(2.0)),
        ];
        SceneData {
            assets: vec![],
            parents: vec![u32::MAX, 0, 0],
            columns: vec![
                Column::new::<Transform>(vec![0, 2], transforms),
                Column::new::<Name>(vec![1], vec![Name::new("child")]),
                Column::new::<Health>(vec![0, 1, 2], vec![Health(3), Health(4), Health(5)]),
            ],
        }
    }

    /// Writes `scene` in both layouts, loads each and spawns it under a fresh root.
    #[test]
    fn both_layouts_round_trip() {
        IoTaskPool::get_or_init(TaskPool::new);
        let dir = std::env::temp_dir().join(format!("core-scene-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (file, ron) in [("a.scene", false), ("a.scene.ron", true)] {
            let mut bytes = Vec::new();
            scene().write(&mut bytes, ron).unwrap();
            std::fs::write(dir.join(file), bytes).unwrap();
        }

        let mut app = App::new();
        app.add_plugins((
            bevy::app::TaskPoolPlugin::default(),
            AssetPlugin {
                file_path: dir.to_string_lossy().into_owned(),
                ..Default::default()
            },
        ))
        .init_asset::<Scene>()
        .register_type::<Health>()
        .register_type_data::<Transform, ReflectSceneComponent>()
        .register_type_data::<Name, ReflectSceneComponent>()
        .init_asset_loader::<SceneLoader>();

        for file in ["a.scene", "a.scene.ron"] {
            let handle: Handle<Scene> = app.world().resource::<AssetServer>().load(file);
            while !matches!(
                app.world().resource::<AssetServer>().load_state(&handle),
                LoadState::Loaded | LoadState::Failed(_)
            ) {
                app.update();
            }
            let world = app.world_mut();
            let root = world.spawn_empty().id();
            world.resource_scope(|world, scenes: bevy::ecs::world::Mut<Assets<Scene>>| {
                spawn_scene(world, root, scenes.get(&handle).expect(file));
            });
            let children = world.get::<Children>(root).unwrap().to_vec();
            assert_eq!(children.len(), 1, "{file}");
            let top = children[0];
            let below = world.get::<Children>(top).unwrap().to_vec();
            assert_eq!(below.len(), 2, "{file}");
            assert_eq!(world.get::<Transform>(top).unwrap().translation.y, 1.0);
            assert_eq!(world.get::<Health>(top), Some(&Health(3)));
            assert_eq!(world.get::<Name>(below[0]).unwrap().as_str(), "child");
            assert!(world.get::<Transform>(below[0]).is_none());
            assert_eq!(
                world.get::<Transform>(below[1]).unwrap().scale,
                Vec3::splat(2.0)
            );
            assert_eq!(world.get::<Health>(below[1]), Some(&Health(5)));
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn ron_lists_components_per_entity() {
        let mut bytes = Vec::new();
        scene().write(&mut bytes, true).unwrap();
        let ron: RonScene = ron::de::from_bytes(&bytes).unwrap();
        assert_eq!(ron.entities.len(), 3);
        assert_eq!(ron.entities[1].parent, Some(0));
        assert_eq!(ron.entities[0].parent, None);
        let names: Vec<_> = ron.entities[0].components.keys().cloned().collect();
        assert_eq!(names, ["Health", "Transform"]);
        let name: String = ron.entities[1].components["Name"].into_rust().unwrap();
        assert_eq!(name, "child");
    }
}

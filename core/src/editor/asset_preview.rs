//! Asset browser previews: names and texture previews read straight from baked scene files into fixed-size caches and one atlas.
use std::{
    collections::{HashMap, HashSet},
    hash::Hash,
    path::{Path, PathBuf},
    sync::{
        Arc,
        mpsc::{Receiver, Sender, channel},
    },
};

use anyhow::Result;
use bevy::{
    asset::{
        AssetServer,
        io::{AssetSourceId, Reader},
    },
    tasks::IoTaskPool,
};
use glam::{UVec2, Vec2};
use lava::{
    bindless::BindlessHandle,
    image::{Image, format::R8G8B8A8Srgb, usage::Sampled},
};
use tracing::warn;

use crate::assets::{
    mesh::{BrowseInfo, read_browse_info, read_preview},
    texture::PREVIEW_SIZE,
};

/// Scene files whose names stay cached.
const CACHED_FILES: usize = 16;
/// Previews per atlas row and column.
const ATLAS_CELLS: u32 = 16;
const ATLAS_SIZE: u32 = ATLAS_CELLS * PREVIEW_SIZE;

/// A fixed number of slots holding the most recently used keys. When all are taken, a new key
/// replaces the one that went unused for longest.
struct SlotCache<K, V> {
    slots: Vec<Option<Slot<K, V>>>,
    lookup: HashMap<K, usize>,
    tick: u64,
}

struct Slot<K, V> {
    key: K,
    value: V,
    last_used: u64,
}

impl<K: Hash + Eq + Clone, V> SlotCache<K, V> {
    fn new(capacity: usize) -> Self {
        Self {
            slots: (0..capacity).map(|_| None).collect(),
            lookup: HashMap::new(),
            tick: 0,
        }
    }

    /// The slot and value of `key`, which counts as a use.
    fn get(&mut self, key: &K) -> Option<(usize, &V)> {
        let index = *self.lookup.get(key)?;
        self.tick += 1;
        let slot = self.slots[index].as_mut()?;
        slot.last_used = self.tick;
        Some((index, &slot.value))
    }

    /// Stores `value` under `key` and returns its slot: the one `key` already has, else a free
    /// one, else that of the key unused for longest.
    fn insert(&mut self, key: K, value: V) -> usize {
        self.tick += 1;
        let index = self.lookup.get(&key).copied().unwrap_or_else(|| {
            let last_used = |slot: &Option<Slot<K, V>>| slot.as_ref().map(|s| s.last_used);
            (0..self.slots.len())
                .min_by_key(|index| last_used(&self.slots[*index]))
                .expect("a slot cache needs at least one slot")
        });
        let slot = Slot {
            key: key.clone(),
            value,
            last_used: self.tick,
        };
        if let Some(old) = self.slots[index].replace(slot) {
            self.lookup.remove(&old.key);
        }
        self.lookup.insert(key, index);
        index
    }

    /// Frees the slots of all keys that don't pass `keep`.
    fn retain(&mut self, keep: impl Fn(&K) -> bool) {
        for slot in &mut self.slots {
            if slot.as_ref().is_some_and(|slot| !keep(&slot.key)) {
                *slot = None;
            }
        }
        self.lookup.retain(|key, _| keep(key));
    }
}

/// What is known about a scene file while browsing it.
pub(crate) enum FileState {
    Loading,
    Failed,
    Ready(Arc<BrowseInfo>),
}

/// Where a preview sits in the atlas.
#[derive(Clone, Copy)]
pub(crate) struct AtlasCell {
    pub image: BindlessHandle,
    pub uv_min: Vec2,
    pub uv_size: Vec2,
}

#[derive(Clone, PartialEq, Eq, Hash)]
enum Request {
    Info(PathBuf),
    Preview(PathBuf, u32),
}

enum Loaded {
    Info(BrowseInfo),
    Preview(Vec<u8>),
}

/// Everything the asset browser shows of scene files without loading them: the names of their
/// meshes and textures, and the texture previews in one atlas. Both are bounded, the least
/// recently shown entries make room for new ones.
pub(crate) struct Previews {
    infos: SlotCache<PathBuf, Arc<BrowseInfo>>,
    cells: SlotCache<(PathBuf, u32), ()>,
    /// Created with the first preview.
    atlas: Option<Image<R8G8B8A8Srgb, Sampled>>,
    /// Requests a task is working on.
    reading: HashSet<Request>,
    /// Requests that are not retried until the file is forgotten.
    failed: HashSet<Request>,
    sender: Sender<(Request, Result<Loaded>)>,
    receiver: Receiver<(Request, Result<Loaded>)>,
}

impl Default for Previews {
    fn default() -> Self {
        let (sender, receiver) = channel();
        Self {
            infos: SlotCache::new(CACHED_FILES),
            cells: SlotCache::new((ATLAS_CELLS * ATLAS_CELLS) as usize),
            atlas: None,
            reading: HashSet::new(),
            failed: HashSet::new(),
            sender,
            receiver,
        }
    }
}

impl Previews {
    /// Takes in what the read tasks finished since the last call.
    pub fn poll(&mut self) {
        while let Ok((request, loaded)) = self.receiver.try_recv() {
            self.reading.remove(&request);
            let loaded = match loaded {
                Ok(loaded) => loaded,
                Err(err) => {
                    warn!(%err, "failed to read asset preview data");
                    self.failed.insert(request);
                    continue;
                }
            };
            match (request, loaded) {
                (Request::Info(path), Loaded::Info(info)) => {
                    self.infos.insert(path, Arc::new(info));
                }
                (Request::Preview(path, index), Loaded::Preview(pixels)) => {
                    if let Err(err) = self.upload(path, index, &pixels) {
                        warn!(%err, "failed to upload an asset preview");
                    }
                }
                _ => {}
            }
        }
    }

    /// Writes a preview into the atlas cell it gets assigned.
    fn upload(&mut self, path: PathBuf, index: u32, pixels: &[u8]) -> lava::error::Result<()> {
        let atlas = match &mut self.atlas {
            Some(atlas) => atlas,
            None => self.atlas.insert(Image::new(ATLAS_SIZE, ATLAS_SIZE)?),
        };
        let key = (path, index);
        let cell = self.cells.insert(key.clone(), ());
        // Other texels would be shown as this preview, so the cell is given up again.
        atlas
            .copy_region_from(pixels, 0, cell_origin(cell), UVec2::splat(PREVIEW_SIZE))
            .inspect_err(|_| self.cells.retain(|other| *other != key))
    }

    /// The names in the scene file at `path` (relative to the asset directory). Starts reading
    /// them if they are not cached.
    pub fn info(&mut self, server: &AssetServer, path: &Path) -> FileState {
        if let Some((_, info)) = self.infos.get(&path.to_path_buf()) {
            return FileState::Ready(info.clone());
        }
        let request = Request::Info(path.to_path_buf());
        if self.failed.contains(&request) {
            return FileState::Failed;
        }
        if !self.reading.contains(&request) {
            self.request(server, request, None);
        }
        FileState::Loading
    }

    /// The atlas cell with the preview of texture `index`. Starts reading the preview if it
    /// is not in the atlas, in which case there is nothing to draw yet.
    pub fn preview(
        &mut self,
        server: &AssetServer,
        path: &Path,
        info: &Arc<BrowseInfo>,
        index: u32,
    ) -> Option<AtlasCell> {
        let key = (path.to_path_buf(), index);
        if let Some((cell, _)) = self.cells.get(&key) {
            let origin = cell_origin(cell).as_vec2();
            // Half a texel in from the cell border, so filtering doesn't reach the neighbours.
            return Some(AtlasCell {
                image: self.atlas.as_ref()?.handle,
                uv_min: (origin + 0.5) / ATLAS_SIZE as f32,
                uv_size: Vec2::splat((PREVIEW_SIZE - 1) as f32 / ATLAS_SIZE as f32),
            });
        }
        let request = Request::Preview(key.0, index);
        if !self.reading.contains(&request) && !self.failed.contains(&request) {
            self.request(server, request, Some(info.clone()));
        }
        None
    }

    /// Drops everything cached of the file at `path`, so it is read again.
    pub fn forget(&mut self, path: &Path) {
        self.infos.retain(|other| other != path);
        self.cells.retain(|(other, _)| other != path);
        self.failed.retain(|request| match request {
            Request::Info(other) | Request::Preview(other, _) => other != path,
        });
    }

    fn request(&mut self, server: &AssetServer, request: Request, info: Option<Arc<BrowseInfo>>) {
        self.reading.insert(request.clone());
        let server = server.clone();
        let sender = self.sender.clone();
        IoTaskPool::get()
            .spawn(async move {
                let loaded = read(&server, &request, info.as_deref()).await;
                // The browser may be gone by now.
                let _ = sender.send((request, loaded));
            })
            .detach();
    }
}

/// Top-left texel of atlas cell `cell`.
fn cell_origin(cell: usize) -> UVec2 {
    let cell = cell as u32;
    UVec2::new(cell % ATLAS_CELLS, cell / ATLAS_CELLS) * PREVIEW_SIZE
}

/// Reads only what `request` asks for from the baked file. The processed reader waits for
/// the asset processor, so a file that is still being imported is not read half-written.
async fn read(
    server: &AssetServer,
    request: &Request,
    info: Option<&BrowseInfo>,
) -> Result<Loaded> {
    let (Request::Info(path) | Request::Preview(path, _)) = request;
    let source = server.get_source(AssetSourceId::Default)?;
    let mut reader = source.processed_reader()?.read(path).await?;
    let reader: &mut dyn Reader = &mut *reader;
    match (request, info) {
        (Request::Preview(_, index), Some(info)) => Ok(Loaded::Preview(
            read_preview(reader, info, *index as usize).await?,
        )),
        _ => Ok(Loaded::Info(read_browse_info(reader).await?)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_slots_fill_first() {
        let mut cache = SlotCache::new(3);
        assert_eq!(cache.insert("a", 1), 0);
        assert_eq!(cache.insert("b", 2), 1);
        assert_eq!(cache.insert("c", 3), 2);
        assert_eq!(cache.get(&"b"), Some((1, &2)));
        assert_eq!(cache.get(&"d"), None);
    }

    #[test]
    fn the_longest_unused_key_makes_room() {
        let mut cache = SlotCache::new(2);
        cache.insert("a", 1);
        cache.insert("b", 2);
        // "a" was used after "b", so "b" goes.
        cache.get(&"a");
        assert_eq!(cache.insert("c", 3), 1);
        assert_eq!(cache.get(&"b"), None);
        assert_eq!(cache.get(&"a"), Some((0, &1)));
        assert_eq!(cache.get(&"c"), Some((1, &3)));
        // Now "a" is the oldest.
        assert_eq!(cache.insert("d", 4), 0);
        assert_eq!(cache.get(&"a"), None);
    }

    #[test]
    fn inserting_a_cached_key_keeps_its_slot() {
        let mut cache = SlotCache::new(2);
        cache.insert("a", 1);
        cache.insert("b", 2);
        assert_eq!(cache.insert("a", 5), 0);
        assert_eq!(cache.get(&"a"), Some((0, &5)));
        assert_eq!(cache.get(&"b"), Some((1, &2)));
    }

    #[test]
    fn retain_frees_slots() {
        let mut cache = SlotCache::new(2);
        cache.insert("a", 1);
        cache.insert("b", 2);
        cache.retain(|key| *key != "a");
        assert_eq!(cache.get(&"a"), None);
        assert_eq!(cache.insert("c", 3), 0);
        assert_eq!(cache.get(&"b"), Some((1, &2)));
    }

    #[test]
    fn cells_tile_the_atlas() {
        assert_eq!(cell_origin(0), UVec2::ZERO);
        assert_eq!(cell_origin(1), UVec2::new(PREVIEW_SIZE, 0));
        assert_eq!(
            cell_origin(ATLAS_CELLS as usize),
            UVec2::new(0, PREVIEW_SIZE)
        );
        let last = (ATLAS_CELLS * ATLAS_CELLS - 1) as usize;
        assert_eq!(
            cell_origin(last) + PREVIEW_SIZE,
            UVec2::splat(ATLAS_SIZE),
            "the last cell ends at the atlas corner"
        );
    }
}

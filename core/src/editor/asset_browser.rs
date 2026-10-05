//! Editor asset browser: directory grid with draggable assets that load when dropped, and dropping them into the viewport.
use std::{
    any::TypeId,
    collections::HashMap,
    fs,
    ops::Range,
    path::{Path, PathBuf},
    sync::mpsc::{Receiver, channel},
    time::Instant,
};

use bevy::{
    asset::{Asset, AssetPath, AssetServer, Assets},
    ecs::{
        entity::Entity,
        name::Name,
        query::With,
        system::{Commands, Local, Query, Res, ResMut, Single},
    },
    input::{ButtonInput, mouse::MouseButton, touch::Touches},
    math::{Dir3A, Rect, bounding::RayCast3d},
    transform::components::{GlobalTransform, Transform},
    window::Window,
};
use bytemuck::Zeroable;
use glam::{UVec2, Vec2, Vec4};
use lava::{
    bindless::BindlessHandle,
    image::{Image, format::R8G8B8A8Srgb, usage::Sampled},
};
use notify::{EventKind, RecursiveMode, Watcher};
use tracing::warn;

use crate::{
    ASSET_DIR,
    assets::{
        mesh::{GpuMesh, MESH_EXTENSION, MaterialSet, SCENE_EXTENSION, Scene},
        texture::{GpuTexture, PREVIEW_SIZE, TEXTURE_EXTENSION, read_preview},
    },
    editor::{picking::Selected, viewport::ViewPort},
    physics::bvh::Raycast,
    render::world::InstanceFlags,
    scene::{Instance, MaterialSettings, SpawnScene, camera::Camera},
    ui::{
        MultiInput, UiContext,
        builder::{UiBuilder, UiWindowBuilder},
        dragdrop::{AssetDrag, DragDrop},
        from_pos_size,
        window::{BorderSettings, DrawSettings, Drawable},
    },
};

const TILE_SIZE: Vec2 = Vec2::new(180.0, 120.0);
const TILE_PAD: f32 = 8.0;
const BADGE_HEIGHT: f32 = 72.0;
const DROP_DISTANCE: f32 = 5.0;
/// Previews per atlas row and column.
const ATLAS_CELLS: u32 = 32;
const ATLAS_SIZE: u32 = ATLAS_CELLS * PREVIEW_SIZE;

const FOLDER_COLOR: Vec4 = Vec4::new(0.843, 0.600, 0.129, 1.0);
const SCENE_COLOR: Vec4 = Vec4::new(0.118, 0.565, 0.831, 1.0);
const MESH_COLOR: Vec4 = Vec4::new(0.557, 0.753, 0.486, 1.0);
const TEXTURE_COLOR: Vec4 = Vec4::new(0.827, 0.525, 0.608, 1.0);

/// The drag payload for the `A` at `path`. Nothing is loaded until it is dropped.
fn drag<A: Asset>(server: &AssetServer, path: AssetPath<'static>) -> AssetDrag {
    let server = server.clone();
    AssetDrag::new(TypeId::of::<A>(), move || server.load::<A>(path).untyped())
}

struct AssetKind {
    extension: &'static str,
    badge: &'static str,
    color: Vec4,
    drag: fn(&AssetServer, AssetPath<'static>) -> AssetDrag,
}

const ASSET_KINDS: &[AssetKind] = &[
    AssetKind {
        extension: SCENE_EXTENSION,
        badge: "SCENE",
        color: SCENE_COLOR,
        drag: drag::<Scene>,
    },
    AssetKind {
        extension: MESH_EXTENSION,
        badge: "MESH",
        color: MESH_COLOR,
        drag: drag::<GpuMesh>,
    },
    AssetKind {
        extension: TEXTURE_EXTENSION,
        badge: "TEX",
        color: TEXTURE_COLOR,
        drag: drag::<GpuTexture>,
    },
];

fn kind_of(file_name: &str) -> Option<&'static AssetKind> {
    let extension = Path::new(file_name).extension()?.to_str()?;
    ASSET_KINDS
        .iter()
        .find(|kind| kind.extension.eq_ignore_ascii_case(extension))
}

struct DirEntry {
    name: String,
    is_dir: bool,
}

enum Nav {
    /// To a directory, relative to `ASSET_DIR`.
    To(PathBuf),
    Back,
}

/// The previews of the texture files the browser shows, read without loading the textures and
/// kept in one atlas. The least recently shown make room for new ones.
struct Previews {
    /// Created with the first preview.
    atlas: Option<Image<R8G8B8A8Srgb, Sampled>>,
    /// The atlas cell of a texture file, by its path relative to `ASSET_DIR`.
    cells: HashMap<PathBuf, usize>,
    /// The tick each cell was last shown at, 0 for a free one.
    last_used: Vec<u64>,
    tick: u64,
}

impl Default for Previews {
    fn default() -> Self {
        Self {
            atlas: None,
            cells: HashMap::new(),
            last_used: vec![0; (ATLAS_CELLS * ATLAS_CELLS) as usize],
            tick: 0,
        }
    }
}

impl Previews {
    /// The cell of `path`, which counts as showing it.
    fn cell(&mut self, path: &Path) -> Option<usize> {
        let cell = *self.cells.get(path)?;
        self.tick += 1;
        self.last_used[cell] = self.tick;
        Some(cell)
    }

    /// Gives `path` a free cell, else the one that was not shown for longest.
    fn take_cell(&mut self, path: &Path) -> usize {
        let cell = (0..self.last_used.len())
            .min_by_key(|cell| self.last_used[*cell])
            .expect("the atlas has cells");
        self.cells.retain(|_, taken| *taken != cell);
        self.cells.insert(path.to_path_buf(), cell);
        self.tick += 1;
        self.last_used[cell] = self.tick;
        cell
    }

    /// Drops the preview of the file at `path`, so it is read again when it is next shown.
    fn forget(&mut self, path: &Path) {
        if let Some(cell) = self.cells.remove(path) {
            self.last_used[cell] = 0;
        }
    }

    /// The atlas image and the UV origin and size of the preview of the texture file at
    /// `path`, which is read from the file the first time.
    fn preview(&mut self, path: &Path) -> Option<(BindlessHandle, Vec2, Vec2)> {
        let origin = |cell: usize| {
            UVec2::new(cell as u32 % ATLAS_CELLS, cell as u32 / ATLAS_CELLS) * PREVIEW_SIZE
        };
        let cell = match self.cell(path) {
            Some(cell) => cell,
            None => {
                let file = fs::File::open(Path::new(ASSET_DIR).join(path)).ok()?;
                let pixels = read_preview(file).ok()?;
                if self.atlas.is_none() {
                    self.atlas = Some(Image::new(ATLAS_SIZE, ATLAS_SIZE).ok()?);
                }
                let cell = self.take_cell(path);
                let size = UVec2::splat(PREVIEW_SIZE);
                if let Err(err) =
                    (self.atlas.as_mut()?).copy_region_from(&pixels, 0, origin(cell), size)
                {
                    warn!(%err, "failed to upload an asset preview");
                    self.forget(path);
                    return None;
                }
                cell
            }
        };
        // Half a texel in from the cell border, so filtering doesn't reach the neighbours.
        Some((
            self.atlas.as_ref()?.handle,
            (origin(cell).as_vec2() + 0.5) / ATLAS_SIZE as f32,
            Vec2::splat((PREVIEW_SIZE - 1) as f32 / ATLAS_SIZE as f32),
        ))
    }
}

pub(crate) struct BrowserState {
    /// The directory the grid shows, relative to `ASSET_DIR`.
    location: PathBuf,
    history: Vec<PathBuf>,
    /// The entries of `location`, read when it is opened or changes on disk.
    entries: Vec<DirEntry>,
    previews: Previews,
    /// Sends the paths (relative to `ASSET_DIR`) that changed on disk. Without it the
    /// directory is still read when it is opened.
    watcher: Option<(notify::RecommendedWatcher, Receiver<PathBuf>)>,
}

impl Default for BrowserState {
    fn default() -> Self {
        let (sender, changes) = channel();
        let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            // Reading a file is an event too, and must not make us read it again.
            let Ok(event) = event else {
                return;
            };
            if matches!(event.kind, EventKind::Access(_)) {
                return;
            }
            for path in &event.paths {
                if let Ok(path) = path.strip_prefix(ASSET_DIR) {
                    // The browser may be gone by now.
                    let _ = sender.send(path.to_path_buf());
                }
            }
        })
        .and_then(|mut watcher| {
            watcher.watch(Path::new(ASSET_DIR), RecursiveMode::Recursive)?;
            Ok((watcher, changes))
        })
        .inspect_err(|err| warn!(%err, "the asset browser can't watch for file changes"))
        .ok();
        let mut state = Self {
            location: PathBuf::new(),
            history: Vec::new(),
            entries: Vec::new(),
            previews: Previews::default(),
            watcher,
        };
        state.read_dir();
        state
    }
}

impl BrowserState {
    /// Reads the entries of `location`, folders first.
    fn read_dir(&mut self) {
        self.entries.clear();
        let Ok(read_dir) = fs::read_dir(Path::new(ASSET_DIR).join(&self.location)) else {
            return;
        };
        self.entries.extend(read_dir.filter_map(|entry| {
            let entry = entry.ok()?;
            Some(DirEntry {
                name: entry.file_name().into_string().ok()?,
                is_dir: entry.path().is_dir(),
            })
        }));
        self.entries
            .sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.cmp(&b.name)));
    }

    fn navigate(&mut self, nav: Nav) {
        match nav {
            Nav::Back => {
                let Some(location) = self.history.pop() else {
                    return;
                };
                self.location = location;
            }
            Nav::To(location) => {
                if location == self.location {
                    return;
                }
                let previous = std::mem::replace(&mut self.location, location);
                self.history.push(previous);
            }
        }
        self.read_dir();
    }

    /// Takes in what the watcher saw since the last call.
    fn apply_changes(&mut self) {
        let Some((_, changes)) = &self.watcher else {
            return;
        };
        let mut listing_changed = false;
        while let Ok(path) = changes.try_recv() {
            // A file that was baked again has a new preview.
            self.previews.forget(&path);
            listing_changed |= path.parent() == Some(self.location.as_path());
        }
        if listing_changed {
            self.read_dir();
        }
    }
}

/// The part of `name` to draw so that it takes at most `max_chars` characters, and whether
/// it was cut. A cut name is followed by "..", which the kept part leaves room for.
fn fit_name(name: &str, max_chars: usize) -> (&str, bool) {
    if name.chars().count() <= max_chars {
        return (name, false);
    }
    let end = name
        .char_indices()
        .nth(max_chars.saturating_sub(2))
        .map_or(name.len(), |(index, _)| index);
    (&name[..end], true)
}

/// The rows of a grid starting at `top` that reach into `clip_min..clip_max`. A row and the
/// gap below it are `stride` high.
fn visible_rows(top: f32, clip_min: f32, clip_max: f32, stride: f32, rows: usize) -> Range<usize> {
    let row = |y: f32| ((y - top) / stride).max(0.0);
    let start = (row(clip_min).floor() as usize).min(rows);
    let end = (row(clip_max).ceil() as usize).clamp(start, rows);
    start..end
}

/// Draws one tile of the grid. `preview` is drawn in place of the badge.
fn draw_tile(
    ui: &mut UiWindowBuilder,
    name: &str,
    badge_text: &str,
    badge_color: Vec4,
    dim: bool,
    hovered: bool,
    preview: Option<(BindlessHandle, Vec2, Vec2)>,
) {
    let mut ds = DrawSettings::new(hovered, false);
    if hovered {
        ds = ds.border_color(UiContext::GRAB_HOT);
    }
    ui.rect(TILE_SIZE, ds);

    let rect = ui.prev_element;
    let clip = rect.intersect(ui.clip_rect);
    let viewport_size = ui.ctx.viewport_size;
    let char_width = UiContext::text_len(" ");
    let centered = |chars: usize, y: f32| {
        let width = chars as f32 * char_width;
        Vec2::new(rect.center().x - width / 2.0, y).round()
    };

    let badge = from_pos_size(
        rect.min + Vec2::splat(TILE_PAD),
        Vec2::new(TILE_SIZE.x - TILE_PAD * 2.0, BADGE_HEIGHT),
    );
    if let Some((image, uv_min, uv_size)) = preview {
        let square = Rect::from_center_size(badge.center(), Vec2::splat(BADGE_HEIGHT));
        ui.ctx.window.draw_rect(
            square,
            Some((uv_min, uv_size)),
            Vec4::ONE,
            viewport_size,
            clip,
            false,
            image,
        );
    } else {
        ui.ctx.window.draw_box(
            badge,
            DrawSettings {
                color: badge_color.with_w(if dim { 0.4 } else { 1.0 }),
                border: Some(BorderSettings::uniform(
                    UiContext::BG_DARK,
                    UiContext::BORDER,
                )),
                ..Default::default()
            },
            viewport_size,
            clip,
        );
        ui.ctx.window.draw_text(
            centered(
                badge_text.chars().count(),
                badge.center().y - UiContext::ATLAS_CELL_SIZE.y as f32 / 2.0,
            ),
            UiContext::BG_DARK,
            badge_text,
            viewport_size,
            clip,
            false,
        );
    }

    let max_chars = ((TILE_SIZE.x - TILE_PAD) / char_width) as usize;
    let (name, cut) = fit_name(name, max_chars);
    let chars = name.chars().count() + if cut { 2 } else { 0 };
    let color = if dim {
        UiContext::TEXT_DIM
    } else {
        UiContext::TEXT
    };
    let end = ui.ctx.window.draw_text(
        centered(chars, badge.max.y + TILE_PAD / 2.0),
        color,
        name,
        viewport_size,
        clip,
        false,
    );
    if cut {
        ui.ctx
            .window
            .draw_text(end, color, "..", viewport_size, clip, false);
    }
}

/// Text on a row of buttons, lowered so both share a baseline.
fn row_text(ui: &mut UiWindowBuilder, text: &str) {
    let offset = UiWindowBuilder::child_offset().y;
    ui.cursor.y += offset;
    ui.text(text);
    ui.cursor.y -= offset;
}

pub(crate) fn asset_browser(
    mut ui: UiBuilder,
    asset_server: Res<AssetServer>,
    mut state: Local<BrowserState>,
) {
    let start = Instant::now();
    let state = &mut *state;
    state.apply_changes();

    let count = state.entries.len();

    let mut nav = None;
    ui.build("Asset Browser", |ui| {
        ui.horizontal();
        ui.disabled(state.history.is_empty());
        if ui.button("<") && !state.history.is_empty() {
            nav = Some(Nav::Back);
        }
        ui.disabled(false);
        if ui.button("assets") {
            nav = Some(Nav::To(PathBuf::new()));
        }
        for (depth, segment) in state.location.iter().enumerate() {
            row_text(ui, "/");
            if ui.button(segment.to_string_lossy()) {
                nav = Some(Nav::To(state.location.iter().take(depth + 1).collect()));
            }
        }
        ui.vertical();

        if count == 0 {
            ui.colored_text("Empty", UiContext::TEXT_DIM);
            return;
        }

        let gap = UiContext::ELEMENT_GAP.as_vec2();
        let columns = (((ui.remaining_width() + gap.x) / (TILE_SIZE.x + gap.x)) as usize).max(1);
        let rows = count.div_ceil(columns);
        let stride = TILE_SIZE.y + gap.y;
        let top = ui.cursor;
        // Rows scrolled out of view only take up their space.
        let visible = visible_rows(top.y, ui.clip_rect.min.y, ui.clip_rect.max.y, stride, rows);
        ui.cursor.y += visible.start as f32 * stride;
        for row in visible.clone() {
            ui.horizontal();
            for index in row * columns..((row + 1) * columns).min(count) {
                let entry = &state.entries[index];
                let kind = kind_of(&entry.name).filter(|_| !entry.is_dir);
                let (badge, color) = match kind {
                    _ if entry.is_dir => ("DIR", FOLDER_COLOR),
                    Some(kind) => (kind.badge, kind.color),
                    None => ("?", UiContext::GRAB),
                };
                // Relative to `ASSET_DIR`, and only put together when it is needed.
                let path = || state.location.join(&entry.name);
                let rect = from_pos_size(ui.cursor, TILE_SIZE);
                let hovered = ui.hoverd(rect);
                // Only tiles on screen ask for their preview, so the atlas fills as you scroll.
                let preview = kind
                    .filter(|kind| kind.extension == TEXTURE_EXTENSION)
                    .filter(|_| !rect.intersect(ui.clip_rect).is_empty())
                    .and_then(|_| state.previews.preview(&path()));
                let dim = kind.is_none() && !entry.is_dir;
                let draw = |ui: &mut UiWindowBuilder| {
                    draw_tile(ui, &entry.name, badge, color, dim, hovered, preview)
                };
                match kind {
                    Some(kind) => ui.drag_source(
                        index,
                        || (kind.drag)(&asset_server, path().into()),
                        draw,
                        |ui| ui.text(&entry.name),
                    ),
                    None => draw(ui),
                }
                if entry.is_dir && hovered && ui.ctx.input.primary_pressed {
                    nav = Some(Nav::To(path()));
                }
            }
            ui.vertical();
        }
        ui.cursor.y += (rows - visible.end) as f32 * stride;
        let size = Vec2::new(
            columns.min(count) as f32 * (TILE_SIZE.x + gap.x) - gap.x,
            rows as f32 * stride - gap.y,
        );
        ui.content_max = ui.content_max.max(top + size);
    });

    if let Some(nav) = nav {
        state.navigate(nav);
    }
    tracing::info!("{:#?}", start.elapsed());
}

/// Spawns an asset released over the 3D viewport at the point under the cursor.
pub(crate) fn drop_in_viewport(
    mut cmd: Commands,
    dnd: Res<DragDrop>,
    viewport: Res<ViewPort>,
    window: Single<&Window>,
    mouse: Res<ButtonInput<MouseButton>>,
    touches: Res<Touches>,
    camera: Single<(&Camera, &GlobalTransform)>,
    raycast: Raycast,
    selected: Query<Entity, With<Selected>>,
    mut material_sets: ResMut<Assets<MaterialSet>>,
    textures: Res<Assets<GpuTexture>>,
) {
    if !dnd.active() {
        return;
    }
    let input = MultiInput::new(&window, &mouse, &touches);
    let Some(cursor_pos) = input.cursor_pos.filter(|_| input.primary_released) else {
        return;
    };
    // `hovered` is false while another window covers the viewport: that drop is theirs.
    if !viewport.hovered || !viewport.rect.contains(cursor_pos) {
        return;
    }

    let (camera, camera_transform) = *camera;
    let origin = camera_transform.translation();
    let direction = camera.ray_direction(
        camera_transform,
        cursor_pos - viewport.rect.min,
        viewport.rect.size().as_uvec2(),
    );
    let ray = RayCast3d::new(
        origin,
        Dir3A::new(direction.to_vec3a()).unwrap_or(Dir3A::Z),
        1000.0,
    );
    let distance = raycast.raycast(&ray).map_or(DROP_DISTANCE, |hit| hit.t);
    let transform = Transform::from_translation(origin + direction * distance);

    let entity = if let Some(scene) = dnd.take_asset::<Scene>() {
        let name = scene
            .path()
            .and_then(|path| path.path().file_stem())
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Scene".to_string());
        cmd.spawn((transform, Name::new(name), SpawnScene { scene }))
            .id()
    } else if let Some(mesh) = dnd.take_asset::<GpuMesh>() {
        // A mesh on its own has no scene to take a material from.
        let material = MaterialSettings::default().into_material(Zeroable::zeroed());
        let set = MaterialSet::new(&[material], vec![Default::default()]);
        set.resolve_textures(&textures);
        cmd.spawn((
            transform,
            Instance {
                mesh,
                material_set: material_sets.add(set),
                material_index: 0,
                flags: InstanceFlags::empty(),
            },
        ))
        .id()
    } else {
        return;
    };

    for e in &selected {
        cmd.entity(e).remove::<Selected>();
    }
    cmd.entity(entity).insert(Selected);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_names_are_cut() {
        assert_eq!(fit_name("box.glb", 12), ("box.glb", false));
        // Ten characters, which leaves room for the "..".
        assert_eq!(fit_name("stanford_bunny.glb", 12), ("stanford_b", true));
        assert_eq!(fit_name("äöüäöü", 4), ("äö", true));
    }

    #[test]
    fn kinds_match_on_extension() {
        assert_eq!(kind_of("box.scene").map(|k| k.badge), Some("SCENE"));
        assert_eq!(kind_of("BOX.SCENE").map(|k| k.badge), Some("SCENE"));
        assert_eq!(kind_of("Cube.1.mesh").map(|k| k.badge), Some("MESH"));
        assert_eq!(kind_of("wood.tex").map(|k| k.badge), Some("TEX"));
        assert!(kind_of("notes.txt").is_none());
        assert!(kind_of("scene").is_none());
    }

    #[test]
    fn only_rows_in_view_are_visible() {
        // Rows of 100 with the grid starting at 50: row 0 is 50..150, row 1 is 150..250, ...
        assert_eq!(visible_rows(50.0, 0.0, 1000.0, 100.0, 3), 0..3);
        assert_eq!(visible_rows(50.0, 0.0, 120.0, 100.0, 10), 0..1);
        // Scrolled up by 300.
        assert_eq!(visible_rows(-250.0, 0.0, 120.0, 100.0, 10), 2..4);
        // Scrolled past the end, or not reached yet.
        assert_eq!(visible_rows(-5000.0, 0.0, 120.0, 100.0, 10), 10..10);
        assert_eq!(visible_rows(500.0, 0.0, 120.0, 100.0, 10), 0..0);
        // A window with no room for content.
        assert_eq!(visible_rows(50.0, 80.0, 20.0, 100.0, 10), 0..0);
    }

    #[test]
    fn the_least_recently_shown_preview_makes_room() {
        let mut previews = Previews {
            last_used: vec![0; 2],
            ..Default::default()
        };
        let [a, b, c, d] = ["a.tex", "b.tex", "c.tex", "d.tex"].map(Path::new);
        assert_eq!(previews.take_cell(a), 0);
        assert_eq!(previews.take_cell(b), 1);
        // "a" was shown after "b", so "b" goes.
        assert_eq!(previews.cell(a), Some(0));
        assert_eq!(previews.take_cell(c), 1);
        assert_eq!(previews.cell(b), None);
        // A forgotten preview frees its cell, even though "a" is the older one.
        previews.forget(c);
        assert_eq!(previews.take_cell(d), 1);
        assert_eq!(previews.cell(a), Some(0));
    }
}

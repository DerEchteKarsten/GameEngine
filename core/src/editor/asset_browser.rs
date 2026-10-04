//! Editor asset browser: directory grid with draggable assets, and dropping them into the viewport.
use std::{
    borrow::Cow,
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use bevy::{
    asset::{AssetServer, Assets, Handle, LoadState, UntypedHandle},
    ecs::{
        entity::Entity,
        name::Name,
        query::With,
        system::{Commands, Local, Query, Res, Single},
    },
    input::{ButtonInput, mouse::MouseButton, touch::Touches},
    math::{Dir3A, Rect, bounding::RayCast3d},
    transform::components::{GlobalTransform, Transform},
    window::Window,
};
use glam::{Vec2, Vec4};
use lava::bindless::BindlessHandle;

use crate::{
    ASSET_DIR,
    assets::{
        mesh::{GpuMesh, Scene},
        texture::GpuTexture,
    },
    editor::{picking::Selected, viewport::ViewPort},
    physics::bvh::Raycast,
    render::world::InstanceFlags,
    scene::{Instance, MaterialSettings, SpawnScene, camera::Camera},
    ui::{
        MultiInput, UiContext,
        builder::{UiBuilder, UiWindowBuilder},
        dragdrop::DragDrop,
        from_pos_size,
        window::{BorderSettings, DrawSettings, Drawable},
    },
};

const TILE_SIZE: Vec2 = Vec2::new(180.0, 120.0);
const TILE_PAD: f32 = 8.0;
const BADGE_HEIGHT: f32 = 72.0;
const REFRESH_INTERVAL: Duration = Duration::from_secs(1);
const DOUBLE_CLICK: Duration = Duration::from_millis(400);
const DROP_DISTANCE: f32 = 5.0;

const FOLDER_COLOR: Vec4 = Vec4::new(0.843, 0.600, 0.129, 1.0);
const SCENE_COLOR: Vec4 = Vec4::new(0.118, 0.565, 0.831, 1.0);
const MESH_COLOR: Vec4 = Vec4::new(0.557, 0.753, 0.486, 1.0);
const TEXTURE_COLOR: Vec4 = Vec4::new(0.827, 0.525, 0.608, 1.0);

struct AssetKind {
    extension: &'static str,
    badge: &'static str,
    color: Vec4,
    load: fn(&AssetServer, &Path) -> UntypedHandle,
    container: bool,
}

const ASSET_KINDS: &[AssetKind] = &[AssetKind {
    extension: "glb",
    badge: "GLB",
    color: SCENE_COLOR,
    load: |server, path| server.load::<Scene>(path.to_path_buf()).untyped(),
    container: true,
}];

fn kind_of(file_name: &str) -> Option<&'static AssetKind> {
    let extension = Path::new(file_name).extension()?.to_str()?;
    ASSET_KINDS
        .iter()
        .find(|kind| kind.extension.eq_ignore_ascii_case(extension))
}

/// What the grid shows: a directory, or the sub-assets of a scene file inside it. Paths are
/// relative to `ASSET_DIR`.
#[derive(Clone, Default, PartialEq)]
struct Location {
    dir: PathBuf,
    scene: Option<PathBuf>,
}

struct DirEntry {
    name: String,
    is_dir: bool,
}

enum Nav {
    To(Location),
    Back,
}

#[derive(Default)]
pub(crate) struct BrowserState {
    location: Location,
    history: Vec<Location>,
    entries: Vec<DirEntry>,
    last_read: Option<Instant>,
    /// Strong handles of everything opened or dragged, so repeated drops don't reload.
    handles: HashMap<PathBuf, UntypedHandle>,
    selected: Option<String>,
    last_click: Option<(Instant, String)>,
}

impl BrowserState {
    fn refresh(&mut self) {
        self.last_read = Some(Instant::now());
        self.entries = read_entries(&Path::new(ASSET_DIR).join(&self.location.dir));
    }

    fn navigate(&mut self, nav: Nav, asset_server: &AssetServer) {
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
        if let Some(scene) = &self.location.scene
            && let Some(kind) = scene.to_str().and_then(kind_of)
        {
            self.handles
                .entry(scene.clone())
                .or_insert_with(|| (kind.load)(asset_server, scene));
        }
        self.selected = None;
        self.last_click = None;
        self.refresh();
    }

    /// The tiles of the current location, or a status line when there is nothing to show.
    fn tiles(
        &self,
        asset_server: &AssetServer,
        scenes: &Assets<Scene>,
        textures: &Assets<GpuTexture>,
    ) -> Result<Vec<Tile>, &'static str> {
        let tiles = match &self.location.scene {
            Some(path) => {
                let handle = self.handles.get(path).ok_or("Not loaded")?;
                let scene = handle
                    .id()
                    .try_typed::<Scene>()
                    .ok()
                    .and_then(|id| scenes.get(id));
                let Some(scene) = scene else {
                    return Err(match asset_server.load_state(handle.id()) {
                        LoadState::Failed(_) => "Failed to load",
                        _ => "Loading...",
                    });
                };
                let meshes = scene.meshes.iter().enumerate().map(|(i, mesh)| Tile {
                    name: format!("mesh_{i}"),
                    badge: "MESH",
                    color: MESH_COLOR,
                    content: Content::Mesh(mesh.clone()),
                    preview: None,
                });
                let textures = scene.textures.iter().enumerate().map(|(i, texture)| Tile {
                    name: format!("texture_{i}"),
                    badge: "TEX",
                    color: TEXTURE_COLOR,
                    content: Content::Texture(texture.clone()),
                    preview: textures.get(texture).map(|texture| texture.handle()),
                });
                meshes.chain(textures).collect::<Vec<_>>()
            }
            None => self
                .entries
                .iter()
                .map(|entry| {
                    let path = self.location.dir.join(&entry.name);
                    let (badge, color, content) = if entry.is_dir {
                        ("DIR", FOLDER_COLOR, Content::Folder(path))
                    } else if let Some(kind) = kind_of(&entry.name) {
                        (kind.badge, kind.color, Content::File(kind, path))
                    } else {
                        ("?", UiContext::GRAB, Content::Unknown)
                    };
                    Tile {
                        name: entry.name.clone(),
                        badge,
                        color,
                        content,
                        preview: None,
                    }
                })
                .collect(),
        };
        if tiles.is_empty() {
            Err("Empty")
        } else {
            Ok(tiles)
        }
    }
}

fn read_entries(dir: &Path) -> Vec<DirEntry> {
    let Ok(read_dir) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut entries: Vec<DirEntry> = read_dir
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let name = entry.file_name().into_string().ok()?;
            if name.ends_with(".meta") {
                return None;
            }
            Some(DirEntry {
                name,
                is_dir: entry.path().is_dir(),
            })
        })
        .collect();
    sort_entries(&mut entries);
    entries
}

fn sort_entries(entries: &mut [DirEntry]) {
    entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.cmp(&b.name)));
}

/// `name` shortened to at most `max_chars` characters, ending in ".." when cut.
fn fit_name(name: &str, max_chars: usize) -> Cow<'_, str> {
    if name.chars().count() <= max_chars {
        return Cow::Borrowed(name);
    }
    let kept: String = name.chars().take(max_chars.saturating_sub(2)).collect();
    Cow::Owned(format!("{kept}.."))
}

enum Content {
    Folder(PathBuf),
    File(&'static AssetKind, PathBuf),
    Unknown,
    Mesh(Handle<GpuMesh>),
    Texture(Handle<GpuTexture>),
}

struct Tile {
    name: String,
    badge: &'static str,
    color: Vec4,
    content: Content,
    /// Image drawn in place of the badge, once it is loaded.
    preview: Option<BindlessHandle>,
}

impl Tile {
    fn draw(&self, ui: &mut UiWindowBuilder, hovered: bool, selected: bool) {
        let dim = matches!(self.content, Content::Unknown);
        let mut ds = DrawSettings::new(hovered, selected);
        if selected {
            ds = ds.border_color(UiContext::ACENT);
        } else if hovered {
            ds = ds.border_color(UiContext::GRAB_HOT);
        }
        ui.rect(TILE_SIZE, ds);

        let rect = ui.prev_element;
        let clip = rect.intersect(ui.clip_rect);
        let viewport_size = ui.ctx.viewport_size;
        let char_width = UiContext::text_len(" ");
        let centered = |text: &str, y: f32| {
            let width = text.chars().count() as f32 * char_width;
            Vec2::new(rect.center().x - width / 2.0, y).round()
        };

        let badge = from_pos_size(
            rect.min + Vec2::splat(TILE_PAD),
            Vec2::new(TILE_SIZE.x - TILE_PAD * 2.0, BADGE_HEIGHT),
        );
        if let Some(preview) = self.preview {
            let square = Rect::from_center_size(badge.center(), Vec2::splat(BADGE_HEIGHT));
            ui.ctx.window.draw_rect(
                square,
                Some((Vec2::ZERO, Vec2::ONE)),
                Vec4::ONE,
                viewport_size,
                clip,
                false,
                preview,
            );
        } else {
            ui.ctx.window.draw_box(
                badge,
                DrawSettings {
                    color: self.color.with_w(if dim { 0.4 } else { 1.0 }),
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
                    self.badge,
                    badge.center().y - UiContext::ATLAS_CELL_SIZE.y as f32 / 2.0,
                ),
                UiContext::BG_DARK,
                self.badge,
                viewport_size,
                clip,
                false,
            );
        }

        let max_chars = ((TILE_SIZE.x - TILE_PAD) / char_width) as usize;
        let name = fit_name(&self.name, max_chars);
        ui.ctx.window.draw_text(
            centered(&name, badge.max.y + TILE_PAD / 2.0),
            if dim {
                UiContext::TEXT_DIM
            } else {
                UiContext::TEXT
            },
            &name,
            viewport_size,
            clip,
            false,
        );
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
    scenes: Res<Assets<Scene>>,
    textures: Res<Assets<GpuTexture>>,
    mut state: Local<BrowserState>,
) {
    let state = &mut *state;
    if state
        .last_read
        .is_none_or(|read| read.elapsed() > REFRESH_INTERVAL)
    {
        state.refresh();
    }
    let tiles = state.tiles(&asset_server, &scenes, &textures);

    let mut nav = None;
    let mut refresh = false;
    ui.build("Asset Browser", |ui| {
        ui.horizontal();
        ui.disabled(state.history.is_empty());
        if ui.button("<") && !state.history.is_empty() {
            nav = Some(Nav::Back);
        }
        ui.disabled(false);
        if ui.button("assets") {
            nav = Some(Nav::To(Location::default()));
        }
        let mut dir = PathBuf::new();
        for segment in state.location.dir.iter() {
            dir.push(segment);
            row_text(ui, "/");
            if ui.button(segment.to_string_lossy()) {
                nav = Some(Nav::To(Location {
                    dir: dir.clone(),
                    scene: None,
                }));
            }
        }
        if let Some(name) = state.location.scene.as_ref().and_then(|s| s.file_name()) {
            row_text(ui, "/");
            ui.button(name.to_string_lossy());
        }
        if ui.button("Refresh") {
            refresh = true;
        }
        ui.vertical();

        let tiles = match &tiles {
            Ok(tiles) => tiles,
            Err(status) => {
                ui.colored_text(status, UiContext::TEXT_DIM);
                return;
            }
        };

        let gap = UiContext::ELEMENT_GAP.x as f32;
        let columns = (((ui.remaining_width() + gap) / (TILE_SIZE.x + gap)) as usize).max(1);
        for row in tiles.chunks(columns) {
            ui.horizontal();
            for tile in row {
                let hovered = ui.hoverd(from_pos_size(ui.cursor, TILE_SIZE));
                let selected = state.selected.as_ref() == Some(&tile.name);
                let draw = |ui: &mut UiWindowBuilder| tile.draw(ui, hovered, selected);
                let icon = |ui: &mut UiWindowBuilder| ui.text(&tile.name);
                match &tile.content {
                    Content::Folder(_) | Content::Unknown => draw(ui),
                    Content::File(kind, path) => ui.drag_source(
                        &tile.name,
                        || {
                            state
                                .handles
                                .entry(path.clone())
                                .or_insert_with(|| (kind.load)(&asset_server, path))
                                .clone()
                        },
                        draw,
                        icon,
                    ),
                    Content::Mesh(mesh) => {
                        ui.asset_drag_source(&tile.name, || mesh.clone(), draw, icon)
                    }
                    Content::Texture(texture) => {
                        ui.asset_drag_source(&tile.name, || texture.clone(), draw, icon)
                    }
                }

                if !(hovered && ui.ctx.input.primary_pressed) {
                    continue;
                }
                state.selected = Some(tile.name.clone());
                let now = Instant::now();
                let double_click = state.last_click.as_ref().is_some_and(|(time, name)| {
                    *name == tile.name && now.duration_since(*time) < DOUBLE_CLICK
                });
                if !double_click {
                    state.last_click = Some((now, tile.name.clone()));
                    continue;
                }
                state.last_click = None;
                match &tile.content {
                    Content::Folder(path) => {
                        nav = Some(Nav::To(Location {
                            dir: path.clone(),
                            scene: None,
                        }));
                    }
                    Content::File(kind, path) if kind.container => {
                        nav = Some(Nav::To(Location {
                            dir: state.location.dir.clone(),
                            scene: Some(path.clone()),
                        }));
                    }
                    _ => {}
                }
            }
            ui.vertical();
        }
    });

    if refresh {
        state.refresh();
    }
    if let Some(nav) = nav {
        state.navigate(nav, &asset_server);
    }
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
        cmd.spawn((
            transform,
            Instance {
                mesh,
                material: MaterialSettings::default(),
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
    fn folders_sort_before_files() {
        let entry = |name: &str, is_dir| DirEntry {
            name: name.to_string(),
            is_dir,
        };
        let mut entries = vec![
            entry("b.glb", false),
            entry("zoo", true),
            entry("a.glb", false),
            entry("art", true),
        ];
        sort_entries(&mut entries);
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["art", "zoo", "a.glb", "b.glb"]);
    }

    #[test]
    fn long_names_are_cut() {
        assert_eq!(fit_name("box.glb", 12), "box.glb");
        assert_eq!(fit_name("stanford_bunny.glb", 12), "stanford_b..");
        assert_eq!(fit_name("stanford_bunny.glb", 12).chars().count(), 12);
    }

    #[test]
    fn kinds_match_on_extension() {
        assert_eq!(kind_of("box.glb").map(|k| k.badge), Some("GLB"));
        assert_eq!(kind_of("BOX.GLB").map(|k| k.badge), Some("GLB"));
        assert!(kind_of("notes.txt").is_none());
        assert!(kind_of("glb").is_none());
    }
}

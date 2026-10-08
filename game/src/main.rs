//! Sample game binary: boots the engine and spawns an editor camera and glTF scene
use core::{
    CorePlugin,
    editor::camera::EditorCamera,
    scene::{Skybox, SpawnScene, camera::CameraBundle, file::Scene},
    ui::{
        UiContext,
        builder::{UiBuilder, UiWindowBuilder},
    },
};

use bevy::{
    app::{App, Startup, Update},
    asset::{AssetServer, Handle, LoadState},
    ecs::{
        resource::Resource,
        system::{Commands, Res},
    },
    time::Time,
    transform::components::Transform,
};
use glam::{Vec2, Vec3};

fn init(mut cmd: Commands, asset_server: Res<AssetServer>) {
    let camera = CameraBundle::new(
        Transform::from_translation(Vec3::new(0.0, 0.0, 0.0)),
        65.0_f32.to_radians(),
        0.01,
        100.0,
    );
    cmd.spawn((camera, EditorCamera));
    cmd.insert_resource(Skybox {
        image: asset_server.load("kloofendal_48d_partly_cloudy_puresky_4k.tex"),
    });
    // cmd.spawn((Transform::default(), SpawnScene { scene: handle }));
}

// fn loading_window(
//     mut ui: UiBuilder,
//     asset_server: Res<AssetServer>,
//     scene: Res<LoadingScene>,
//     tracker: Res<SceneLoadTracker>,
// ) {
//     let path = scene.0.path();
//     let name = path.map(|p| p.to_string()).unwrap_or_default();
//     let state = asset_server.load_state(&scene.0);
//     let progress = path.and_then(|p| tracker.get(p));

//     if matches!(state, LoadState::Loaded) {
//         return;
//     }

//     ui.window("Loading")
//         .position(Vec2::new(20.0, 20.0))
//         .size(Vec2::new(420.0, 140.0))
//         .build(|ui| {
//             ui.text(&name);
//             let (fraction, status) = match (&state, progress) {
//                 (LoadState::Failed(_), p) => {
//                     (p.map_or(0.0, |p| p.fraction()), "Failed".to_string())
//                 }
//                 (_, Some(p)) => {
//                     let stage = match p.stage {
//                         SceneLoadStage::Textures => "Textures",
//                         SceneLoadStage::Meshes => "Meshes",
//                     };
//                     (p.fraction(), format!("{stage} {}/{}", p.done, p.total))
//                 }
//                 (_, None) => (0.0, "Importing".to_string()),
//             };
//             let width = ui.remaining_width() - UiWindowBuilder::child_offset().x * 2.0;
//             ui.progress_bar(fraction, width, format!("{:.0}%", fraction * 100.0));
//             ui.text(status);
//             if let LoadState::Failed(error) = &state {
//                 ui.wrapping_text(error.to_string(), width, UiContext::ERROR);
//             }
//         });
// }

fn update_mesh(
    // mut cmd: Commands,
    // mut ui: ResMut<UiBuilder>,
    // mut model: Single<(Entity, &mut Transform), With<MyModel>>,
    // asset_server: Res<AssetServer>,
    // mut local: Local<(usize, Vec<String>)>,
    // mut gizzmos: DrawGizzmos,
    // viewport: ViewPortProxy,
    time: Res<Time>,
) {
    // println!("time: {:#?}", time.delta());
    // if let Some(pos) = window.cursor_position() {
    //     let cam_pos = settings.freez_pos.unwrap_or(camera.1.translation().extend(0.0)).xyz();
    //     gizzmos.draw_gizzmo(&ArrowGizzmo {
    //         color: Vec4::new(0.1, 0.1, 0.1, 0.4),
    //         start: cam_pos,
    //         end: cam_pos + camera.0.ray_direction(camera.1, pos, window.physical_size()),
    //         width: 0.1,
    //     });
    // }

    // let (index, elements) = &mut *local;

    // let empty = elements.is_empty();
    // if empty {
    //     *elements = WalkDir::new("/home/karsten/code/GameEngine/game/assets")
    //         .into_iter()
    //         .filter(|f| f.as_ref().unwrap().file_type().is_file())
    //         .map(|e| e.unwrap().file_name().to_str().unwrap().to_owned())
    //         .collect();
    // }

    // if let Some(mesh) = assets.get(&model.1.model) {
    //     for i in 0..mesh.instance_mesh.len() {
    //         let mesh_index = mesh.instance_mesh[i];
    //         let sub_mesh = &mesh.meshes[mesh_index as usize];
    //         let transform = mesh.instance_transforms[i as usize];
    //         let offset = sub_mesh.header.cull_data_offset as usize;
    //         // for cull_data in sub_mesh.buffer.range(offset..sub_mesh.header.vertex_offset as usize).cast::<bindings::CullData>() {
    //         //     if cull_data.aabb.center_and_error.w > 0.0001 {
    //         //         continue;
    //         //     }
    //         //     let entity = cmd
    //         //         .spawn((
    //         //             BoxGizzmo::new(cull_data.aabb.center_and_error.xyz(), cull_data.aabb.half_extent.xyz(), Vec4::new(0.0, 0.0, 1.0, 0.3)),
    //         //             Transform::from_matrix(transform),
    //         //         )).id();
    //         //     cmd.entity(model.0).add_child(entity);

    //         // }
    //     }
    // }

    // let Some(ui) = ui.ui() else {
    //     return;
    // };

    // ui.window("Scene##scene").build(|| {
    //     if let Some(combo) = ui.begin_combo("Model", &elements[*index]) {
    //         for (i, file) in elements.iter().enumerate() {
    //             if ui.selectable_config(&file).selected(i == *index).build() {
    //                 *index = i;
    //                 let handle = asset_server.load(file);
    //                 cmd.entity(model.0)
    //                     .despawn_children()
    //                     .insert(SpawnScene { scene: handle });
    //             }
    //             if *index == i {
    //                 ui.set_item_default_focus();
    //             }
    //         }
    //     }
    // });
}

fn main() {
    // Before the app exists, so that the spans of every system record.
    core::register_tracing();
    println!(
        "cargo:rustc-env=WORKSPACE_ROOT={}",
        env!("CARGO_MANIFEST_DIR")
    );

    let mut app = App::new();
    app.add_plugins(CorePlugin);
    #[cfg(feature = "profiling")]
    app.add_plugins(profiler_plugin());
    app.add_systems(Startup, init)
        .add_systems(Update, update_mesh)
        .run();
}

/// `GAME_PROFILE=DIR[:FRAMES]` saves a profile of the first FRAMES (default 300) frames.
#[cfg(feature = "profiling")]
fn profiler_plugin() -> core::profiler::ProfilerPlugin {
    let capture = std::env::var("GAME_PROFILE").ok().map(|value| {
        let split = value.rsplit_once(':');
        match split.and_then(|(dir, frames)| Some((frames.parse().ok()?, dir.into()))) {
            Some(capture) => capture,
            None => (core::profiler::MAX_FRAMES, std::path::PathBuf::from(value)),
        }
    });
    core::profiler::ProfilerPlugin {
        capture,
        ..Default::default()
    }
}

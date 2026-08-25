use core::{
    CorePlugin,
    assets::mesh::Scene,
    editor::{
        camera::EditorCamera,
        gizzmos::{ArrowGizzmo, BoxGizzmo, DrawGizzmos, SphereGizzmo},
        viewport::ViewPortProxy,
    },
    physics::bvh::Raycast,
    render::render::RenderSettings,
    scene::{
        SpawnScene,
        camera::{Camera, CameraBundle},
    },
    ui::builder::UiBuilder,
};
use std::{
    f32::consts::PI,
    fs::FileType,
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

use bevy::{
    app::{App, Startup, Update},
    asset::{AssetServer, Assets},
    ecs::{
        component::Component,
        entity::Entity,
        message::MessageReader,
        query::With,
        system::{Commands, Local, Res, ResMut, Single},
    },
    input::{
        ButtonInput,
        mouse::{MouseButton, MouseButtonInput, MouseMotion},
        touch::Touches,
    },
    log::{self, tracing},
    math::{
        Dir3A, VectorSpace,
        bounding::{BoundingVolume, RayCast3d},
    },
    reflect::{self, Reflect},
    time::Time,
    transform::components::{GlobalTransform, Transform},
    window::Window,
};
use glam::{Mat4, Quat, UVec2, Vec2, Vec3, Vec4, Vec4Swizzles, vec3};

use bevy::ecs::reflect::ReflectComponent;

#[derive(Component)]
struct MyModel;

fn init(mut cmd: Commands, asset_server: Res<AssetServer>) {
    // let handle = asset_server.load("tower.glb");
    let camera = CameraBundle::new(
        Transform::from_translation(Vec3::new(0.0, 0.0, 0.0)),
        65.0_f32.to_radians(),
        0.01,
        100.0,
    );
    cmd.spawn((camera, EditorCamera));
    // cmd.spawn((
    //     Transform::from_scale(Vec3::splat(0.1)),
    //     MyModel,
    //     SpawnScene { scene: handle },
    // ));
}

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
    // log::info!("{:#?}", time.delta());
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
    App::new()
        .add_plugins(CorePlugin)
        .add_systems(Startup, init)
        .add_systems(Update, update_mesh)
        .run();
}

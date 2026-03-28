use core::{
    CorePlugin,
    assets::Mesh,
    components::{
        camera::{Camera, Controls},
        gizzmos::{ArrowGizzmo, BoxGizzmo, SphereGizzmo},
    },
    physics::bvh::Raycast,
    render::world::Model,
    ui::{UiBuilder, UiContext},
};
use std::{
    f32::consts::PI,
    fs::FileType,
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

use bevy::{
    log::{self, tracing},
    math::bounding::{BoundingVolume, RayCast3d},
    prelude::*,
};
use glam::{Vec3, vec3};
use walkdir::WalkDir;

#[derive(Component)]
struct MyModel;

fn init(mut cmd: Commands, asset_server: Res<AssetServer>) {
    let controles = Controls {
        ..Default::default()
    };
    let handle = asset_server.load("tower.glb");
    let camera = Camera::new(vec3(0.0, 0.0, 0.0), 65.0_f32.to_radians(), 0.01, 100.0);
    cmd.insert_resource(controles);
    cmd.spawn(camera);
    cmd.spawn((
        Transform::from_scale(Vec3::splat(0.1)),
        MyModel,
        Model {
            model: handle.clone(),
        },
    ));
}

fn update_mesh(
    mut cmd: Commands,
    mut ui: ResMut<UiBuilder>,
    mut model: Single<(Entity, &mut Model, &mut Transform), With<MyModel>>,
    asset_server: Res<AssetServer>,
    mut local: Local<(usize, Vec<String>, bool)>,
    assets: Res<Assets<Mesh>>,
) {
    if local.1.is_empty() {
        local.1 = WalkDir::new("/home/karsten/code/GameEngine/game/assets")
            .into_iter()
            .filter(|f| f.as_ref().unwrap().file_type().is_file())
            .map(|e| e.unwrap().file_name().to_str().unwrap().to_owned())
            .collect();
    }

    let (index, elements, uploaded) = &mut *local;

    if let Some(mesh) = assets.get(&model.1.model)
        && !*uploaded
    {
        for i in 0..mesh.instance_mesh.len() {
            // let mesh_index = mesh.instance_mesh[i];
            // let sub_mesh = &mesh.meshes[mesh_index as usize];
            // let entity = cmd
            //     .spawn((
            //         AabbGizzmo {
            //             center: sub_mesh.aabb.center_and_error.xyz(),
            //             half_extend: sub_mesh.aabb.half_extent.xyz(),
            //             color: Vec4::new(rand::random(), rand::random(), rand::random(), 0.25),
            //         },
            //         Transform::from_matrix(mesh.instance_transforms[i]),
            //     ))
            //     .id();
            // cmd.entity(model.0).add_child(entity);
        }
        *uploaded = true;
    }

    let Some(ui) = ui.ui() else {
        return;
    };

    ui.window("Scene").build(|| {
        if let Some(combo) = ui.begin_combo("Model", &elements[*index]) {
            for (i, file) in elements.iter().enumerate() {
                if ui.selectable_config(&file).selected(i == *index).build() {
                    *index = i;
                    let handle = asset_server.load(file);
                    model.1.model = handle;
                    cmd.entity(model.0).despawn_children();
                    *uploaded = false;
                }
                if *index == i {
                    ui.set_item_default_focus();
                }
            }
        }
        ui.input_float3("Scale", &mut model.2.scale).build();
        ui.input_float3("Position", &mut model.2.translation)
            .build();
        ui.input_float4("Rotation", unsafe {
            std::mem::transmute::<&mut Quat, &mut Vec4>(&mut model.2.rotation)
        })
        .build();
    });
}

fn ray_trace(mut cmd: Commands, raycast: Raycast, camera: Single<&Camera>) {
    let ray = RayCast3d::new(
        camera.position,
        Dir3A::new(camera.direction.into()).unwrap_or(Dir3A::Z),
        1000.0,
    );
    let res = raycast.raycast(&ray);
    if let Some(hit) = res {
        let transform = raycast.get_instance_transform(&hit);
        // cmd.spawn(BoxGizzmo::with_local_tranform(
        //     hit.aabb.center().into(),
        //     hit.aabb.half_size().into(),
        //     Vec4::new(1.0, 0.0, 0.0, 1.0),
        //     transform,
        // ));
        cmd.spawn(SphereGizzmo {
            color: Vec4::new(1.0, 0.0, 0.0, 1.0),
            pos: ray.origin.to_vec3() + ray.direction.to_vec3() * hit.t,
            radius: 0.1,
        });
    }
}

fn main() {
    App::new()
        .add_plugins(CorePlugin)
        .add_systems(Startup, init)
        .add_systems(Update, (update_mesh, ray_trace))
        .run();
}

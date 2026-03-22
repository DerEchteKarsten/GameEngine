use core::{
    CorePlugin,
    assets::Mesh,
    components::camera::{Camera, Controls},
    render::world::Model, ui::{UiBuilder, UiContext},
};
use std::{f32::consts::PI, fs::FileType, path::{Path, PathBuf}, thread, time::Duration};

use bevy::{log::{self, tracing}, prelude::*};
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
        Transform::from_scale(Vec3::splat(0.1)).with_rotation(Quat::from_axis_angle(Vec3::new(0.0, 0.0, 1.0), PI)),
        MyModel,
        Model {
            model: handle,
        }
    ));
}


fn update(mut cmd: Commands, time: Res<Time>, mut ui: ResMut<UiBuilder>, mut model: Single<(Entity, &mut Transform), With<MyModel>>, asset_server: Res<AssetServer>, mut local: Local<(usize, Vec<String>)>) {
    // if let Some((transform, children)) = model.iter().last() {

    //     // transform.position.z = time.elapsed_secs_wrapped().sin() * 2.0;
    // log::info!("{:?}", time.delta());

    //     for i in children.iter() {
    //         log::debug!("parent: {:?}, child: {:?}", transform, qchildren.get(*i).unwrap().as_matrix());
    //     }
    // }
    if local.1.is_empty() {
        local.1 = WalkDir::new("/home/karsten/code/GameEngine/game/assets").into_iter().filter(|f| f.as_ref().unwrap().file_type().is_file()).map(|e| e.unwrap().file_name().to_str().unwrap().to_owned()).collect();
    }


    let (index, elements) = &mut *local;

    let Some(ui) = ui.ui() else {
        return;
    };

    ui.window("Scene").build(|| {
        if let Some(combo) = ui.begin_combo("Model", &elements[*index]) {
            for (i, file) in elements.iter().enumerate() {
                if ui.selectable_config(&file).selected(i == *index).build() {
                    *index = i;
                    let handle = asset_server.load(file);
                    cmd.get_entity(model.0).unwrap()
                        .entry::<Model>()
                        .and_modify(|mut m| {
                            m.model = handle;
                        });
                }
                if *index == i {
                    ui.set_item_default_focus();
                }
            }
        }
        ui.input_float3("Scale", &mut model.1.scale).build();
        ui.input_float3("Position", &mut model.1.translation).build();
        ui.input_float4("Rotation", unsafe { std::mem::transmute::<&mut Quat, &mut Vec4>(&mut model.1.rotation) }).build();
    });
}

fn main() {
    App::new()
        .add_plugins(CorePlugin)
        .add_systems(Startup, init)
        .add_systems(Update, update)
        .run();
}
use core::{
    CorePlugin,
    assets::Mesh,
    components::camera::{Camera, Controls},
    render::world::Model,
};
use std::{thread, time::Duration};

use bevy::{log, prelude::*};
use glam::{Vec3, vec3};

fn init(mut cmd: Commands, asset_server: Res<AssetServer>) {
    let controles = Controls {
        ..Default::default()
    };

    let camera = Camera::new(vec3(0.0, 0.0, 0.0), 65.0_f32.to_radians(), 0.01, 100.0);
    let model: Handle<Mesh> = asset_server.load("stanford_bunny.glb");
    cmd.insert_resource(controles);
    cmd.spawn(camera);

    // // let model2: Handle<Mesh> = asset_server.load("mat.glb");

    for _x in 0..1 {
        for _y in 0..1 {
            cmd.spawn((
                Model {
                    model: model.clone(),
                },
                Transform::IDENTITY,
            ));
        }
    }
}

fn update(_cmd: Commands, time: Res<Time>) {
    // if let Some((transform, children)) = model.iter().last() {

    //     // transform.position.z = time.elapsed_secs_wrapped().sin() * 2.0;
    // log::info!("{:?}", time.delta());

    //     for i in children.iter() {
    //         log::debug!("parent: {:?}, child: {:?}", transform, qchildren.get(*i).unwrap().as_matrix());
    //     }
    // }
}

fn main() {
    App::new()
        .add_plugins(CorePlugin)
        .add_systems(Startup, init)
        .add_systems(Update, update)
        .run();
}

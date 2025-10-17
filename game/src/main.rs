use core::{
    CorePlugin, INITIAL_WINDOW_SIZE,
    assets::Mesh,
    components::{
        camera::{Camera, Controls},
        transform::Transform,
    },
    world::Instance,
};
use std::f32::consts::PI;

use bevy_app::{App, Startup, Update};
use bevy_asset::{AssetServer, Handle};
use bevy_ecs::{
    component::Component,
    hierarchy::Children,
    query::With,
    system::{Commands, Query, Res},
};
use bevy_time::Time;
use glam::{Vec3, vec3};

#[derive(Component)]
struct Model;

fn init(mut cmd: Commands) {
    let controles = Controls {
        ..Default::default()
    };

    let camera = Camera::new(vec3(0.0, 0.0, 0.0), 65.0_f32.to_radians(), 0.1, 1000.0);
    // let model: Handle<Mesh> = asset_server.load("stanford_dragon.glb");
    cmd.insert_resource(controles);
    cmd.spawn(camera);

    // // let model2: Handle<Mesh> = asset_server.load("mat.glb");

    // for x in 0..1 {
    //     for y in 0..1 {
    //         cmd.spawn((
    //             Instance {
    //                 model: model.clone(),
    //             },
    //             Transform::new_euler(
    //                 Vec3::new(0.0, 0.0, 1.0),
    //                 Vec3::new(10.0, 10.0, 10.0),
    //                 Vec3::new(PI, 0.0, 0.0),
    //             ),
    //             Model,
    //         ));
    //     }
    // }
}

fn update(
    mut cmd: Commands,
    mut model: Query<(&Transform, &Children), With<Model>>,
    qchildren: Query<&Transform>,
    time: Res<Time>,
) {
    // if let Some((transform, children)) = model.iter().last() {

    //     // transform.position.z = time.elapsed_secs_wrapped().sin() * 2.0;
    // log::info!("{:?}", time.delta());

    //     for i in children.iter() {
    //         log::debug!("parent: {:?}, child: {:?}", transform, qchildren.get(*i).unwrap().as_matrix());
    //     }
    // }
    // println!("{:?}", time.delta());
}

fn main() {
    App::new()
        .add_plugins(CorePlugin)
        .add_systems(Startup, init)
        .add_systems(Update, update)
        .run();
}

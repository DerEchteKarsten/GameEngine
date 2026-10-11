//! Entity selection: viewport raycast picking, world-space move gizmo, and the hierarchy panel, which also saves a subtree as a scene file.
use crate::{
    assets::mesh::GpuMesh,
    editor::{
        gizzmos::{ArrowGizzmo, DrawGizzmos},
        viewport::ViewPortProxy,
    },
    id,
    physics::bvh::Raycast,
    scene::{Instance, camera::Camera, file::save_scene},
    ui::{
        MultiInput,
        builder::{UiBuilder, UiWindowBuilder},
    },
};
use bevy::{
    asset::Assets,
    ecs::{
        component::{Component, Components},
        entity::Entity,
        hierarchy::{ChildOf, Children},
        name::Name,
        query::{Has, With},
        reflect::ReflectComponent,
        resource::IsResource,
        system::{Commands, Local, Query, Res, Single},
        world::World,
    },
    input::{ButtonInput, keyboard::KeyCode, mouse::MouseButton, touch::Touches},
    log::{error, info},
    math::{Dir3A, bounding::RayCast3d},
    reflect::Reflect,
    transform::{
        commands::BuildChildrenTransformExt,
        components::{GlobalTransform, Transform},
    },
    window::Window,
};
use glam::{Mat3, Mat4, Vec3};

#[derive(Component, Reflect)]
#[component(storage = "SparseSet")]
#[reflect(Component)]
pub struct Selected;

pub(crate) fn hierarchy_ui(
    mut ui: UiBuilder,
    mut cmd: Commands,
    instances: Query<(
        Entity,
        Has<Selected>,
        Option<&Name>,
        Option<&Children>,
        Option<&ChildOf>,
        Option<&Instance>,
        Option<&IsResource>,
    )>,
    selected: Query<Entity, With<Selected>>,
    keys: Res<ButtonInput<KeyCode>>,
    components: &Components,
    mut save_path: Local<String>,
) {
    ui.build("Hierarchy", |ui| {
        if ui.button("insert") {
            cmd.spawn(Transform::default());
        }
        // Saves the selected entity's descendants; `.ron` picks the RON layout.
        ui.row(|ui| {
            ui.text_input(id!(), &mut save_path, 200.0);
            if ui.button("save")
                && let Ok(root) = selected.single()
            {
                let path = save_path.clone();
                cmd.queue(
                    move |world: &mut World| match save_scene(world, root, &path) {
                        Ok(()) => info!("saved {path}"),
                        Err(err) => error!("saving {path}: {err:#}"),
                    },
                );
            }
        });

        let mut roots: Vec<Entity> = instances
            .iter()
            .filter(|(_, _, _, _, parent, _, _)| parent.is_none())
            .map(|(e, _, _, _, _, _, _)| e)
            .collect();
        roots.sort();
        for root in roots {
            draw_entity_node(&mut cmd, ui, root, &instances, &selected, components);
        }
        if keys.just_pressed(KeyCode::Delete) && ui.ctx.focused.is_some() {
            for e in &selected {
                // Despawning a resource panics the next system that reads it.
                if instances.get(e).is_ok_and(|i| i.6.is_none()) {
                    cmd.entity(e).despawn();
                }
            }
        }
        // Dropping an entity below the tree makes it a root.
        ui.space(ui.available_size());
        if let Some(entity) = ui.drop_target::<Entity>(|_| true) {
            cmd.entity(entity).remove_parent_in_place();
        }
    });
}

fn draw_entity_node(
    cmd: &mut Commands,
    ui: &mut UiWindowBuilder,
    this_entity: Entity,
    instances: &Query<(
        Entity,
        Has<Selected>,
        Option<&Name>,
        Option<&Children>,
        Option<&ChildOf>,
        Option<&Instance>,
        Option<&IsResource>,
    )>,
    selected: &Query<Entity, With<Selected>>,
    components: &Components,
) {
    let Ok((_, is_selected, name, children, _, instance, resource)) = instances.get(this_entity)
    else {
        return;
    };

    let label = if let Some(name) = name {
        name.to_string()
    } else if let Some(inst) = instance {
        inst.mesh
            .path()
            .map(|p| p.to_string())
            .unwrap_or_else(|| format!("Entity {}", this_entity.index()))
    } else if let Some(name) = resource.and_then(|r| components.get_name(r.resource_component_id()))
    {
        name.shortname().to_string()
    } else {
        format!("Entity {}", this_entity.index())
    };

    let open = if children.is_some() {
        ui.header(this_entity, &label, true, is_selected)
    } else {
        ui.selectable(&label, ui.text_color(), is_selected);
        false
    };
    ui.drag_source(this_entity, || this_entity, |ui| ui.text(&label));
    if let Some(entity) = ui.drop_target::<Entity>(|e| *e != this_entity) {
        cmd.entity(entity).set_parent_in_place(this_entity);
    }
    if ui.prev_clicked() {
        for e in selected {
            cmd.entity(e).remove::<Selected>();
        }
        cmd.entity(this_entity).insert(Selected);
    }

    if open && let Some(children) = children {
        let mut children = children.iter().copied().collect::<Vec<_>>();
        children.sort();
        ui.indent(|ui| {
            for child in children {
                draw_entity_node(cmd, ui, child, instances, selected, components);
            }
        });
    }
}

/// Length of the move arrows as a fraction of the viewport's half height, so they keep the
/// same size on screen at any distance.
const GIZMO_SCREEN_SIZE: f32 = 0.2;

pub struct DragState {
    /// Unit axis and a point on it, in world space.
    axis: Vec3,
    origin: Vec3,
    /// Maps a world-space offset into the space `Transform::translation` lives in.
    world_to_parent: Mat3,
    start_pos: Vec3,
    start_t: f32,
}

pub(crate) fn picking(
    mut cmd: Commands,
    mut gizzmos: DrawGizzmos,
    raycast: Raycast,
    mouse: Res<ButtonInput<MouseButton>>,
    touches: Res<Touches>,
    viewport: ViewPortProxy,
    window: Single<&Window>,
    camera: Single<(&Camera, &GlobalTransform)>,
    assets: Res<Assets<GpuMesh>>,
    mut picked: Query<
        (
            &GlobalTransform,
            Option<&Instance>,
            &mut Transform,
            Option<&ChildOf>,
        ),
        With<Selected>,
    >,
    parents: Query<&GlobalTransform>,
    all_picked: Query<Entity, With<Selected>>,
    mut local: Local<Option<DragState>>,
) {
    let mut input = MultiInput::new(&window, &mouse, &touches);

    if let Some(viewport) = &viewport.view_port {
        input = input.to_viewport(viewport);
    }

    if input.primary_released {
        *local = None;
    }

    if let Some((global_transform, instance, mut transform, child_of)) = picked.iter_mut().next() {
        if let Some(drag) = local.as_ref()
            && let Some(pos) = input.cursor_pos
        {
            let t = camera.0.closest_t_on_axis(
                camera.1,
                pos,
                viewport.size(),
                drag.origin,
                drag.axis,
            ) - drag.start_t;
            let offset = drag.world_to_parent * (drag.axis * t);
            if offset.is_finite() {
                transform.translation = drag.start_pos + offset;
            }
        }

        // The arrows sit at the mesh center (or the entity origin) and are built in world
        // space, so the entity's scale changes neither their size nor how far a drag moves.
        let origin = if let Some(instance) = instance
            && let Some(mesh) = assets.get(&instance.mesh)
        {
            global_transform.transform_point(Vec3::from(mesh.header.aabb.center))
        } else {
            global_transform.translation()
        };
        let distance = origin.distance(camera.1.translation());
        let size = distance * (camera.0.fov * 0.5).tan() * GIZMO_SCREEN_SIZE;

        // Arrows are drawn every frame, but only hit-tested on the press itself.
        let click = input.cursor_pos.filter(|_| input.primary_pressed);

        let matrix = global_transform.affine().matrix3;
        for (axis, color) in [
            (matrix.x_axis, Vec3::X),
            (matrix.y_axis, Vec3::Y),
            (matrix.z_axis, Vec3::Z),
        ] {
            let Some(axis) = Vec3::from(axis).try_normalize() else {
                continue;
            };
            if size <= 0.0 {
                continue;
            }
            if gizzmos.draw_gizzmo_check_clicked(
                &ArrowGizzmo {
                    color: color.extend(1.0),
                    start: origin,
                    end: origin + axis * size,
                    width: size,
                },
                click,
                Mat4::IDENTITY,
            ) && local.is_none()
                && let Some(cursor_pos) = input.cursor_pos
            {
                let world_to_parent = child_of
                    .and_then(|child_of| parents.get(child_of.parent()).ok())
                    .map(|parent| Mat3::from(parent.affine().matrix3).inverse())
                    .unwrap_or(Mat3::IDENTITY);
                *local = Some(DragState {
                    axis,
                    origin,
                    world_to_parent,
                    start_pos: transform.translation,
                    start_t: camera.0.closest_t_on_axis(
                        camera.1,
                        cursor_pos,
                        viewport.size(),
                        origin,
                        axis,
                    ),
                });
            }
        }
    }
    if !viewport.focused() || local.is_some() || !input.primary_pressed {
        return;
    }
    let Some(cursor_pos) = input.cursor_pos else {
        return;
    };

    for e in &all_picked {
        cmd.entity(e).remove::<Selected>();
    }

    let view_dir = camera
        .0
        .ray_direction(camera.1, cursor_pos, viewport.size());
    let ray = RayCast3d::new(
        camera.1.translation(),
        Dir3A::new(view_dir.to_vec3a()).unwrap_or(Dir3A::Z),
        1000.0,
    );
    let hit = raycast.raycast(&ray);
    if let Some(hit) = hit {
        cmd.entity(hit.entity).insert(Selected);
    }
}

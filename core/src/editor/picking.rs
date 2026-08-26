
use crate::{
    assets::mesh::GpuMesh,
    editor::{
        dragndrop::EntityDragAndDropProvider,
        gizzmos::{ArrowGizzmo, DrawGizzmos},
        viewport::ViewPortProxy,
    },
    physics::bvh::Raycast,
    scene::{Instance, camera::Camera},
    ui::{
        MultiInput, UiContext,
        builder::{UiBuilder, UiWindowBuilder},
    },
};
use bevy::{
    asset::Assets,
    ecs::{
        component::Component,
        entity::Entity,
        hierarchy::{ChildOf, Children},
        name::Name,
        query::{Has, With},
        reflect::ReflectComponent,
        system::{Commands, Local, Query, Res, Single},
    },
    input::{
        ButtonInput,
        keyboard::KeyCode,
        mouse::MouseButton,
        touch::Touches,
    },
    math::{Dir3A, bounding::RayCast3d},
    reflect::Reflect,
    transform::{
        commands::BuildChildrenTransformExt,
        components::{GlobalTransform, Transform},
    },
    window::Window,
};
use glam::{Vec2, Vec3};

#[derive(Component, Reflect)]
#[component(storage = "SparseSet")]
#[reflect(Component)]
pub struct Selected;

pub(crate) fn hierarchy_ui(
    mut ui: UiBuilder,
    mut cmd: Commands,
    mut instances: Query<(
        Entity,
        Has<Selected>,
        Option<&Name>,
        Option<&Children>,
        Option<&ChildOf>,
        Option<&Instance>,
    )>,
    selected: Query<Entity, With<Selected>>,
    keys: Res<ButtonInput<KeyCode>>,
    dragndrop: Res<EntityDragAndDropProvider>,
) {
    ui.build("Hierarchy", |ui| {
        if ui.button("insert") {
            cmd.spawn(Transform::default());
        }

        ui.droppable(
            &mut cmd,
            || dragndrop.drop_valid(),
            |ui, cmd| {
                let mut roots: Vec<Entity> = instances
                    .iter()
                    .filter(|(_, _, _, _, parent, _)| parent.is_none())
                    .map(|(e, _, _, _, _, _)| e)
                    .collect();

                roots.sort();

                let mut content_max = ui.content_max;
                for root in roots {
                    draw_entity_node(
                        cmd,
                        ui,
                        root,
                        &mut instances,
                        &selected,
                        &dragndrop,
                        &mut content_max,
                    );
                }
                ui.content_max = ui.content_max.max(content_max);
                if keys.just_pressed(KeyCode::Delete) && ui.ctx.focused.is_some() {
                    for e in selected {
                        cmd.entity(e).despawn();
                    }
                }
                ui.content_max = ui
                    .content_max
                    .max(ui.clip_rect.size() - UiContext::WINDOW_PAD.as_vec2());
            },
            |cmd| {
                let entity = dragndrop.drop();
                if let Some(entity) = entity {
                    cmd.entity(entity).remove_parent_in_place();
                }
            },
        );
    });
}

fn draw_entity_node(
    cmd: &mut Commands,
    ui: &mut UiWindowBuilder,
    this_entity: Entity,
    instances: &mut Query<(
        Entity,
        Has<Selected>,
        Option<&Name>,
        Option<&Children>,
        Option<&ChildOf>,
        Option<&Instance>,
    )>,
    selected: &Query<Entity, With<Selected>>,
    dragndrop: &EntityDragAndDropProvider,
    content_max: &mut Vec2,
) {
    let Ok((_, is_selected, name, children, _, instance)) = instances.get(this_entity) else {
        return;
    };

    let label = if let Some(name) = name {
        name.to_string()
    } else if let Some(inst) = instance {
        inst.mesh
            .path()
            .map(|p| p.to_string())
            .unwrap_or_else(|| format!("Entity {}", this_entity.index()))
    } else {
        format!("Entity {}", this_entity.index())
    };
    let has_children = children.is_some();

    ui.disabled(!is_selected);
    ui.draggable(
        this_entity,
        |ui, state| {
            ui.droppable(
                cmd,
                || dragndrop.drop_valid(),
                |ui, cmd| {
                    ui.content_max = if has_children {
                        ui.collapsable(&label, |ui| {
                            let size = ui.content_max;
                            let Ok((_, _, _, children, _, _)) = instances.get(this_entity) else {
                                return size;
                            };
                            let mut children = children
                                .as_ref()
                                .unwrap()
                                .iter()
                                .copied()
                                .collect::<Vec<_>>();
                            children.sort();
                            for child in children {
                                draw_entity_node(
                                    cmd,
                                    ui,
                                    child,
                                    instances,
                                    selected,
                                    dragndrop,
                                    content_max,
                                );
                            }
                            *content_max = content_max.max(ui.content_max);
                            size
                        })
                        .unwrap_or(ui.content_max)
                    } else {
                        ui.text(&label);
                        ui.content_max
                    };
                },
                |cmd| {
                    if let Some(entity) = dragndrop.drop()
                        && entity != this_entity
                    {
                        cmd.entity(entity).set_parent_in_place(this_entity);
                    }
                },
            );

            dragndrop.drag(state, this_entity);
        },
        |ui| ui.text(&label),
    );
    ui.disabled(true);

    if ui.prev_element_hoverd && ui.ctx.input.primary_pressed {
        for e in selected {
            cmd.entity(e).remove::<Selected>();
        }
        cmd.entity(this_entity).insert(Selected);
    }
}

pub struct DragState {
    world_space_axis: Vec3,
    local_space_axis: Vec3,
    world_space_axis_origin: Vec3,
    start_pos: Vec3,
    start_t: f32,
    scale: f32,
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
        (Entity, &GlobalTransform, Option<&Instance>, &mut Transform),
        With<Selected>,
    >,
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

    if let Some((_entity, global_transform, instance, mut transform)) = picked.iter_mut().next() {
        if let Some(drag) = local.as_ref() {
            if let Some(pos) = input.cursor_pos {
                let t = drag.start_t
                    - camera.0.closest_t_on_axis(
                        camera.1,
                        pos,
                        viewport.size(),
                        drag.world_space_axis_origin,
                        drag.world_space_axis,
                    );
                transform.translation = drag.start_pos + drag.local_space_axis * (t / drag.scale);
            }
        }

        let center = if let Some(instance) = instance
            && let Some(mesh) = assets.get(&instance.mesh)
        {
            Vec3::from(mesh.header.aabb.center)
        } else {
            global_transform.translation()
        };

        if global_transform.affine().matrix3.row(0).length() == 0.0
            || global_transform.affine().matrix3.row(1).length() == 0.0
            || global_transform.affine().matrix3.row(2).length() == 0.0
        {
            return;
        }

        if !input.primary_pressed {
            return;
        }

        let directions = [
            (global_transform.right(), transform.right()),
            (global_transform.up(), transform.up()),
            (global_transform.forward(), transform.forward()),
        ];
        let scale = global_transform.scale();
        for (global_dir, local_dir) in directions {
            let scale_factor = scale.dot(*global_dir) * 5.0;
            if gizzmos.draw_gizzmo_check_clicked(
                &ArrowGizzmo {
                    color: (*local_dir).abs().extend(1.0),
                    start: center,
                    end: center + (*local_dir / scale_factor),
                    width: 1.0 / scale_factor,
                },
                input.cursor_pos,
                global_transform.to_matrix(),
            ) && local.is_none()
            {
                let world_space_axis_origin = global_transform.transform_point(center);
                let scale =
                    global_transform.scale().dot(*global_dir) / transform.scale.dot(*local_dir);
                *local = Some(DragState {
                    scale,
                    start_t: camera.0.closest_t_on_axis(
                        camera.1,
                        input.cursor_pos.unwrap(),
                        viewport.size(),
                        world_space_axis_origin,
                        *global_dir,
                    ),
                    world_space_axis_origin,
                    start_pos: transform.translation,
                    world_space_axis: *global_dir,
                    local_space_axis: *local_dir,
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

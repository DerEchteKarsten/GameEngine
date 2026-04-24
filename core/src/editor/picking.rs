use std::{ops::DerefMut, sync::Arc};

use crate::{
    assets::mesh::{GpuMesh, Scene},
    editor::{
        gizzmos::{ArrowGizzmo, DrawGizzmos, SphereGizzmo},
        viewport::ViewPortProxy,
    },
    physics::bvh::Raycast,
    render::world::InstanceFlags,
    scene::{Instance, camera::Camera},
    ui::{UiBuilder, UiContext},
};
use bevy::{
    asset::{AssetId, Assets, Handle, StrongHandle},
    ecs::{
        archetype::Archetypes,
        component::{Component, ComponentId},
        entity::{Entities, Entity},
        hierarchy::{ChildOf, Children},
        name::Name,
        query::{Has, With},
        reflect::{AppTypeRegistry, ReflectComponent},
        resource::Resource,
        system::{Commands, Local, Query, Res, ResMut, Single},
        world::{EntityRef, Mut, World},
    },
    input::{
        ButtonInput,
        keyboard::{KeyCode, KeyboardInput},
        mouse::{AccumulatedMouseMotion, MouseButton},
        touch::Touches,
    },
    log,
    math::{Dir3A, bounding::RayCast3d},
    reflect::{PartialReflect, Reflect, ReflectMut, TypeRegistry},
    transform::{
        commands::BuildChildrenTransformExt,
        components::{GlobalTransform, Transform},
    },
    window::Window,
};
use glam::{Affine3A, Mat3A, Mat4, Quat, Vec2, Vec3, Vec3Swizzles, Vec4};
use imgui::{
    TreeNodeFlags, Ui, WindowFocusedFlags,
    drag_drop::{DragDropPayload, DragDropPayloadPod},
};

#[derive(Component, Reflect)]
#[component(storage = "SparseSet")]
#[reflect(Component)]
pub struct Selected;

pub(crate) fn hierarchy_ui(
    mut ui: ResMut<UiBuilder>,
    mut cmd: Commands,
    mut instances: Query<(
        Entity,
        Has<Selected>,
        Option<&Name>,
        Option<&Children>,
        Option<&ChildOf>,
        Option<&Instance>,
    )>,
    viewport: ViewPortProxy,
    keys: Res<ButtonInput<KeyCode>>,
    selected: Query<Entity, With<Selected>>,
) {
    let Some(ui) = ui.ui() else { return };

    ui.window("Hierarchy##hierarchy")
        .size([250.0, 400.0], imgui::Condition::FirstUseEver)
        .build(|| {
            ui.child_window("dnd_target").build(|| {
                if ui.button("insert") {
                    cmd.spawn(Transform::default());
                }
                ui.separator();

                let mut roots: Vec<Entity> = instances
                    .iter()
                    .filter(|(_, _, _, _, parent, _)| parent.is_none())
                    .map(|(e, _, _, _, _, _)| e)
                    .collect();

                roots.sort();

                for root in roots {
                    draw_entity_node(&mut cmd, ui, root, &mut instances, &selected);
                }
            });
            if let Some(target) = ui.drag_drop_target() {
                if let Some(payload) =
                    target.accept_payload::<u64, _>("ENTITY_DND", imgui::DragDropFlags::empty())
                {
                    if let Ok(bits) = payload {
                        let dragged = Entity::from_bits(bits.data);
                        cmd.entity(dragged).remove_parent_in_place();
                    }
                }
            }
            if keys.just_pressed(KeyCode::Delete)
                && (ui.is_window_focused_with_flags(WindowFocusedFlags::ROOT_AND_CHILD_WINDOWS)
                    || viewport.focused())
            {
                for e in selected {
                    cmd.entity(e).despawn();
                }
            }
        });
}

fn draw_entity_node(
    cmd: &mut Commands,
    ui: &Ui,
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
) {
    let Ok((_, is_selected, name, children, _, instance)) = instances.get(this_entity) else {
        return;
    };

    let has_children = children.map(|c| !c.is_empty()).unwrap_or(false);
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

    let mut flags_tree = TreeNodeFlags::OPEN_ON_ARROW
        | TreeNodeFlags::SPAN_AVAIL_WIDTH
        | TreeNodeFlags::FRAME_PADDING;

    if is_selected {
        flags_tree |= TreeNodeFlags::SELECTED;
    }
    if !has_children {
        flags_tree |= TreeNodeFlags::LEAF;
    }

    let node = ui
        .tree_node_config(format!("{}##{:?}", label, this_entity))
        .flags(flags_tree)
        .push();

    if ui.is_item_hovered()
        && ui.is_mouse_released(imgui::MouseButton::Left)
        && !ui.is_item_toggled_open()
        && !ui.is_mouse_dragging(imgui::MouseButton::Left)
    {
        for e in selected {
            cmd.entity(e).remove::<Selected>();
        }
        cmd.entity(this_entity).insert(Selected);
    }

    if let Some(_drag) = ui
        .drag_drop_source_config("ENTITY_DND")
        .flags(imgui::DragDropFlags::SOURCE_ALLOW_NULL_ID)
        .begin_payload(this_entity.to_bits())
    {
        ui.text(format!("{:?}", label));
    }

    if let Some(target) = ui.drag_drop_target() {
        if let Some(payload) =
            target.accept_payload::<u64, _>("ENTITY_DND", imgui::DragDropFlags::empty())
        {
            if let Ok(bits) = payload {
                let dragged = Entity::from_bits(bits.data);
                if dragged != this_entity {
                    cmd.entity(dragged).set_parent_in_place(this_entity);
                }
            }
        }
    }

    if let Some(_node) = node
        && let Some(children) = children.as_ref()
    {
        let mut children = children.iter().copied().collect::<Vec<_>>();
        children.sort();
        for child in children {
            draw_entity_node(cmd, ui, child, instances, selected);
        }
    }
}

pub struct DragState {
    world_space_axis: Vec3,
    local_space_axis: Vec3,
    world_space_axis_origin: Vec3,
    start_pos: Vec3,
    start_t: f32,
    scale: f32,
    touchid: Option<u64>,
}

pub(crate) fn picking(
    mut cmd: Commands,
    mut gizzmos: DrawGizzmos,
    raycast: Raycast,
    mouse: Res<ButtonInput<MouseButton>>,
    touches: Res<Touches>,
    viewport: ViewPortProxy,
    camera: Single<(&Camera, &GlobalTransform)>,
    assets: Res<Assets<GpuMesh>>,
    mut picked: Query<
        (Entity, &GlobalTransform, Option<&Instance>, &mut Transform),
        With<Selected>,
    >,
    all_picked: Query<Entity, With<Selected>>,
    mut local: Local<Option<DragState>>,
) {
    let mut click_pos = None;
    let mut touch_id = None;
    if let Some(touch) = touches.iter_just_pressed().next() {
        click_pos = viewport.to_viewport_pos(touch.position());
        touch_id = Some(touch.id());
    } else if mouse.just_pressed(MouseButton::Left) {
        click_pos = viewport.cursor_position();
        touch_id = None;
    }

    if let Some(l) = local.as_ref()
        && let Some(touch) = l.touchid
        && touches.just_released(touch)
    {
        *local = None;
    } else if mouse.just_released(MouseButton::Left) {
        *local = None;
    }

    if let Some((entity, global_transform, instance, mut transform)) = picked.iter_mut().next() {
        if let Some(drag) = local.as_ref() {
            if let Some(pos) = if let Some(touch) = drag.touchid {
                touches
                    .get_pressed(touch)
                    .and_then(|e| viewport.to_viewport_pos(e.position()))
            } else {
                viewport.cursor_position()
            } {
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
                click_pos,
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
                        click_pos.unwrap(),
                        viewport.size(),
                        world_space_axis_origin,
                        *global_dir,
                    ),
                    world_space_axis_origin,
                    start_pos: transform.translation,
                    world_space_axis: *global_dir,
                    local_space_axis: *local_dir,
                    touchid: touch_id,
                });
            }
        }
    }
    if !viewport.focused() || local.is_some() {
        return;
    }
    let Some(click_pos) = click_pos else { return };

    for e in &all_picked {
        cmd.entity(e).remove::<Selected>();
    }

    let view_dir = camera.0.ray_direction(camera.1, click_pos, viewport.size());
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

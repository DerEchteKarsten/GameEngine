use std::ops::DerefMut;

use crate::{
    assets::GpuMeshletMesh,
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
    asset::Assets,
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
use glam::{Mat4, Quat, Vec2, Vec3, Vec3Swizzles, Vec4};
use imgui::{TreeNodeFlags, Ui};

#[derive(Component, Reflect)]
#[reflect(Component)]
pub struct Selected;

pub(crate) fn selected_ui(world: &mut World) {
    let mut q = world.query_filtered::<Entity, With<Selected>>();
    let entity = q.iter(world).last();

    let type_registry = world.resource::<AppTypeRegistry>().clone();
    let registry = type_registry.read();

    let components: Vec<(String, ComponentId, Option<Box<dyn Reflect>>)> =
        if let Some(entity) = entity {
            let entity_ref = world.entity(entity);
            entity_ref
                .archetype()
                .components()
                .iter()
                .filter_map(|component_id| {
                    let info = world.components().get_info(*component_id)?;
                    let type_id = info.type_id()?;
                    if let Some(registration) = registry.get(type_id) {
                        let reflect_component = registration.data::<ReflectComponent>()?;
                        let reflected = reflect_component.reflect(entity_ref)?;
                        Some((
                            registration
                                .type_info()
                                .type_path_table()
                                .short_path()
                                .to_string(),
                            *component_id,
                            reflected.reflect_clone().ok(),
                        ))
                    } else {
                        Some((
                            world.components().get_name(*component_id)?.to_string(),
                            *component_id,
                            None,
                        ))
                    }
                })
                .collect()
        } else {
            Vec::new()
        };

    let mut mutations: Vec<(ComponentId, Box<dyn PartialReflect>)> = vec![];

    world.resource_scope(|_world, mut ui_builder: Mut<UiBuilder>| {
        let Some(ui) = ui_builder.ui() else { return };

        ui.window("Selected##selected").build(|| {
            if let Some(entity) = entity {
                ui.text(format!("Entity: {:?}", entity));
                ui.separator();

                for (name, component_id, mut reflected) in components {
                    if let Some(mut reflected) = reflected {
                        if ui.collapsing_header(&name, TreeNodeFlags::DEFAULT_OPEN) {
                            let changed = draw_reflect_value_mut(
                                ui,
                                &name,
                                &format!("{}", component_id.index()),
                                Some(reflected.as_mut() as &mut dyn PartialReflect),
                                &registry,
                            );
                            if changed {
                                mutations.push((component_id, reflected));
                            }
                        }
                    } else {
                        ui.text(name);
                    }
                }
            } else {
                ui.text("Nothing Selected");
            }
        });
    });

    if let Some(entity) = entity {
        for (component_id, new_value) in mutations {
            let info = world.components().get_info(component_id).unwrap();
            let type_id = info.type_id().unwrap();
            let registration = registry.get(type_id).unwrap();
            let reflect_component = registration.data::<ReflectComponent>().unwrap();
            reflect_component.apply(&mut world.entity_mut(entity), new_value.as_ref());
        }
    }
}

/// Returns true if any field was changed
fn draw_reflect_value_mut(
    ui: &imgui::Ui,
    name: &str,
    id: &str,
    value: Option<&mut dyn PartialReflect>,
    registry: &TypeRegistry,
) -> bool {
    let mut changed = false;
    let module_name = value
        .as_ref()
        .and_then(|n| n.reflect_crate_name().map(|v| v.to_owned()));
    let type_ident = value
        .as_ref()
        .and_then(|n| n.reflect_type_ident().map(|v| v.to_owned()));
    match value.map(|v| v.reflect_mut()) {
        Some(ReflectMut::Struct(s)) => {
            if let Some(module_name) = module_name
                && module_name == "glam"
            {
                changed |= draw_primitive_mut(ui, name, id, s);
            } else {
                let field_count = s.field_len();
                for i in 0..field_count {
                    let field_name = s.name_at(i).unwrap_or("?").to_string();
                    let field_val = s.field_at_mut(i);
                    let id = format!("{}_{}", id, i);
                    let name = format!("{field_name}");
                    changed |= draw_reflect_value_mut(ui, &name, &id, field_val, registry);
                }
            }
        }
        Some(ReflectMut::TupleStruct(ts)) => {
            let field_count = ts.field_len();
            for i in 0..field_count {
                let field_val = ts.field_mut(i);
                let id = format!("{}{}", id, i);
                let name = format!("[{}]", i);
                changed |= draw_reflect_value_mut(ui, &name, &id, field_val, registry);
            }
        }
        Some(ReflectMut::Tuple(t)) => {
            let field_count = t.field_len();
            for i in 0..field_count {
                let field_val = t.field_mut(i);
                let id = format!("{}{}", id, i);
                let name = format!("({})", i);
                changed |= draw_reflect_value_mut(ui, &name, &id, field_val, registry);
            }
        }
        Some(ReflectMut::Enum(e)) => {
            ui.text(format!("variant: {}", e.variant_name()));
            let field_count = e.field_len();
            for i in 0..field_count {
                let field_name = e.name_at(i).unwrap_or("?").to_string();
                let field_val = e.field_at_mut(i);
                let id = format!("{}::{}", id, field_name);
                let name = format!("::{}", field_name);
                changed |= draw_reflect_value_mut(ui, &id, &name, field_val, registry);
            }
        }
        Some(ReflectMut::List(l)) => {
            let len = l.len();
            for i in 0..len {
                let item = l.get_mut(i);
                let id = format!("{}{}", id, i);
                let name = format!("[{}]", i);
                changed |= draw_reflect_value_mut(ui, &name, &id, item, registry);
            }
        }
        Some(ReflectMut::Opaque(v)) => {
            changed |= draw_primitive_mut(ui, name, id, v);
        }
        _ => {
            ui.text(format!("{}##{}", name, id));
        }
    }
    changed
}

fn draw_primitive_mut(
    ui: &imgui::Ui,
    name: &str,
    id: &str,
    value: &mut dyn PartialReflect,
) -> bool {
    let label = format!("{}##{}", name, id);
    if let Some(v) = value.try_downcast_mut::<f32>() {
        let mut val = *v;
        if ui.input_float(label, &mut val).build() {
            *v = val;
            return true;
        }
    } else if let Some(v) = value.try_downcast_mut::<f64>() {
        let mut val = *v as f32;
        if ui.input_float(label, &mut val).build() {
            *v = val as f64;
            return true;
        }
    } else if let Some(v) = value.try_downcast_mut::<i32>() {
        let mut val = *v;
        if ui.input_int(label, &mut val).build() {
            *v = val;
            return true;
        }
    } else if let Some(v) = value.try_downcast_mut::<u32>() {
        let mut val = *v as i32;
        if ui.input_int(label, &mut val).build() {
            *v = val.max(0) as u32;
            return true;
        }
    } else if let Some(v) = value.try_downcast_mut::<u64>() {
        let mut val = *v as i32;
        if ui.input_int(label, &mut val).build() {
            *v = val.max(0) as u64;
            return true;
        }
    } else if let Some(v) = value.try_downcast_mut::<bool>() {
        if ui.checkbox(label, v) {
            return true;
        }
    } else if let Some(v) = value.try_downcast_mut::<String>() {
        if ui.input_text(label, v).build() {
            return true;
        }
    } else if let Some(v) = value.try_downcast_mut::<glam::f32::Vec2>() {
        let mut val = [v.x, v.y];
        if ui.input_float2(label, &mut val).build() {
            *v = glam::f32::Vec2::from(val);
            return true;
        }
    } else if let Some(v) = value.try_downcast_mut::<glam::f32::Vec3>() {
        let mut val = [v.x, v.y, v.z];
        if ui.input_float3(label, &mut val).build() {
            *v = glam::f32::Vec3::from(val);
            return true;
        }
    } else if let Some(v) = value.try_downcast_mut::<glam::f32::Vec4>() {
        let mut val = [v.x, v.y, v.z, v.w];
        if ui.input_float4(label, &mut val).build() {
            *v = glam::f32::Vec4::from(val);
            return true;
        }
    } else if let Some(v) = value.try_downcast_mut::<glam::f32::Quat>() {
        // Edit as euler angles — much more intuitive than raw xyzw
        let (z, x, y) = v.to_euler(glam::EulerRot::ZXY);
        let mut euler = [x.to_degrees(), y.to_degrees(), z.to_degrees()];
        if ui.input_float3(label, &mut euler).build() {
            *v = glam::f32::Quat::from_euler(
                glam::EulerRot::ZXY,
                euler[2].to_radians(),
                euler[0].to_radians(),
                euler[1].to_radians(),
            );
            return true;
        }
    } else if let Some(v) = value.try_downcast_mut::<glam::f32::Mat4>() {
        let mut changed = false;
        let mut cols = v.to_cols_array_2d();
        for (i, col) in cols.iter_mut().enumerate() {
            let id = format!("{}[{}]", label, i);
            if ui.input_float4(&id, col).build() {
                changed = true;
            }
        }
        if changed {
            *v = glam::f32::Mat4::from_cols_array_2d(&cols);
            return true;
        }
    } else if let Some(v) = value.try_downcast_mut::<Entity>() {
        ui.text(format!("{}: Entity {}", name, *v));
        if let Some(target) = ui.drag_drop_target() {
            if let Some(payload) =
                target.accept_payload::<u64, _>("ENTITY_DND", imgui::DragDropFlags::empty())
            {
                if let Ok(bits) = payload {
                    let dragged = Entity::from_bits(bits.data);
                    *v = dragged;
                    return true;
                }
            }
        }
    } else {
        ui.text(format!("{}: <{}>", name, value.reflect_type_path()));
    }
    false
}

#[derive(Default)]
pub struct HierarchyState {
    context_target: Option<Entity>,
}

pub(crate) fn hierarchy_ui(
    mut ui: ResMut<UiBuilder>,
    mut state: Local<HierarchyState>,
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
) {
    let Some(ui) = ui.ui() else { return };

    ui.window("Hierarchy##hierarchy")
        .size([250.0, 400.0], imgui::Condition::FirstUseEver)
        .build(|| {
            ui.child_window("dnd_target").build(|| {
                if ui.button("+ Add") {
                    cmd.spawn(Transform::default());
                }
                ui.same_line();
                if ui.button("- Remove") {
                    for e in &selected {
                        cmd.entity(e).despawn();
                    }
                }
                ui.separator();

                let mut roots: Vec<Entity> = instances
                    .iter()
                    .filter(|(_, _, _, _, parent, _)| parent.is_none())
                    .map(|(e, _, _, _, _, _)| e)
                    .collect();

                roots.sort();

                for root in roots {
                    draw_entity_node(&mut cmd, ui, root, &mut instances, &selected, &mut state);
                }
            });
            if let Some(target) = state.context_target {
                ui.popup("##entity_context", || {
                    // if ui.menu_item("Duplicate") {
                    //     state.context_target = None;
                    // }
                    // if ui.menu_item("Delete") {
                    //     commands.entity(target).despawn();
                    //     state.context_target = None;
                    // }
                    // if ui.menu_item("Rename") {
                    //     state.rename_target = Some(target);
                    //     state.rename_buf = format!("{:?}", target);
                    //     state.context_target = None;
                    // }
                    ui.text("test");
                });
            }
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
    state: &mut HierarchyState,
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

    if ui.is_item_clicked_with_button(imgui::MouseButton::Right) {
        state.context_target = Some(this_entity);
        ui.open_popup("##entity_context");
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
            draw_entity_node(cmd, ui, child, instances, selected, state);
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
    assets: Res<Assets<GpuMeshletMesh>>,
    ui: Res<UiContext>,
    mut picked: Query<(Entity, &GlobalTransform, &Instance, &mut Transform), With<Selected>>,
    all_picked: Query<Entity, With<Selected>>,
    mut local: Local<Option<DragState>>,
) {
    let mut click_pos = None;
    let mut touch_id = None;
    if let Some(touch) = touches.iter_just_pressed().next() {
        click_pos = Some(touch.position());
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

    if let Some((entity, global_transfrom, instance, mut transform)) = picked.iter_mut().next() {
        if let Some(drag) = local.as_ref() {
            if let Some(pos) = if let Some(touch) = drag.touchid {
                touches.get_pressed(touch).map(|e| e.position())
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

        let Some(mesh) = assets.get(&instance.mesh) else {
            return;
        };
        let center = Vec3::from(mesh.header.aabb.center);
        let directions = [
            (global_transfrom.right(), transform.right()),
            (global_transfrom.up(), transform.up()),
            (global_transfrom.forward(), transform.forward()),
        ];
        for (global_dir, local_dir) in directions {
            if gizzmos.draw_gizzmo_check_clicked(
                &ArrowGizzmo {
                    color: (*local_dir).abs().extend(1.0),
                    start: center,
                    end: center + *local_dir,
                    width: 1.0,
                },
                click_pos,
                global_transfrom.to_matrix(),
            ) && local.is_none()
            {
                let world_space_axis_origin = global_transfrom.transform_point(center);
                let scale =
                    global_transfrom.scale().dot(*global_dir) / transform.scale.dot(*local_dir);
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

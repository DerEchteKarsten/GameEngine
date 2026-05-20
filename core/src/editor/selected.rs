use std::{any::TypeId};

use bevy::{
    asset::{Asset, Handle},
    ecs::{
        component::ComponentId,
        entity::Entity,
        query::With,
        reflect::{AppTypeRegistry, ReflectComponent},
        world::{Mut, World},
    },
    reflect::{
        PartialReflect, Reflect, ReflectMut, TypeData, TypeInfo, TypeRegistry, reflect_trait,
    },
};
use glam::Mat3A;
use imgui::{TreeNodeFlags, Ui, drag_drop::DragDropPayloadPod};

use crate::{
    editor::{asset_browser::AssetDND, picking::Selected},
    ui::UiBuilder,
};
use std::sync::Arc;

#[reflect_trait]
pub trait EditorView {
    fn ui(
        &mut self,
        world: &mut World,
        disabled: bool,
        ui: &imgui::Ui,
        name: &str,
        id: &str,
        registry: &TypeRegistry,
    ) -> bool;
}

impl EditorView for f32 {
    fn ui(
        &mut self,
        world: &mut World,
        disabled: bool,
        ui: &Ui,
        name: &str,
        id: &str,
        _registry: &TypeRegistry,
    ) -> bool {
        let _d = ui.begin_disabled(disabled);
        let mut val = *self;
        if ui
            .input_float(format!("{}##{}", name, id), &mut val)
            .build()
        {
            *self = val;
            return true;
        }
        false
    }
}

impl EditorView for f64 {
    fn ui(
        &mut self,
        world: &mut World,
        disabled: bool,
        ui: &Ui,
        name: &str,
        id: &str,
        _registry: &TypeRegistry,
    ) -> bool {
        let _d = ui.begin_disabled(disabled);
        let mut val = *self as f32;
        if ui
            .input_float(format!("{}##{}", name, id), &mut val)
            .build()
        {
            *self = val as f64;
            return true;
        }
        false
    }
}

impl EditorView for i32 {
    fn ui(
        &mut self,
        world: &mut World,
        disabled: bool,
        ui: &Ui,
        name: &str,
        id: &str,
        _registry: &TypeRegistry,
    ) -> bool {
        let _d = ui.begin_disabled(disabled);
        let mut val = *self;
        if ui.input_int(format!("{}##{}", name, id), &mut val).build() {
            *self = val;
            return true;
        }
        false
    }
}

impl EditorView for u32 {
    fn ui(
        &mut self,
        world: &mut World,
        disabled: bool,
        ui: &Ui,
        name: &str,
        id: &str,
        _registry: &TypeRegistry,
    ) -> bool {
        let _d = ui.begin_disabled(disabled);
        let mut val = *self as i32;
        if ui.input_int(format!("{}##{}", name, id), &mut val).build() {
            *self = val.max(0) as u32;
            return true;
        }
        false
    }
}

impl EditorView for u64 {
    fn ui(
        &mut self,
        world: &mut World,
        disabled: bool,
        ui: &Ui,
        name: &str,
        id: &str,
        _registry: &TypeRegistry,
    ) -> bool {
        let _d = ui.begin_disabled(disabled);
        let mut val = *self as i32;
        if ui.input_int(format!("{}##{}", name, id), &mut val).build() {
            *self = val.max(0) as u64;
            return true;
        }
        false
    }
}

impl EditorView for bool {
    fn ui(
        &mut self,
        world: &mut World,
        disabled: bool,
        ui: &Ui,
        name: &str,
        id: &str,
        _registry: &TypeRegistry,
    ) -> bool {
        let _d = ui.begin_disabled(disabled);
        ui.checkbox(format!("{}##{}", name, id), self)
    }
}

impl EditorView for String {
    fn ui(
        &mut self,
        world: &mut World,
        disabled: bool,
        ui: &Ui,
        name: &str,
        id: &str,
        _registry: &TypeRegistry,
    ) -> bool {
        let _d = ui.begin_disabled(disabled);
        ui.input_text(format!("{}##{}", name, id), self).build()
    }
}

impl EditorView for glam::Vec2 {
    fn ui(
        &mut self,
        world: &mut World,
        disabled: bool,
        ui: &Ui,
        name: &str,
        id: &str,
        _registry: &TypeRegistry,
    ) -> bool {
        let _d = ui.begin_disabled(disabled);
        let mut val = [self.x, self.y];
        if ui
            .input_float2(format!("{}##{}", name, id), &mut val)
            .build()
        {
            *self = glam::Vec2::from(val);
            return true;
        }
        false
    }
}

impl EditorView for glam::Vec3 {
    fn ui(
        &mut self,
        world: &mut World,
        disabled: bool,
        ui: &Ui,
        name: &str,
        id: &str,
        _registry: &TypeRegistry,
    ) -> bool {
        let _d = ui.begin_disabled(disabled);
        let mut val = [self.x, self.y, self.z];
        if ui
            .input_float3(format!("{}##{}", name, id), &mut val)
            .build()
        {
            *self = glam::Vec3::from(val);
            return true;
        }
        false
    }
}

impl EditorView for glam::Vec4 {
    fn ui(
        &mut self,
        world: &mut World,
        disabled: bool,
        ui: &Ui,
        name: &str,
        id: &str,
        _registry: &TypeRegistry,
    ) -> bool {
        let _d = ui.begin_disabled(disabled);
        let mut val = [self.x, self.y, self.z, self.w];
        if ui
            .input_float4(format!("{}##{}", name, id), &mut val)
            .build()
        {
            *self = glam::Vec4::from(val);
            return true;
        }
        false
    }
}

impl EditorView for glam::Quat {
    fn ui(
        &mut self,
        world: &mut World,
        disabled: bool,
        ui: &Ui,
        name: &str,
        id: &str,
        _registry: &TypeRegistry,
    ) -> bool {
        let _d = ui.begin_disabled(disabled);
        let (z, x, y) = self.to_euler(glam::EulerRot::ZXY);
        let mut euler = [x.to_degrees(), y.to_degrees(), z.to_degrees()];
        if ui
            .input_float3(format!("{}##{}", name, id), &mut euler)
            .build()
        {
            *self = glam::Quat::from_euler(
                glam::EulerRot::ZXY,
                euler[2].to_radians(),
                euler[0].to_radians(),
                euler[1].to_radians(),
            );
            return true;
        }
        false
    }
}

impl EditorView for glam::Mat4 {
    fn ui(
        &mut self,
        world: &mut World,
        disabled: bool,
        ui: &Ui,
        name: &str,
        id: &str,
        _registry: &TypeRegistry,
    ) -> bool {
        let _d = ui.begin_disabled(disabled);
        let mut changed = false;
        let mut cols = self.transpose().to_cols_array_2d();
        for (i, col) in cols.iter_mut().enumerate() {
            if ui
                .input_float4(format!("{}[{}]##{}", name, i, id), col)
                .build()
            {
                changed = true;
            }
        }
        if changed {
            *self = glam::Mat4::from_cols_array_2d(&cols);
        }
        changed
    }
}

impl EditorView for glam::Affine3A {
    fn ui(
        &mut self,
        world: &mut World,
        disabled: bool,
        ui: &Ui,
        name: &str,
        id: &str,
        _registry: &TypeRegistry,
    ) -> bool {
        let _d = ui.begin_disabled(disabled);
        let mut changed = false;
        let mut mat = self.matrix3.transpose().to_cols_array_2d();
        for (i, row) in mat.iter_mut().enumerate() {
            if ui
                .input_float3(format!("{} Matrix[{}]##{}", name, i, id), row)
                .build()
            {
                changed = true;
            }
        }
        if ui
            .input_float3(
                format!("{} Translation##{}", name, id),
                &mut self.translation,
            )
            .build()
        {
            changed = true;
        }
        if changed {
            self.matrix3 = Mat3A::from_cols_array_2d(&mat).transpose();
        }
        changed
    }
}

impl EditorView for Entity {
    fn ui(
        &mut self,
        world: &mut World,
        disabled: bool,
        ui: &Ui,
        name: &str,
        id: &str,
        _registry: &TypeRegistry,
    ) -> bool {
        let val = format!("Entity {}", self.index());
        object_field::<u64>(
            disabled,
            ui,
            id,
            name,
            Some(&val),
            "Entity",
            "ENTITY_DND",
            |payload| {
                *self = Entity::from_bits(payload.data);
            },
        )
    }
}

impl<A: Asset> EditorView for Handle<A> {
    fn ui(
        &mut self,
        world: &mut World,
        disabled: bool,
        ui: &Ui,
        name: &str,
        id: &str,
        _registry: &TypeRegistry,
    ) -> bool {
        let path = self.path();
        let ident = A::type_ident();
        let text_color = if disabled {
            UiBuilder::TEXT_DIM
        } else if path.is_some() {
            UiBuilder::TEXT
        } else {
            UiBuilder::TEXT_DIM
        };
        let ident_text = ident.unwrap_or("Unknown Asset");
        let default_label = format!("None ({})", ident_text);
        let display_text = path
            .map(|v| format!("{}({})", ident_text, v.to_string()))
            .unwrap_or(default_label);
        let draw = ui.get_window_draw_list();
        let pos = ui.cursor_screen_pos();
        let available = (ui.content_region_avail()[0] - ui.calc_text_size(name)[0]).min(240.0);
        let height = 20.0;
        let size = [available, height];

        draw.add_rect(pos, [pos[0] + size[0], pos[1] + size[1]], UiBuilder::S0)
            .rounding(1.0)
            .build();

        ui.invisible_button(id, size);
        let hovered = ui.is_item_hovered();

        let border_color = if disabled {
            UiBuilder::S0
        } else if hovered {
            UiBuilder::BLUE
        } else {
            UiBuilder::S2
        };
        draw.add_rect(pos, [pos[0] + size[0], pos[1] + size[1]], border_color)
            .rounding(1.0)
            .thickness(1.0)
            .build();

        let text_pos = [
            pos[0] + 6.0,
            pos[1] + (height - ui.text_line_height()) * 0.5,
        ];
        draw.add_text(text_pos, text_color, display_text);

        let mut changed = false;
        unsafe {
            if imgui::sys::igBeginDragDropTarget() {
                let string = format!("ASSET_DND_{:?}\0", TypeId::of::<A>());
                let payload = imgui::sys::igAcceptDragDropPayload(
                    string.as_ptr() as *const i8,
                    imgui::sys::ImGuiDragDropFlags_None as i32,
                );
                if !payload.is_null() {
                    if let Some(untyped) = world
                        .get_resource_mut::<AssetDND>()
                        .as_mut()
                        .and_then(|v| v.0.take())
                    {
                        if let Ok(typed) = untyped.try_typed::<A>() {
                            *self = typed;
                            changed = true;
                        }
                    }
                }
                imgui::sys::igEndDragDropTarget();
            }
        }
        ui.same_line();
        ui.text(name);
        changed
    }
}

fn object_field<T: Copy + 'static>(
    disabled: bool,
    ui: &Ui,
    id: &str,
    label: &str,
    content: Option<&str>,
    type_name: &str,
    dnd_name: impl AsRef<str>,
    payload_handler: impl FnOnce(DragDropPayloadPod<T>),
) -> bool {
    let text_color = if disabled {
        UiBuilder::TEXT_DIM
    } else if content.is_some() {
        UiBuilder::TEXT
    } else {
        UiBuilder::TEXT_DIM
    };
    let default_label = format!("None ({})", type_name);
    let display_text = content.unwrap_or(&default_label);
    let draw = ui.get_window_draw_list();
    let pos = ui.cursor_screen_pos();
    let available = (ui.content_region_avail()[0] - ui.calc_text_size(label)[0]).min(240.0);
    let height = 20.0;
    let size = [available, height];

    draw.add_rect(pos, [pos[0] + size[0], pos[1] + size[1]], UiBuilder::S0)
        .rounding(1.0)
        .build();

    ui.invisible_button(id, size);
    let hovered = ui.is_item_hovered();

    let border_color = if disabled {
        UiBuilder::S0
    } else if hovered {
        UiBuilder::BLUE
    } else {
        UiBuilder::S2
    };
    draw.add_rect(pos, [pos[0] + size[0], pos[1] + size[1]], border_color)
        .rounding(1.0)
        .thickness(1.0)
        .build();

    let text_pos = [
        pos[0] + 6.0,
        pos[1] + (height - ui.text_line_height()) * 0.5,
    ];
    draw.add_text(text_pos, text_color, display_text);

    let mut changed = false;
    if let Some(target) = ui.drag_drop_target() {
        if let Some(payload) =
            target.accept_payload::<T, _>(dnd_name, imgui::DragDropFlags::empty())
        {
            if let Ok(playoad) = payload {
                payload_handler(playoad);
                changed = true;
            }
        }
    }

    ui.same_line();
    ui.text(label);
    changed
}

fn draw_reflect_value_mut(
    world: &mut World,
    disabled: bool,
    ui: &imgui::Ui,
    name: Option<&str>,
    id: &str,
    mut value: Option<&mut dyn PartialReflect>,
    registry: &TypeRegistry,
) -> bool {
    let mut changed = false;

    let type_short = value
        .as_ref()
        .map(|v| v.reflect_short_type_path().to_owned())
        .unwrap_or_default();

    // Check for EditorView first — covers all primitives + custom types
    if let Some(v) = value.as_ref().and_then(|v| v.try_as_reflect()) {
        if let Some(reg) = registry.get(v.type_id()) {
            if let Some(editor_view) = reg.data::<ReflectEditorView>() {
                if let Some(v) = value.as_mut().and_then(|v| v.try_as_reflect_mut()) {
                    if let Some(concrete) = editor_view.get_mut(v) {
                        let _d = ui.begin_disabled(disabled && name.is_some());
                        return concrete.ui(world, disabled, ui, name.unwrap_or(""), id, registry);
                    }
                }
            }
        }
    }

    // Otherwise recurse into the structure
    let _disabled = ui.begin_disabled(disabled && name.is_some());

    match value {
        Some(v) => match v.reflect_mut() {
            ReflectMut::Struct(s) => {
                let field_count = s.field_len();
                if let Some(node) = draw_reflect_header(ui, name, &type_short, id, field_count == 0)
                {
                    ui.indent();
                    for i in 0..field_count {
                        let field_name = s.name_at(i).unwrap_or("?").to_string();
                        let field_val = s.field_at_mut(i);
                        let child_id = format!("{}_{}", id, i);
                        changed |= draw_reflect_value_mut(
                            world,
                            disabled,
                            ui,
                            Some(&field_name),
                            &child_id,
                            field_val,
                            registry,
                        );
                    }
                    ui.unindent();
                }
            }
            ReflectMut::TupleStruct(ts) => {
                let field_count = ts.field_len();
                if let Some(node) = draw_reflect_header(ui, name, &type_short, id, field_count == 0)
                {
                    ui.indent();
                    for i in 0..field_count {
                        let field_val = ts.field_mut(i);
                        let child_id = format!("{}{}", id, i);
                        let fname = format!("({})", i);
                        changed |= draw_reflect_value_mut(
                            world,
                            disabled,
                            ui,
                            Some(&fname),
                            &child_id,
                            field_val,
                            registry,
                        );
                    }
                    ui.unindent();
                }
            }
            ReflectMut::Tuple(t) => {
                let field_count = t.field_len();
                if let Some(token) =
                    draw_reflect_header(ui, name, &type_short, id, field_count == 0)
                {
                    ui.indent();
                    for i in 0..field_count {
                        let field_val = t.field_mut(i);
                        let child_id = format!("{}{}", id, i);
                        let fname = format!("({})", i);
                        changed |= draw_reflect_value_mut(
                            world,
                            disabled,
                            ui,
                            Some(&fname),
                            &child_id,
                            field_val,
                            registry,
                        );
                    }
                    ui.unindent();
                }
            }
            ReflectMut::Enum(e) => {
                let variant = e.variant_name().to_string();
                let header_name = format!("{}::{}", type_short, variant);
                let field_count = e.field_len();
                if let Some(toke) =
                    draw_reflect_header(ui, name, &header_name, id, field_count == 0)
                {
                    ui.indent();

                    // // Variant selector
                    // let variant_names: Vec<&str> = if let Some(TypeInfo::Enum(ei)) = e.get_represented_type_info() {
                    //     ei.iter().map(|v| v.name()).collect()
                    // } else { vec![] };
                    // let current = variant_names.iter().position(|v| *v == variant).unwrap_or(0);
                    // let mut selected = current;
                    // ui.set_next_item_width(ui.content_region_avail()[0]);
                    // if ui.combo_simple_string(format!("##variant_{id}"), &mut selected, &variant_names) {
                    //     if selected as usize != current {
                    //         changed |= apply_enum_variant(e, variant_names[selected as usize], registry);
                    //     }
                    // }
                    for i in 0..field_count {
                        let field_name = e.name_at(i).unwrap_or("?").to_string();
                        let field_val = e.field_at_mut(i);
                        let child_id = format!("{}::{}", id, field_name);
                        changed |= draw_reflect_value_mut(
                            world,
                            disabled,
                            ui,
                            Some(&field_name),
                            &child_id,
                            field_val,
                            registry,
                        );
                    }
                    ui.unindent();
                }
            }
            ReflectMut::List(l) => {
                let len = l.len();
                let header_name = format!("{}[{}]", type_short, len);
                if let Some(toke) = draw_reflect_header(ui, name, &header_name, id, len == 0) {
                    ui.indent();
                    for i in 0..len {
                        let item = l.get_mut(i);
                        let child_id = format!("{}{}", id, i);
                        let fname = format!("[{}]", i);
                        changed |= draw_reflect_value_mut(
                            world,
                            disabled,
                            ui,
                            Some(&fname),
                            &child_id,
                            item,
                            registry,
                        );
                    }
                    ui.unindent();
                }
            }
            ReflectMut::Opaque(v) => {
                if let Some(name) = name {
                    ui.text_disabled(format!("<{}>: {}", v.reflect_short_type_path(), name));
                } else {
                    ui.text_disabled(format!("<{}>", v.reflect_short_type_path()));
                }
            }
            _ => {
                ui.text_disabled(type_short);
            }
        },
        None => {
            ui.text_disabled(type_short);
        }
    }

    changed
}

fn draw_reflect_header<'a>(
    ui: &'a imgui::Ui,
    field_name: Option<&str>,
    type_name: &str,
    id: &str,
    is_leaf: bool,
) -> Option<imgui::TreeNodeToken<'a>> {
    let label = if let Some(field_name) = field_name {
        format!("{type_name} {field_name}##{id}")
    } else {
        format!("{type_name}##{id}")
    };
    let flags = TreeNodeFlags::OPEN_ON_ARROW
        | TreeNodeFlags::SPAN_AVAIL_WIDTH
        | TreeNodeFlags::DEFAULT_OPEN
        | TreeNodeFlags::FRAME_PADDING
        | TreeNodeFlags::FRAMED
        | if is_leaf {
            TreeNodeFlags::LEAF
        } else {
            TreeNodeFlags::empty()
        };

    ui.tree_node_config(&label).flags(flags).push()
}

pub(crate) fn selected_ui(world: &mut World) {
    let mut q = world.query_filtered::<Entity, With<Selected>>();
    let entity = q.iter(world).last();

    let type_registry = world.resource::<AppTypeRegistry>().clone();
    let registry = type_registry.read();

    let mut components: Vec<(bool, String, ComponentId, Option<Box<dyn Reflect>>)> =
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
                            info.mutable(),
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
                            false,
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

    world.resource_scope(|world, mut ui_builder: Mut<UiBuilder>| {
        let Some(ui) = ui_builder.ui() else { return };

        ui.window("Selected##selected").build(|| {
            if let Some(entity) = entity {
                ui.text(format!("Entity: {:?}", entity));
                ui.separator();

                for (mutable, name, component_id, reflected) in components {
                    let mutable = mutable && name != "Children";
                    if let Some(mut reflected) = reflected {
                        let changed = draw_reflect_value_mut(
                            world,
                            !mutable,
                            ui,
                            None,
                            &format!("{}", component_id.index()),
                            Some(reflected.as_mut() as &mut dyn PartialReflect),
                            &registry,
                        );
                        if changed && mutable {
                            mutations.push((component_id, reflected));
                        }
                    } else {
                        let _dis = ui.begin_disabled(true);
                        ui.collapsing_header(
                            format!(
                                "{}##{}",
                                name.split("::").last().unwrap_or("?"),
                                component_id.index()
                            ),
                            TreeNodeFlags::SPAN_AVAIL_WIDTH
                                | TreeNodeFlags::FRAME_PADDING
                                | TreeNodeFlags::FRAMED
                                | TreeNodeFlags::LEAF,
                        );
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

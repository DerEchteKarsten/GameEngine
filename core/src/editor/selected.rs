//! Reflection-driven inspector panel for editing components of the selected entity.
use std::hash::{DefaultHasher, Hash, Hasher};

use bevy::{
    asset::{Asset, Handle},
    ecs::{
        component::ComponentId,
        entity::Entity,
        query::With,
        reflect::{AppTypeRegistry, ReflectComponent},
        system::SystemState,
        world::World,
    },
    reflect::{PartialReflect, Reflect, ReflectMut, TypeRegistry, reflect_trait},
};
use glam::{EulerRot, Quat, Vec2, Vec3};

use crate::{
    editor::picking::Selected,
    ui::{
        UiContext,
        builder::{UiBuilder, UiWindowBuilder},
        widgets::{Ellipsis, fit_text},
        window::DrawSettings,
    },
};

#[reflect_trait]
pub trait EditorView {
    fn ui(
        &mut self,
        ui: &mut UiWindowBuilder,
        name: &str,
        id: u64,
        registry: &TypeRegistry,
    ) -> bool;
}

/// `EditorView` for number types, edited with `$input` through `$wide`.
macro_rules! number_view {
    ($input:ident, $wide:ty: $($t:ty),*) => {$(
        impl EditorView for $t {
            fn ui(&mut self, ui: &mut UiWindowBuilder, name: &str, id: u64, _: &TypeRegistry) -> bool {
                let val = *self;
                *self = ui.property(name, |ui, width| ui.$input(id, val as $wide, width)) as $t;
                *self != val
            }
        }
    )*};
}
number_view!(float_input, f64: f32, f64);
number_view!(int_input, i64: i32, u32, u64);

/// `EditorView` for vector types, one number field per component.
macro_rules! vector_view {
    ($($t:ty),*) => {$(
        impl EditorView for $t {
            fn ui(&mut self, ui: &mut UiWindowBuilder, name: &str, id: u64, _: &TypeRegistry) -> bool {
                let val = *self;
                let values = val.to_array().map(|c| c as f64);
                let values = ui.property(name, |ui, width| ui.float_inputs(id, values, width));
                *self = <$t>::from_array(values.map(|c| c as f32));
                *self != val
            }
        }
    )*};
}
vector_view!(Vec2, Vec3, glam::Vec4);

impl EditorView for bool {
    fn ui(&mut self, ui: &mut UiWindowBuilder, name: &str, _: u64, _: &TypeRegistry) -> bool {
        let val = *self;
        *self = ui.property(name, |ui, _| ui.checkbox("", val));
        *self != val
    }
}

impl EditorView for String {
    fn ui(&mut self, ui: &mut UiWindowBuilder, name: &str, id: u64, _: &TypeRegistry) -> bool {
        ui.property(name, |ui, width| ui.text_input(id, self, width))
    }
}

impl EditorView for Quat {
    fn ui(&mut self, ui: &mut UiWindowBuilder, name: &str, id: u64, _: &TypeRegistry) -> bool {
        let val = *self;
        let (x, y, z) = val.to_euler(EulerRot::XYZ);
        let [x, y, z] = ui
            .property(name, |ui, width| {
                ui.float_inputs(id, [x, y, z].map(|c| c as f64), width)
            })
            .map(|c| c as f32);
        *self = Quat::from_euler(EulerRot::XYZ, x, y, z);
        *self != val
    }
}

impl EditorView for glam::Mat4 {
    fn ui(&mut self, ui: &mut UiWindowBuilder, name: &str, id: u64, registry: &TypeRegistry) -> bool {
        ui.text(name);
        ui.indent(|ui| {
            let axes = ["x-Axis", "y-Axis", "z-Axis", "w-Axis"];
            let mut changed = false;
            for (i, name) in axes.into_iter().enumerate() {
                changed |= self.col_mut(i).ui(ui, name, child_id(id, i), registry);
            }
            changed
        })
    }
}

impl EditorView for glam::Affine3A {
    fn ui(&mut self, ui: &mut UiWindowBuilder, name: &str, id: u64, registry: &TypeRegistry) -> bool {
        ui.text(name);
        let (mut scale, mut rotation, mut translation) = self.to_scale_rotation_translation();
        let changed = ui.indent(|ui| {
            let scale = scale.ui(ui, "scale", child_id(id, 0), registry);
            let rotation = rotation.ui(ui, "rotation", child_id(id, 1), registry);
            let translation = translation.ui(ui, "translation", child_id(id, 2), registry);
            scale || rotation || translation
        });
        if changed {
            *self = glam::Affine3A::from_scale_rotation_translation(
                scale,
                rotation.normalize(),
                translation,
            );
        }
        changed
    }
}

impl EditorView for Entity {
    fn ui(&mut self, ui: &mut UiWindowBuilder, name: &str, _: u64, _: &TypeRegistry) -> bool {
        ui.property(name, |ui, _| ui.label(format!("Entity {}", self.index())));
        false
    }
}

/// A boxed asset path, `width` wide, that accepts dropped `Handle<A>`s.
fn handle_slot<A: Asset>(
    ui: &mut UiWindowBuilder,
    handle: Option<&Handle<A>>,
    width: f32,
) -> Option<Handle<A>> {
    let label = match handle {
        Some(handle) => handle
            .path()
            .map(|path| path.to_string())
            .unwrap_or_else(|| "Unknown".to_string()),
        None => "None".to_string(),
    };
    let color = if handle.is_none() {
        UiContext::TEXT_DIM
    } else {
        ui.text_color()
    };
    let pad = UiWindowBuilder::child_offset();
    ui.rect(
        Vec2::new(width, UiWindowBuilder::widget_height()),
        DrawSettings::default(),
    );
    let rect = ui.prev_element;
    // Keep the end of a long path: that is where the file name and sub-asset label are.
    let label = fit_text(&label, width - pad.x * 2.0, Ellipsis::Start);
    ui.paint_text((rect.min + pad).round(), &label, color, rect);
    ui.asset_drop_target::<A>()
}

impl<A: Asset> EditorView for Handle<A> {
    fn ui(&mut self, ui: &mut UiWindowBuilder, name: &str, _: u64, _: &TypeRegistry) -> bool {
        let dropped = ui.property(name, |ui, width| handle_slot(ui, Some(&*self), width));
        dropped.map(|handle| *self = handle).is_some()
    }
}

impl<A: Asset> EditorView for Option<Handle<A>> {
    fn ui(&mut self, ui: &mut UiWindowBuilder, name: &str, _: u64, _: &TypeRegistry) -> bool {
        const CLEAR: &str = "x";
        let clear_width = UiContext::text_len(CLEAR)
            + UiWindowBuilder::child_offset().x * 2.0
            + UiContext::ELEMENT_GAP.x as f32;
        let (dropped, cleared) = ui.property(name, |ui, width| {
            let dropped = handle_slot(ui, self.as_ref(), width - clear_width);
            (dropped, self.is_some() && ui.button(CLEAR))
        });
        let changed = dropped.is_some() || cleared;
        if let Some(handle) = dropped {
            *self = Some(handle);
        } else if cleared {
            *self = None;
        }
        changed
    }
}

fn child_id(id: u64, i: impl Hash) -> u64 {
    let mut hash = DefaultHasher::new();
    i.hash(&mut hash);
    id.hash(&mut hash);
    hash.finish()
}

fn draw_reflect_value(
    ui: &mut UiWindowBuilder,
    name: Option<&str>,
    id: u64,
    value: &mut dyn PartialReflect,
    registry: &TypeRegistry,
) -> bool {
    let mut changed = false;

    let type_short = value.reflect_short_type_path().to_owned();

    if let Some(v) = value.try_as_reflect_mut()
        && let Some(reg) = registry.get(v.type_id())
        && let Some(editor_view) = reg.data::<ReflectEditorView>()
        && let Some(concrete) = editor_view.get_mut(v.as_reflect_mut())
    {
        return concrete.ui(ui, name.unwrap_or(""), id, registry);
    }
    // ui.text("Doesnt have Editor View");
    // ui.text("Not in registry");
    // ui.text("Cant Reflect");

    let label = name.map(|n| format!("{}: ", n)).unwrap_or("".to_string());

    match value.reflect_mut() {
        ReflectMut::Struct(s) => {
            ui.collapsable(true, id, format!("{}{type_short}", label), |ui| {
                for i in 0..s.field_len() {
                    let field_name = s.name_at(i).unwrap_or("UnknownField").to_string();
                    let Some(field_val) = s.field_at_mut(i) else {
                        continue;
                    };
                    changed |= draw_reflect_value(
                        ui,
                        Some(&field_name),
                        child_id(id, &field_name),
                        field_val,
                        registry,
                    );
                }
            });
        }
        ReflectMut::TupleStruct(ts) => {
            ui.collapsable(true, id, format!("{}{type_short}", label), |ui| {
                for i in 0..ts.field_len() {
                    let Some(field_val) = ts.field_mut(i) else {
                        continue;
                    };
                    let child_id = child_id(id, i);
                    changed |= draw_reflect_value(
                        ui,
                        Some(&format!("({})", i)),
                        child_id,
                        field_val,
                        registry,
                    );
                }
            });
        }
        ReflectMut::Tuple(t) => {
            ui.collapsable(true, id, format!("{}{type_short}", label), |ui| {
                for i in 0..t.field_len() {
                    let Some(field_val) = t.field_mut(i) else {
                        continue;
                    };
                    let child_id = child_id(id, i);
                    changed |= draw_reflect_value(
                        ui,
                        Some(&format!("({})", i)),
                        child_id,
                        field_val,
                        registry,
                    );
                }
            });
        }
        ReflectMut::Enum(e) => {
            let variant = e.variant_name().to_string();
            ui.collapsable(
                true,
                id,
                format!("{}{type_short}::{}", label, variant),
                |ui| {
                    for i in 0..e.field_len() {
                        let field_name = e.name_at(i).unwrap_or("0").to_string();
                        let Some(field_val) = e.field_at_mut(i) else {
                            continue;
                        };
                        let child_id = child_id(id, i);
                        changed |= draw_reflect_value(
                            ui,
                            Some(field_name.as_ref()),
                            child_id,
                            field_val,
                            registry,
                        );
                    }
                },
            );
        }
        ReflectMut::List(l) => {
            ui.collapsable(
                true,
                id,
                format!("{}{}[{}]", label, type_short, l.len()),
                |ui| {
                    for i in 0..l.len() {
                        let child_id = child_id(id, i);
                        let Some(child) = l.get_mut(i) else { continue };
                        changed |= draw_reflect_value(
                            ui,
                            Some(&format!("[{}]", i)),
                            child_id,
                            child,
                            registry,
                        );
                    }
                },
            );
        }
        ReflectMut::Opaque(v) => {
            if let Some(name) = name {
                ui.text(format!(
                    "{}: {} opaque {}",
                    name,
                    type_short,
                    v.reflect_short_type_path(),
                ));
            } else {
                ui.text(format!(
                    "{} opaque {}",
                    type_short,
                    v.reflect_short_type_path()
                ));
            }
        }
        ReflectMut::Set(v) => {
            ui.collapsable(
                true,
                id,
                format!("{}{}{{{}}}", label, type_short, v.len()),
                |ui| {
                    for (i, item) in v.iter().enumerate() {
                        changed |= ui.disabled(true, |ui| {
                            draw_reflect_value(
                                ui,
                                Some(""),
                                child_id(id, i),
                                unsafe {
                                    (item as *const dyn PartialReflect as *mut dyn PartialReflect)
                                        .as_mut()
                                        .unwrap()
                                },
                                registry,
                            )
                        });
                    }
                },
            );
        }
        ReflectMut::Array(v) => {
            ui.collapsable(
                true,
                id,
                format!("{}{}[{}]", label, type_short, v.len()),
                |ui| {
                    for i in 0..v.len() {
                        let Some(value) = v.get_mut(i) else { continue };
                        let child_id = child_id(id, i);
                        changed |= draw_reflect_value(
                            ui,
                            Some(&format!("[{}]", i)),
                            child_id,
                            value,
                            registry,
                        );
                    }
                },
            );
        }
        ReflectMut::Map(_v) => {
            ui.collapsable(true, id, format!("{}{}", label, type_short), |ui| {
                // for (i, (key, _)) in v.iter().enumerate() {
                //     let mut hash = DefaultHasher::new();
                //     id.hash(&mut hash);
                //     i.hash(&mut hash);
                //     let child_id = hash.finish();

                //     ui.horizontal();
                //     let before = ui.disabled;
                //     ui.disabled(true);
                //     changed |= draw_reflect_value(
                //         ui,
                //         Some(""),
                //         child_id,
                //         unsafe {
                //             (key as *const dyn PartialReflect as *mut dyn PartialReflect)
                //                 .as_mut()
                //                 .unwrap()
                //         }, //TODO
                //         registry,
                //     );
                //     ui.disabled(before);
                //     ui.text(" : ");
                //     if let Some(value) = v.get_mut(key) {
                //         changed |= draw_reflect_value(ui, Some(""), child_id, value, registry);
                //     }
                //     ui.vertical();
                // }
                ui.text("Fuck hashmaps");
            });
        }
    }

    changed
}

pub(crate) fn selected_ui(world: &mut World, state: &mut SystemState<UiBuilder<'static, 'static>>) {
    let mut q = world.query_filtered::<Entity, With<Selected>>();
    let entity = q.iter(world).last();

    let type_registry = world.resource::<AppTypeRegistry>().clone();
    let registry = type_registry.read();

    let components: Vec<(bool, String, ComponentId, Option<Box<dyn Reflect>>)> =
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

    let mut ui = state.get_mut(world).unwrap();
    ui.build("Selected", |ui| {
        if let Some(entity) = entity {
            ui.text(format!("Entity: {:?}", entity));

            for (mutable, name, component_id, reflected) in components {
                let mutable = mutable && name != "Children";
                if let Some(mut reflected) = reflected {
                    let changed = ui.disabled(!mutable, |ui| {
                        draw_reflect_value(
                            ui,
                            None,
                            component_id.index() as u64,
                            reflected.as_mut() as &mut dyn PartialReflect,
                            &registry,
                        )
                    });
                    if changed && mutable {
                        mutations.push((component_id, reflected));
                    }
                } else {
                    ui.colored_text(
                        format!("Unreflected {}", name.split("::").last().unwrap_or("?")),
                        UiContext::TEXT_DIM,
                    );
                }
            }
        } else {
            ui.colored_text("Nothing Selected", UiContext::TEXT_DIM);
        };
    });

    if let Some(entity) = entity {
        for (component_id, new_value) in mutations {
            let info = world.components().get_info(component_id).unwrap();
            let type_id = info.type_id().unwrap();
            let registration = registry.get(type_id).unwrap();
            let reflect_component = registration.data::<ReflectComponent>().unwrap();
            reflect_component.apply(world.entity_mut(entity), new_value.as_ref());
        }
    }
}

use core::f32;
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
use glam::{EulerRot, Mat3, Quat, Vec3};

use crate::{
    editor::picking::Selected,
    ui::{
        UiContext,
        builder::{UiBuilder, UiWindowBuilder},
    },
};

const INPUT_WIDTH: f32 = 300.0;
const INPUT_SPACING: f32 = 13.0;

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

impl EditorView for f32 {
    fn ui(
        &mut self,
        ui: &mut UiWindowBuilder,
        name: &str,
        id: u64,
        _registry: &TypeRegistry,
    ) -> bool {
        ui.horizontal();
        ui.text(name);
        let before = ui.cursor.x;
        let val = *self;
        ui.cursor.x += ui.remaining_width() - (INPUT_WIDTH + INPUT_SPACING);
        *self = ui.float_input(id, val as f64, INPUT_WIDTH) as f32;
        ui.cursor.x = before;
        ui.vertical();
        *self != val
    }
}

impl EditorView for f64 {
    fn ui(
        &mut self,
        ui: &mut UiWindowBuilder,
        name: &str,
        id: u64,
        _registry: &TypeRegistry,
    ) -> bool {
        ui.horizontal();
        ui.text(name);
        let before = ui.cursor.x;
        let val = *self;
        ui.cursor.x += ui.remaining_width() - (INPUT_WIDTH + INPUT_SPACING);
        *self = ui.float_input(id, val, INPUT_WIDTH);
        ui.cursor.x = before;
        ui.vertical();
        *self != val
    }
}

impl EditorView for i32 {
    fn ui(
        &mut self,
        ui: &mut UiWindowBuilder,
        name: &str,
        id: u64,
        _registry: &TypeRegistry,
    ) -> bool {
        ui.horizontal();
        ui.text(name);
        let before = ui.cursor.x;
        let val = *self;
        ui.cursor.x += ui.remaining_width() - (INPUT_WIDTH + INPUT_SPACING);
        *self = ui.int_input(id, val as i64, INPUT_WIDTH) as i32;
        ui.cursor.x = before;
        ui.vertical();
        *self != val
    }
}

impl EditorView for u32 {
    fn ui(
        &mut self,
        ui: &mut UiWindowBuilder,
        name: &str,
        id: u64,
        _registry: &TypeRegistry,
    ) -> bool {
        ui.horizontal();
        ui.text(name);
        let before = ui.cursor.x;
        let val = *self;
        ui.cursor.x += ui.remaining_width() - (INPUT_WIDTH + INPUT_SPACING);
        *self = ui.int_input(id, val as i64, INPUT_WIDTH) as u32;
        ui.cursor.x = before;
        ui.vertical();
        *self != val
    }
}

impl EditorView for u64 {
    fn ui(
        &mut self,
        ui: &mut UiWindowBuilder,
        name: &str,
        id: u64,
        _registry: &TypeRegistry,
    ) -> bool {
        ui.horizontal();
        ui.text(name);
        let before = ui.cursor.x;
        let val = *self;
        ui.cursor.x += ui.remaining_width() - (INPUT_WIDTH + INPUT_SPACING);
        *self = ui.int_input(id, val as i64, INPUT_WIDTH) as u64;
        ui.cursor.x = before;
        ui.vertical();
        *self != val
    }
}

impl EditorView for bool {
    fn ui(
        &mut self,
        ui: &mut UiWindowBuilder,
        name: &str,
        _id: u64,
        _registry: &TypeRegistry,
    ) -> bool {
        ui.horizontal();
        let before = ui.cursor.x;
        ui.text(name);
        let val = *self;
        ui.cursor.x += ui.remaining_width() - (UiContext::ATLAS_CELL_SIZE.x as f32 + INPUT_SPACING);
        *self = ui.checkbox(val);
        ui.cursor.x = before;
        ui.vertical();
        *self != val
    }
}

impl EditorView for String {
    fn ui(
        &mut self,
        ui: &mut UiWindowBuilder,
        name: &str,
        id: u64,
        _registry: &TypeRegistry,
    ) -> bool {
        ui.horizontal();
        ui.text(name);
        let before = ui.cursor.x;
        ui.cursor.x += ui.remaining_width() - (INPUT_WIDTH + INPUT_SPACING);
        let changed = ui.text_input(id, self, INPUT_WIDTH);
        ui.cursor.x = before;
        ui.vertical();
        changed
    }
}

impl EditorView for glam::Vec2 {
    fn ui(
        &mut self,
        ui: &mut UiWindowBuilder,
        name: &str,
        id: u64,
        _registry: &TypeRegistry,
    ) -> bool {
        ui.horizontal();
        ui.text(name);
        let val = *self;
        let before = ui.cursor.x;
        ui.cursor.x += ui.remaining_width() - (INPUT_WIDTH + INPUT_SPACING);
        let width = width(2.0);
        self.x = ui.float_input(id, val.x as f64, width) as f32;
        self.y = ui.float_input(id + 1, val.y as f64, width) as f32;
        ui.cursor.x = before;
        ui.vertical();
        *self != val
    }
}

fn width(n: f32) -> f32 {
    INPUT_WIDTH / n
        - UiContext::ELEMENT_GAP.x as f32
        - UiContext::CHILD_PAD.x as f32
        - UiContext::ROUNDING.max(UiContext::BORDER) as f32
}

impl EditorView for glam::Vec3 {
    fn ui(
        &mut self,
        ui: &mut UiWindowBuilder,
        name: &str,
        id: u64,
        _registry: &TypeRegistry,
    ) -> bool {
        ui.horizontal();
        ui.text(name);
        let val = *self;
        let before = ui.cursor.x;
        ui.cursor.x += ui.remaining_width() - (INPUT_WIDTH + INPUT_SPACING);
        let width = width(3.0);
        self.x = ui.float_input(id, val.x as f64, width) as f32;
        self.y = ui.float_input(id + 1, val.y as f64, width) as f32;
        self.z = ui.float_input(id + 2, val.z as f64, width) as f32;
        ui.cursor.x = before;
        ui.vertical();
        *self != val
    }
}

impl EditorView for glam::Vec4 {
    fn ui(
        &mut self,
        ui: &mut UiWindowBuilder,
        name: &str,
        id: u64,
        _registry: &TypeRegistry,
    ) -> bool {
        ui.horizontal();
        ui.text(name);
        let val = *self;
        let before = ui.cursor.x;
        ui.cursor.x += ui.remaining_width() - (INPUT_WIDTH + INPUT_SPACING);
        let width = width(4.0);

        self.x = ui.float_input(id, val.x as f64, width) as f32;
        self.y = ui.float_input(id + 1, val.y as f64, width) as f32;
        self.z = ui.float_input(id + 2, val.z as f64, width) as f32;
        self.w = ui.float_input(id + 3, val.w as f64, width) as f32;
        ui.cursor.x = before;
        ui.vertical();
        *self != val
    }
}

impl EditorView for glam::Quat {
    fn ui(
        &mut self,
        ui: &mut UiWindowBuilder,
        name: &str,
        id: u64,
        _registry: &TypeRegistry,
    ) -> bool {
        ui.horizontal();
        ui.text(name);
        let val = *self;

        let before = ui.cursor.x;
        ui.cursor.x += ui.remaining_width() - (INPUT_WIDTH + INPUT_SPACING);
        let width = width(3.0);
        let (x, y, z) = self.to_euler(EulerRot::XYZ);
        let x = ui.float_input(id, x as f64, width) as f32;
        let y = ui.float_input(id + 1, y as f64, width) as f32;
        let z = ui.float_input(id + 2, z as f64, width) as f32;
        *self = glam::Quat::from_euler(EulerRot::XYZ, x, y, z);
        ui.cursor.x = before;
        ui.vertical();
        *self != val
    }
}

impl EditorView for glam::Mat4 {
    fn ui(
        &mut self,
        ui: &mut UiWindowBuilder,
        name: &str,
        id: u64,
        _registry: &TypeRegistry,
    ) -> bool {
        ui.text(name);
        let x_axis = self.x_axis.ui(ui, "|    x-Axis", id, _registry);
        let y_axis = self.y_axis.ui(ui, "|    y-Axis", id + 4, _registry);
        let z_axis = self.z_axis.ui(ui, "|    z-Axis", id + 8, _registry);
        let w_axis = self.w_axis.ui(ui, "|    w-Axis", id + 12, _registry);
        x_axis || y_axis || z_axis || w_axis
    }
}

impl EditorView for glam::Affine3A {
    fn ui(
        &mut self,
        ui: &mut UiWindowBuilder,
        name: &str,
        id: u64,
        _registry: &TypeRegistry,
    ) -> bool {
        ui.text(name);
        let det = self.matrix3.determinant();

        let mut scale = Vec3::new(
            self.matrix3.x_axis.length() * det.signum(),
            self.matrix3.y_axis.length(),
            self.matrix3.z_axis.length(),
        );

        let inv_scale = scale.recip();

        let mut rotation = Quat::from_mat3(&Mat3::from_cols(
            (self.matrix3.x_axis * inv_scale.x).into(),
            (self.matrix3.y_axis * inv_scale.y).into(),
            (self.matrix3.z_axis * inv_scale.z).into(),
        ));

        let changed = scale.ui(ui, "|    scale", id, _registry)
            || rotation.ui(ui, "|    rotation", id + 3, _registry)
            || self
                .translation
                .to_vec3()
                .ui(ui, "|    translation", id + 6, _registry);
        if changed {
            let rotation = rotation.normalize();
            *self = glam::Affine3A::from_scale_rotation_translation(
                scale,
                rotation,
                self.translation.into(),
            );
        }
        changed
    }
}

impl EditorView for Entity {
    fn ui(
        &mut self,
        ui: &mut UiWindowBuilder,
        name: &str,
        _id: u64,
        _registry: &TypeRegistry,
    ) -> bool {
        let label = format!("Entity {}", self.index());
        let width = UiContext::text_len(&label);
        let before = ui.cursor.x;
        ui.horizontal();
        ui.text(name);
        ui.cursor.x = ui.remaining_width() - width;
        ui.text(label);
        ui.cursor.x = before;
        ui.vertical();

        false
    }
}

impl<A: Asset> EditorView for Handle<A> {
    fn ui(
        &mut self,
        ui: &mut UiWindowBuilder,
        _name: &str,
        _id: u64,
        _registry: &TypeRegistry,
    ) -> bool {
        self.path()
            .map(|path| ui.text(path.to_string()))
            .unwrap_or_else(|| ui.text("Unknown"));
        false
    }
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
            ui.collapsable(format!("{}{type_short}", label), |ui| {
                for i in 0..s.field_len() {
                    let field_name = s.name_at(i).unwrap_or("UnknownField").to_string();
                    let Some(field_val) = s.field_at_mut(i) else {
                        continue;
                    };
                    let mut hash = DefaultHasher::new();
                    field_name.hash(&mut hash);
                    id.hash(&mut hash);
                    let child_id = hash.finish();
                    changed |=
                        draw_reflect_value(ui, Some(&field_name), child_id, field_val, registry);
                }
            });
        }
        ReflectMut::TupleStruct(ts) => {
            ui.collapsable(format!("{}{type_short}", label), |ui| {
                for i in 0..ts.field_len() {
                    let Some(field_val) = ts.field_mut(i) else {
                        continue;
                    };
                    let mut hash = DefaultHasher::new();
                    i.hash(&mut hash);
                    id.hash(&mut hash);
                    let child_id = hash.finish();
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
            ui.collapsable(format!("{}{type_short}", label), |ui| {
                for i in 0..t.field_len() {
                    let Some(field_val) = t.field_mut(i) else {
                        continue;
                    };
                    let mut hash = DefaultHasher::new();
                    i.hash(&mut hash);
                    id.hash(&mut hash);
                    let child_id = hash.finish();
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
            ui.collapsable(format!("{}{type_short}::{}", label, variant), |ui| {
                for i in 0..e.field_len() {
                    let field_name = e.name_at(i).unwrap_or("0").to_string();
                    let Some(field_val) = e.field_at_mut(i) else {
                        continue;
                    };
                    let mut hash = DefaultHasher::new();
                    i.hash(&mut hash);
                    id.hash(&mut hash);
                    let child_id = hash.finish();
                    changed |= draw_reflect_value(
                        ui,
                        Some(field_name.as_ref()),
                        child_id,
                        field_val,
                        registry,
                    );
                }
            });
        }
        ReflectMut::List(l) => {
            ui.collapsable(format!("{}{}[{}]", label, type_short, l.len()), |ui| {
                for i in 0..l.len() {
                    let mut hash = DefaultHasher::new();
                    i.hash(&mut hash);
                    id.hash(&mut hash);
                    let child_id = hash.finish();
                    let Some(child) = l.get_mut(i) else { continue };
                    changed |= draw_reflect_value(
                        ui,
                        Some(&format!("[{}]", i)),
                        child_id,
                        child,
                        registry,
                    );
                }
            });
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
            ui.collapsable(format!("{}{}{{{}}}", label, type_short, v.len()), |ui| {
                for i in v.iter() {
                    let before = ui.disable_all_input;
                    ui.disabled(true);
                    changed |= draw_reflect_value(
                        ui,
                        Some(""),
                        id,
                        unsafe {
                            (i as *const dyn PartialReflect as *mut dyn PartialReflect)
                                .as_mut()
                                .unwrap()
                        },
                        registry,
                    );
                    ui.disabled(before);
                }
            });
        }
        ReflectMut::Array(v) => {
            ui.collapsable(format!("{}{}[{}]", label, type_short, v.len()), |ui| {
                for i in 0..v.len() {
                    let Some(value) = v.get_mut(i) else { continue };
                    changed |=
                        draw_reflect_value(ui, Some(&format!("[{}]", i)), id, value, registry);
                }
            });
        }
        ReflectMut::Map(_v) => {
            ui.collapsable(format!("{}{}", label, type_short), |ui| {
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

pub(crate) fn selected_ui(world: &mut World) {
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

    let mut state: SystemState<UiBuilder> = SystemState::new(world);

    let mut ui = state.get_mut(world);
    ui.build("Selected", |ui| {
        if let Some(entity) = entity {
            ui.text(format!("Entity: {:?}", entity));

            for (mutable, name, component_id, reflected) in components {
                let mutable = mutable && name != "Children";
                if let Some(mut reflected) = reflected {
                    ui.disabled(!mutable);
                    let changed = draw_reflect_value(
                        ui,
                        None,
                        component_id.index() as u64,
                        reflected.as_mut() as &mut dyn PartialReflect,
                        &registry,
                    );
                    if changed && mutable {
                        mutations.push((component_id, reflected));
                    }
                } else {
                    ui.disabled(true);
                    ui.text(format!(
                        "Unreflected {}",
                        name.split("::").last().unwrap_or("?")
                    ));
                    ui.disabled(false);
                }
            }
        } else {
            ui.disabled(true);
            ui.text("Nothing Selected");
            ui.disabled(false);
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

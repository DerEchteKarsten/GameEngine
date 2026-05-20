// ============================================================
// NUI TEST SCENARIOS
// Drop these into your app alongside test_ui, register the
// components & systems just like TestWindow / test_ui.
// ============================================================

use bevy::app::{App, Update};
use bevy::ecs::component::Component;
use bevy::ecs::reflect::ReflectComponent;
use bevy::reflect::Reflect;
use bevy::ecs::system::{Commands, Local};
use glam::Vec4;

use crate::id;
use crate::ui::new_ui::{UiBuilder, UiWindow};

// ─── Marker components ──────────────────────────────────────

#[derive(Component, Reflect)] #[reflect(Component)] pub struct TestWindowStress;
#[derive(Component, Reflect)] #[reflect(Component)] pub struct TestWindowInputs;
#[derive(Component, Reflect)] #[reflect(Component)] pub struct TestWindowColorPicker;
#[derive(Component, Reflect)] #[reflect(Component)] pub struct TestWindowContainers;
#[derive(Component, Reflect)] #[reflect(Component)] pub struct TestWindowWindowA;
#[derive(Component, Reflect)] #[reflect(Component)] pub struct TestWindowWindowB;
#[derive(Component, Reflect)] #[reflect(Component)] pub struct TestWindowEdgeCases;
#[derive(Component, Reflect)] #[reflect(Component)] pub struct TestWindowMixedLayout;
#[derive(Component, Reflect)] #[reflect(Component)] pub struct TestWindowDeepCollapse;

// ============================================================
// SCENARIO 1 — Stress layout
//   Many elements, deep nesting, scrollbar exercise.
//   Expected: scrollbar appears, content clips correctly,
//   thumb dragging moves content.
// ============================================================
pub fn test_stress(
    mut cmd: Commands,
    mut ui: UiBuilder<TestWindowStress>,
    mut state: Local<(f32, usize)>,
) {
    ui.build_or(
        || { cmd.spawn((UiWindow::new("Stress Layout"), TestWindowStress)); },
        |b| {
            b.text("── Stress: 40 rows of mixed content ──");

            for i in 0..40 {
                b.horizontal();
                b.text(format!("Row {:02}", i));
                state.0 = b.slider(id!(), 0.0, 1.0, 80.0, state.0);
                b.text(format!("{:.2}", state.0));
                if b.button(if i % 2 == 0 { "Even" } else { "Odd" }) {
                    state.1 = i;
                }
                b.vertical();
            }

            b.text(format!("Last clicked row: {}", state.1));
        },
    );
}

// ============================================================
// SCENARIO 2 — Input gauntlet
//   Every input widget back-to-back.
//   Tests: tab focus chain, keyboard nav in text fields,
//   float parse fallback, dropdown open/close.
// ============================================================
pub fn test_inputs(
    mut cmd: Commands,
    mut ui: UiBuilder<TestWindowInputs>,
    mut state: Local<(String, f32, i32, bool, usize, String)>,
) {
    ui.build_or(
        || { cmd.spawn((UiWindow::new("Input Gauntlet"), TestWindowInputs)); },
        |b| {
            b.text("Text input (tab → next field):");
            b.text_input(id!(), &mut state.0, 200.0);

            b.text("Float input (parse fallback on invalid):");
            state.1 = b.float_input(id!(), state.1, 120.0);
            b.text(format!("= {:.4}", state.1));

            b.text("Slider [-100 … 100]:");
            state.2 = b.slider(id!(), -100.0, 100.0, 200.0, state.2 as f32) as i32;
            b.text(format!("= {}", state.2));

            b.text("Checkbox:");
            state.3 = b.check_box(state.3);
            b.text(if state.3 { "checked ✓" } else { "unchecked" });

            b.text("Dropdown:");
            state.4 = b.dropdown(id!(), state.4, &[
                "Alpha", "Beta", "Gamma", "Delta", "Epsilon",
            ]);
            b.text(format!("selected index: {}", state.4));

            b.text("Second text field (tab from above reaches here):");
            b.text_input(id!(), &mut state.5, 200.0);
            b.text(format!("value: \"{}\"", state.5));
        },
    );
}

// ============================================================
// SCENARIO 3 — Color picker showcase
//   Isolated color picker with RGBA breakdown text.
//   Tests: SV drag, hue bar drag, alpha bar drag, float inputs.
// ============================================================
pub fn test_color_picker(
    mut cmd: Commands,
    mut ui: UiBuilder<TestWindowColorPicker>,
    mut color: Local<Vec4>,
) {
    if color.w == 0.0 { *color = Vec4::new(0.2, 0.6, 1.0, 1.0); }

    ui.build_or(
        || { cmd.spawn((UiWindow::new("Color Picker"), TestWindowColorPicker)); },
        |b| {
            b.text("Drag the SV square, hue bar, and alpha bar:");
            *color = b.color_picker(id!(), *color);

            b.text(format!(
                "R:{:.3}  G:{:.3}  B:{:.3}  A:{:.3}",
                color.x, color.y, color.z, color.w
            ));
            b.text(format!(
                "#{:02X}{:02X}{:02X}{:02X}",
                (color.x * 255.0) as u8,
                (color.y * 255.0) as u8,
                (color.z * 255.0) as u8,
                (color.w * 255.0) as u8,
            ));
        },
    );
}

// ============================================================
// SCENARIO 4 — Nested scrollable containers
//   Outer container (300×200) wrapping an inner container
//   (250×400) wrapping 300 text lines.
//   Tests: independent scroll state, scroll consumption
//   (inner vs outer should not fight), thumb sizing.
// ============================================================
pub fn test_containers(
    mut cmd: Commands,
    mut ui: UiBuilder<TestWindowContainers>,
) {
    ui.build_or(
        || { cmd.spawn((UiWindow::new("Nested Containers"), TestWindowContainers)); },
        |b| {
            b.text("Outer container 300×200 → inner 250×400 → 300 lines");

            b.container("outer", glam::Vec2::new(300.0, 200.0), |outer| {
                outer.text("── outer container ──");

                outer.container("inner", glam::Vec2::new(250.0, 400.0), |inner| {
                    inner.text("── inner container ──");
                    for i in 0..300 {
                        inner.text(format!("line {:03}", i));
                    }
                });

                outer.text("── after inner ──");
                for i in 0..10 {
                    outer.text(format!("outer extra {}", i));
                }
            });

            b.text("Below the outer container");
        },
    );
}

// ============================================================
// SCENARIO 5 — Multiple windows / layer management
//   Two windows that overlap; clicking one brings it to front.
//   Tests: layer sorting in update_windows, draw order in
//   nextract_ui.
// ============================================================
pub fn test_multi_window_a(
    mut cmd: Commands,
    mut ui: UiBuilder<TestWindowWindowA>,
    mut n: Local<u32>,
) {
    ui.build_or(
        || {
            let mut w = UiWindow::new("Window A (click to focus)");
            w.size = bevy::math::Rect::from_corners(
                glam::Vec2::new(60.0, 60.0),
                glam::Vec2::new(340.0, 340.0),
            );
            cmd.spawn((w, TestWindowWindowA));
        },
        |b| {
            b.text("I am Window A.");
            b.text("Click me to bring me in front of B.");
            if b.button("Count") { *n += 1; }
            b.text(format!("Pressed {} times", *n));
        },
    );
}

pub fn test_multi_window_b(
    mut cmd: Commands,
    mut ui: UiBuilder<TestWindowWindowB>,
    mut n: Local<u32>,
) {
    ui.build_or(
        || {
            let mut w = UiWindow::new("Window B (overlaps A)");
            w.size = bevy::math::Rect::from_corners(
                glam::Vec2::new(200.0, 200.0),
                glam::Vec2::new(480.0, 480.0),
            );
            cmd.spawn((w, TestWindowWindowB));
        },
        |b| {
            b.text("I am Window B.");
            b.text("Click me to come to the front.");
            if b.button("Count") { *n += 1; }
            b.text(format!("Pressed {} times", *n));
        },
    );
}

// ============================================================
// SCENARIO 6 — Edge case inputs
//   Empty strings, unicode, very long overflow, float edges.
//   Tests: cursor clamping at string boundaries, UTF-8 safety,
//   text clip rect, float_input parse fallback.
// ============================================================
pub fn test_edge_cases(
    mut cmd: Commands,
    mut ui: UiBuilder<TestWindowEdgeCases>,
    mut state: Local<(String, String, f32, String)>,
) {
    // Seed defaults once
    if state.1.is_empty() {
        state.1 = "Hello 🌍 Unicode".to_string(); // NOTE: only chars in atlas will render
        state.3 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string();
    }

    ui.build_or(
        || { cmd.spawn((UiWindow::new("Edge Cases"), TestWindowEdgeCases)); },
        |b| {
            b.text("Empty string field (start empty, type, delete all):");
            b.text_input(id!(), &mut state.0, 200.0);
            b.text(format!("len={}", state.0.len()));

            b.text("Pre-filled field (Home/End/Ctrl+A/Ctrl+Backspace):");
            b.text_input(id!(), &mut state.1, 200.0);

            b.text("Float: type 'abc' → should keep last valid value:");
            state.2 = b.float_input(id!(), state.2, 120.0);
            b.text(format!("= {}", state.2));

            b.text("Long overflow field (scroll with cursor):");
            b.text_input(id!(), &mut state.3, 150.0);

            b.text("Slider at exact min:");
            let v = b.slider(id!(), 0.0, 1.0, 150.0, 0.0);
            b.text(format!("= {:.6}", v));

            b.text("Slider at exact max:");
            let v = b.slider(id!(), 0.0, 1.0, 150.0, 1.0);
            b.text(format!("= {:.6}", v));

            b.text("Zero-width range slider (min==max guard):");
            let v = b.slider(id!(), 5.0, 5.0, 100.0, 5.0);
            b.text(format!("= {:.6}", v));
        },
    );
}

// ============================================================
// SCENARIO 7 — Complex mixed horizontal/vertical layout
//   Interleaved row/column switches; tests that prev_cursor
//   and line_height accounting don't drift after many toggles.
// ============================================================
pub fn test_mixed_layout(
    mut cmd: Commands,
    mut ui: UiBuilder<TestWindowMixedLayout>,
    mut vals: Local<[f32; 6]>,
) {
    ui.build_or(
        || { cmd.spawn((UiWindow::new("Mixed Layout"), TestWindowMixedLayout)); },
        |b| {
            // Row 1: label + 3 sliders
            b.horizontal();
            b.text("Row 1 sliders:");
            vals[0] = b.slider(id!(), 0.0, 1.0, 60.0, vals[0]);
            vals[1] = b.slider(id!(), 0.0, 1.0, 60.0, vals[1]);
            vals[2] = b.slider(id!(), 0.0, 1.0, 60.0, vals[2]);
            b.vertical();

            // Vertical block
            b.text(format!("  sum = {:.3}", vals[0] + vals[1] + vals[2]));

            // Row 2: buttons + checkbox
            b.horizontal();
            if b.button("Reset A") { vals[0] = 0.0; vals[1] = 0.0; vals[2] = 0.0; }
            if b.button("Set Max") { vals[0] = 1.0; vals[1] = 1.0; vals[2] = 1.0; }
            b.vertical();

            b.text("── second group ──");

            // Row 3: label + dropdown + float
            b.horizontal();
            b.text("Pick:");
            vals[3] = b.slider(id!(), -1.0, 1.0, 80.0, vals[3]);
            vals[4] = b.float_input(id!(), vals[4], 70.0);
            b.vertical();

            b.text(format!("product = {:.4}", vals[3] * vals[4]));

            // Deeply nested mixed inside a container
            b.container("nested_mix", glam::Vec2::new(320.0, 180.0), |inner| {
                inner.text("Inside container:");
                inner.horizontal();
                inner.text("A");
                vals[5] = inner.slider(id!(), 0.0, 100.0, 100.0, vals[5]);
                inner.text(format!("{:.0}", vals[5]));
                inner.vertical();
                if inner.button("Zero") { vals[5] = 0.0; }
            });
        },
    );
}

// ============================================================
// SCENARIO 8 — Deep collapsable tree (4 levels)
//   Tests: open_headers HashSet keyed by stable ids, indentation
//   accounting, content_max after a closed subtree.
// ============================================================
pub fn test_deep_collapse(
    mut cmd: Commands,
    mut ui: UiBuilder<TestWindowDeepCollapse>,
    mut n: Local<u32>,
) {
    ui.build_or(
        || { cmd.spawn((UiWindow::new("Deep Collapsable Tree"), TestWindowDeepCollapse)); },
        |b| {
            b.text("Expand the tree fully to verify indentation & scroll:");

            b.collapsable("Level 1 – Animals", |b| {
                b.text("Mammals:");
                b.collapsable("Level 2 – Mammals", |b| {
                    b.collapsable("Level 3 – Cats", |b| {
                        b.collapsable("Level 4 – Domestic", |b| {
                            if b.button("Meow") { *n += 1; }
                            b.text(format!("meows: {}", *n));
                        });
                        b.button("Wild cat")
                    });
                    b.collapsable("Level 3 – Dogs", |b| {
                        b.text("Labrador");
                        b.text("Poodle");
                    });
                });
                b.text("Birds:");
                b.collapsable("Level 2 – Birds", |b| {
                    b.collapsable("Level 3 – Raptors", |b| {
                        b.text("Eagle");
                        b.text("Hawk");
                    });
                    b.text("Parrot");
                });
            });

            b.collapsable("Level 1 – Vehicles", |b| {
                b.text("Cars, planes, boats.");
                b.collapsable("Level 2 – Cars", |b| {
                    b.text("Sedan");
                    b.text("SUV");
                    b.collapsable("Level 3 – Sports", |b| {
                        b.text("Porsche 911");
                        b.text("Ferrari F40");
                    });
                });
            });

            b.text("── content below tree ──");
            b.text("This line should sit correctly below all closed/open headers.");
        },
    );
}

pub fn add_tests(app: &mut App) {
      app.add_systems(Update, (
          test_stress,
          test_inputs,
          test_color_picker,
          test_containers,
          test_multi_window_a,
          test_multi_window_b,
          test_edge_cases,
          test_mixed_layout,
          test_deep_collapse,
      ));
    
      app.register_type::<TestWindowStress>()
         .register_type::<TestWindowInputs>()
         .register_type::<TestWindowColorPicker>()
         .register_type::<TestWindowContainers>()
         .register_type::<TestWindowWindowA>()
         .register_type::<TestWindowWindowB>()
         .register_type::<TestWindowEdgeCases>()
         .register_type::<TestWindowMixedLayout>()
         .register_type::<TestWindowDeepCollapse>();
}
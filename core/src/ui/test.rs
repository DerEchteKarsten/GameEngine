use bevy::{app::{App, Update}, log, math::Rect, prelude::Res};
use glam::{Vec2, Vec4};

use crate::{id, ui::new_ui::{DockingNode, UiBuilder, UiWindowBuilder}};

// fn tree(nodes: &[DockingNode], area: Rect, node: usize, ui: &mut UiWindowBuilder) {
//     match &nodes[node] {
//         DockingNode::Leaf { windows, root } => {
//             for window in windows {
//                 ui.text(format!("{}", *window));
//             }
//         },
//         DockingNode::Node { split, extend, left, right } => {
//             // let left_area = Rect {min: area.min, max: area.max - split.direction_vec() * *extend};
//             // let right_area = Rect {min: area.min + split.direction_vec() * *extend, max: area.max};
//             let left = *left as usize;
//             let right = *right as usize;
//             // ui.window.rect(left_area, None, Vec4::new(1.0, 0.0, 0.0, 0.5), ui.viewport_size, Rect::from_corners(Vec2::ZERO, ui.viewport_size), true);
//             // ui.window.rect(right_area, None, Vec4::new(0.0, 1.0, 0.0, 0.5), ui.viewport_size, Rect::from_corners(Vec2::ZERO, ui.viewport_size), true);
            
//             ui.collapsable(format!("Children of {}", left), |ui| {
//                 tree(nodes, area, left, ui);
//             });
//             ui.collapsable(format!("Children of {}", right), |ui| {
//                 tree(nodes, area, right, ui);
//             });
//         },
//     }
// }

// fn tree_visulizer(mut ui: UiBuilder) {
//     ui.build("Elements", |ui| {
//         let ctx = Res::clone(&ui.ctx);
//         tree(&ctx.docking_nodes, Rect::from_corners(Vec2::ZERO, ui.viewport_size), 0, ui);
//     });
// }

// --- Layout tests ---

/// All elements stacked vertically (default direction)
fn test_vertical_layout(mut ui: UiBuilder) {
    ui.build("Vertical Layout", |ui| {
        ui.text("Line 1");
        ui.text("Line 2");
        ui.text("Line 3");
        ui.button("Button A");
        ui.button("Button B");
        ui.check_box(false);
        ui.check_box(true);
    });
}

/// Elements laid out horizontally, then back to vertical
fn test_horizontal_layout(mut ui: UiBuilder) {
    ui.build("Horizontal Layout", |ui| {
        ui.text("Before horizontal:");
        ui.horizontal();
        ui.button("Left");
        ui.button("Middle");
        ui.button("Right");
        ui.vertical();
        ui.text("After vertical:");
        ui.button("Solo button");
    });
}

/// Multiple horizontal rows
fn test_multiple_horizontal_rows(mut ui: UiBuilder) {
    ui.build("Multi-Row Horizontal", |ui| {
        for row in 0..5 {
            ui.horizontal();
            for col in 0..4 {
                ui.button(format!("R{row}C{col}"));
            }
            ui.vertical();
        }
    });
}

/// Horizontal row mixing different element types
fn test_mixed_horizontal_row(mut ui: UiBuilder) {
    ui.build("Mixed Horizontal", |ui| {
        ui.horizontal();
        ui.text("Label:");
        ui.check_box(true);
        ui.button("Click me");
        ui.color_picker(id!(), Vec4::new(1.0, 0.0, 0.0, 1.0));
        ui.vertical();
        ui.text("Back to vertical");
    });
}

// --- Collapsable / nesting tests ---

/// Nested collapsables
fn test_nested_collapsables(mut ui: UiBuilder) {
    ui.build("Nested Collapsables", |ui| {
        ui.collapsable("Outer", |ui| {
            ui.text("Outer content");
            ui.collapsable("Inner A", |ui| {
                ui.text("Inner A content");
                ui.button("Inner A button");
            });
            ui.collapsable("Inner B", |ui| {
                ui.text("Inner B content");
                ui.collapsable("Deeply Nested", |ui| {
                    ui.text("Deep content");
                    ui.check_box(false);
                });
            });
        });
        ui.text("After collapsable");
    });
}

/// Collapsable with horizontal layout inside
fn test_collapsable_with_horizontal(mut ui: UiBuilder) {
    ui.build("Collapsable + Horizontal", |ui| {
        ui.collapsable("Horizontal inside collapsable", |ui| {
            ui.horizontal();
            ui.button("A");
            ui.button("B");
            ui.button("C");
            ui.vertical();
            ui.text("Below buttons");
        });
    });
}

/// Multiple sibling collapsables
fn test_sibling_collapsables(mut ui: UiBuilder) {
    ui.build("Sibling Collapsables", |ui| {
        for i in 0..6 {
            ui.collapsable(format!("Section {i}"), |ui| {
                ui.text(format!("Content of section {i}"));
                ui.button(format!("Action {i}"));
                ui.check_box(i % 2 == 0);
            });
        }
    });
}

// --- Container tests ---

/// Container with scrollable content
fn test_scrollable_container(mut ui: UiBuilder) {
    ui.build("Scrollable Container", |ui| {
        ui.text("Above container");
        ui.container(id!(), glam::Vec2::new(200.0, 100.0), |ui| {
            for i in 0..20 {
                ui.text(format!("Item {i}"));
            }
        });
        ui.text("Below container");
    });
}

/// Nested containers
fn test_nested_containers(mut ui: UiBuilder) {
    ui.build("Nested Containers", |ui| {
        ui.container(id!(), glam::Vec2::new(300.0, 200.0), |ui| {
            ui.text("Outer container");
            ui.container(id!(), glam::Vec2::new(150.0, 80.0), |ui| {
                for i in 0..10 {
                    ui.text(format!("Inner item {i}"));
                }
            });
            ui.text("After inner container");
        });
    });
}

/// Container with horizontal elements inside
fn test_container_with_horizontal(mut ui: UiBuilder) {
    ui.build("Container + Horizontal", |ui| {
        ui.container(id!(), glam::Vec2::new(250.0, 150.0), |ui| {
            ui.horizontal();
            ui.button("X");
            ui.button("Y");
            ui.button("Z");
            ui.vertical();
            for i in 0..5 {
                ui.text(format!("Row {i}"));
            }
        });
    });
}

// --- Text edge cases ---

/// Empty and whitespace strings
fn test_text_edge_cases(mut ui: UiBuilder) {
    ui.build("Text Edge Cases", |ui| {
        ui.text("");
        ui.text(" ");
        ui.text("   spaces   ");
        ui.text("newline\nhere");
        ui.text("multiple\n\nnewlines");
        ui.text("tab\there");
        // Very long single line
        ui.text("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA");
        // Unicode
        ui.text("Unicode: こんにちは 🎉 café");
    });
}

// --- Slider edge cases ---

/// Sliders at boundary values and zero-range
fn test_slider_edge_cases(mut ui: UiBuilder) {
    ui.build("Slider Edge Cases", |ui| {
        // Normal range
        ui.slider(id!(), 0.0, 1.0, 150.0, 0.5);
        // Value at min
        ui.slider(id!(), 0.0, 1.0, 150.0, 0.0);
        // Value at max
        ui.slider(id!(), 0.0, 1.0, 150.0, 1.0);
        // Value below min (clamping)
        ui.slider(id!(), 0.0, 1.0, 150.0, -99.0);
        // Value above max (clamping)
        ui.slider(id!(), 0.0, 1.0, 150.0, 99.0);
        // Zero range (min == max, should not divide by zero)
        ui.slider(id!(), 5.0, 5.0, 150.0, 5.0);
        // Negative range
        ui.slider(id!(), -100.0, -1.0, 150.0, -50.0);
        // Large range
        ui.slider(id!(), 0.0, 1_000_000.0, 150.0, 500_000.0);
        // Very narrow widget
        ui.slider(id!(), 0.0, 1.0, 1.0, 0.5);
    });
}

// --- Dropdown edge cases ---

/// Dropdown with one option, many options, and long labels
fn test_dropdown_edge_cases(mut ui: UiBuilder) {
    ui.build("Dropdown Edge Cases", |ui| {
        // Single option
        ui.dropdown(id!(), 0, &["Only option"]);
        // Two options
        ui.dropdown(id!(), 0, &["First", "Second"]);
        // Many options
        ui.dropdown(id!(), 3, &["A", "B", "C", "D", "E", "F", "G", "H"]);
        // Long labels
        ui.dropdown(id!(), 0, &[
            "Short",
            "A much longer option label that takes up space",
            "Another long one here too",
        ]);
        // Selected index at last element
        ui.dropdown(id!(), 2, &["X", "Y", "Z"]);
    });
}

// --- Color picker edge cases ---

/// Color pickers with boundary and special colors
fn test_color_picker_edge_cases(mut ui: UiBuilder) {
    ui.build("Color Picker Edge Cases", |ui| {
        ui.color_picker(id!(), Vec4::ZERO);                       // black, transparent
        ui.color_picker(id!(), Vec4::ONE);                        // white, opaque
        ui.color_picker(id!(), Vec4::new(1.0, 0.0, 0.0, 1.0));   // red
        ui.color_picker(id!(), Vec4::new(0.0, 1.0, 0.0, 1.0));   // green
        ui.color_picker(id!(), Vec4::new(0.0, 0.0, 1.0, 1.0));   // blue
        ui.color_picker(id!(), Vec4::new(0.5, 0.5, 0.5, 0.5));   // mid grey, half alpha
        // Out of range values (should clamp)
        ui.color_picker(id!(), Vec4::new(-1.0, 2.0, -0.5, 1.5));
    });
}

// --- Checkbox edge cases ---

fn test_checkbox_edge_cases(mut ui: UiBuilder) {
    ui.build("Checkbox Edge Cases", |ui| {
        // Many checkboxes in a row
        ui.horizontal();
        for _ in 0..10 {
            ui.check_box(true);
        }
        ui.vertical();
        ui.horizontal();
        for _ in 0..10 {
            ui.check_box(false);
        }
        ui.vertical();
    });
}

// --- Float input edge cases ---

fn test_float_input_edge_cases(mut ui: UiBuilder) {
    ui.build("Float Input Edge Cases", |ui| {
        ui.float_input(id!(), 0.0, 80.0);
        ui.float_input(id!(), f32::MAX, 80.0);
        ui.float_input(id!(), f32::MIN, 80.0);
        ui.float_input(id!(), f32::INFINITY, 80.0);
        ui.float_input(id!(), f32::NEG_INFINITY, 80.0);
        ui.float_input(id!(), f32::NAN, 80.0);
        ui.float_input(id!(), -0.0, 80.0);
        ui.float_input(id!(), 1.234567890123456, 80.0);
    });
}

// --- Combined stress / kitchen sink ---

/// Everything together in a single window with deep nesting
fn test_kitchen_sink(mut ui: UiBuilder) {
    ui.build("Kitchen Sink", |ui| {
        ui.text("Top-level text");
        ui.horizontal();
        ui.button("H-Button 1");
        ui.button("H-Button 2");
        ui.check_box(true);
        ui.vertical();

        ui.collapsable("Settings", |ui| {
            ui.horizontal();
            ui.text("Color:");
            ui.color_picker(id!(), Vec4::new(0.2, 0.6, 1.0, 1.0));
            ui.vertical();

            ui.horizontal();
            ui.text("Speed:");
            ui.slider(id!(), 0.0, 10.0, 100.0, 3.0);
            ui.vertical();

            ui.horizontal();
            ui.text("Mode:");
            ui.dropdown(id!(), 1, &["Off", "Low", "High"]);
            ui.vertical();

            ui.collapsable("Advanced", |ui| {
                ui.float_input(id!(), 1.0, 60.0);
                ui.check_box(false);
                ui.container(id!(), glam::Vec2::new(180.0, 60.0), |ui| {
                    for i in 0..8 {
                        ui.text(format!("Option {i}"));
                    }
                });
            });
        });

        ui.collapsable("Log", |ui| {
            ui.container(id!(), glam::Vec2::new(250.0, 120.0), |ui| {
                for i in 0..30 {
                    ui.text(format!("[INFO] Log line {i}"));
                }
            });
        });

        ui.text(format!("{:#?}", ui.content_max));
    });
}

pub fn add_tests(app: &mut App) {
    app.add_systems(Update, (
        // tree_visulizer,
        // test_vertical_layout,
        // test_horizontal_layout,
        // test_multiple_horizontal_rows,
        // test_mixed_horizontal_row,
        // test_nested_collapsables,
        // test_collapsable_with_horizontal,
        // test_sibling_collapsables,
        // test_scrollable_container,
        // test_nested_containers,
        // test_container_with_horizontal,
        // test_text_edge_cases,
        // test_slider_edge_cases,
        // test_dropdown_edge_cases,
        test_color_picker_edge_cases,
        test_checkbox_edge_cases,
        test_float_input_edge_cases,
        test_kitchen_sink,
    ));
}
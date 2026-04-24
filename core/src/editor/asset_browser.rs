use std::{
    fs::FileType,
    mem::{replace, swap, take},
    path::{Path, PathBuf},
};

use anyhow::{Result, anyhow};
use bevy::{
    asset::{AssetServer, Handle, UntypedHandle},
    ecs::{
        resource::Resource,
        system::{Local, ResMut},
    },
    log,
    window::FileDragAndDrop,
};
use glam::{UVec2, Vec2};
use imgui::{DragDropFlags, StyleVar};

use crate::{
    assets::mesh::{GpuMesh, Scene},
    ui::UiBuilder,
};

#[derive(Resource)]
pub struct AssetDND(pub Option<UntypedHandle>);

//TODO make faster
pub(crate) fn asset_browser(
    mut ui: ResMut<UiBuilder>,
    mut asset_server: ResMut<AssetServer>,
    mut dnd: ResMut<AssetDND>,
    mut local: Local<(Option<PathBuf>, Vec<PathBuf>, usize)>,
) {
    let (cwd, replay, redo_cursor) = &mut *local;
    let cwd =
        cwd.get_or_insert_with(|| std::env::current_dir().unwrap().join("game").join("assets"));

    let Some(ui) = ui.ui() else { return };

    let res: Option<std::result::Result<(), anyhow::Error>> =
        ui.window("Asset Browser##asset_browser").build(|| {
            let item_size = [150.0, 150.0];
            let item_padding = 8.0;

            let _disalbe = ui.begin_disabled(*redo_cursor == 0);
            if ui.button("<") {
                *redo_cursor -= 1;
                replay.push(cwd.clone());
                *cwd = replay[*redo_cursor].clone();
            }
            _disalbe.end();
            ui.same_line();
            let _disalbe = ui.begin_disabled(*redo_cursor + 1 >= replay.len());
            if ui.button(">") {
                *redo_cursor += 1;
                *cwd = replay[*redo_cursor].clone();
            }
            _disalbe.end();
            ui.same_line();

            ui.text("");

            let color = ui.push_style_color(imgui::StyleColor::Button, [0.0; 4]);
            let border = ui.push_style_color(imgui::StyleColor::Border, [0.0; 4]);
            let pad = ui.push_style_var(StyleVar::FramePadding([0.0, 3.0]));

            let pad2 = ui.push_style_var(StyleVar::ItemSpacing([0.0, 4.0]));
            let mut path = PathBuf::new();
            for (i, seg) in cwd.iter().enumerate() {
                path.push(seg);
                if let Some(str) = seg.to_str() {
                    ui.same_line();
                    if ui.button(str) {
                        if *redo_cursor != replay.len() {
                            replay.clear();
                        }
                        replay.push(replace(cwd, path));
                        *redo_cursor = replay.len();
                        break;
                    }
                    ui.same_line();
                    if i != 0 {
                        ui.text("/");
                    }
                }
            }
            pad2.pop();
            pad.pop();
            border.pop();
            color.pop();
            ui.separator();

            let dir = std::fs::read_dir(&*cwd)?;
            let mut entries: Vec<(String, PathBuf, FileType, String)> = dir
                .filter_map(|e| e.ok())
                .filter_map(|e| {
                    let name = e.file_name().into_string().ok()?;
                    let ft = e.file_type().ok()?;
                    let extention = e
                        .path()
                        .extension()
                        .and_then(|v| v.to_owned().into_string().ok())
                        .unwrap_or(String::new());
                    Some((name, e.path(), ft, extention))
                })
                .collect();

            entries.sort_by(|a, b| b.2.is_dir().cmp(&a.2.is_dir()).then(a.0.cmp(&b.0)));

            let available_width = ui.content_region_avail()[0];
            let cell_size = item_size[0] + item_padding;
            let cols = ((available_width / cell_size) as usize).max(1);

            let draw = ui.get_window_draw_list();
            let _pad = ui.push_style_var(StyleVar::ItemSpacing([item_padding; 2]));
            for (i, (name, path, file_type, extention)) in entries.iter().enumerate() {
                if i % cols != 0 {
                    ui.same_line();
                }

                let pos = ui.cursor_screen_pos();

                ui.invisible_button(&format!("##asset_{}", i), item_size);

                let hovered = ui.is_item_hovered();
                let double_clicked =
                    ui.is_mouse_double_clicked(imgui::MouseButton::Left) && hovered;

                draw.add_rect(
                    pos,
                    [pos[0] + item_size[0], pos[1] + item_size[1]],
                    if hovered {
                        UiBuilder::S1
                    } else {
                        UiBuilder::S0
                    },
                )
                .filled(true)
                .rounding(3.0)
                .build();

                // let icon_pad = 16.0;
                let icon_bottom = pos[1] + item_size[1] * 0.62;
                // let icon_color = if *is_dir { UiBuilder::WARN } else { UiBuilder::BLUE };
                // draw.add_rect(
                //     [pos[0] + icon_pad, pos[1] + icon_pad],
                //     [pos[0] + item_size[0] - icon_pad, icon_bottom],
                //     icon_color,
                // )
                // .filled(true)
                // .rounding(2.0)
                // .build();

                let label_y = icon_bottom + 4.0;
                let max_chars = ((item_size[0] - 8.0) / 8.0) as usize;
                let display_name = if name.len() > max_chars {
                    format!("{}...", &name[..max_chars.saturating_sub(1)])
                } else {
                    name.clone()
                };
                let text_size = ui.calc_text_size(&display_name);
                let text_x = pos[0] + (item_size[0] - text_size[0]) * 0.5;
                draw.add_text([text_x, label_y], UiBuilder::TEXT, &display_name);

                // Hover border
                if hovered {
                    draw.add_rect(
                        pos,
                        [pos[0] + item_size[0], pos[1] + item_size[1]],
                        UiBuilder::BLUE,
                    )
                    .rounding(3.0)
                    .thickness(1.0)
                    .build();
                }

                if double_clicked && file_type.is_dir() {
                    *cwd = path.clone();
                }

                if !file_type.is_dir() {
                    // in asset browser, when dragging a file
                    if ui.is_item_active() {
                        unsafe {
                            // if let loader = asset_server.get_source(path.clone()) {

                            //     if imgui::sys::igBeginDragDropSource(imgui::sys::ImGuiDragDropFlags_None as i32) {
                            //         // empty payload — the actual data lives in the resource
                            //         let string = format!("ASSET_DND_{:?}\0", id.type_id());
                            //         imgui::sys::igSetDragDropPayload(
                            //             string.as_ptr() as *const i8,
                            //             std::ptr::null(),
                            //             0,
                            //             imgui::sys::ImGuiCond_Once as i32,
                            //         );
                            //         imgui::sys::igText(c"%s".as_ptr(), name.as_ptr());
                            //         imgui::sys::igEndDragDropSource();
                            //         log::info!("Loading Asset");
                            //         dnd.0 = Some(asset_server.load_untyped(path.clone()).untyped());
                            //     }
                            // }
                        }
                    }
                }
            }

            Ok(())
        });

    if let Some(Err(e)) = res {
        log::error!("{:#?}", e);
    }
}

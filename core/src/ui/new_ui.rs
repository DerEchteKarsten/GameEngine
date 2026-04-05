use std::path::PathBuf;
use lava::image::{Image, format, usage};
use fontdue::*;

pub struct UiBuilder {

}

pub struct UiContext {
    font: PathBuf,
    font_settings: FontSettings,
}

pub struct UiResources {
    font_atlas: Image<format::R8Unorm, usage::Sampled>,
}
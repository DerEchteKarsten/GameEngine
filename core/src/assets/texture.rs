// use bevy::{asset::{Asset, AssetLoader}, ecs::reflect, reflect::{Reflect, TypePath}};
// use lava::image::{Image, usage, format};

// #[derive(TypePath)]
// pub struct TextureAssetLoader;

// #[derive(TypePath, Asset)]
// pub struct Texture {
//     image: Image<format::R8G8B8A8Srgb, usage::Sampled>,
// }

// #[derive(TypePath, Asset)]
// pub struct HdriTexture {
//     image: Image<format::R32G32B32A32Sfloat, usage::Sampled>,
// }

// impl AssetLoader for TextureAssetLoader {
//     type Asset = Texture;
//     type Error = anyhow::Error;
//     type Settings = ();
//     fn extensions(&self) -> &[&str] {
//         &[".png", ".jpg"]
//     }
//     fn load(
//             &self,
//             reader: &mut dyn bevy::asset::io::Reader,
//             settings: &Self::Settings,
//             load_context: &mut bevy::asset::LoadContext,
//         ) -> impl bevy::tasks::ConditionalSendFuture<Output = Result<Self::Asset, Self::Error>> {
//         image::load(std::io::BufReader::new(reader), image::ImageFormat::)
//     }
// }

use std::path::PathBuf;

use spirv_builder::{Capability, MetadataPrintout, SpirvBuilder, SpirvMetadata};

fn main() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");

    println!("cargo::rerun-if-changed={}/../shaders/src/", manifest_dir);
    let crate_path = [manifest_dir, "..", "shaders"]
        .iter()
        .copied()
        .collect::<PathBuf>();

    let _ = SpirvBuilder::new(crate_path, "spirv-unknown-vulkan1.4")
        .print_metadata(MetadataPrintout::Full)
        .shader_panic_strategy(spirv_builder::ShaderPanicStrategy::DebugPrintfThenExit {
            print_inputs: true,
            print_backtrace: true,
        })
        .release(false)
        .capability(Capability::Linkage)
        .capability(Capability::Shader)
        .spirv_metadata(SpirvMetadata::Full)
        .extension("SPV_KHR_non_semantic_info")
        .build()
        .unwrap();

    unsafe {
        std::env::set_var("VK_LAYER_PRINTF_ONLY_PRESET", "0");
        std::env::set_var("VK_LAYER_PRINTF_TO_STDOUT", "1");
        std::env::set_var("VK_LAYER_PRINTF_BUFFER_SIZE", "10000");
    }
}

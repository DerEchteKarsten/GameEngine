use std::{path::PathBuf, process::Command};

use spirv_builder::{Capability, MetadataPrintout, SpirvBuilder};


fn main() {
    println!("cargo::rerun-if-changed=./../shaders/src/*");

    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let crate_path = [manifest_dir, "..", "shaders"]
        .iter()
        .copied()
        .collect::<PathBuf>();

    _ = SpirvBuilder::new(crate_path, "spirv-unknown-vulkan1.4")
        .print_metadata(MetadataPrintout::Full)
        .shader_panic_strategy(spirv_builder::ShaderPanicStrategy::DebugPrintfThenExit {
            print_inputs: true,
            print_backtrace: true,
        })
        .capability(Capability::Linkage)
        .build()
        .unwrap();

    unsafe {
        std::env::set_var("VK_LAYER_PRINTF_ONLY_PRESET", "0");
        std::env::set_var("VK_LAYER_PRINTF_TO_STDOUT", "1");
        std::env::set_var("VK_LAYER_PRINTF_BUFFER_SIZE", "10000");
    }
}

use std::{
    env,
    fs::{self, File},
    io::{self, Write},
    path::{Path, PathBuf},
};

use spirv_builder::{Capability, MetadataPrintout, SpirvBuilder, SpirvMetadata};

fn main() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");

    let generated_dir = PathBuf::from(manifest_dir)
        .join("..")
        .join("shaders")
        .join("target")
        .join("bindings");

    println!("cargo:rerun-if-changed={}", generated_dir.display());
    println!("cargo:rerun-if-changed={}/../shaders/src/", manifest_dir);

    let crate_path = [manifest_dir, "..", "shaders"]
        .iter()
        .copied()
        .collect::<PathBuf>();


    let out_file = PathBuf::from(manifest_dir)
        .join("src")
        .join("bindings.rs");

    let mut files: Vec<_> = fs::read_dir(&generated_dir).unwrap()
        .map(|entry| {
            entry.ok().unwrap().path()
        })
        .collect();

    files.sort();

    let mut out = File::create(&out_file).unwrap();
    writeln!(out, "use glam::*;").unwrap();
    for file in files {
        let contents = fs::read_to_string(&file).unwrap();
        writeln!(out, "// ===== auto-included: {} =====", file.display()).unwrap();
        writeln!(out, "{}", contents).unwrap();
        writeln!(out).unwrap();
    }

    let _ = SpirvBuilder::new(crate_path, "spirv-unknown-vulkan1.4")
        .print_metadata(MetadataPrintout::Full)
        .shader_panic_strategy(spirv_builder::ShaderPanicStrategy::DebugPrintfThenExit {
            print_inputs: true,
            print_backtrace: true,
        })
        .release(false)
        .extension("SPV_KHR_non_semantic_info")
        .extension("SPV_KHR_physical_storage_buffer")
        .capability(Capability::Linkage)
        .capability(Capability::Shader)
        .capability(Capability::Int64)
        .capability(Capability::VariablePointers)
        .capability(Capability::VariablePointersStorageBuffer)
        .capability(Capability::PhysicalStorageBufferAddresses)
        // .capability(Capability::Addresses)
        .spirv_metadata(SpirvMetadata::Full)
        .build()
        .unwrap();

    unsafe {
        std::env::set_var("VK_LAYER_PRINTF_ONLY_PRESET", "0");
        std::env::set_var("VK_LAYER_PRINTF_TO_STDOUT", "1");
        std::env::set_var("VK_LAYER_PRINTF_BUFFER_SIZE", "10000");
    }
}

//! Compiles the engine's Slang passes with lava-build-slang into the bindings `src/bindings.rs` includes
fn main() {
    lava_build_slang::Shaders::new()
        .passes("../shaders/passes")
        .include("../shaders/include")
        .compile();
}

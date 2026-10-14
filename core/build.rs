//! Compiles the engine's Slang passes with lava-build into the bindings `src/bindings.rs` includes
fn main() {
    lava_build::Shaders::new()
        .passes("../shaders/passes")
        .include("../shaders/include")
        .compile();
}

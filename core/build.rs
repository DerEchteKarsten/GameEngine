use std::process::Command;

fn main() {
    // println!(r"cargo:rustc-link-search=./lib");
    // println!("cargo::rustc-link-lib=DGFBaker");
    println!("cargo::rerun-if-changed=shaders");
    match Command::new("./compile.sh").output() {
        Ok(out) => {
            if out.status.success() {
                println!("cargo::warning={:?}", String::from_utf8(out.stdout));
            } else {
                println!("cargo::error={:?}", String::from_utf8(out.stderr));
            }
        }
        Err(err) => {
            println!("cargo::error={:?}", err);
        }
    }

    unsafe {
        std::env::set_var("VK_LAYER_PRINTF_ONLY_PRESET", "0");
        std::env::set_var("VK_LAYER_PRINTF_TO_STDOUT", "1");
        std::env::set_var("VK_LAYER_PRINTF_BUFFER_SIZE", "10000");
    }
}

//! The wasm module's stack (D1111): rustc's default shadow stack on wasm32 is 1 MiB, too
//! little for the engine's nesting limit (about 3 KiB per function call level in
//! release builds, 15 KiB in debug, D1059). The module gets 16 MiB, the same budget as
//! ShellConfig's native default; the shell's `max_depth` is set from it (src/shell.rs).

fn main() {
    let target = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    if target == "wasm32" {
        println!(
            "cargo:rustc-link-arg-cdylib=-zstack-size={}",
            16 * 1024 * 1024
        );
    }
    println!("cargo:rerun-if-changed=build.rs");
}

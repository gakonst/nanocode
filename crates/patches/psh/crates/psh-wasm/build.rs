//! The wasm module's stack (D1111): rustc's default shadow stack on wasm32 is 1 MiB, too
//! little for the engine's nesting limit (about 3 KiB per function call level in
//! release builds, 15 KiB in debug, D1059). The module gets 16 MiB, the same budget as
//! ShellConfig's native default; the shell's `max_depth` is set from it (src/shell.rs).

fn main() {
    // The same sources are two modules (nanocode's addition): psh-wasm, and
    // psh-wasm-core, which leaves git to the first and is loaded where no repository
    // is in sight. The package name says which one this is.
    // That one is always the shell: it has no parser-only build, and so no feature.
    println!("cargo::rustc-check-cfg=cfg(psh_without_git)");
    if std::env::var("CARGO_PKG_NAME").as_deref() == Ok("psh-wasm-core") {
        println!("cargo::rustc-cfg=psh_without_git");
        println!("cargo::rustc-check-cfg=cfg(feature, values(\"shell\"))");
        println!("cargo::rustc-cfg=feature=\"shell\"");
    }
    let target = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    if target == "wasm32" {
        println!(
            "cargo:rustc-link-arg-cdylib=-zstack-size={}",
            16 * 1024 * 1024
        );
    }
    println!("cargo:rerun-if-changed=build.rs");
}

fn main() {
    println!("cargo:rerun-if-env-changed=WASI_SYSROOT");
    if std::env::var("CARGO_CFG_TARGET_FAMILY").as_deref() != Ok("wasm") {
        return;
    }
    let Ok(sysroot) = std::env::var("WASI_SYSROOT") else {
        panic!(
            "WASM builds compile QuickJS against wasi-libc: set WASI_SYSROOT to a wasi-sdk sysroot (share/wasi-sysroot)"
        );
    };
    println!("cargo:rustc-link-search=native={sysroot}/lib/wasm32-wasip1");
    println!("cargo:rustc-link-lib=static=c");
}

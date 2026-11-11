fn main() {
    println!("cargo:rerun-if-env-changed=NC_LITE_SYMBOL_ORDER");
    println!("cargo:rerun-if-env-changed=NC_LITE_LINK_MAP");
    if std::env::var("TARGET").as_deref() == Ok("x86_64-unknown-linux-gnu") {
        println!("cargo:rustc-link-arg-bins=-Wl,--icf=all");
        if let Ok(order) = std::env::var("NC_LITE_SYMBOL_ORDER") {
            println!("cargo:rerun-if-changed={order}");
            println!("cargo:rustc-link-arg-bins=-Wl,--symbol-ordering-file={order}");
        }
        if let Ok(map) = std::env::var("NC_LITE_LINK_MAP") {
            println!("cargo:rustc-link-arg-bins=-Wl,-Map={map},--no-demangle");
        }
    }
}

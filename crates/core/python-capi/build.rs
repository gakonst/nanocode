use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

fn sorted(dir: &Path, keep: impl Fn(&Path) -> bool) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap_or_else(|error| panic!("{}: {error}", dir.display()))
        .map(|entry| entry.expect("a directory entry").path())
        .filter(|path| keep(path))
        .collect();
    paths.sort();
    paths
}

fn archive(objects: &[PathBuf]) -> Vec<u8> {
    let mut out = b"!<arch>\n".to_vec();
    for (index, object) in objects.iter().enumerate() {
        let bytes = fs::read(object).expect("a compiled object");
        let header = format!(
            "{:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`\n",
            format!("{index}.o/"),
            0,
            0,
            0,
            644,
            bytes.len()
        );
        out.extend_from_slice(header.as_bytes());
        out.extend_from_slice(&bytes);
        if bytes.len() % 2 == 1 {
            out.push(b'\n');
        }
    }
    out
}

fn main() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let target = env::var("TARGET").expect("TARGET");
    let host = env::var("HOST").expect("HOST");
    let wasm = env::var("CARGO_CFG_TARGET_FAMILY")
        .is_ok_and(|families| families.split(',').any(|family| family == "wasm"));
    let variable = format!("CC_{}", target.replace('-', "_"));
    println!("cargo:rerun-if-env-changed=NANOCODE_CC");
    println!("cargo:rerun-if-env-changed={variable}");
    println!("cargo:rerun-if-changed=include");
    println!("cargo:rerun-if-changed=ext");
    let compiler = env::var_os("NANOCODE_CC")
        .or_else(|| env::var_os(&variable))
        .unwrap_or_else(|| {
            if wasm || target != host {
                "clang".into()
            } else {
                "cc".into()
            }
        });
    let include = root.join("include");
    let mut objects = Vec::new();
    let mut table = String::new();
    let mut entries = String::new();
    for dir in sorted(&root.join("ext"), |path| path.is_dir()) {
        let name = dir
            .file_name()
            .and_then(|name| name.to_str())
            .expect("a module directory named by its dotted name")
            .to_owned();
        let symbol = format!("PyInit_{}", name.replace('.', "_"));
        let last = name.rsplit('.').next().unwrap_or(&name);
        for source in sorted(&dir, |path| {
            path.extension().is_some_and(|extension| extension == "c")
        }) {
            let object = out.join(format!("{}.o", objects.len()));
            let mut command = Command::new(&compiler);
            if target != host {
                command.arg(format!("--target={target}"));
            }
            if wasm {
                command
                    .args(["-ffreestanding", "-nostdlibinc"])
                    .arg("-I")
                    .arg(include.join("libc"));
            } else {
                command.arg("-fPIC");
            }
            command
                .args(["-O2", "-DNDEBUG", "-fvisibility=hidden"])
                .arg(format!("-DPyInit_{last}={symbol}"))
                .arg("-I")
                .arg(&include)
                .arg("-I")
                .arg(include.join("cpython"))
                .arg("-c")
                .arg(&source)
                .arg("-o")
                .arg(&object);
            let status = command
                .status()
                .unwrap_or_else(|error| panic!("{}: {error}", compiler.to_string_lossy()));
            assert!(status.success(), "compiling {} failed", source.display());
            objects.push(object);
        }
        table.push_str(&format!("    fn {symbol}() -> *mut Object;\n"));
        entries.push_str(&format!(
            "    Extension {{ name: {name:?}, init: {symbol} }},\n"
        ));
    }
    fs::write(out.join("libnanocode_python_capi_ext.a"), archive(&objects)).expect("the archive");
    fs::write(
        out.join("extensions.rs"),
        format!("unsafe extern \"C\" {{\n{table}}}\n\npub static EXTENSIONS: &[Extension] = &[\n{entries}];\n"),
    )
    .expect("the extension table");
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=nanocode_python_capi_ext");
}

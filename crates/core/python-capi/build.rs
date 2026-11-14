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

fn leb(bytes: &[u8], at: &mut usize) -> u64 {
    let mut result = 0;
    let mut shift = 0;
    loop {
        let byte = bytes[*at];
        *at += 1;
        result |= u64::from(byte & 0x7f) << shift;
        shift += 7;
        if byte & 0x80 == 0 {
            return result;
        }
    }
}

fn index(bytes: &[u8], at: &mut usize) -> usize {
    usize::try_from(leb(bytes, at)).expect("an index that fits")
}

fn string(bytes: &[u8], at: &mut usize) -> String {
    let length = index(bytes, at);
    let text = String::from_utf8_lossy(&bytes[*at..*at + length]).into_owned();
    *at += length;
    text
}

fn limits(bytes: &[u8], at: &mut usize) {
    let flags = bytes[*at];
    *at += 1;
    leb(bytes, at);
    if flags & 1 != 0 {
        leb(bytes, at);
    }
}

const SYMBOL_TABLE: u8 = 8;
const UNDEFINED: u64 = 0x10;
const EXPLICIT_NAME: u64 = 0x40;
const TABLE_INDEX: [u8; 6] = [1, 2, 12, 18, 19, 24];
const ADDEND: [u8; 14] = [3, 4, 5, 8, 9, 11, 14, 15, 16, 17, 21, 22, 23, 25];

fn identifier(name: &str) -> bool {
    name.chars()
        .next()
        .is_some_and(|first| !first.is_ascii_digit())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn address_taken(object: &Path) -> Vec<(String, usize)> {
    let bytes = fs::read(object).expect("a compiled object");
    let mut at = 8;
    let mut types = Vec::new();
    let mut imported = 0;
    let mut functions = Vec::new();
    let mut symbols: Vec<(u8, u64, usize, String)> = Vec::new();
    let mut taken = std::collections::BTreeSet::new();
    while at < bytes.len() {
        let id = bytes[at];
        at += 1;
        let size = index(&bytes, &mut at);
        let end = at + size;
        let mut p = at;
        match id {
            1 => {
                for _ in 0..index(&bytes, &mut p) {
                    p += 1;
                    let params = index(&bytes, &mut p);
                    p += params;
                    let results = index(&bytes, &mut p);
                    p += results;
                    types.push(params);
                }
            }
            2 => {
                for _ in 0..index(&bytes, &mut p) {
                    string(&bytes, &mut p);
                    string(&bytes, &mut p);
                    let kind = bytes[p];
                    p += 1;
                    match kind {
                        0 => {
                            leb(&bytes, &mut p);
                            imported += 1;
                        }
                        1 => {
                            p += 1;
                            limits(&bytes, &mut p);
                        }
                        2 => limits(&bytes, &mut p),
                        3 => p += 2,
                        _ => {
                            p += 1;
                            leb(&bytes, &mut p);
                        }
                    }
                }
            }
            3 => {
                for _ in 0..index(&bytes, &mut p) {
                    functions.push(index(&bytes, &mut p));
                }
            }
            0 => {
                let section = string(&bytes, &mut p);
                if section == "linking" {
                    leb(&bytes, &mut p);
                    while p < end {
                        let kind = bytes[p];
                        p += 1;
                        let length = index(&bytes, &mut p);
                        let next = p + length;
                        if kind == SYMBOL_TABLE {
                            for _ in 0..index(&bytes, &mut p) {
                                let kind = bytes[p];
                                p += 1;
                                let flags = leb(&bytes, &mut p);
                                match kind {
                                    1 => {
                                        let name = string(&bytes, &mut p);
                                        if flags & UNDEFINED == 0 {
                                            leb(&bytes, &mut p);
                                            leb(&bytes, &mut p);
                                            leb(&bytes, &mut p);
                                        }
                                        symbols.push((kind, flags, 0, name));
                                    }
                                    3 => {
                                        let target = index(&bytes, &mut p);
                                        symbols.push((kind, flags, target, String::new()));
                                    }
                                    _ => {
                                        let target = index(&bytes, &mut p);
                                        let name = if flags & UNDEFINED == 0
                                            || flags & EXPLICIT_NAME != 0
                                        {
                                            string(&bytes, &mut p)
                                        } else {
                                            String::new()
                                        };
                                        symbols.push((kind, flags, target, name));
                                    }
                                }
                            }
                        }
                        p = next;
                    }
                } else if section.starts_with("reloc.") {
                    leb(&bytes, &mut p);
                    for _ in 0..index(&bytes, &mut p) {
                        let kind = bytes[p];
                        p += 1;
                        leb(&bytes, &mut p);
                        let symbol = index(&bytes, &mut p);
                        if ADDEND.contains(&kind) {
                            leb(&bytes, &mut p);
                        }
                        if TABLE_INDEX.contains(&kind) {
                            taken.insert(symbol);
                        }
                    }
                }
            }
            _ => {}
        }
        at = end;
    }
    taken
        .into_iter()
        .filter_map(|symbol| {
            let (kind, flags, function, name) = symbols.get(symbol)?;
            (*kind == 0 && flags & UNDEFINED == 0 && *function >= imported && identifier(name))
                .then(|| (name.clone(), types[functions[function - imported]]))
        })
        .collect()
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
    let mut arities = String::new();
    let mut tables = String::new();
    println!("cargo:rerun-if-changed=c");
    let compile = |source: &Path, object: &Path, defines: &[String]| {
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
            .args([
                "-O2",
                "-DNDEBUG",
                "-DCYTHON_LIMITED_API=1",
                "-fvisibility=hidden",
            ])
            .args(defines)
            .arg("-I")
            .arg(&include)
            .arg("-I")
            .arg(include.join("cpython"))
            .arg("-c")
            .arg(source)
            .arg("-o")
            .arg(object);
        let status = command
            .status()
            .unwrap_or_else(|error| panic!("{}: {error}", compiler.to_string_lossy()));
        assert!(status.success(), "compiling {} failed", source.display());
    };
    for source in sorted(&root.join("c"), |path| {
        path.extension().is_some_and(|extension| extension == "c")
    }) {
        let object = out.join(format!("{}.o", objects.len()));
        compile(&source, &object, &[]);
        objects.push(object);
    }
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
            let mut defines = vec![
                format!("-DPyInit_{last}={symbol}"),
                format!("-I{}", dir.display()),
            ];
            defines.extend(
                sorted(&dir, |path| path.is_dir())
                    .iter()
                    .map(|include| format!("-I{}", include.display())),
            );
            let object = out.join(format!("{}.o", objects.len()));
            compile(&source, &object, &defines);
            if wasm {
                let functions = address_taken(&object);
                let table = format!("nanocode_arity_{}", objects.len());
                let mut wrapper = format!(
                    "#include {:?}\n__attribute__((used)) const void *const {table}[] = {{\n",
                    source.display().to_string()
                );
                for (function, params) in &functions {
                    wrapper.push_str(&format!(
                        "    (const void *){function}, (const void *){params},\n"
                    ));
                }
                wrapper.push_str("    0, 0\n};\n");
                let path = out.join(format!("{}.c", objects.len()));
                fs::write(&path, wrapper).expect("the arity wrapper");
                compile(&path, &object, &defines);
                arities.push_str(&format!("    static {table}: [usize; 0];\n"));
                tables.push_str(&format!("        &raw const {table} as *const usize,\n"));
            }
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
        format!("unsafe extern \"C\" {{\n{table}{arities}}}\n\npub static EXTENSIONS: &[Extension] = &[\n{entries}];\n\npub fn arity_tables() -> Vec<*const usize> {{\n    vec![\n{tables}    ]\n}}\n"),
    )
    .expect("the extension table");
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=nanocode_python_capi_ext");
}

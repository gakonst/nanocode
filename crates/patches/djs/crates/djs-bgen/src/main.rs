use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "\
usage: djs-bgen [--check] [--report] [--dir DIR]

Compiles the builtins (builtins/intrinsics.d.ts and builtins/*.ts) and writes
builtins/gen/prelude.js, builtins/gen/prelude-slim.js (without the files marked
`// @slim-omit` or `// @embed-only`), builtins/gen/prelude-noembed.js (without those
marked `// @embed-only`), builtins/gen/resume-points.txt and the Rust backend's
crates/djs-core/src/builtins/gen/natives.rs.

  --check     do not write; exit 1 if the generated files are out of date
  --report    print the resume-point report instead of writing files
  --dir DIR   the builtins directory (default: builtins/ of the workspace)
";

fn main() -> ExitCode {
    let mut check = false;
    let mut report = false;
    let mut dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../builtins");
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--check" => check = true,
            "--report" => report = true,
            "--dir" => match args.next() {
                Some(d) => dir = PathBuf::from(d),
                None => return usage("--dir needs a value"),
            },
            "-h" | "--help" => {
                print!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            other => return usage(&format!("unknown argument {other}")),
        }
    }
    let dir = fs::canonicalize(&dir).unwrap_or(dir);
    let out = match djs_bgen::compile_dir(&dir) {
        Ok(o) => o,
        Err(e) => {
            eprint!("{e}");
            return ExitCode::from(1);
        }
    };
    if report {
        print!("{}", out.report);
        return ExitCode::SUCCESS;
    }
    let files = [
        (djs_bgen::PRELUDE_PATH, &out.prelude),
        (djs_bgen::PRELUDE_SLIM_PATH, &out.prelude_slim),
        (djs_bgen::PRELUDE_NOEMBED_PATH, &out.prelude_noembed),
        (djs_bgen::REPORT_PATH, &out.report),
        (djs_bgen::NATIVES_PATH, &out.natives),
    ];
    if check {
        let stale: Vec<&str> = files
            .iter()
            .filter(|(path, text)| {
                fs::read_to_string(dir.join(path)).ok().as_deref() != Some(text.as_str())
            })
            .map(|(path, _)| *path)
            .collect();
        if stale.is_empty() {
            return ExitCode::SUCCESS;
        }
        eprintln!(
            "djs-bgen: out of date: {} (run scripts/bgen.sh)",
            stale.join(", ")
        );
        return ExitCode::from(1);
    }
    for (path, text) in files {
        let p = dir.join(path);
        if let Some(parent) = p.parent()
            && let Err(e) = fs::create_dir_all(parent)
        {
            eprintln!("djs-bgen: {}: {e}", parent.display());
            return ExitCode::from(1);
        }
        if fs::read_to_string(&p).ok().as_deref() != Some(text.as_str())
            && let Err(e) = fs::write(&p, text)
        {
            eprintln!("djs-bgen: {}: {e}", p.display());
            return ExitCode::from(1);
        }
    }
    ExitCode::SUCCESS
}

fn usage(msg: &str) -> ExitCode {
    eprintln!("djs-bgen: {msg}\n\n{USAGE}");
    ExitCode::from(2)
}

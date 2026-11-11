//! The `psh` program: `psh::run` with the process's arguments (`lib.rs`).

fn main() -> std::process::ExitCode {
    psh::run(std::env::args_os().skip(1).collect())
}

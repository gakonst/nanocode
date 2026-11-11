#![cfg(not(target_family = "wasm"))]

#[global_allocator]
static ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() -> std::process::ExitCode {
    nanocode_cli::run()
}

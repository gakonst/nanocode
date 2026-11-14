#![cfg(not(target_family = "wasm"))]

#[global_allocator]
static ALLOCATOR: nanocode_sys::alloc::Heap = nanocode_sys::alloc::Heap;

fn main() -> std::process::ExitCode {
    nanocode_cli::run()
}

#![cfg(unix)]

#[global_allocator]
static ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() {
    std::process::exit(nanocode_python::serve());
}

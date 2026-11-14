#![cfg(unix)]

#[global_allocator]
static ALLOCATOR: nanocode_sys::alloc::Heap = nanocode_sys::alloc::Heap;

fn main() {
    std::process::exit(nanocode_python::serve());
}

//! Unit tests that need the crate's internals: panics inside the guard.

use crate::{Error, Limits, Snapshot, Source};

fn snapshot() -> Snapshot {
    Snapshot::builder()
        .source(Source::script(
            "s.js",
            "globalThis.n = 1; function f() { return n; }",
        ))
        .build()
        .unwrap()
}

/// A panic in engine code poisons only its instance (D3184).
#[test]
fn a_panic_poisons_only_its_instance() {
    let snap = snapshot();
    let mut a = snap.instantiate(Limits::default()).unwrap();
    let mut b = snap.instantiate(Limits::default()).unwrap();
    // Stands in for a djs bug: the guard sees a panic from the engine call.
    let r = a.guard(|_| panic!("engine bug"));
    assert_eq!(r, Err(Error::Panicked("engine bug".into())));
    assert!(a.is_poisoned());
    assert_eq!(a.call("f", &[], |_| Ok(())), Err(Error::Poisoned));
    assert_eq!(a.reset(), Err(Error::Poisoned));
    // The other instance and the snapshot are fine.
    assert_eq!(b.call("f", &[], |_| Ok(())).unwrap(), 1.into());
    let mut c = snap.instantiate(Limits::default()).unwrap();
    assert_eq!(c.call("f", &[], |_| Ok(())).unwrap(), 1.into());
}

/// A real one: a corrupted heap makes djs panic (a bad cell is a panic, not UB).
#[test]
fn a_corrupt_heap_panics_into_an_error() {
    let snap = snapshot();
    let mut a = snap.instantiate(Limits::default()).unwrap();
    a.start("f", &[]).unwrap();
    // Overwrite everything above the builtins with garbage (the builtins are guarded by a
    // write barrier in debug builds).
    let (start, len) = (a.vm.heap().base_end(), a.vm.heap().arena().len());
    let bytes = a.vm.heap_mut().arena_mut().slice_mut(start, len - start);
    bytes.fill(0xA5);
    let r = a.step();
    assert!(matches!(r, Err(Error::Panicked(_))), "{r:?}");
    assert_eq!(a.step(), Err(Error::Poisoned));
    let mut c = snap.instantiate(Limits::default()).unwrap();
    assert_eq!(c.call("f", &[], |_| Ok(())).unwrap(), 1.into());
}

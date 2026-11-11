# djs-embed changes

The crate follows semver for 0.x: a patch release (0.1.x) only adds, a minor release (0.2)
may break the API. Hosts that pin a git revision read this file before moving the pin;
every incompatible change is listed here with what to do instead.

## nanocode's additions (not upstream)

- `Instance::resolve_unhandled`, `Instance::seed_random`, `Instance::set_time_zone_offset`.
- `Instance::park` and `Parked`: an instance between two steps held as what differs from
  its snapshot's image (kilobytes for a call that waits, instead of its heap), and
  `Parked::resume`, which makes the instance again. `park` also gives the instance back,
  reset, when that cost only what the call wrote, and `Parked::resume_in` resumes in
  such a spare's heap, so neither side copies the image. `Parked::to_bytes` and
  `Snapshot::parked` are the same state as bytes a host can store and load in another
  process of the same build; `Snapshot::fingerprint` tells whether two snapshots share
  an image.
- An instance notes which pages of its image its calls write (`Vm::track_image`; the
  interpreter's own cells go last in a snapshot's image and count as written):
  `Instance::reset_written` is `reset` by copying those pages back instead of the image
  (`false` once a collection ran), `reset` tries it first, and `park` looks for
  differences in those pages only.
- `Interrupt` and `Instance::set_interrupt`: another thread stops a call whose step is
  inside a long native operation (a split or a join of hundreds of megabytes, where the
  slice's fuel is not checked for seconds); the step ends with `Error::Interrupted` at
  its next allocation.
- A native instance's arena starts with 64 KiB beyond its image and grows to its
  collection threshold at once (wasm32 as upstream).

## 0.1.1

- `Instance::heap_size`, `Instance::heap_used` and `Instance::shrink_heap`: what the heap
  holds after `reset` (which keeps the allocation a call grew it to), and giving it back.
- JSON modules: `ModuleSource::json`, `ModuleMap::json` (`import x from "./x.json" with {
  type: "json" }`), and top-level `await` in setup modules (setup fails with
  `Error::NeverSettles` if the evaluation cannot settle).

## 0.1.0

The first version (djs 9cc1291d): snapshots, instances, typed host functions, plain data,
budgets, ES modules with synthetic modules and `import()`.

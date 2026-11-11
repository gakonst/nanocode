# djs-embed changes

The crate follows semver for 0.x: a patch release (0.1.x) only adds, a minor release (0.2)
may break the API. Hosts that pin a git revision read this file before moving the pin;
every incompatible change is listed here with what to do instead.

## 0.1.1

- `Instance::heap_size`, `Instance::heap_used` and `Instance::shrink_heap`: what the heap
  holds after `reset` (which keeps the allocation a call grew it to), and giving it back.
- JSON modules: `ModuleSource::json`, `ModuleMap::json` (`import x from "./x.json" with {
  type: "json" }`), and top-level `await` in setup modules (setup fails with
  `Error::NeverSettles` if the evaluation cannot settle).

## 0.1.0

The first version (djs 9cc1291d): snapshots, instances, typed host functions, plain data,
budgets, ES modules with synthetic modules and `import()`.

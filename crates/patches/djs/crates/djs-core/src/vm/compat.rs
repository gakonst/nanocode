//! What a VM image depends on (D026, D2265): its *dependency descriptor*.
//!
//! An image restores into any djs build with the same descriptor, not only into the
//! build that wrote it. The components are the numberings and conventions an image
//! stores numbers of:
//!
//! | component | what | where it comes from |
//! | --- | --- | --- |
//! | layout | the cells and the well-known atoms | `heap::cells::LAYOUT_HASH` |
//! | features | the cargo features that change images: the builtin set (`full`), the Unicode tables | [`FEATURES`] (build.rs, `compat_inputs.rs`) |
//! | compat | changes of meaning none of the hashes see | [`COMPAT_VERSION`], bumped by hand |
//! | ops | the instruction table (bytecode in every closure) | `bytecode::OPS_HASH` |
//! | intrinsics | the intrinsic ids (`%Name` calls in builtins) | `bytecode::intrinsics::INTRINSICS_HASH` |
//! | records | object classes, realm slots, flags, record slots, native and fast builtin indexes, RegExp program codes | [`RECORDS_HASH`] (build.rs, `compat_inputs.rs`) |
//!
//! [`crate::vm::Vm::new`] writes them into the roots cell, and loading an image checks
//! them first and names the component that differs ([`ImageDep`]). Images stay inside a
//! process (D2700): the base, prelude images, the stress modes and a wasm host's module
//! transport; persistence is a continuation dump. The engine build hash (`vm::BUILD_HASH`) is not part of
//! it: it changes with every source edit, and an image only cares about these.

use crate::heap::cells::{self, Roots};

// `RECORDS_HASH`, `FEATURES` and `FEATURE_NAMES` (build.rs, `compat_inputs.rs`).
include!(concat!(env!("OUT_DIR"), "/image_deps.rs"));

/// The compatibility version (D2265): bump it when a change alters what an image means
/// without changing one of the hashed tables. Images of another version are refused. See
/// TESTING.md "Image compatibility" for when; the usual cases are a change in what an
/// existing slot or record field holds, in how the builtins' JS reads internal records
/// (`%RecordGet` indexes), in Map/Set or string invariants, in what an inline cache word
/// or an opcode's immediate means, or in the frame layout of suspended generators.
pub const COMPAT_VERSION: u32 = 8;

/// One component of the descriptor.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ImageDep {
    Layout,
    Features,
    Compat,
    Ops,
    Intrinsics,
    Records,
}

impl ImageDep {
    /// In the order they are checked: the layout first, since the other fields are read
    /// at the offsets it gives.
    /// The features next: a slim image in a full build differs in more than them, and
    /// they are the reason.
    pub const ALL: [ImageDep; 6] = [
        ImageDep::Layout,
        ImageDep::Features,
        ImageDep::Compat,
        ImageDep::Ops,
        ImageDep::Intrinsics,
        ImageDep::Records,
    ];

    /// What it is, for messages: "the image is from a djs with another opcode table".
    pub fn name(self) -> &'static str {
        match self {
            ImageDep::Layout => "cell layout",
            ImageDep::Features => "feature set",
            ImageDep::Compat => "compatibility version",
            ImageDep::Ops => "opcode table",
            ImageDep::Intrinsics => "intrinsic table",
            ImageDep::Records => "internal id tables",
        }
    }

    /// A value of it as messages show it: the features by name ("full,
    /// regexp-unicode-props" or "none"), the version in decimal, a hash in hex.
    pub fn show(self, v: u32) -> alloc::string::String {
        match self {
            ImageDep::Features => {
                let mut out = alloc::string::String::new();
                for bit in 0..32 {
                    if v & 1 << bit != 0 {
                        if !out.is_empty() {
                            out.push_str(", ");
                        }
                        match FEATURE_NAMES.get(bit) {
                            Some(name) => out.push_str(name),
                            None => crate::text::cat_to!(&mut out, "bit ", bit as u32),
                        }
                    }
                }
                if out.is_empty() { "none".into() } else { out }
            }
            ImageDep::Compat => crate::text::cat!(v),
            _ => crate::text::cat!(crate::text::hex(v, 8)),
        }
    }

    /// Its roots field.
    pub fn field(self) -> crate::heap::layout::Field<u32> {
        match self {
            ImageDep::Layout => Roots::VERSION,
            ImageDep::Features => Roots::FEATURES,
            ImageDep::Compat => Roots::COMPAT,
            ImageDep::Ops => Roots::OPS,
            ImageDep::Intrinsics => Roots::INTRINSICS,
            ImageDep::Records => Roots::RECORDS,
        }
    }
}

/// An image's dependency descriptor (without the prefix hash, which is a container's).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ImageDeps {
    pub layout: u32,
    pub features: u32,
    pub compat: u32,
    pub ops: u32,
    pub intrinsics: u32,
    pub records: u32,
}

/// This build's descriptor: what [`crate::vm::Vm::new`] writes into every VM.
pub const IMAGE_DEPS: ImageDeps = ImageDeps {
    layout: cells::LAYOUT_HASH,
    features: FEATURES,
    compat: COMPAT_VERSION,
    ops: crate::bytecode::OPS_HASH,
    intrinsics: crate::bytecode::intrinsics::INTRINSICS_HASH,
    records: RECORDS_HASH,
};

impl ImageDeps {
    pub fn get(&self, d: ImageDep) -> u32 {
        match d {
            ImageDep::Layout => self.layout,
            ImageDep::Features => self.features,
            ImageDep::Compat => self.compat,
            ImageDep::Ops => self.ops,
            ImageDep::Intrinsics => self.intrinsics,
            ImageDep::Records => self.records,
        }
    }

    /// The descriptor in the roots cell of `image`, or None if it is not a heap image
    /// (no roots cell with the magic). The fields after the layout hash are only
    /// meaningful when it is this build's.
    pub fn of(image: &[u8]) -> Option<ImageDeps> {
        let u32_at = |f: crate::heap::layout::Field<u32>| {
            let at = f.offset as usize;
            image
                .get(at..at + 4)
                .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        };
        if image.first() != Some(&(crate::heap::CellKind::Roots as u8))
            || u32_at(Roots::MAGIC) != Some(cells::MAGIC)
        {
            return None;
        }
        Some(ImageDeps {
            layout: u32_at(Roots::VERSION)?,
            features: u32_at(Roots::FEATURES).unwrap_or(0),
            compat: u32_at(Roots::COMPAT).unwrap_or(0),
            ops: u32_at(Roots::OPS).unwrap_or(0),
            intrinsics: u32_at(Roots::INTRINSICS).unwrap_or(0),
            records: u32_at(Roots::RECORDS).unwrap_or(0),
        })
    }

    /// The first component (in [`ImageDep::ALL`] order) where `self` (an image's) differs
    /// from `ours`, with both values.
    pub fn first_difference(&self, ours: &ImageDeps) -> Option<(ImageDep, u32, u32)> {
        ImageDep::ALL
            .into_iter()
            .find(|&d| self.get(d) != ours.get(d))
            .map(|d| (d, self.get(d), ours.get(d)))
    }

    /// All components in one hash: what a session image's header records of its base.
    pub fn hash(&self) -> u32 {
        let mut h: u32 = 0x811c_9dc5;
        for d in ImageDep::ALL {
            for b in self.get(d).to_le_bytes() {
                h = (h ^ b as u32).wrapping_mul(0x0100_0193);
            }
        }
        h
    }

    /// Writes the descriptor into a heap's roots cell.
    pub(crate) fn write(&self, heap: &mut crate::heap::Heap) {
        let a = heap.arena_mut();
        for d in ImageDep::ALL {
            a.set(0, d.field(), self.get(d));
        }
    }
}

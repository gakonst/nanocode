//! The dependency descriptor (D2265, D2266): every component changes when its input does,
//! images carry it, and loading refuses each differing component by name. The gate runs
//! these: a table change that left its hash alone would let an image restore into a build
//! that reads it differently.

extern crate std;

use std::boxed::Box;
use std::string::{String, ToString};
use std::vec::Vec;

use super::compat::{COMPAT_VERSION, IMAGE_DEPS, ImageDep, ImageDeps, RECORDS_HASH};
use super::{Vm, VmOptions};
use crate::heap::LoadError;

mod inputs {
    extern crate std;
    use std::string::{String, ToString};
    use std::vec::Vec;
    include!("../../compat_inputs.rs");
}

#[test]
fn the_descriptor_is_the_tables_hashes() {
    assert_eq!(
        IMAGE_DEPS,
        ImageDeps {
            layout: crate::heap::cells::LAYOUT_HASH,
            features: super::FEATURES,
            compat: COMPAT_VERSION,
            ops: crate::bytecode::OPS_HASH,
            intrinsics: crate::bytecode::intrinsics::INTRINSICS_HASH,
            records: RECORDS_HASH,
        }
    );
    // The source hash is not in it: editing code that images do not depend on keeps
    // every image loadable. (BUILD_HASH mixes the same tables with it.)
    let h = IMAGE_DEPS;
    for d in ImageDep::ALL {
        assert_ne!(h.get(d), super::SOURCE_HASH, "{d:?}");
    }
    assert_ne!(h.hash(), super::BUILD_HASH);
}

#[test]
fn vms_and_their_images_carry_the_descriptor() {
    let mut vm = Vm::new(VmOptions::default()).unwrap();
    assert_eq!(ImageDeps::of(vm.bytes()), Some(IMAGE_DEPS));
    vm.collect().unwrap();
    assert_eq!(ImageDeps::of(vm.image()), Some(IMAGE_DEPS));
    assert_eq!(ImageDeps::of(b"not an image"), None);
    let bare = crate::heap::Heap::new().unwrap();
    let d = ImageDeps::of(bare.bytes()).unwrap();
    assert_eq!((d.layout, d.compat, d.ops), (IMAGE_DEPS.layout, 0, 0));
}

#[test]
fn loading_names_the_component_that_differs() {
    let mut vm = Vm::new(VmOptions::default()).unwrap();
    vm.collect().unwrap();
    let image = vm.image().to_vec();
    for dep in ImageDep::ALL {
        let mut other = image.clone();
        let at = dep.field().offset as usize;
        other[at] ^= 1;
        let expected = IMAGE_DEPS.get(dep);
        let found = expected ^ 1;
        let err = Vm::from_bytes(other.clone(), VmOptions::default()).err();
        assert_eq!(
            err,
            Some(LoadError::Incompatible {
                dep,
                found,
                expected
            }),
            "{dep:?}"
        );
        let msg = err.unwrap().text();
        assert!(
            msg.contains(&(String::from("another ") + dep.name())),
            "{msg}"
        );
        assert_eq!(
            ImageDeps::of(&other).unwrap().first_difference(&IMAGE_DEPS),
            Some((dep, found, expected))
        );
    }
    let mut other = image.clone();
    let at = ImageDep::Compat.field().offset as usize;
    other[at..at + 4].copy_from_slice(&(COMPAT_VERSION + 1).to_le_bytes());
    assert_eq!(
        Vm::from_bytes(other, VmOptions::default())
            .err()
            .unwrap()
            .text(),
        String::from("the image is from a djs with another compatibility version (image: ")
            + &(COMPAT_VERSION + 1).to_string()
            + "; this build: "
            + &COMPAT_VERSION.to_string()
            + ")"
    );
    // The same image loads, and so does any image with this descriptor.
    assert!(Vm::from_bytes(image, VmOptions::default()).is_ok());
}

#[test]
fn the_opcode_hash_sees_every_op_change() {
    use crate::bytecode::ops::hash_ops;
    use crate::bytecode::{OP_NAMES, OPS, OPS_HASH};
    assert_eq!(hash_ops(OPS, &OP_NAMES), OPS_HASH);
    let mut ops = OPS.to_vec();
    ops[3].pop += 1;
    assert_ne!(hash_ops(&ops, &OP_NAMES), OPS_HASH, "a stack effect");
    let mut ops = OPS.to_vec();
    ops[7].imms = ops[8].imms;
    if ops[7].imms != OPS[7].imms {
        assert_ne!(hash_ops(&ops, &OP_NAMES), OPS_HASH, "an immediate");
    }
    let mut ops = OPS.to_vec();
    ops.swap(10, 11);
    assert_ne!(hash_ops(&ops, &OP_NAMES), OPS_HASH, "the numbering");
    let mut ops = OPS.to_vec();
    ops.pop();
    assert_ne!(hash_ops(&ops, &OP_NAMES), OPS_HASH, "a removed op");
    let mut names = OP_NAMES;
    names[0] = "Renamed";
    assert_ne!(hash_ops(OPS, &names), OPS_HASH, "a name");
}

#[test]
fn the_intrinsic_hash_sees_every_entry_change() {
    use crate::bytecode::intrinsics::{
        INTRINSICS, INTRINSICS_HASH, IntrinsicKind, hash_intrinsics,
    };
    assert_eq!(hash_intrinsics(INTRINSICS), INTRINSICS_HASH);
    let mut t = INTRINSICS.to_vec();
    t.swap(0, 1);
    assert_ne!(hash_intrinsics(&t), INTRINSICS_HASH, "the numbering");
    let mut t = INTRINSICS.to_vec();
    t[5].params += 1;
    assert_ne!(hash_intrinsics(&t), INTRINSICS_HASH, "an arity");
    let mut t = INTRINSICS.to_vec();
    t[5].kind = match t[5].kind {
        IntrinsicKind::Leaf => IntrinsicKind::Reentrant,
        _ => IntrinsicKind::Leaf,
    };
    assert_ne!(hash_intrinsics(&t), INTRINSICS_HASH, "a kind");
    let mut t = INTRINSICS.to_vec();
    t.push(t[0]);
    assert_ne!(hash_intrinsics(&t), INTRINSICS_HASH, "an added entry");
}

#[test]
fn the_layout_hash_sees_every_field_change() {
    use crate::heap::cells::{LAYOUT_HASH, LAYOUTS};
    use crate::heap::layout::hash_layouts;
    let atoms = crate::heap::atom::WELL_KNOWN_NAMES_FOR_HASH;
    assert_eq!(hash_layouts(LAYOUTS, atoms), LAYOUT_HASH);
    let roots = LAYOUTS.iter().position(|l| l.name == "Roots").unwrap();
    // A moved field.
    let mut fields = LAYOUTS[roots].fields.to_vec();
    fields[2].offset += 4;
    let mut layouts = LAYOUTS.to_vec();
    layouts[roots].fields = Box::leak(fields.into_boxed_slice());
    assert_ne!(hash_layouts(&layouts, atoms), LAYOUT_HASH, "a field offset");
    // A changed kind id, cell size or well-known atom.
    let mut layouts = LAYOUTS.to_vec();
    layouts[roots].kind += 100;
    assert_ne!(hash_layouts(&layouts, atoms), LAYOUT_HASH, "a kind id");
    let mut layouts = LAYOUTS.to_vec();
    layouts[roots].min_size += 8;
    assert_ne!(hash_layouts(&layouts, atoms), LAYOUT_HASH, "a size");
    let mut names = atoms.to_vec();
    names.swap(0, 1);
    assert_ne!(
        hash_layouts(LAYOUTS, &names),
        LAYOUT_HASH,
        "the atom numbering"
    );
}

fn crate_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn source(path: &str) -> String {
    std::fs::read_to_string(crate_root().join(path)).unwrap()
}

#[test]
fn the_records_hash_is_the_regions_of_these_sources() {
    assert_eq!(
        inputs::records_hash(&crate_root()),
        Ok(RECORDS_HASH),
        "build.rs hashed other sources: rebuild"
    );
    // Every region is there, and holds what it should.
    for &(path, cut) in inputs::REGIONS {
        let files = if path.ends_with('/') {
            inputs::rust_files(&crate_root(), path)
        } else {
            Vec::from([String::from(path)])
        };
        assert!(!files.is_empty(), "{path}");
        for f in files {
            inputs::cut(&source(&f), cut).unwrap_or_else(|e| panic!("{f}: {e}"));
        }
    }
    let classes = inputs::cut(
        &source("src/heap/object.rs"),
        inputs::Cut::Item("pub enum ObjectClass"),
    )
    .unwrap();
    assert!(
        classes.starts_with("pubenumObjectClass{Ordinary=0,"),
        "{classes}"
    );
    assert!(classes.ends_with('}'));
    let fast = inputs::cut(
        &source("src/builtins/fast.rs"),
        inputs::Cut::Item("pub(crate) static FAST"),
    )
    .unwrap();
    assert!(
        fast.contains("f(\"ToString\",to_string)") && fast.ends_with("];"),
        "{fast}"
    );
    let consts = inputs::cut(&source("src/interp/frame.rs"), inputs::Cut::TopConsts).unwrap();
    assert!(consts.contains("pubconstHEADER:u32=6;"), "{consts}");
}

#[test]
fn the_records_hash_sees_numbers_not_comments() {
    use inputs::{Cut, cut};
    let src = "\
/// The classes.
#[repr(u32)]
pub enum ObjectClass {
    Ordinary = 0,
    /// An array.
    Array = 1, // dense
}

impl ObjectClass {
    fn f() -> u32 { 1 }
}

/// Slots.
pub(crate) const STATE: u32 = 0;
const TABLE: [u8; 2] = [
    1, 2,
];
pub const fn helper() -> u32 { 3 }
const _: () = assert!(true);
const GET: u16 = id(\"Get\");
pub(super) const CALL: u16 =
    super::intrinsics::id(\"Call\");
const NOT_AN_ID: u16 = id(\"Get\") + 1;
fn body() { const INNER: u32 = 9; let s = \"// } not a comment\"; let c = '}'; }
";
    let enum_of = |s: &str| cut(s, Cut::Item("pub enum ObjectClass")).unwrap();
    let consts_of = |s: &str| cut(s, Cut::TopConsts).unwrap();
    let e = enum_of(src);
    assert_eq!(e, "pubenumObjectClass{Ordinary=0,Array=1,}");
    // Intrinsic ids by name are the intrinsic table's (its hash covers them).
    assert_eq!(
        consts_of(src),
        "pub(crate)constSTATE:u32=0;constTABLE:[u8;2]=[1,2,];constNOT_AN_ID:u16=id(\"Get\")+1;"
    );
    // A renumbered entry, an added one, a changed slot: other text.
    assert_ne!(enum_of(&src.replace("Array = 1", "Array = 2")), e);
    assert_ne!(
        enum_of(&src.replace("Array = 1,", "Array = 1, Map = 2,")),
        e
    );
    assert_ne!(
        consts_of(&src.replace("STATE: u32 = 0", "STATE: u32 = 1")),
        consts_of(src)
    );
    assert_ne!(
        consts_of(&src.replace("fn body", "const MORE: u32 = 1;\nfn body")),
        consts_of(src)
    );
    // Comments, layout and code outside the regions: the same text.
    let edited = src
        .replace("/// An array.", "/// The Array exotic object.")
        .replace("// dense", "")
        .replace("    Ordinary = 0,\n", "    Ordinary\n        = 0,\n")
        .replace("{ 1 }", "{ 2 }")
        .replace("INNER: u32 = 9", "INNER: u32 = 10");
    assert_eq!(enum_of(&edited), e);
    assert_eq!(consts_of(&edited), consts_of(src));
    // A strings or char with comment or brace characters does not cut the item short.
    let tricky = "pub(crate) static FAST: &[F] = &[\n    f(\"a/*b\", x),\n    f(\"}\", y), // end\n    g('{'),\n];\nfn after() {}\n";
    assert_eq!(
        cut(tricky, Cut::Item("pub(crate) static FAST")).unwrap(),
        "pub(crate)staticFAST:&[F]=&[f(\"a/*b\",x),f(\"}\",y),g('{'),];"
    );
    // A region that is gone fails the build instead of hashing nothing.
    assert!(cut(src, Cut::Item("pub enum Missing")).is_err());
    assert!(
        cut(
            &(String::from(src) + src),
            Cut::Item("pub enum ObjectClass")
        )
        .is_err()
    );
}

#[test]
fn features_are_classified_and_named() {
    // The enabled features, as the source hash lists them.
    let enabled: Vec<String> = super::BUILD_INPUTS
        .iter()
        .filter_map(|i| i.strip_prefix("feature "))
        .map(String::from)
        .collect();
    assert_eq!(inputs::feature_mask(&enabled), Ok(super::FEATURES));
    assert_eq!(super::compat::FEATURE_NAMES, inputs::IMAGE_FEATURES);
    let all = (1 << inputs::IMAGE_FEATURES.len()) - 1;
    assert_eq!(
        ImageDep::Features.show(all),
        inputs::IMAGE_FEATURES.join(", ")
    );
    assert_eq!(ImageDep::Features.show(0), "none");
    // Neutral features change nothing; an unclassified one is an error (the build fails).
    let mut more = enabled.clone();
    more.push(String::from("unsafe-heap"));
    more.push(String::from("default"));
    assert_eq!(inputs::feature_mask(&more), Ok(super::FEATURES));
    more.push(String::from("something-new"));
    assert!(
        inputs::feature_mask(&more)
            .unwrap_err()
            .contains("something-new")
    );
    // A slim image (no `full`) in a full build, and the other way: refused by name.
    let mut vm = Vm::new(VmOptions::default()).unwrap();
    vm.collect().unwrap();
    let mut other = vm.image().to_vec();
    let at = ImageDep::Features.field().offset as usize;
    other[at] ^= 1; // bit 0: `full`
    let msg = Vm::from_bytes(other, VmOptions::default())
        .err()
        .unwrap()
        .text();
    assert!(msg.contains("another feature set (image: "), "{msg}");
    assert!(msg.contains("full"), "{msg}");
}

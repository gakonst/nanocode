//! The compile cache (0219, D3040): a compile's cells as a blob that another VM of the same
//! build imports instead of compiling the same text again.
//!
//! A compile only appends cells to the arena (allocation never collects) and interns atoms.
//! While a compile is recorded, the heap logs every interning with the arena length before
//! and after it (`Heap::intern_log`). The blob is that history: the cells the compile
//! appended, in order, and the internings between them. An import replays it: it copies
//! each cell (references inside the blob rebased) and interns the same texts at the same
//! points, then renumbers the atom ids the templates hold if the importing VM gave them
//! other ids. The internings happen in the order the compile made them, so the importing
//! VM ends up as if it had compiled the text itself: from the same state, byte for byte.

use alloc::vec::Vec;

use super::Vm;
use crate::bytecode::imm::ImmKind;
use crate::bytecode::{Op, insns, template};
use crate::compiler::{CompileError, CompileErrorKind, CompileOptions};
use crate::heap::cells::{FuncInfo, FuncTemplate, Str8, Str16, U32Array};
use crate::heap::walk::{Slot, for_each_slot};
use crate::heap::{Atom, CellKind, Heap, HeapError, Ref};
use crate::value::{Str, Value};

/// What an interning interned (the log, `Heap::intern_log`).
#[derive(Clone, Copy, Debug)]
pub(crate) enum Source {
    /// A text (`atom_str`, `atom_utf16`).
    Text,
    /// The string cell at this offset (`intern`).
    Str(u32),
}

/// One interning while a compile is recorded: the atom, and the arena's length before and
/// after it (the cells between are the interning's own: the atom's string, table growth).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Intern {
    pub atom: Atom,
    pub before: u32,
    pub after: u32,
    pub source: Source,
}

const MAGIC: u32 = u32::from_le_bytes(*b"DJCC");
const VERSION: u32 = 1;
const CELL: u8 = 0;
const TEXT: u8 = 1;
const STR: u8 = 2;

impl Vm {
    /// [`Vm::compile`] through the compile cache (D3040): with `blob` from an earlier
    /// compile of the same text and options in a VM of this build, imports its cells
    /// instead of compiling. Returns the function and, when it compiled, the blob of this
    /// compile for the host to keep (`None` for a plain dump, which is not cached). A blob
    /// that does not fit (another text, options or build) is compiled over.
    pub fn compile_cached(
        &mut self,
        src: &[u8],
        opts: &CompileOptions<'_>,
        blob: Option<&[u8]>,
    ) -> Result<(Value, Option<Vec<u8>>), CompileError> {
        if src.starts_with(b"\"djs-dump") && opts.params.is_empty() {
            return self.compile(src, opts).map(|f| (f, None));
        }
        let key = self.cache_key(src, opts);
        if let Some(blob) = blob {
            match import(&mut self.heap, blob, key) {
                Ok(Some(t)) => return self.instantiate(t).map(|f| (f, None)).map_err(heap_error),
                Ok(None) => {}
                Err(e) => return Err(heap_error(e)),
            }
        }
        let top0 = self.heap.arena().len();
        self.heap.intern_log = Some(Vec::new());
        let t = self.compile_unit(src, opts);
        let log = self.heap.intern_log.take().unwrap_or_default();
        let t = t?;
        let blob = export(&self.heap, t, top0, &log, key);
        let f = self.instantiate(t).map_err(heap_error)?;
        Ok((f, blob))
    }

    /// What a blob is for: the text, the options, the file name and parameters, lazy or
    /// not (FNV-1a 64).
    fn cache_key(&self, src: &[u8], opts: &CompileOptions<'_>) -> u64 {
        #[cfg(feature = "lazy")]
        let lazy = self.opts.lazy_functions;
        #[cfg(not(feature = "lazy"))]
        let lazy = false;
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut eat = |bytes: &[u8]| {
            for &b in bytes {
                h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
            }
            h = (h ^ 0xff).wrapping_mul(0x0100_0000_01b3);
        };
        eat(&opts.bits().to_le_bytes());
        eat(&[u8::from(lazy)]);
        eat(opts.filename.as_bytes());
        for p in opts.params {
            eat(p.as_bytes());
        }
        eat(src);
        h
    }
}

fn heap_error(e: HeapError) -> CompileError {
    CompileError {
        kind: if e == HeapError::HostOutOfMemory {
            CompileErrorKind::HostOutOfMemory
        } else {
            CompileErrorKind::OutOfMemory
        },
        message: alloc::string::String::from(e.as_str()),
        pos: 0,
        line: 0,
        col: 0,
    }
}

fn put32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put64(out: &mut Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_le_bytes());
}

/// The units of atom `a`'s text.
fn atom_units(heap: &Heap, a: Atom) -> Vec<u16> {
    heap.atom_name(a).encode_utf16().collect()
}

/// The blob of a compile that appended the cells from `top0` on, its top-level template
/// `t`; `None` if a cell refers to one the compile did not make (then nothing is cached).
fn export(heap: &Heap, t: Ref, top0: u32, log: &[Intern], key: u64) -> Option<Vec<u8>> {
    let a = heap.arena();
    let top1 = a.len();
    // The first interning of each atom (the later ones change nothing).
    let mut order: Vec<(u32, usize)> = log
        .iter()
        .enumerate()
        .map(|(k, i)| (i.atom.id(), k))
        .collect();
    crate::sort::sort_by(&mut order, |a, b| a < b);
    let mut first: Vec<usize> = order
        .iter()
        .enumerate()
        .filter(|&(j, e)| j == 0 || order[j - 1].0 != e.0)
        .map(|(_, e)| e.1)
        .collect();
    crate::sort::sort_by(&mut first, |a, b| a < b);
    let interns: Vec<&Intern> = first.iter().map(|&k| &log[k]).collect();
    // The internings' own cells (the windows only grow: the log is in arena order).
    let windows: Vec<(u32, u32)> = log
        .iter()
        .filter(|i| i.after > i.before)
        .map(|i| (i.before, i.after))
        .collect();
    let owned = |off: u32| {
        let k = windows.partition_point(|w| w.1 <= off);
        windows.get(k).is_some_and(|w| w.0 <= off)
    };
    // The atom strings among the internings' own cells: an import has its own.
    let mut atom_strings: Vec<u32> = interns
        .iter()
        .filter_map(|i| heap.atom_string_of(i.atom).map(|s| s.0))
        .filter(|&s| s >= top0)
        .collect();
    crate::sort::sort_by(&mut atom_strings, |a, b| a < b);
    let mut out = Vec::new();
    put32(&mut out, MAGIC);
    put32(&mut out, VERSION);
    put32(&mut out, super::BUILD_HASH);
    put64(&mut out, key);
    put64(&mut out, heap.hash_key());
    put32(&mut out, top0);
    put32(&mut out, t.0);
    let count_at = out.len();
    put32(&mut out, 0);
    let mut events = 0u32;
    let mut next = interns.iter().peekable();
    let emit_intern = |out: &mut Vec<u8>, i: &Intern| {
        let units = atom_units(heap, i.atom);
        // The atom's string, where the compile made it (rebased like a cell).
        let own = heap.atom_string_of(i.atom).map_or(0, |s| s.0);
        match i.source {
            Source::Text => out.push(TEXT),
            Source::Str(s) => {
                out.push(STR);
                put32(out, s);
            }
        }
        put32(out, i.atom.id());
        put32(out, if own >= top0 { own } else { 0 });
        put32(out, units.len() as u32);
        for u in units {
            out.extend_from_slice(&u.to_le_bytes());
        }
    };
    let cells: Vec<_> = heap
        .cells_from(top0)
        .take_while(|c| c.offset < top1)
        .collect();
    for c in cells {
        while let Some(i) = next.next_if(|i| i.before <= c.offset) {
            emit_intern(&mut out, i);
            events += 1;
        }
        if owned(c.offset) {
            continue;
        }
        // Only references to cells of the compile (or to atom strings it interned).
        let mut outside = false;
        for_each_slot(c.kind, c.offset, c.size, |slot| {
            let r = match slot {
                Slot::Value(s) => a.read::<Value>(s).ref_offset(),
                Slot::Ref(s) => a.read::<Ref>(s).get(),
            };
            if r.is_some_and(|r| r < top0 || (owned(r) && atom_strings.binary_search(&r).is_err()))
            {
                outside = true;
            }
        });
        if outside {
            return None;
        }
        out.push(CELL);
        put32(&mut out, c.offset);
        put32(&mut out, c.size);
        out.extend_from_slice(a.slice(c.offset, c.size));
        events += 1;
    }
    for i in next {
        emit_intern(&mut out, i);
        events += 1;
    }
    out[count_at..count_at + 4].copy_from_slice(&events.to_le_bytes());
    Some(out)
}

/// Reads a blob.
struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn bytes(&mut self, n: usize) -> Option<&[u8]> {
        let s = self.b.get(self.at..self.at + n)?;
        self.at += n;
        Some(s)
    }

    fn u8(&mut self) -> Option<u8> {
        self.bytes(1).map(|b| b[0])
    }

    fn u32(&mut self) -> Option<u32> {
        self.bytes(4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
    }

    fn u64(&mut self) -> Option<u64> {
        self.bytes(8)
            .map(|b| u64::from_le_bytes(b.try_into().unwrap()))
    }

    fn units(&mut self) -> Option<Vec<u16>> {
        let n = self.u32()? as usize;
        let b = self.bytes(2 * n)?;
        Some(
            b.as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_le_bytes(*c))
                .collect(),
        )
    }
}

/// Old to new offsets of the imported cells, sorted by old.
struct Moves(Vec<(u32, u32)>);

impl Moves {
    fn get(&self, old: u32) -> Option<u32> {
        let i = self.0.partition_point(|m| m.0 < old);
        self.0.get(i).filter(|m| m.0 == old).map(|m| m.1)
    }
}

/// Imports `blob` (for compile key `key`): the new top-level template, or `None` if the
/// blob is not one for this build and key.
fn import(heap: &mut Heap, blob: &[u8], key: u64) -> Result<Option<Ref>, HeapError> {
    let mut r = Reader { b: blob, at: 0 };
    let head = (|| {
        let ok = r.u32()? == MAGIC && r.u32()? == VERSION && r.u32()? == super::BUILD_HASH;
        let k = r.u64()?;
        let hash_key = r.u64()?;
        let top0 = r.u32()?;
        let root = r.u32()?;
        let n = r.u32()?;
        (ok && k == key).then_some((hash_key, top0, root, n))
    })();
    let Some((hash_key, top0, root, n)) = head else {
        return Ok(None);
    };
    // A blob is checked before anything is allocated: a bad one changes nothing.
    let start = r.at;
    for _ in 0..n {
        let ok = match r.u8() {
            Some(CELL) => r
                .u32()
                .and(r.u32())
                .and_then(|size| r.bytes(size as usize))
                .is_some(),
            Some(TEXT) => r.u32().and(r.u32()).and(r.units()).is_some(),
            Some(STR) => r.u32().and(r.u32()).and(r.u32()).and(r.units()).is_some(),
            _ => false,
        };
        if !ok {
            return Ok(None);
        }
    }
    r.at = start;
    let mut moves: Vec<(u32, u32)> = Vec::new();
    let mut atoms: Vec<(u32, u32)> = Vec::new();
    let mut copied: Vec<(u32, CellKind, u32)> = Vec::new();
    let mut strings: Vec<(u32, Atom)> = Vec::new();
    for _ in 0..n {
        match r.u8() {
            Some(CELL) => {
                let (old, size) = (r.u32().unwrap_or(0), r.u32().unwrap_or(0));
                let bytes = r.bytes(size as usize).unwrap_or_default();
                let kind = CellKind::from_u8(bytes[0]).ok_or(HeapError::OutOfMemory)?;
                let new = heap.alloc_cell_init(kind, size, 0, bytes)?;
                moves.push((old, new));
                copied.push((new, kind, size));
            }
            Some(tag) => {
                let s = if tag == STR { r.u32().unwrap_or(0) } else { 0 };
                let old_atom = r.u32().unwrap_or(0);
                let own = r.u32().unwrap_or(0);
                let units = r.units().unwrap_or_default();
                let moved = (tag == STR && s >= top0)
                    .then(|| moves.iter().rev().find(|m| m.0 == s).map(|m| m.1))
                    .flatten();
                let a = match moved {
                    Some(new) => heap.intern(Str(new))?,
                    None => heap.atom_utf16(&units)?,
                };
                atoms.push((old_atom, a.id()));
                if own != 0 {
                    strings.push((own, a));
                }
            }
            None => return Ok(None),
        }
    }
    // References to the atom strings the compile made: the importing VM's own.
    for (own, a) in strings {
        if let Some(s) = heap.atom_string_of(a) {
            moves.push((own, s.0));
        }
    }
    crate::sort::sort_by(&mut moves, |a, b| a.0 < b.0);
    let moves = Moves(moves);
    // Rebase the references, and forget string hashes made under another key.
    let rekey = hash_key != heap.hash_key();
    for &(c, kind, size) in &copied {
        let mut fix: Vec<(u32, Option<Value>, Option<Ref>)> = Vec::new();
        let a = heap.arena();
        for_each_slot(kind, c, size, |slot| match slot {
            Slot::Value(s) => {
                let v = a.read::<Value>(s);
                if let Some(old) = v.ref_offset()
                    && let Some(new) = moves.get(old)
                {
                    let bits = (v.to_bits() & !0xFFFF_FFFF) | u64::from(new);
                    fix.push((s, Some(Value::from_raw(bits)), None));
                }
            }
            Slot::Ref(s) => {
                if let Some(old) = a.read::<Ref>(s).get()
                    && let Some(new) = moves.get(old)
                {
                    fix.push((s, None, Some(Ref(new))));
                }
            }
        });
        let a = heap.arena_mut();
        for (s, v, r) in fix {
            match (v, r) {
                (Some(v), _) => a.write(s, v),
                (_, Some(r)) => a.write(s, r),
                _ => {}
            }
        }
        if rekey {
            match kind {
                CellKind::Str8 => a.set(c, Str8::HASH, 0),
                CellKind::Str16 => a.set(c, Str16::HASH, 0),
                _ => {}
            }
        }
    }
    // Atom ids the importing VM numbered otherwise.
    if atoms.iter().any(|&(o, n)| o != n) {
        crate::sort::sort_by(&mut atoms, |a, b| a.0 < b.0);
        let moved = |id: u32| {
            let i = atoms.partition_point(|m| m.0 < id);
            atoms.get(i).filter(|m| m.0 == id).map_or(id, |m| m.1)
        };
        for &(c, kind, _) in &copied {
            if kind == CellKind::FuncTemplate {
                renumber(heap, c, &moved);
            }
        }
    }
    Ok(moves.get(root).map(Ref))
}

/// Renumbers the atom ids template `t` holds by `moved`: its name, its local and capture
/// names, its code's atom immediates and the global declaration list in its constants.
fn renumber(heap: &mut Heap, t: u32, moved: &impl Fn(u32) -> u32) {
    let name = heap.arena().get(t, FuncTemplate::NAME);
    heap.arena_mut().set(t, FuncTemplate::NAME, moved(name));
    if let Some(info) = heap.arena().get::<Ref>(t, FuncTemplate::INFO).get()
        && let Some(n) = heap.arena().get::<Ref>(info, FuncInfo::NAMES).get()
    {
        for i in 0..heap.arena().get(n, U32Array::LEN) {
            let at = U32Array::DATA.at(n, i);
            let id = heap.arena().read::<u32>(at);
            heap.arena_mut().write(at, moved(id));
        }
    }
    let mut patches = Vec::new();
    let mut decls = None;
    for insn in insns(template::code(heap, t)).flatten() {
        let mut at = insn.pc + 1;
        for (k, kind) in insn.info().imms.iter().enumerate() {
            if *kind == ImmKind::Atom {
                patches.push((at, moved(insn.imm(k))));
            }
            at += u32::from(kind.width());
        }
        if insn.op == Op::GlobalDecls {
            decls = Some(insn.imm(0));
        }
    }
    for (at, id) in patches {
        heap.arena_mut().write(FuncTemplate::DATA.at(t, at), id);
    }
    // `n_var, n_func, n_let, n_const`, then the atom ids (ops.rs `global_decls`).
    if let (Some(idx), Some(p)) = (
        decls,
        heap.arena().get::<Ref>(t, FuncTemplate::CONSTS).get(),
    ) {
        let count =
            |heap: &Heap, k: u32| heap.value_array_get(p, idx + k).as_int().unwrap_or(0) as u32;
        let n: u32 = (0..4).map(|k| count(heap, k)).sum();
        for i in idx + 4..idx + 4 + n {
            let id = heap.value_array_get(p, i).as_int().unwrap_or(0) as u32;
            heap.value_array_set(p, i, Value::int(moved(id) as i32));
        }
    }
}

#[cfg(test)]
mod tests;

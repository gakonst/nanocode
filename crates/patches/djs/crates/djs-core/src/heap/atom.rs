//! Atoms (interned property keys) and symbols.
//!
//! An [`Atom`] is a `u32` id. Every property key is an atom:
//!
//! ```text
//! bit 31 = 1             integer index atom; bits 0-30 are the index (0 ..= 2^31-1)
//! bit 31 = 0, bit 30 = 1 symbol atom; bits 0-29 index the atom table
//! bit 31 = 0, bit 30 = 0 string atom; bits 0-29 index the atom table (0 = null atom)
//! ```
//!
//! Integer index atoms are computed, never interned, so element access does not touch the
//! atom table. A string that is a canonical array index below 2^31 always becomes an
//! integer atom; larger array indexes (up to 2^32-2) are string atoms, which
//! [`Heap::atom_array_index`] still recognizes.
//!
//! The atom table (a heap cell) maps a table index to its string or symbol, and has an
//! open-addressing hash index for interning strings. Atom ids are stable across GC (they
//! are indexes, not offsets), so bytecode and property tables can hold them without
//! being rewritten. Every symbol gets a table entry too, which makes symbols ordinary
//! property keys. For now atoms are never freed; atom GC is a follow-up.
//!
//! The well-known atoms (and the well-known symbols) have fixed ids, generated from the
//! list below, and are interned in order when the heap is created.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use super::cells::{AtomTable, Roots, Symbol, ValueArray};
use super::layout::Ref;
use super::string::{BASE_HASH_KEY, StrView, hash_latin1_with, hash_units_with};
use super::{CellKind, Heap, HeapError};
use crate::value::{Str, Sym, Value};

pub(crate) const INT_BIT: u32 = 1 << 31;
pub(crate) const SYM_BIT: u32 = 1 << 30;
pub(crate) const INDEX_MASK: u32 = SYM_BIT - 1;

/// The largest integer index atom.
pub const MAX_INT_ATOM: u32 = INT_BIT - 1;

/// The largest array index (2^32 - 2).
pub const MAX_ARRAY_INDEX: u32 = u32::MAX - 1;

const ATOM_MEMO_BITS: u32 = 8;
pub(crate) const ATOM_MEMO_LEN: usize = 1 << ATOM_MEMO_BITS;
const FIB_HASH: u32 = 0x9E37_79B1;

#[inline]
fn atom_memo_slot(s: Str) -> usize {
    (s.0.wrapping_mul(FIB_HASH) >> (u32::BITS - ATOM_MEMO_BITS)) as usize
}

/// An interned property key: an integer index, a string or a symbol. See the module docs.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Atom(pub(crate) u32);

impl Atom {
    /// The null atom: never a valid key (marks deleted property entries).
    pub const NULL: Atom = Atom(0);

    /// The integer atom for `i`, if `i <= MAX_INT_ATOM`. For any `u32` index use
    /// [`Heap::atom_from_index`].
    #[inline]
    pub const fn int(i: u32) -> Option<Atom> {
        if i <= MAX_INT_ATOM {
            Some(Atom(INT_BIT | i))
        } else {
            None
        }
    }

    /// The raw id.
    #[inline]
    pub const fn id(self) -> u32 {
        self.0
    }

    /// Rebuilds an atom from a raw id (for example a bytecode operand).
    #[inline]
    pub const fn from_id(id: u32) -> Atom {
        Atom(id)
    }

    #[inline]
    pub const fn is_null(self) -> bool {
        self.0 == 0
    }

    #[inline]
    pub const fn is_int(self) -> bool {
        self.0 & INT_BIT != 0
    }

    #[inline]
    pub const fn is_symbol(self) -> bool {
        self.0 & (INT_BIT | SYM_BIT) == SYM_BIT
    }

    /// A string key (integer atoms are string keys in JS too, but this is false for them).
    #[inline]
    pub const fn is_string(self) -> bool {
        self.0 & (INT_BIT | SYM_BIT) == 0 && self.0 != 0
    }

    /// The index of an integer atom.
    #[inline]
    pub const fn as_int(self) -> Option<u32> {
        if self.is_int() {
            Some(self.0 & !INT_BIT)
        } else {
            None
        }
    }

    #[inline]
    const fn table_index(self) -> u32 {
        self.0 & INDEX_MASK
    }
}

impl fmt::Debug for Atom {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(i) = self.as_int() {
            write!(f, "Atom(int {i})")
        } else if self.is_symbol() {
            write!(f, "Atom(sym #{})", self.table_index())
        } else {
            write!(f, "Atom(str #{})", self.table_index())
        }
    }
}

macro_rules! well_known {
    (
        strings { $( $name:ident = $s:literal, )* }
        symbols { $( $sname:ident = $sd:literal, )* }
    ) => {
        // Discriminants give the table indexes: strings first, then symbols.
        #[allow(non_camel_case_types, clippy::upper_case_acronyms, dead_code)]
        #[repr(u32)]
        enum WellKnown {
            Null,
            $( $name, )*
            $( $sname, )*
        }

        /// The well-known atoms, with fixed ids.
        pub mod atoms {
            use super::{Atom, SYM_BIT, WellKnown};
            $( #[doc = concat!("`\"", $s, "\"`")]
               pub const $name: Atom = Atom(WellKnown::$name as u32); )*
            $( #[doc = concat!("The well-known symbol `", $sd, "`.")]
               pub const $sname: Atom = Atom(SYM_BIT | WellKnown::$sname as u32); )*
        }

        const WELL_KNOWN_STRINGS: &[&str] = &[ $( $s, )* ];
        const WELL_KNOWN_SYMBOLS: &[&str] = &[ $( $sd, )* ];
        pub(crate) const WELL_KNOWN_NAMES_FOR_HASH: &[&str] = &[ $( $s, )* "|", $( $sd, )* ];
    };
}

well_known! {
    strings {
        EMPTY = "",
        LENGTH = "length",
        PROTOTYPE = "prototype",
        CONSTRUCTOR = "constructor",
        TO_STRING = "toString",
        TO_LOCALE_STRING = "toLocaleString",
        VALUE_OF = "valueOf",
        PROTO = "__proto__",
        NAME = "name",
        MESSAGE = "message",
        STACK = "stack",
        CAUSE = "cause",
        ERRORS = "errors",
        VALUE = "value",
        STATUS = "status",
        REASON = "reason",
        FULFILLED = "fulfilled",
        REJECTED = "rejected",
        GET = "get",
        SET = "set",
        WRITABLE = "writable",
        ENUMERABLE = "enumerable",
        CONFIGURABLE = "configurable",
        UNDEFINED = "undefined",
        NULL = "null",
        TRUE = "true",
        FALSE = "false",
        NUMBER = "number",
        STRING = "string",
        BOOLEAN = "boolean",
        OBJECT = "object",
        FUNCTION = "function",
        SYMBOL = "symbol",
        BIGINT = "bigint",
        ARGUMENTS = "arguments",
        CALLEE = "callee",
        CALLER = "caller",
        DEFAULT = "default",
        DONE = "done",
        NEXT = "next",
        RETURN = "return",
        THROW = "throw",
        THEN = "then",
        CATCH = "catch",
        FINALLY = "finally",
        RESOLVE = "resolve",
        REJECT = "reject",
        TO_JSON = "toJSON",
        JOIN = "join",
        INDEX = "index",
        INPUT = "input",
        GROUPS = "groups",
        LAST_INDEX = "lastIndex",
        SOURCE = "source",
        FLAGS = "flags",
        RAW = "raw",
        KEYS = "keys",
        VALUES = "values",
        ENTRIES = "entries",
        SIZE = "size",
        NAN = "NaN",
        INFINITY = "Infinity",
        NEG_INFINITY = "-Infinity",
        GLOBAL_THIS = "globalThis",
        ERROR = "Error",
        TYPE_ERROR = "TypeError",
        RANGE_ERROR = "RangeError",
        SYNTAX_ERROR = "SyntaxError",
        REFERENCE_ERROR = "ReferenceError",
        // RegExp protocol names, looked up on every split/replace (0056).
        EXEC = "exec",
        GLOBAL = "global",
        IGNORE_CASE = "ignoreCase",
        MULTILINE = "multiline",
        DOT_ALL = "dotAll",
        UNICODE = "unicode",
        UNICODE_SETS = "unicodeSets",
        STICKY = "sticky",
        HAS_INDICES = "hasIndices",
        REG_EXP = "RegExp",
        RAW_JSON = "rawJSON",
        PREPARE_STACK_TRACE = "prepareStackTrace",
        STACK_TRACE_LIMIT = "stackTraceLimit",
    }
    symbols {
        SYM_ITERATOR = "Symbol.iterator",
        SYM_ASYNC_ITERATOR = "Symbol.asyncIterator",
        SYM_HAS_INSTANCE = "Symbol.hasInstance",
        SYM_TO_PRIMITIVE = "Symbol.toPrimitive",
        SYM_TO_STRING_TAG = "Symbol.toStringTag",
        SYM_SPECIES = "Symbol.species",
        SYM_IS_CONCAT_SPREADABLE = "Symbol.isConcatSpreadable",
        SYM_UNSCOPABLES = "Symbol.unscopables",
        SYM_MATCH = "Symbol.match",
        SYM_MATCH_ALL = "Symbol.matchAll",
        SYM_REPLACE = "Symbol.replace",
        SYM_SEARCH = "Symbol.search",
        SYM_SPLIT = "Symbol.split",
        SYM_DISPOSE = "Symbol.dispose",
        SYM_ASYNC_DISPOSE = "Symbol.asyncDispose",
        SYM_METADATA = "Symbol.metadata",
    }
}

/// Initial atom table capacity (entries) and hash index size (buckets, a power of two).
const INITIAL_ATOMS: u32 = 256;
const INITIAL_BUCKETS: u32 = 512;

/// Parses a canonical array index ("0", or no leading zero, at most 2^32-2).
fn parse_array_index(units: impl Iterator<Item = u16>) -> Option<u32> {
    let mut n: u64 = 0;
    let mut len = 0;
    for u in units {
        if !(b'0' as u16..=b'9' as u16).contains(&u) || (len > 0 && n == 0) || len >= 10 {
            return None;
        }
        n = n * 10 + (u - b'0' as u16) as u64;
        len += 1;
    }
    if len == 0 || n > MAX_ARRAY_INDEX as u64 {
        None
    } else {
        Some(n as u32)
    }
}

/// Writes `n` in decimal into `buf` and returns the used tail.
fn format_u32(n: u32, buf: &mut [u8; 10]) -> &[u8] {
    let mut i = buf.len();
    let mut n = n;
    loop {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            return &buf[i..];
        }
    }
}

/// What an atom lookup looks for: a heap string or code units.
#[derive(Clone, Copy)]
enum Needle<'a> {
    Str(Str),
    /// Latin-1 (here: ASCII) code units.
    Latin1(&'a [u8]),
    Units(&'a [u16]),
}

impl Heap {
    /// Creates the atom table and interns the well-known atoms and symbols.
    pub(crate) fn init_atoms(&mut self) -> Result<(), HeapError> {
        let table = self.alloc_cell(CellKind::AtomTable, AtomTable::LAYOUT.min_size)?;
        let entries = self.alloc_value_array(INITIAL_ATOMS)?;
        let index = self.alloc_u32_array(INITIAL_BUCKETS)?;
        let a = self.arena_mut();
        a.set(entries, ValueArray::LEN, 1); // slot 0 is the null atom
        a.set(table, AtomTable::ENTRIES, Ref(entries));
        a.set(table, AtomTable::INDEX, Ref(index));
        a.set(0, Roots::ATOMS, Ref(table));
        for (i, s) in WELL_KNOWN_STRINGS.iter().enumerate() {
            let atom = self.atom_str(s)?;
            assert_eq!(
                atom.0 as usize,
                i + 1,
                "well-known atom {s:?} is not unique"
            );
        }
        for (i, d) in WELL_KNOWN_SYMBOLS.iter().enumerate() {
            let desc = self.alloc_str(d)?;
            let sym = self.new_symbol(Value::string(desc))?;
            let atom = self.symbol_atom(sym);
            assert_eq!(atom.0, SYM_BIT | (WELL_KNOWN_STRINGS.len() + 1 + i) as u32);
        }
        Ok(())
    }

    #[inline]
    pub(super) fn atom_table(&self) -> u32 {
        self.arena().get(0, Roots::ATOMS).0
    }

    #[inline]
    fn atom_entries(&self) -> u32 {
        self.arena().get(self.atom_table(), AtomTable::ENTRIES).0
    }

    /// The number of the base image's atoms (D2431), the first index of a VM's own; 0
    /// without a base.
    #[inline]
    fn atom_base_count(&self) -> u32 {
        self.arena().get(self.atom_table(), AtomTable::BASE_COUNT)
    }

    /// Number of atom table entries (string and symbol atoms; slot 0 included).
    pub fn atom_count(&self) -> u32 {
        self.atom_base_count() + self.value_array_len(self.atom_entries())
    }

    /// The table entry of a string or symbol atom.
    fn atom_entry(&self, a: Atom) -> Value {
        debug_assert!(!a.is_int() && !a.is_null());
        match self.atom_entry_at(a.table_index()) {
            Some(v) => v,
            None => super::fail::invalid_atom(a),
        }
    }

    /// The table entry at atom index `i`, from the base's entries or the VM's own.
    #[inline]
    pub(super) fn atom_entry_at(&self, i: u32) -> Option<Value> {
        let table = self.atom_table();
        let n: u32 = self.arena().get(table, AtomTable::BASE_COUNT);
        let (arr, k) = if i < n {
            (self.arena().get::<Ref>(table, AtomTable::BASE_ENTRIES).0, i)
        } else {
            (self.atom_entries(), i - n)
        };
        (k < self.value_array_len(arr)).then(|| self.value_array_get(arr, k))
    }

    /// Appends an atom table entry and returns its index.
    fn push_atom_entry(&mut self, v: Value) -> Result<u32, HeapError> {
        let table = self.atom_table();
        let entries = self.atom_entries();
        let len = self.value_array_len(entries);
        let first = self.atom_base_count();
        if first + len > INDEX_MASK {
            return Err(HeapError::OutOfMemory);
        }
        // By a quarter, not doubling: pi's sealed prelude image keeps its table, and
        // doubling left KBs of empty slots in it (D2057).
        let entries = if len < self.value_array_capacity(entries) {
            entries
        } else {
            self.value_array_regrow(entries, len, len + len / 4 + 8)?
        };
        self.arena_mut()
            .set(table, AtomTable::ENTRIES, Ref(entries));
        self.value_array_set(entries, len, v);
        self.arena_mut().set(entries, ValueArray::LEN, len + 1);
        Ok(first + len)
    }

    /// Finds a string atom among the VM's own atoms by its keyed hash (D2433), then among
    /// the base's by its base hash (D2431), computed only if needed. A string is in one
    /// of the two levels; the VM's own goes first since its index is small and its strings
    /// cache their hashes.
    fn find_string_atom(&self, hash: u32, key: Needle) -> Option<Atom> {
        let table = self.atom_table();
        let a = self.arena();
        let found = self.find_in_level(
            a.get::<Ref>(table, AtomTable::INDEX).0,
            a.get::<Ref>(table, AtomTable::ENTRIES).0,
            a.get(table, AtomTable::BASE_COUNT),
            hash,
            false,
            key,
        );
        if found.is_some() {
            return found;
        }
        self.find_base_atom(key)
    }

    /// Finds a string atom among the base's atoms alone, by the base hash.
    fn find_base_atom(&self, key: Needle) -> Option<Atom> {
        let table = self.atom_table();
        let a = self.arena();
        let base: Ref = a.get(table, AtomTable::BASE_INDEX);
        if base.is_null() {
            return None;
        }
        let hash = match key {
            Needle::Str(s) => self.str_hash_base(s),
            Needle::Latin1(b) => hash_latin1_with(BASE_HASH_KEY, b),
            Needle::Units(u) => hash_units_with(BASE_HASH_KEY, u),
        };
        let entries = a.get::<Ref>(table, AtomTable::BASE_ENTRIES).0;
        self.find_in_level(base.0, entries, 0, hash, true, key)
    }

    /// Probes one level's index (`first`: the atom index of its entry 0; `fixed`: by the
    /// base hash, else the keyed one).
    fn find_in_level(
        &self,
        index: u32,
        entries: u32,
        first: u32,
        hash: u32,
        fixed: bool,
        key: Needle,
    ) -> Option<Atom> {
        // (The probe loop written out: `Heap::probe` here stayed a call.)
        let len = self.u32_array_len(index);
        if !len.is_power_of_two() {
            super::fail::bad_hash_index(index, len);
        }
        let mask = len - 1;
        let base = self.base_end();
        let mut b = hash & mask;
        for _ in 0..len {
            let idx = self.u32_array_get(index, b);
            if idx == 0 {
                return None;
            }
            let s = self
                .value_array_get(entries, idx - first)
                .as_string()
                .expect("string atom");
            // The atom's own string (an atom turned back into a key, `push_atom`).
            if matches!(key, Needle::Str(t) if t == s) {
                return Some(Atom(idx));
            }
            // The cached hash is the one wanted when the string is on that level's side of
            // the prefix's end (D2433), as the VM's own atoms and the base's are.
            let cached: u32 = self.arena().get(s.0, super::cells::Str8::HASH);
            let h = if cached != 0 && (s.0 < base) == fixed {
                cached
            } else if fixed {
                self.str_hash_base(s)
            } else {
                self.str_hash_uncached(s)
            };
            if h == hash && self.needle_eq(s, key) {
                return Some(Atom(idx));
            }
            b = (b + 1) & mask;
        }
        super::fail::bad_hash_index(index, len)
    }

    fn needle_eq(&self, s: Str, key: Needle) -> bool {
        match key {
            Needle::Str(t) => self.str_eq(s, t),
            Needle::Latin1(b) => matches!(self.str_view(s), Some(StrView::Latin1(x)) if x == b),
            Needle::Units(u) => match self.str_view(s) {
                Some(v) => v.len() == u.len() && v.units().eq(u.iter().copied()),
                None => false,
            },
        }
    }

    /// Rebuilds the VM's own atom index under the current hash key (`set_hash_key`).
    pub(super) fn rekey_atoms(&mut self) {
        let table = self.atom_table();
        let index = self.arena().get::<Ref>(table, AtomTable::INDEX).0;
        for b in 0..self.u32_array_len(index) {
            self.u32_array_set(index, b, 0);
        }
        let entries = self.atom_entries();
        let first = self.atom_base_count();
        for i in 0..self.value_array_len(entries) {
            if let Some(t) = self.value_array_get(entries, i).as_string() {
                let h = self.str_hash(t);
                self.index_insert(index, h, first + i);
            }
        }
    }

    fn index_insert(&mut self, index: u32, hash: u32, atom_index: u32) {
        let b = self.probe_empty(index, hash);
        self.u32_array_set(index, b, atom_index);
    }

    /// Adds the flat string `s` (not yet interned, hash cached) as a new string atom.
    fn add_string_atom(&mut self, s: Str, hash: u32) -> Result<Atom, HeapError> {
        let table = self.atom_table();
        let strings = self.arena().get(table, AtomTable::STRINGS) + 1;
        let mut index = self.arena().get(table, AtomTable::INDEX).0;
        // Keep the load factor at or below 1/4 while the index is small, 1/2 past 1024
        // buckets (D2433): every lookup of a base atom's name misses here first, and a
        // miss walks to an empty bucket; a big table (pi's prelude) stays compact. Grow
        // before pushing the entry so a failed allocation leaves the table unchanged.
        let len = self.u32_array_len(index);
        if strings * if len < 1024 { 4 } else { 2 } > len {
            let new = self.alloc_u32_array(self.u32_array_len(index) * 2)?;
            let entries = self.atom_entries();
            let first = self.atom_base_count();
            for i in 0..self.value_array_len(entries) {
                if let Some(t) = self.value_array_get(entries, i).as_string() {
                    let h = self.str_hash_uncached(t);
                    self.index_insert(new, h, first + i);
                }
            }
            index = new;
            self.arena_mut().set(table, AtomTable::INDEX, Ref(index));
        }
        let idx = self.push_atom_entry(Value::string(s))?;
        self.index_insert(index, hash, idx);
        self.arena_mut().set(table, AtomTable::STRINGS, strings);
        Ok(Atom(idx))
    }

    /// Forgets the string atoms whose table index `keep` does not list (indexes past its
    /// end are kept): the other entries move down over theirs, into an entries array of
    /// exactly their size, and the intern index is rebuilt without them, sized for the
    /// rest. The well-known atoms and every symbol are kept. Only for the base image
    /// (D2253, D2447): the caller guarantees that nothing in the heap refers to a
    /// forgotten atom, which interns again as a new id if it is needed later, and
    /// renumbers every atom id in the heap by the returned map ([`Heap::moved_atom`]).
    pub(crate) fn forget_string_atoms(&mut self, keep: &[bool]) -> Result<Vec<u32>, HeapError> {
        assert_eq!(self.atom_base_count(), 0, "the atom table is split already");
        let table = self.atom_table();
        let entries = self.atom_entries();
        let n = self.value_array_len(entries);
        let fixed = (WELL_KNOWN_STRINGS.len() + WELL_KNOWN_SYMBOLS.len()) as u32;
        // The kept entries move down over the forgotten ones (D2447): `map[old]` is the
        // new index (`u32::MAX` for a forgotten atom). The well-known atoms keep theirs.
        let mut map = alloc::vec![u32::MAX; n as usize];
        map[0] = 0;
        let mut next = 1;
        let mut strings: u32 = 0;
        for i in 1..n {
            let v = self.value_array_get(entries, i);
            let string = v.as_string().is_some();
            if string && i > fixed && keep.get(i as usize) == Some(&false) {
                continue;
            }
            strings += u32::from(string);
            map[i as usize] = next;
            self.value_array_set(entries, next, v);
            next += 1;
        }
        let compact = self.alloc_value_array(next)?;
        let a = self.arena_mut();
        a.copy_within(
            ValueArray::VALUES.at(entries, 0),
            8 * next,
            ValueArray::VALUES.at(compact, 0),
        );
        a.set(compact, ValueArray::LEN, next);
        a.set(table, AtomTable::ENTRIES, Ref(compact));
        // The index at a load factor of at most 1/2, as `add_string_atom` keeps it.
        let len = (2 * strings).next_power_of_two().max(INITIAL_BUCKETS);
        let index = self.alloc_u32_array(len)?;
        for i in 1..next {
            if let Some(t) = self.value_array_get(compact, i).as_string() {
                let h = self.str_hash_uncached(t);
                self.index_insert(index, h, i);
            }
        }
        let a = self.arena_mut();
        a.set(table, AtomTable::INDEX, Ref(index));
        a.set(table, AtomTable::STRINGS, strings);
        Ok(map)
    }

    /// The atom of `a` after [`Heap::forget_string_atoms`] moved the entries (`map`):
    /// integer atoms and the null atom stay.
    pub(crate) fn moved_atom(map: &[u32], a: Atom) -> Atom {
        if a.is_int() || a.is_null() {
            return a;
        }
        let new = map[a.table_index() as usize];
        assert!(new != u32::MAX, "a forgotten atom is still in use: {a:?}");
        Atom((a.0 & SYM_BIT) | new)
    }

    /// Renumbers the atom ids that heap structures hold by `map` (after
    /// [`Heap::forget_string_atoms`], D2447): property table keys (whose hash indexes
    /// are rebuilt), shape descriptors (shared along a chain: each array once) with their
    /// parents' transition lists re-sorted, and symbols' own atoms. Code and templates
    /// are the caller's (`vm::image`).
    pub(crate) fn renumber_heap_atoms(&mut self, map: &[u32]) -> Result<(), HeapError> {
        use super::cells::{PropEntry, PropTable, RefArray, Shape, U32Array};
        let cells: Vec<(u32, CellKind)> = self.cells().map(|c| (c.offset, c.kind)).collect();
        let moved = |a: u32| Self::moved_atom(map, Atom(a)).0;
        let mut descs = alloc::collections::BTreeSet::new();
        for &(c, kind) in &cells {
            match kind {
                CellKind::PropTable => {
                    for i in 0..self.arena().get(c, PropTable::COUNT) {
                        let e = PropTable::ENTRIES.at(c, i);
                        let k = self.arena().get(e, PropEntry::KEY);
                        self.arena_mut().set(e, PropEntry::KEY, moved(k));
                    }
                    if self.arena().get::<Ref>(c, PropTable::INDEX).get().is_some() {
                        self.build_table_index_dense(c)?;
                    }
                }
                CellKind::Shape => {
                    if let Some(d) = self.arena().get::<Ref>(c, Shape::DESCS).get()
                        && descs.insert(d)
                    {
                        let used = self.arena().read::<u32>(U32Array::DATA.at(d, 0));
                        for i in 0..used {
                            let at = U32Array::DATA.at(d, 1 + 2 * i);
                            let k = self.arena().read::<u32>(at);
                            self.arena_mut().write(at, moved(k));
                        }
                    }
                }
                CellKind::Symbol => {
                    let k = self.arena().get(c, Symbol::ATOM);
                    self.arena_mut().set(c, Symbol::ATOM, moved(k));
                }
                _ => {}
            }
        }
        // Transition lists are sorted by the key and flags of the child's last descriptor.
        for &(s, kind) in &cells {
            if kind != CellKind::Shape {
                continue;
            }
            let Some(tr) = self.arena().get::<Ref>(s, Shape::TRANSITIONS).get() else {
                continue;
            };
            let count = self.shape_count(s);
            let len = self.arena().get(tr, RefArray::LEN);
            let mut children: Vec<(u32, u32, u32)> = (0..len)
                .map(|i| {
                    let c = self.arena().read::<Ref>(RefArray::REFS.at(tr, i)).0;
                    let (k, f) = self.shape_desc(c, count);
                    (k.0, f, c)
                })
                .collect();
            crate::sort::sort_by(&mut children, |a, b| (a.0, a.1) < (b.0, b.1));
            for (i, &(_, _, c)) in children.iter().enumerate() {
                self.arena_mut()
                    .write(RefArray::REFS.at(tr, i as u32), Ref(c));
            }
        }
        Ok(())
    }

    /// Makes the atom table two-level for a base image (D2431): its entries and index
    /// become the base's, which the image puts in the builtin prefix, and the atoms a VM
    /// adds go to new arrays with room for `room` of them, which the image puts in its
    /// tail. Only for the base image's build.
    pub(crate) fn split_atom_table(&mut self, room: u32) -> Result<(), HeapError> {
        assert_eq!(self.atom_base_count(), 0, "the atom table is split already");
        let table = self.atom_table();
        let count = self.atom_count();
        let entries = self.alloc_value_array(room.max(1))?;
        let index = self.alloc_u32_array((4 * room).next_power_of_two().max(8))?;
        // The base's index by the base hash (D2433): the same in every VM of the build.
        let base_index: Ref = self.arena().get(table, AtomTable::INDEX);
        let base: Ref = self.arena().get(table, AtomTable::ENTRIES);
        for b in 0..self.u32_array_len(base_index.0) {
            self.u32_array_set(base_index.0, b, 0);
        }
        for i in 1..count {
            if let Some(t) = self.value_array_get(base.0, i).as_string() {
                let h = self.str_hash_base(t);
                self.index_insert(base_index.0, h, i);
            }
        }
        let a = self.arena_mut();
        a.set(entries, ValueArray::LEN, 0);
        a.set(table, AtomTable::BASE_ENTRIES, base);
        a.set(table, AtomTable::BASE_INDEX, base_index);
        a.set(table, AtomTable::ENTRIES, Ref(entries));
        a.set(table, AtomTable::INDEX, Ref(index));
        a.set(table, AtomTable::STRINGS, 0);
        a.set(table, AtomTable::BASE_COUNT, count);
        Ok(())
    }

    /// Interns a heap string. Canonical array indexes below 2^31 become integer atoms.
    pub fn intern(&mut self, s: Str) -> Result<Atom, HeapError> {
        #[cfg(feature = "compile-cache")]
        if self.intern_log.is_some() {
            let before = self.arena().len();
            let a = self.intern_inner(s)?;
            self.log_intern(a, before, crate::vm::compile_cache::Source::Str(s.0));
            return Ok(a);
        }
        self.intern_inner(s)
    }

    /// Records interning `a` (the compile cache, D3040): the arena was `before` long.
    #[cfg(feature = "compile-cache")]
    fn log_intern(&mut self, a: Atom, before: u32, source: crate::vm::compile_cache::Source) {
        let after = self.arena().len();
        if let Some(log) = self.intern_log.as_mut() {
            log.push(crate::vm::compile_cache::Intern {
                atom: a,
                before,
                after,
                source,
            });
        }
    }

    #[inline(always)]
    fn intern_inner(&mut self, s: Str) -> Result<Atom, HeapError> {
        let slot = atom_memo_slot(s);
        if self.atom_memo[slot][0] == s.0 {
            return Ok(Atom(self.atom_memo[slot][1]));
        }
        let a = self.intern_uncached(s)?;
        self.atom_memo[slot] = [s.0, a.0];
        Ok(a)
    }

    fn intern_uncached(&mut self, s: Str) -> Result<Atom, HeapError> {
        if let Some(i) = self.str_array_index(s)
            && let Some(a) = Atom::int(i)
        {
            return Ok(a);
        }
        // A string of the builtin prefix is most likely a base atom's own (a builtin's
        // name, a constant of the prelude): its fixed hash is cached and its keyed one is
        // not, so it looks among the base's atoms first.
        let s = self.resolve_str(s);
        if s.0 < self.base_end()
            && let Some(a) = self.find_base_atom(Needle::Str(s))
        {
            return Ok(a);
        }
        let hash = self.str_hash(s);
        if let Some(a) = self.find_string_atom(hash, Needle::Str(s)) {
            return Ok(a);
        }
        let mut flat = self.flatten(s)?;
        if flat.0 < self.base_end() {
            // The VM's own atoms are strings above the prefix, which cache their keyed
            // hash (D2433): a prefix string would be hashed again at every lookup.
            flat = self.copy_flat(flat)?;
        }
        self.str_hash(flat);
        self.add_string_atom(flat, hash)
    }

    /// The string atom of `s` if it is interned already (never an integer atom).
    pub(crate) fn find_string_atom_of(&mut self, s: Str) -> Option<Atom> {
        let memo = self.atom_memo[atom_memo_slot(s)];
        if memo[0] == s.0 && !Atom(memo[1]).is_int() {
            return Some(Atom(memo[1]));
        }
        if self.str_array_index(s).is_some() {
            return None;
        }
        // As `intern` looks (D2433): a prefix string among the base's atoms first.
        let s = self.resolve_str(s);
        if s.0 < self.base_end()
            && let Some(a) = self.find_base_atom(Needle::Str(s))
        {
            return Some(a);
        }
        let hash = self.str_hash(s);
        self.find_string_atom(hash, Needle::Str(s))
    }

    /// The string atom of `t` where it is interned already (not an index), without
    /// allocating.
    #[cfg(feature = "dump")]
    pub(crate) fn find_atom_str(&self, t: &str) -> Option<Atom> {
        if parse_array_index(t.bytes().map(u16::from)).is_some() {
            return None;
        }
        let units: alloc::vec::Vec<u16>;
        let (hash, key) = if super::string::is_ascii(t.as_bytes()) {
            let b = t.as_bytes();
            (self.keyed_hash_latin1(b), Needle::Latin1(b))
        } else {
            units = t.encode_utf16().collect();
            (self.keyed_hash_units(&units), Needle::Units(&units))
        };
        self.find_string_atom(hash, key)
    }

    /// Interns a Rust string; allocates only if it is not interned yet.
    pub fn atom_str(&mut self, t: &str) -> Result<Atom, HeapError> {
        #[cfg(feature = "compile-cache")]
        if self.intern_log.is_some() {
            let before = self.arena().len();
            let a = self.atom_str_inner(t)?;
            self.log_intern(a, before, crate::vm::compile_cache::Source::Text);
            return Ok(a);
        }
        self.atom_str_inner(t)
    }

    #[inline(always)]
    fn atom_str_inner(&mut self, t: &str) -> Result<Atom, HeapError> {
        if let Some(i) = parse_array_index(t.bytes().map(u16::from))
            && let Some(a) = Atom::int(i)
        {
            return Ok(a);
        }
        let units: alloc::vec::Vec<u16>;
        let (hash, key) = if super::string::is_ascii(t.as_bytes()) {
            let b = t.as_bytes();
            (self.keyed_hash_latin1(b), Needle::Latin1(b))
        } else {
            units = t.encode_utf16().collect();
            (self.keyed_hash_units(&units), Needle::Units(&units))
        };
        if let Some(a) = self.find_string_atom(hash, key) {
            return Ok(a);
        }
        let s = self.alloc_str(t)?;
        self.str_hash(s);
        self.add_string_atom(s, hash)
    }

    /// Interns a string given as UTF-16 code units; allocates only if it is not interned
    /// yet.
    pub fn atom_utf16(&mut self, units: &[u16]) -> Result<Atom, HeapError> {
        #[cfg(feature = "compile-cache")]
        if self.intern_log.is_some() {
            let before = self.arena().len();
            let a = self.atom_utf16_inner(units)?;
            self.log_intern(a, before, crate::vm::compile_cache::Source::Text);
            return Ok(a);
        }
        self.atom_utf16_inner(units)
    }

    #[inline(always)]
    fn atom_utf16_inner(&mut self, units: &[u16]) -> Result<Atom, HeapError> {
        if let Some(i) = parse_array_index(units.iter().copied())
            && let Some(a) = Atom::int(i)
        {
            return Ok(a);
        }
        let hash = self.keyed_hash_units(units);
        if let Some(a) = self.find_string_atom(hash, Needle::Units(units)) {
            return Ok(a);
        }
        let s = self.alloc_utf16(units)?;
        self.str_hash(s);
        self.add_string_atom(s, hash)
    }

    /// The atom for an array index: an integer atom below 2^31, else an interned string.
    pub fn atom_from_index(&mut self, i: u32) -> Result<Atom, HeapError> {
        if let Some(a) = Atom::int(i) {
            return Ok(a);
        }
        let mut buf = [0; 10];
        let digits = format_u32(i, &mut buf);
        let t = core::str::from_utf8(digits).expect("ascii digits");
        self.atom_str(t)
    }

    /// The array index a key denotes (integer atoms, and string atoms such as
    /// `"4294967294"`), or `None`.
    pub fn atom_array_index(&self, a: Atom) -> Option<u32> {
        if let Some(i) = a.as_int() {
            return Some(i);
        }
        if !a.is_string() {
            return None;
        }
        let s = self.atom_entry(a).as_string()?;
        // Shorter strings would have become integer atoms.
        if self.str_len(s) != 10 {
            return None;
        }
        self.str_array_index(s)
    }

    /// Parses `s` as a canonical array index (0 ..= 2^32-2).
    pub fn str_array_index(&self, s: Str) -> Option<u32> {
        let len = self.str_len(s);
        if len == 0 || len > 10 {
            return None;
        }
        let first = self.char_code_at(s, 0)?;
        if !(b'0' as u16..=b'9' as u16).contains(&first) {
            return None;
        }
        parse_array_index(self.str_units(s))
    }

    /// The string of a string or integer atom. Integer atoms allocate a new string.
    /// Panics on a symbol atom.
    pub fn atom_string(&mut self, a: Atom) -> Result<Str, HeapError> {
        if let Some(i) = a.as_int() {
            let mut buf = [0; 10];
            return self.alloc_latin1(format_u32(i, &mut buf));
        }
        assert!(!a.is_symbol(), "atom_string on a symbol atom");
        Ok(self.atom_entry(a).as_string().expect("string atom"))
    }

    /// The string of a string atom, without allocating; `None` for integer and symbol
    /// atoms (and the null atom).
    pub(crate) fn atom_str_value(&self, a: Atom) -> Option<Str> {
        if a.is_int() || a.is_null() || a.is_symbol() {
            return None;
        }
        self.atom_entry(a).as_string()
    }

    /// The key as a JS value: a string or a symbol. Integer atoms allocate a string.
    pub fn atom_to_value(&mut self, a: Atom) -> Result<Value, HeapError> {
        if a.is_symbol() {
            Ok(self.atom_entry(a))
        } else {
            Ok(Value::string(self.atom_string(a)?))
        }
    }

    /// The string of a string atom; `None` for an array index or a symbol.
    #[cfg(feature = "dump")]
    pub fn atom_string_of(&self, a: Atom) -> Option<crate::value::Str> {
        if a.is_int() || a.is_null() || a.is_symbol() {
            return None;
        }
        self.atom_entry(a).as_string()
    }

    /// The symbol of a symbol atom.
    pub fn atom_symbol(&self, a: Atom) -> Option<Sym> {
        if a.is_symbol() {
            self.atom_entry(a).as_symbol()
        } else {
            None
        }
    }

    /// A readable name for debugging: the string, `[description]` for symbols.
    pub fn atom_name(&self, a: Atom) -> String {
        if let Some(i) = a.as_int() {
            let mut buf = [0; 10];
            return String::from(core::str::from_utf8(format_u32(i, &mut buf)).unwrap());
        }
        if a.is_null() {
            return String::from("<null atom>");
        }
        let v = self.atom_entry(a);
        if let Some(s) = v.as_string() {
            return self.str_to_string(s);
        }
        let sym = v.as_symbol().expect("symbol atom");
        let mut out = String::from("[");
        if let Some(d) = self.symbol_description(sym).as_string() {
            out.push_str(&self.str_to_string(d));
        }
        out.push(']');
        out
    }

    /// Creates a new symbol. `description` is a string value or `undefined`.
    pub fn new_symbol(&mut self, description: Value) -> Result<Sym, HeapError> {
        debug_assert!(description.is_undefined() || description.as_string().is_some());
        let cell = self.alloc_cell(CellKind::Symbol, Symbol::LAYOUT.min_size)?;
        self.arena_mut().set(cell, Symbol::DESCRIPTION, description);
        let idx = self.push_atom_entry(Value::symbol(Sym(cell)))?;
        self.arena_mut().set(cell, Symbol::ATOM, SYM_BIT | idx);
        Ok(Sym(cell))
    }

    /// Creates a private name (D1400): a symbol flagged [`SYM_PRIVATE`] and `flags`
    /// ([`SYM_PRIVATE_METHOD`], [`SYM_PRIVATE_READONLY`]).
    ///
    /// [`SYM_PRIVATE`]: super::cells::flags::SYM_PRIVATE
    /// [`SYM_PRIVATE_METHOD`]: super::cells::flags::SYM_PRIVATE_METHOD
    /// [`SYM_PRIVATE_READONLY`]: super::cells::flags::SYM_PRIVATE_READONLY
    pub fn new_private_symbol(&mut self, description: Str, flags: u16) -> Result<Sym, HeapError> {
        let s = self.new_symbol(Value::string(description))?;
        self.set_cell_flags(s.0, super::cells::flags::SYM_PRIVATE | flags);
        Ok(s)
    }

    /// The symbol's private-name flags (0 for an ordinary symbol).
    pub fn symbol_private_flags(&self, s: Sym) -> u16 {
        self.cell_flags(s.0)
    }

    /// `a` is the key of a private name.
    pub fn is_private_atom(&self, a: Atom) -> bool {
        a.is_symbol()
            && self.atom_symbol(a).is_some_and(|s| {
                self.symbol_private_flags(s) & super::cells::flags::SYM_PRIVATE != 0
            })
    }

    /// The property key of a symbol.
    pub fn symbol_atom(&self, s: Sym) -> Atom {
        Atom(self.arena().get(s.0, Symbol::ATOM))
    }

    /// A string value or `undefined`.
    pub fn symbol_description(&self, s: Sym) -> Value {
        self.arena().get(s.0, Symbol::DESCRIPTION)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn array_index_parsing() {
        let p = |s: &str| parse_array_index(s.bytes().map(u16::from));
        assert_eq!(p("0"), Some(0));
        assert_eq!(p("7"), Some(7));
        assert_eq!(p("4294967294"), Some(4294967294));
        assert_eq!(p("4294967295"), None);
        assert_eq!(p("10000000000"), None);
        assert_eq!(p("01"), None);
        assert_eq!(p("00"), None);
        assert_eq!(p(""), None);
        assert_eq!(p("-1"), None);
        assert_eq!(p("1.5"), None);
        assert_eq!(p("1e3"), None);
        assert_eq!(p(" 1"), None);
    }

    #[test]
    fn format() {
        let mut b = [0; 10];
        assert_eq!(format_u32(0, &mut b), b"0");
        assert_eq!(format_u32(u32::MAX, &mut b), b"4294967295");
        assert_eq!(format_u32(1050, &mut b), b"1050");
    }

    #[test]
    fn atom_bits() {
        let a = Atom::int(5).unwrap();
        assert!(a.is_int() && !a.is_string() && !a.is_symbol());
        assert_eq!(a.as_int(), Some(5));
        assert_eq!(
            Atom::int(MAX_INT_ATOM).unwrap().as_int(),
            Some(MAX_INT_ATOM)
        );
        assert_eq!(Atom::int(MAX_INT_ATOM + 1), None);
        assert!(atoms::LENGTH.is_string());
        assert!(atoms::SYM_ITERATOR.is_symbol());
        assert!(!Atom::NULL.is_string() && !Atom::NULL.is_symbol());
        assert_eq!(atoms::EMPTY.id(), 1);
    }
}

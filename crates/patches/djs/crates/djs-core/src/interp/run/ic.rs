//! Inline caches (0062, D1620, D1621, 0083): one slot per property or global access
//! site, in the template's `IC` array (a U32Array of [`WORDS`] words per site, allocated
//! when the first slot is filled). A slot remembers where the property was found last
//! time. A hit re-checks what the slot relies on (a key at an index, a shape, the
//! prototype epoch), so a stale slot is only a miss and caches never change semantics.
//! Slots that hold arena offsets set [`IC_OFFSET_SLOT`] and are emptied by the collector,
//! so a VM restored with warm caches behaves like a cold one and images stay
//! deterministic.
//!
//! Slot layouts (word 0 = 0: empty). Offset slots carry a kind in bits 24..30 of word 0:
//! 0 an own property of a shape-mode receiver, 1 to 62 a prototype property at that
//! depth, `KIND_ADD` an addition.
//!
//! - globals: word 0 = entry index + 1, with `IN_LEX` set when the entry is in the
//!   global lexical record; word 1 = the realm's `GlobalLexEpoch` when the entry is in
//!   the global object (a newer lexical binding could shadow it).
//! - own property, dictionary-mode receiver: word 0 = entry index + 1 of an own data
//!   property. Only keys that are not array indexes are cached: for those, an own
//!   property is always a named property, whatever the object's class.
//! - own property, shape-mode receiver (0083): word 0 = offset slot, kind 0, slot + 1;
//!   word 1 = the receiver's shape; word 2 = 1 if writable. A hit is a shape compare: the
//!   shape fixes the key and its attributes.
//! - prototype property reads (D1621): word 0 = offset slot, kind = depth of the holder,
//!   entry index + 1 in the holder; word 1 = the offset of the receiver's prototype (an
//!   object's, or the realm's prototype of a primitive); word 2 = `Roots::PROTO_EPOCH`;
//!   word 3 = the receiver's shape word (`Heap::receiver_shape`, 0083), or 0. The objects
//!   from that prototype to the holder are marked `OBJ_IS_PROTO`, so a key added to one
//!   of them or a prototype change bumps the epoch. The receiver must not have an own
//!   `key`: an equal shape word proves that, otherwise the hit looks it up.
//! - additions (0083): word 0 = offset slot, kind `KIND_ADD`, `ADD_SET` for a
//!   `put_field` site ([[Set]]) or 0 for `define_field` (CreateDataProperty); word 1 =
//!   the receiver's shape word before, word 2 = the shape after. A [[Set]] addition also
//!   relies on the prototype chain having no `key` (nor a proxy or a typed array): word
//!   3 = the receiver's prototype (0 for none), word 4 = the epoch, with every object of
//!   the chain marked `OBJ_IS_PROTO`.
//!
//! The hit checks are [`cached_get`] and [`cached_put`]: every fast path uses them.

use super::*;
use crate::bytecode::template::NO_CACHE;
use crate::heap::cells::{IC_OFFSET_SLOT, IC_WORDS as WORDS, U32Array};

/// The words of a cache slot.
pub(crate) type Slot = [u32; WORDS as usize];

/// Offset slot word 0: the kind (bits 24..30) and an index + 1 (bits 0..24).
const KIND_SHIFT: u32 = 24;
const INDEX_MASK: u32 = (1 << KIND_SHIFT) - 1;
const KIND_MASK: u32 = 63;
/// The kind of an addition slot; prototype depths stay below it.
const KIND_ADD: u32 = 63;
const MAX_DEPTH: u32 = KIND_ADD - 1;
/// Addition slot word 0: a `put_field` site.
const ADD_SET: u32 = 1;

/// The slots of the megamorphic cache (`Heap::mega_ic`), a power of two.
const MEGA_SLOTS: u32 = 256;
const MEGA_KEY: usize = WORDS as usize - 1;

/// Global slot word 0: the entry is in the global lexical record.
const IN_LEX: u32 = 1 << 31;

#[inline(always)]
fn kind(w0: u32) -> u32 {
    w0 >> KIND_SHIFT & KIND_MASK
}

/// Whether `obj` could have an own property `key` that its named properties don't show,
/// so that a shape compare cannot stand in for a lookup: a typed array's numeric keys, a
/// proxy's traps, an array's `length`, a closure's lazy `length` and `name` (D2445).
#[inline(always)]
fn exotic_own(heap: &Heap, obj: Obj, key: Atom) -> bool {
    match heap.object_class(obj) {
        ObjectClass::Proxy | ObjectClass::TypedArray => true,
        ObjectClass::Array => key == crate::heap::atoms::LENGTH,
        ObjectClass::Function => {
            key == crate::heap::atoms::LENGTH || key == crate::heap::atoms::NAME
        }
        _ => false,
    }
}

/// The prototype a cached prototype read on `o` starts from, if `o` cannot have an own
/// property `key` that the cache would skip; `shape` is the receiver shape word the slot
/// remembers (0: none). `None` for a proxy or a typed array, an object with its own
/// `key`, nullish values, and a string's `length` and indexes.
#[inline(always)]
fn proto_start(heap: &Heap, o: Value, key: Atom, shape: u32) -> Option<Obj> {
    match o.as_object() {
        Some(obj) => {
            if exotic_own(heap, obj, key)
                || ((shape == 0 || heap.receiver_shape(obj) != shape)
                    && heap.has_own_property(obj, key))
            {
                return None;
            }
            heap.get_prototype_of(obj)
        }
        None => {
            if o.as_string().is_some()
                && (key == crate::heap::atoms::LENGTH || key.as_int().is_some())
            {
                return None;
            }
            realm::primitive_proto(heap, o)
        }
    }
}

/// The arena offset of cache slot `ic` of `template`, or 0 when the function has no
/// cache array yet (or the site has no slot): what [`cached_get`] and [`cached_put`]
/// take.
#[inline(always)]
pub(crate) fn slot_at(heap: &Heap, template: u32, ic: u32) -> u32 {
    match heap.arena().get::<Ref>(template, FuncTemplate::IC).get() {
        Some(a) if ic != NO_CACHE as u32 => U32Array::DATA.at(a, ic * WORDS),
        _ => 0,
    }
}

/// Word `k` of the slot at `at` (see [`slot_at`]).
#[inline(always)]
fn word(heap: &Heap, at: u32, k: u32) -> u32 {
    heap.arena().read(at + 4 * k)
}

/// The first three words of the slot at `at`, with one bounds check: every hit needs
/// them (words 3 and 4 only prototype reads and [[Set]] additions).
#[inline(always)]
fn head(heap: &Heap, at: u32) -> [u32; 3] {
    let b = heap.arena().slice(at, 12);
    let w = |i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
    [w(0), w(4), w(8)]
}

/// A get site's cache hit: the value of `o[key]` if the slot at `at` (0: none) still
/// applies, else `None`.
#[inline(always)]
pub(crate) fn cached_get(heap: &Heap, at: u32, o: Value, key: Atom) -> Option<Value> {
    if at == 0 {
        return None;
    }
    get_hit(heap, head(heap, at), |h| word(h, at, 3), o, key)
}

/// The hit check of a get slot whose first three words are `[w0, w1, w2]`; `w3` reads
/// word 3 (only prototype reads need it).
#[inline(always)]
fn get_hit(
    heap: &Heap,
    [w0, w1, w2]: [u32; 3],
    w3: impl FnOnce(&Heap) -> u32,
    o: Value,
    key: Atom,
) -> Option<Value> {
    if w0 & IC_OFFSET_SLOT == 0 {
        // Empty, or a verified index into a dictionary.
        return match (w0, o.as_object()) {
            (0, _) | (_, None) => None,
            (_, Some(obj)) => heap.data_at(obj, w0 - 1, key),
        };
    }
    match kind(w0) {
        0 => heap.shaped_get(o.as_object()?, w1, (w0 & INDEX_MASK) - 1),
        KIND_ADD => None,
        depth => {
            if w2 != heap.proto_epoch() {
                return None;
            }
            let proto = proto_start(heap, o, key, w3(heap))?;
            if proto.offset() != w1 {
                return None;
            }
            let mut h = proto;
            for _ in 1..depth {
                h = heap.get_prototype_of(h)?;
            }
            heap.data_at(h, (w0 & INDEX_MASK) - 1, key)
        }
    }
}

/// The index of the first word of `o[key]`'s slot in the megamorphic cache: by the key,
/// the receiver's shape word (its own offset for a dictionary-mode object) and its
/// prototype (a primitive's realm prototype).
#[inline(always)]
fn mega_at(heap: &Heap, o: Value, key: Atom) -> Option<usize> {
    let w = match o.as_object() {
        Some(obj) => {
            let shape = match heap.receiver_shape(obj) {
                0 => obj.offset(),
                shape => shape,
            };
            shape ^ heap.get_prototype_of(obj).map_or(0, |p| p.offset())
        }
        None => realm::primitive_proto(heap, o)?.offset(),
    };
    let h = crate::heap::object::key_hash(key) ^ w >> 3;
    Some(((h & (MEGA_SLOTS - 1)) * WORDS) as usize)
}

/// A get site's megamorphic cache hit: `o[key]` if its slot there applies.
#[inline(never)]
pub(crate) fn mega_get(heap: &Heap, o: Value, key: Atom) -> Option<Value> {
    let i = mega_at(heap, o, key)?;
    let s = heap.mega_ic.get(i..i + WORDS as usize)?;
    if s[MEGA_KEY] != key.0 {
        return None;
    }
    get_hit(heap, [s[0], s[1], s[2]], |_| s[3], o, key)
}

/// A put or define site's cache hit: true if the slot at `at` (0: none) still applies to
/// `obj[key] = v` and `v` was stored (an own writable data property, or a cached
/// addition).
#[inline(always)]
pub(crate) fn cached_put(heap: &mut Heap, at: u32, obj: Obj, key: Atom, v: Value) -> bool {
    if at == 0 {
        return false;
    }
    let [w0, w1, w2] = head(heap, at);
    if w0 & IC_OFFSET_SLOT == 0 {
        return w0 != 0 && heap.set_data_at(obj, w0 - 1, key, v);
    }
    match kind(w0) {
        0 => w2 & 1 != 0 && heap.shaped_set(obj, w1, (w0 & INDEX_MASK) - 1, v),
        KIND_ADD => cached_add(heap, at, w0, obj, v),
        _ => false,
    }
}

/// A `define_field` site's cache hit (its slots are additions without [`ADD_SET`]).
#[inline(always)]
fn cached_define(heap: &mut Heap, at: u32, obj: Obj, v: Value) -> bool {
    let w0 = word(heap, at, 0);
    w0 & (IC_OFFSET_SLOT | KIND_MASK << KIND_SHIFT) == IC_OFFSET_SLOT | KIND_ADD << KIND_SHIFT
        && cached_add(heap, at, w0, obj, v)
}

/// The addition half of [`cached_put`], out of line: it may allocate, and keeping it out
/// of the hot own-property path keeps that path small.
#[inline(never)]
fn cached_add(heap: &mut Heap, at: u32, w0: u32, obj: Obj, v: Value) -> bool {
    if w0 & ADD_SET != 0
        && (word(heap, at, 4) != heap.proto_epoch()
            || heap.get_prototype_of(obj).map_or(0, |p| p.offset()) != word(heap, at, 3))
    {
        return false;
    }
    let (old, new) = (word(heap, at, 1), word(heap, at, 2));
    heap.shaped_add(obj, old, new, v)
}

impl Exec<'_> {
    /// The first words of cache slot `ic` of the current function (zeros if empty), for
    /// the global caches.
    #[inline(always)]
    fn ic_read(&self, ic: u32) -> [u32; 2] {
        match slot_at(self.heap, self.template, ic) {
            0 => [0; 2],
            at => [word(self.heap, at, 0), word(self.heap, at, 1)],
        }
    }

    /// Fills cache slot `ic`, allocating the function's cache array on first use. Out of
    /// memory just leaves the slot empty.
    fn ic_write(&mut self, ic: u32, words: Slot) {
        if ic == NO_CACHE as u32 {
            return;
        }
        let count = self.heap.arena().get(self.template, FuncTemplate::IC_COUNT) as u32;
        if ic >= count {
            return;
        }
        let a = match self
            .heap
            .arena()
            .get::<Ref>(self.template, FuncTemplate::IC)
            .get()
        {
            Some(a) => a,
            None => {
                let Ok(a) = self.heap.alloc_u32_array(count * WORDS) else {
                    return;
                };
                self.heap
                    .arena_mut()
                    .set(self.template, FuncTemplate::IC, Ref(a));
                a
            }
        };
        for (k, w) in words.into_iter().enumerate() {
            let at = U32Array::DATA.at(a, ic * WORDS + k as u32);
            self.heap.arena_mut().write(at, w);
        }
    }

    /// Fills get slot `ic` for `o[key]` with `words`. A slot that held something already
    /// missed for another receiver: the site is polymorphic, and the megamorphic cache
    /// gets the slot too.
    fn ic_get_write(&mut self, o: Value, key: Atom, ic: u32, words: Slot) {
        let at = slot_at(self.heap, self.template, ic);
        if at != 0
            && word(self.heap, at, 0) != 0
            && let Some(i) = mega_at(self.heap, o, key)
        {
            self.mega_write(i, key, words);
        }
        self.ic_write(ic, words);
    }

    fn mega_write(&mut self, i: usize, key: Atom, words: Slot) {
        let mega = &mut self.heap.mega_ic;
        if mega.is_empty() {
            mega.resize((MEGA_SLOTS * WORDS) as usize, 0);
        }
        mega[i..i + WORDS as usize].copy_from_slice(&words);
        mega[i + MEGA_KEY] = key.0;
    }

    #[inline(never)]
    pub(super) fn mega_fill(&mut self, o: Value, key: Atom, holder: Obj) {
        let words = if Value::object(holder) == o {
            self.own_words(holder, key)
        } else {
            self.proto_words(o, key, holder)
        };
        if let Some(words) = words
            && let Some(i) = mega_at(self.heap, o, key)
        {
            self.mega_write(i, key, words);
        }
    }

    fn lex_epoch(&self) -> u32 {
        intrinsic(self.heap, Intrinsic::GlobalLexEpoch)
            .as_int()
            .unwrap_or(0) as u32
    }

    /// A cached global read: the value of data property `key` of the global lexical
    /// record or the global object, if slot `ic` still points at it. An uninitialized
    /// lexical binding is a miss (the slow path throws).
    #[inline(never)]
    pub(super) fn ic_global_get(&self, key: Atom, ic: u32) -> Option<Value> {
        let [w0, w1] = self.ic_read(ic);
        if w0 == 0 {
            return None;
        }
        let i = (w0 & !IN_LEX) - 1;
        if w0 & IN_LEX != 0 {
            let v = self.heap.data_at(realm::global_lex(self.heap), i, key)?;
            return (!v.is_uninitialized()).then_some(v);
        }
        if w1 != self.lex_epoch() {
            return None;
        }
        self.heap
            .data_at(intrinsic_obj(self.heap, Intrinsic::Global), i, key)
    }

    /// A cached global store: true if slot `ic` still points at the writable data
    /// property `key` (an initialized lexical binding, or a global object property) and
    /// `v` was written.
    #[inline(never)]
    pub(super) fn ic_global_put(&mut self, key: Atom, ic: u32, v: Value) -> bool {
        let [w0, w1] = self.ic_read(ic);
        if w0 == 0 {
            return false;
        }
        let i = (w0 & !IN_LEX) - 1;
        if w0 & IN_LEX != 0 {
            let lex = realm::global_lex(self.heap);
            return match self.heap.data_at(lex, i, key) {
                Some(old) if !old.is_uninitialized() => self.heap.set_data_at(lex, i, key, v),
                _ => false,
            };
        }
        if w1 != self.lex_epoch() {
            return false;
        }
        let g = intrinsic_obj(self.heap, Intrinsic::Global);
        self.heap.set_data_at(g, i, key, v)
    }

    /// After a slow global access found `key`: remembers where (an own data property of
    /// the lexical record or of the global object; anything else is not cached).
    #[inline(never)]
    pub(super) fn ic_global_fill(&mut self, key: Atom, ic: u32) {
        let lex = realm::global_lex(self.heap);
        if let Some(i) = self.heap.own_entry_index(lex, key) {
            if self.heap.data_at(lex, i, key).is_some() {
                self.ic_write(ic, [(i + 1) | IN_LEX, 0, 0, 0, 0]);
            }
            return;
        }
        let g = intrinsic_obj(self.heap, Intrinsic::Global);
        if let Some(i) = self.heap.own_entry_index(g, key)
            && self.heap.data_at(g, i, key).is_some()
        {
            let epoch = self.lex_epoch();
            self.ic_write(ic, [i + 1, epoch, 0, 0, 0]);
        }
    }

    /// A cached read: `o[key]` if slot `ic` applies.
    #[inline(always)]
    pub(super) fn ic_get(&self, o: Value, key: Atom, ic: u32) -> Option<Value> {
        cached_get(self.heap, slot_at(self.heap, self.template, ic), o, key)
    }

    /// A cached `define_field` (CreateDataProperty of a new key): true if slot `ic`
    /// applies and `v` was stored. A define site only ever holds an addition
    /// (`ic_add_fill` with `set` false), so this skips `cached_put`'s other kinds.
    #[inline(always)]
    pub(super) fn ic_define(&mut self, obj: Obj, ic: u32, v: Value) -> bool {
        let at = slot_at(self.heap, self.template, ic);
        at != 0 && cached_define(self.heap, at, obj, v)
    }

    /// A cached store: true if slot `ic` applies to `obj[key] = v` and `v` was stored.
    #[inline(always)]
    pub(super) fn ic_put(&mut self, obj: Obj, key: Atom, ic: u32, v: Value) -> bool {
        let at = slot_at(self.heap, self.template, ic);
        cached_put(self.heap, at, obj, key, v)
    }

    /// After a slow lookup found data property `key` on `holder`, on the prototype chain
    /// of `o`: remembers it (D1621), with the receiver's shape word (0083).
    #[inline(never)]
    pub(super) fn ic_proto_fill(&mut self, o: Value, key: Atom, holder: Obj, ic: u32) {
        if let Some(words) = self.proto_words(o, key, holder) {
            self.ic_get_write(o, key, ic, words);
        }
    }

    fn proto_words(&mut self, o: Value, key: Atom, holder: Obj) -> Option<Slot> {
        if key.as_int().is_some() {
            return None;
        }
        let proto = proto_start(self.heap, o, key, 0)?;
        let mut depth = 1;
        let mut h = proto;
        while h != holder {
            match self.heap.get_prototype_of(h) {
                Some(p) if depth < MAX_DEPTH => {
                    h = p;
                    depth += 1;
                }
                _ => return None,
            }
        }
        let i = self.heap.own_entry_index(holder, key)?;
        if i + 1 > INDEX_MASK || self.heap.data_at(holder, i, key).is_none() {
            return None;
        }
        let mut h = proto;
        loop {
            self.heap.mark_proto(h);
            if h == holder {
                break;
            }
            h = self.heap.get_prototype_of(h).expect("walked above");
        }
        let w0 = IC_OFFSET_SLOT | depth << KIND_SHIFT | (i + 1);
        let epoch = self.heap.proto_epoch();
        let shape = o.as_object().map_or(0, |obj| self.heap.receiver_shape(obj));
        Some([w0, proto.offset(), epoch, shape, 0])
    }

    pub(super) fn iterator_method(&mut self, o: Value) -> Option<Value> {
        let key = crate::heap::atoms::SYM_ITERATOR;
        let [w0, w1, w2, w3, _] = self.heap.iterator_ic;
        if let Some(v) = get_hit(self.heap, [w0, w1, w2], |_| w3, o, key) {
            return Some(v);
        }
        let start = match o.as_object() {
            Some(obj) => obj,
            None => realm::primitive_proto(self.heap, o)?,
        };
        let (holder, Property::Data { value, .. }) = self.heap.find_property(start, key)? else {
            return None;
        };
        if let Some(words) = self.proto_words(o, key, holder) {
            self.heap.iterator_ic = words;
        }
        Some(value)
    }

    pub(super) fn lacks_return(&mut self, it: Obj) -> bool {
        let key = crate::heap::atoms::RETURN;
        if self.heap.has_own_property(it, key) {
            return false;
        }
        let proto = self.heap.get_prototype_of(it);
        let p = proto.map_or(0, |p| p.offset());
        let epoch = self.heap.proto_epoch();
        if p != 0 && self.heap.return_ic == [p, epoch] {
            return true;
        }
        if !self.chain_lacks(proto, key) {
            return false;
        }
        let mut q = proto;
        while let Some(h) = q {
            self.heap.mark_proto(h);
            q = self.heap.get_prototype_of(h);
        }
        self.heap.return_ic = [p, self.heap.proto_epoch()];
        true
    }

    /// After a slow access to `obj[key]`: remembers the property if `key` is an own data
    /// property of `obj` (and not an array index).
    #[inline(never)]
    pub(super) fn ic_own_fill(&mut self, obj: Obj, key: Atom, ic: u32) {
        if let Some(words) = self.own_words(obj, key) {
            self.ic_get_write(Value::object(obj), key, ic, words);
        }
    }

    fn own_words(&self, obj: Obj, key: Atom) -> Option<Slot> {
        if key.as_int().is_some() {
            return None;
        }
        if let Some((shape, slot, writable)) = self.heap.shape_slot(obj, key) {
            let w0 = IC_OFFSET_SLOT | (slot + 1);
            return (slot < INDEX_MASK).then_some([w0, shape, writable as u32, 0, 0]);
        }
        let i = self.heap.own_entry_index(obj, key)?;
        self.heap.data_at(obj, i, key)?;
        Some([i + 1, 0, 0, 0, 0])
    }

    /// Whether no object from `proto` up has `key`, a proxy or a typed array (which
    /// could intercept it).
    fn chain_lacks(&self, proto: Option<Obj>, key: Atom) -> bool {
        let mut p = proto;
        while let Some(q) = p {
            if matches!(
                self.heap.object_class(q),
                ObjectClass::Proxy | ObjectClass::TypedArray
            ) || self.heap.has_own_property(q, key)
            {
                return false;
            }
            p = self.heap.get_prototype_of(q);
        }
        true
    }

    /// Whether `obj[key] = v` ([[Set]] with `obj` as the receiver) is just
    /// CreateDataProperty: `obj` is extensible and has no own `key`, nothing on its
    /// prototype chain has `key` or could intercept it, and `key` is not one an exotic
    /// object handles itself. The case an addition cache repeats (0083).
    pub(super) fn plain_addition(&self, obj: Obj, key: Atom) -> bool {
        key.as_int().is_none()
            && !exotic_own(self.heap, obj, key)
            && self.heap.canonical_numeric_index(key).is_none()
            && self.heap.is_extensible(obj)
            && !self.heap.has_own_property(obj, key)
            && self.chain_lacks(self.heap.get_prototype_of(obj), key)
    }

    /// After a slow `put_field` (`set`) or `define_field` that may have added `key` to
    /// `obj`, whose receiver shape word was `old` before: caches the addition (0083). A
    /// [[Set]] addition is only cached when no object of the prototype chain has `key`
    /// and none is a proxy or a typed array; the chain is then marked `OBJ_IS_PROTO`, so
    /// that a change to it bumps the epoch.
    #[inline(never)]
    pub(super) fn ic_add_fill(&mut self, obj: Obj, key: Atom, old: u32, set: bool, ic: u32) {
        if key.as_int().is_some()
            || exotic_own(self.heap, obj, key)
            || self.heap.canonical_numeric_index(key).is_some()
        {
            return;
        }
        let Some(new) = self.heap.added_shape(obj, key, old) else {
            return;
        };
        if !set {
            let w0 = IC_OFFSET_SLOT | KIND_ADD << KIND_SHIFT;
            self.ic_write(ic, [w0, old, new, 0, 0]);
            return;
        }
        let proto = self.heap.get_prototype_of(obj);
        if !self.chain_lacks(proto, key) {
            return;
        }
        let mut p = proto;
        while let Some(q) = p {
            self.heap.mark_proto(q);
            p = self.heap.get_prototype_of(q);
        }
        let w0 = IC_OFFSET_SLOT | KIND_ADD << KIND_SHIFT | ADD_SET;
        let epoch = self.heap.proto_epoch();
        let p = proto.map_or(0, |p| p.offset());
        self.ic_write(ic, [w0, old, new, p, epoch]);
    }

    /// `fast_step` for `define_field` (object literals) when the site's addition cache
    /// applies (0083).
    #[cfg(test)]
    #[inline(never)]
    pub(super) fn fast_define(&mut self, sp0: u32) -> bool {
        let ic = self.imm_u16(5);
        let (o, v) = (self.top(1), self.top(0));
        let Some(obj) = o.as_object() else {
            return false;
        };
        if !self.ic_define(obj, ic, v) {
            return false;
        }
        // `obj v -> obj`
        self.sp -= 1;
        self.advance(Op::DefineField, sp0);
        true
    }

    /// `fast_step` for `get_field`, `get_method` and `put_field` when the site's cache
    /// applies; false, with nothing changed, otherwise.
    #[cfg(test)]
    #[inline(always)]
    pub(super) fn fast_field(&mut self, sp0: u32) -> bool {
        let op = self.byte(self.pc);
        let key = Atom::from_id(self.imm_u32(1));
        let ic = self.imm_u16(5);
        if op == fast::PUT_FIELD {
            let (o, v) = (self.top(1), self.top(0));
            let Some(obj) = o.as_object() else {
                return false;
            };
            if !self.ic_put(obj, key, ic, v) {
                return false;
            }
            self.sp -= 2;
            self.advance(Op::PutField, sp0);
            return true;
        }
        let o = self.top(0);
        if key == crate::heap::atoms::LENGTH && op == fast::GET_FIELD {
            // `a.length` of an Array (a non-configurable own data property, its slot 0)
            // and of a string primitive have no table entry for the IC (0086).
            let len = match (o.as_object(), o.as_string()) {
                (Some(a), _) if self.heap.object_class(a) == ObjectClass::Array => {
                    Some(self.heap.object_slot(a, 0))
                }
                (_, Some(s)) => i32::try_from(self.heap.str_len(s)).ok().map(Value::int),
                _ => None,
            };
            if let Some(v) = len {
                self.set(self.sp - 1, v);
                self.advance(Op::GetField, sp0);
                return true;
            }
        }
        let Some(v) = self.ic_get(o, key, ic) else {
            return false;
        };
        if op == fast::GET_FIELD {
            self.set(self.sp - 1, v);
            self.advance(Op::GetField, sp0);
        } else {
            self.set(self.sp - 1, v);
            self.push(o);
            self.advance(Op::GetMethod, sp0);
        }
        true
    }
}

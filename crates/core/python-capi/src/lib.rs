#![allow(non_snake_case, non_upper_case_globals, clippy::missing_safety_doc)]

use std::{
    alloc::{Layout, alloc, dealloc},
    cell::UnsafeCell,
    ffi::{CStr, c_char, c_int, c_ulong, c_void},
    fmt,
    mem::{align_of, size_of},
    ptr::{self, NonNull, null, null_mut},
    slice,
    sync::atomic::{AtomicIsize, Ordering},
};

#[repr(C)]
pub struct Object {
    refcnt: isize,
    ty: *const Type,
}

#[repr(C)]
struct VarObject {
    base: Object,
    size: isize,
}

type Destructor = Option<unsafe extern "C" fn(*mut Object)>;

#[repr(C)]
pub struct Type {
    base: VarObject,
    name: *const c_char,
    basicsize: isize,
    itemsize: isize,
    dealloc: Destructor,
    before_flags: [*const c_void; 14],
    flags: c_ulong,
    after_flags: [*const c_void; 26],
    version_tag: u32,
    finalize: *const c_void,
    vectorcall: *const c_void,
    watched: u8,
    versions_used: u16,
}

#[repr(transparent)]
pub struct Shared<T>(UnsafeCell<T>);

unsafe impl<T> Sync for Shared<T> {}

impl<T> Shared<T> {
    const fn get(&self) -> *mut T {
        self.0.get()
    }
}

const IMMORTAL: isize = if size_of::<isize>() > 4 {
    u32::MAX as isize
} else {
    (u32::MAX >> 2) as isize
};
const UNICODE_SUBCLASS: c_ulong = 1 << 28;
const TYPE_SUBCLASS: c_ulong = 1 << 31;

const fn type_object(
    name: &'static CStr,
    basicsize: usize,
    flags: c_ulong,
    dealloc: Destructor,
) -> Type {
    Type {
        base: VarObject {
            base: Object {
                refcnt: IMMORTAL,
                ty: ptr::addr_of!(PyType_Type).cast(),
            },
            size: 0,
        },
        name: name.as_ptr(),
        basicsize: basicsize as isize,
        itemsize: 0,
        dealloc,
        before_flags: [null(); 14],
        flags,
        after_flags: [null(); 26],
        version_tag: 0,
        finalize: null(),
        vectorcall: null(),
        watched: 0,
        versions_used: 0,
    }
}

#[unsafe(no_mangle)]
pub static PyType_Type: Shared<Type> = Shared(UnsafeCell::new(type_object(
    c"type",
    size_of::<Type>(),
    TYPE_SUBCLASS,
    None,
)));

#[unsafe(no_mangle)]
pub static PyUnicode_Type: Shared<Type> = Shared(UnsafeCell::new(type_object(
    c"str",
    size_of::<Compact>(),
    UNICODE_SUBCLASS,
    Some(unicode_dealloc),
)));

#[unsafe(no_mangle)]
pub static PyModuleDef_Type: Shared<Type> = Shared(UnsafeCell::new(type_object(
    c"moduledef",
    size_of::<ModuleDef>(),
    0,
    None,
)));

#[repr(C)]
struct Ascii {
    base: Object,
    length: isize,
    hash: isize,
    state: u32,
}

#[repr(C)]
struct Compact {
    base: Ascii,
    utf8_length: isize,
    utf8: *mut c_char,
}

#[repr(C)]
struct Legacy {
    base: Compact,
    data: *mut c_void,
}

const KIND_SHIFT: u32 = 2;
const KIND_MASK: u32 = 7 << KIND_SHIFT;
const COMPACT: u32 = 1 << 5;
const ASCII: u32 = 1 << 6;
const MAX_UNICODE: u32 = 0x10_ffff;

const fn header(ascii: bool) -> usize {
    if ascii {
        size_of::<Ascii>()
    } else {
        size_of::<Compact>()
    }
}

fn unicode_layout(length: usize, kind: usize, ascii: bool) -> Option<Layout> {
    let size = length
        .checked_add(1)?
        .checked_mul(kind)?
        .checked_add(header(ascii))?;
    Layout::from_size_align(size, align_of::<Compact>()).ok()
}

fn kind_of(max: u32) -> (usize, bool) {
    if max <= u32::from(u8::MAX >> 1) {
        (size_of::<u8>(), true)
    } else if max <= u32::from(u8::MAX) {
        (size_of::<u8>(), false)
    } else if max <= u32::from(u16::MAX) {
        (size_of::<u16>(), false)
    } else {
        (size_of::<u32>(), false)
    }
}

unsafe extern "C" fn unicode_dealloc(op: *mut Object) {
    let ascii = op.cast::<Ascii>();
    let (length, state) = unsafe { ((*ascii).length, (*ascii).state) };
    if state & COMPACT == 0 {
        return;
    }
    let kind = ((state & KIND_MASK) >> KIND_SHIFT) as usize;
    if let Some(layout) = unicode_layout(length.unsigned_abs(), kind, state & ASCII != 0) {
        unsafe { dealloc(op.cast(), layout) };
    }
}

fn new_unicode(length: usize, max: u32) -> *mut Object {
    if max > MAX_UNICODE || isize::try_from(length).is_err() {
        return null_mut();
    }
    let (kind, ascii) = kind_of(max);
    let Some(layout) = unicode_layout(length, kind, ascii) else {
        return null_mut();
    };
    let op = unsafe { alloc(layout) };
    if op.is_null() {
        return null_mut();
    }
    let state = ((kind as u32) << KIND_SHIFT) | COMPACT | if ascii { ASCII } else { 0 };
    unsafe {
        op.cast::<Ascii>().write(Ascii {
            base: Object {
                refcnt: 1,
                ty: PyUnicode_Type.get(),
            },
            length: length as isize,
            hash: -1,
            state,
        });
        if !ascii {
            let compact = op.cast::<Compact>();
            (*compact).utf8_length = 0;
            (*compact).utf8 = null_mut();
        }
        ptr::write_bytes(op.add(header(ascii) + length * kind), 0, kind);
    }
    op.cast()
}

#[unsafe(no_mangle)]
pub extern "C" fn PyUnicode_New(size: isize, maxchar: u32) -> *mut Object {
    usize::try_from(size).map_or(null_mut(), |length| new_unicode(length, maxchar))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _Py_Dealloc(op: *mut Object) {
    unsafe {
        if let Some(dealloc) = (*(*op).ty).dealloc {
            dealloc(op);
        }
    }
}

#[repr(C)]
struct ModuleDefBase {
    base: Object,
    init: *const c_void,
    index: isize,
    copy: *mut Object,
}

#[repr(C)]
struct MethodDef {
    name: *const c_char,
    meth: Option<unsafe extern "C" fn()>,
    flags: c_int,
    doc: *const c_char,
}

#[repr(C)]
struct Slot {
    slot: c_int,
    value: *mut c_void,
}

#[repr(C)]
pub struct ModuleDef {
    base: ModuleDefBase,
    name: *const c_char,
    doc: *const c_char,
    size: isize,
    methods: *const MethodDef,
    slots: *const Slot,
    traverse: *const c_void,
    clear: *const c_void,
    free: *const c_void,
}

static MODULES: AtomicIsize = AtomicIsize::new(0);

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyModuleDef_Init(def: *mut ModuleDef) -> *mut Object {
    unsafe {
        if (*def).base.index == 0 {
            (*def).base.base.ty = PyModuleDef_Type.get();
            (*def).base.base.refcnt = 1;
            (*def).base.index = MODULES.fetch_add(1, Ordering::Relaxed) + 1;
        }
    }
    def.cast()
}

const MOD_MULTIPLE_INTERPRETERS: c_int = 3;
const MOD_GIL: c_int = 4;
const METH_VARARGS: c_int = 0x1;
const METH_KEYWORDS: c_int = 0x2;
const METH_NOARGS: c_int = 0x4;
const METH_O: c_int = 0x8;
const METH_FASTCALL: c_int = 0x80;
const METH_METHOD: c_int = 0x200;
const METH_CALLS: c_int =
    METH_VARARGS | METH_KEYWORDS | METH_NOARGS | METH_O | METH_FASTCALL | METH_METHOD;

pub struct Extension {
    pub name: &'static str,
    init: unsafe extern "C" fn() -> *mut Object,
}

include!(concat!(env!("OUT_DIR"), "/extensions.rs"));

#[derive(Debug, Clone, Copy)]
pub enum Refused {
    Failed,
    SinglePhase,
    State(isize),
    Slot(c_int),
    Name,
}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Failed => f.write_str("its init function failed"),
            Self::SinglePhase => {
                f.write_str("it uses single-phase init, which the C API does not run yet")
            }
            Self::State(size) => write!(
                f,
                "it keeps {size} bytes of module state, which the C API does not hold yet"
            ),
            Self::Slot(slot) => write!(
                f,
                "it has module slot {slot}, which the C API does not run yet"
            ),
            Self::Name => f.write_str("a method name is not UTF-8"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    One,
    None,
    Other(c_int),
}

#[derive(Clone, Copy)]
pub struct Method {
    pub name: &'static str,
    pub doc: Option<&'static str>,
    pub kind: Kind,
    address: unsafe extern "C" fn(),
}

fn text(pointer: *const c_char) -> Option<&'static str> {
    if pointer.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(pointer) }.to_str().ok()
}

impl Extension {
    pub fn methods(&self) -> Result<Vec<Method>, Refused> {
        let object = unsafe { (self.init)() };
        if object.is_null() {
            return Err(Refused::Failed);
        }
        if unsafe { (*object).ty } != PyModuleDef_Type.get().cast_const() {
            return Err(Refused::SinglePhase);
        }
        let def = unsafe { &*object.cast::<ModuleDef>() };
        if def.size > 0 {
            return Err(Refused::State(def.size));
        }
        let mut slot = def.slots;
        while !slot.is_null() && unsafe { (*slot).slot } != 0 {
            match unsafe { (*slot).slot } {
                MOD_MULTIPLE_INTERPRETERS | MOD_GIL => {}
                other => return Err(Refused::Slot(other)),
            }
            slot = unsafe { slot.add(1) };
        }
        let mut methods = Vec::new();
        let mut entry = def.methods;
        while !entry.is_null() && !unsafe { (*entry).name }.is_null() {
            let method = unsafe { &*entry };
            let name = text(method.name).ok_or(Refused::Name)?;
            if let Some(address) = method.meth {
                let kind = match method.flags & METH_CALLS {
                    METH_O => Kind::One,
                    METH_NOARGS => Kind::None,
                    other => Kind::Other(other),
                };
                methods.push(Method {
                    name,
                    doc: text(method.doc),
                    kind,
                    address,
                });
            }
            entry = unsafe { entry.add(1) };
        }
        Ok(methods)
    }
}

impl Method {
    pub fn call(&self, arg: Option<&Owned>) -> Option<Owned> {
        let function = unsafe {
            std::mem::transmute::<
                unsafe extern "C" fn(),
                unsafe extern "C" fn(*mut Object, *mut Object) -> *mut Object,
            >(self.address)
        };
        let arg = match (self.kind, arg) {
            (Kind::One, Some(arg)) => arg.0.as_ptr(),
            (Kind::None, None) => null_mut(),
            _ => return None,
        };
        NonNull::new(unsafe { function(null_mut(), arg) }).map(Owned)
    }
}

pub struct Owned(NonNull<Object>);

impl Drop for Owned {
    fn drop(&mut self) {
        let op = self.0.as_ptr();
        unsafe {
            let immortal = if size_of::<isize>() > 4 {
                ((*op).refcnt as i32) < 0
            } else {
                (*op).refcnt == IMMORTAL
            };
            if immortal {
                return;
            }
            (*op).refcnt -= 1;
            if (*op).refcnt == 0 {
                _Py_Dealloc(op);
            }
        }
    }
}

pub enum Units<'a> {
    Latin1(&'a [u8]),
    Ucs2(&'a [u16]),
    Ucs4(&'a [u32]),
}

impl Owned {
    pub fn ascii(bytes: &[u8]) -> Option<Self> {
        let op = NonNull::new(new_unicode(
            bytes.len(),
            u32::from(bytes.iter().copied().max().unwrap_or(0)),
        ))?;
        let ascii = unsafe { (*op.as_ptr().cast::<Ascii>()).state } & ASCII != 0;
        unsafe {
            ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                op.as_ptr().cast::<u8>().add(header(ascii)),
                bytes.len(),
            )
        };
        Some(Self(op))
    }

    pub fn str<I: Iterator<Item = u32> + Clone>(points: I) -> Option<Self> {
        let (length, max) = points.clone().fold((0usize, 0u32), |(length, max), point| {
            (length + 1, max.max(point))
        });
        let op = NonNull::new(new_unicode(length, max))?;
        let (kind, ascii) = kind_of(max);
        let data = unsafe { op.as_ptr().cast::<u8>().add(header(ascii)) };
        for (index, point) in points.enumerate() {
            unsafe {
                match kind {
                    1 => data.add(index).write(point as u8),
                    2 => data.cast::<u16>().add(index).write(point as u16),
                    _ => data.cast::<u32>().add(index).write(point),
                }
            }
        }
        Some(Self(op))
    }

    pub fn is(&self, other: &Self) -> bool {
        self.0 == other.0
    }

    pub fn units(&self) -> Option<Units<'_>> {
        let op = self.0.as_ptr();
        let ty = unsafe { (*op).ty };
        if ty.is_null() || unsafe { (*ty).flags } & UNICODE_SUBCLASS == 0 {
            return None;
        }
        let ascii = op.cast::<Ascii>();
        let (length, state) = unsafe { ((*ascii).length.unsigned_abs(), (*ascii).state) };
        let data = if state & COMPACT != 0 {
            unsafe { op.cast::<u8>().add(header(state & ASCII != 0)) }
        } else {
            unsafe { (*op.cast::<Legacy>()).data.cast::<u8>() }
        };
        if data.is_null() {
            return None;
        }
        Some(match ((state & KIND_MASK) >> KIND_SHIFT) as usize {
            1 => Units::Latin1(unsafe { slice::from_raw_parts(data, length) }),
            2 => Units::Ucs2(unsafe { slice::from_raw_parts(data.cast(), length) }),
            4 => Units::Ucs4(unsafe { slice::from_raw_parts(data.cast(), length) }),
            _ => return None,
        })
    }
}

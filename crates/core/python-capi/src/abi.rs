use std::{
    cell::UnsafeCell,
    ffi::{c_char, c_int, c_uint, c_ulong, c_void},
    mem::size_of,
};

#[repr(C)]
pub struct Object {
    pub refcnt: isize,
    pub ty: *mut Type,
}

#[repr(C)]
pub struct VarObject {
    pub base: Object,
    pub size: isize,
}

pub type Destructor = Option<unsafe extern "C" fn(*mut Object)>;
pub type Unary = Option<unsafe extern "C" fn(*mut Object) -> *mut Object>;
pub type Binary = Option<unsafe extern "C" fn(*mut Object, *mut Object) -> *mut Object>;
pub type Ternary =
    Option<unsafe extern "C" fn(*mut Object, *mut Object, *mut Object) -> *mut Object>;
pub type Length = Option<unsafe extern "C" fn(*mut Object) -> isize>;
pub type Item = Option<unsafe extern "C" fn(*mut Object, isize) -> *mut Object>;
pub type Store = Option<unsafe extern "C" fn(*mut Object, *mut Object, *mut Object) -> c_int>;
pub type Contains = Option<unsafe extern "C" fn(*mut Object, *mut Object) -> c_int>;
pub type Compare = Option<unsafe extern "C" fn(*mut Object, *mut Object, c_int) -> *mut Object>;
pub type Initializer = Option<unsafe extern "C" fn(*mut Object, *mut Object, *mut Object) -> c_int>;
pub type Allocator = Option<unsafe extern "C" fn(*mut Type, isize) -> *mut Object>;
pub type Constructor =
    Option<unsafe extern "C" fn(*mut Type, *mut Object, *mut Object) -> *mut Object>;
pub type Release = Option<unsafe extern "C" fn(*mut c_void)>;
pub type Getter = Option<unsafe extern "C" fn(*mut Object, *mut c_void) -> *mut Object>;
pub type Setter = Option<unsafe extern "C" fn(*mut Object, *mut Object, *mut c_void) -> c_int>;
pub type Opaque = *const c_void;

#[repr(C)]
pub struct Type {
    pub base: VarObject,
    pub name: *const c_char,
    pub basicsize: isize,
    pub itemsize: isize,
    pub dealloc: Destructor,
    pub vectorcall_offset: isize,
    pub getattr: Opaque,
    pub setattr: Opaque,
    pub as_async: Opaque,
    pub repr: Unary,
    pub as_number: *mut NumberMethods,
    pub as_sequence: *mut SequenceMethods,
    pub as_mapping: *mut MappingMethods,
    pub hash: Length,
    pub call: Ternary,
    pub str: Unary,
    pub getattro: Binary,
    pub setattro: Store,
    pub as_buffer: Opaque,
    pub flags: c_ulong,
    pub doc: *const c_char,
    pub traverse: Opaque,
    pub clear: Opaque,
    pub richcompare: Compare,
    pub weaklistoffset: isize,
    pub iter: Unary,
    pub iternext: Unary,
    pub methods: *mut MethodDef,
    pub members: *mut MemberDef,
    pub getset: *mut GetSetDef,
    pub base_type: *mut Self,
    pub dict: *mut Object,
    pub descr_get: Opaque,
    pub descr_set: Opaque,
    pub dictoffset: isize,
    pub init: Initializer,
    pub alloc: Allocator,
    pub new: Constructor,
    pub free: Release,
    pub is_gc: Opaque,
    pub bases: *mut Object,
    pub mro: *mut Object,
    pub cache: *mut Object,
    pub subclasses: *mut c_void,
    pub weaklist: *mut Object,
    pub del: Destructor,
    pub version_tag: c_uint,
    pub finalize: Destructor,
    pub vectorcall: Opaque,
    pub watched: u8,
    pub versions_used: u16,
}

#[repr(C)]
pub struct NumberMethods {
    pub arithmetic: [Opaque; 9],
    pub bool: Option<unsafe extern "C" fn(*mut Object) -> c_int>,
}

#[repr(C)]
pub struct SequenceMethods {
    pub length: Length,
    pub concat: Binary,
    pub repeat: Item,
    pub item: Item,
    pub was_slice: Opaque,
    pub ass_item: Option<unsafe extern "C" fn(*mut Object, isize, *mut Object) -> c_int>,
    pub was_ass_slice: Opaque,
    pub contains: Contains,
    pub inplace_concat: Binary,
    pub inplace_repeat: Item,
}

#[repr(C)]
pub struct MappingMethods {
    pub length: Length,
    pub subscript: Binary,
    pub ass_subscript: Store,
}

#[repr(C)]
pub struct MethodDef {
    pub name: *const c_char,
    pub meth: Option<unsafe extern "C" fn()>,
    pub flags: c_int,
    pub doc: *const c_char,
}

#[repr(C)]
pub struct MemberDef {
    pub name: *const c_char,
    pub kind: c_int,
    pub offset: isize,
    pub flags: c_int,
    pub doc: *const c_char,
}

#[repr(C)]
pub struct GetSetDef {
    pub name: *const c_char,
    pub get: Getter,
    pub set: Setter,
    pub doc: *const c_char,
    pub closure: *mut c_void,
}

#[repr(C)]
pub struct ModuleDefBase {
    pub base: Object,
    pub init: Opaque,
    pub index: isize,
    pub copy: *mut Object,
}

#[repr(C)]
pub struct Slot {
    pub slot: c_int,
    pub value: *mut c_void,
}

#[repr(C)]
pub struct ModuleDef {
    pub base: ModuleDefBase,
    pub name: *const c_char,
    pub doc: *const c_char,
    pub size: isize,
    pub methods: *mut MethodDef,
    pub slots: *mut Slot,
    pub traverse: Opaque,
    pub clear: Opaque,
    pub free: Release,
}

#[repr(C)]
pub struct Ascii {
    pub base: Object,
    pub length: isize,
    pub hash: isize,
    pub state: u32,
}

#[repr(C)]
pub struct Compact {
    pub base: Ascii,
    pub utf8_length: isize,
    pub utf8: *mut c_char,
}

#[repr(C)]
pub struct Long {
    pub base: Object,
    pub tag: usize,
    pub digits: [u32; 1],
}

#[repr(C)]
pub struct Float {
    pub base: Object,
    pub value: f64,
}

#[repr(C)]
pub struct Bytes {
    pub base: VarObject,
    pub hash: isize,
    pub data: [c_char; 1],
}

#[repr(C)]
pub struct Tuple {
    pub base: VarObject,
    pub items: [*mut Object; 1],
}

#[repr(C)]
pub struct List {
    pub base: VarObject,
    pub items: *mut *mut Object,
    pub allocated: isize,
}

#[repr(C)]
pub struct Dict {
    pub base: Object,
    pub used: isize,
    pub watcher_tag: u64,
    pub keys: *mut c_void,
    pub values: *mut c_void,
}

#[repr(C)]
pub struct SliceObject {
    pub base: Object,
    pub start: *mut Object,
    pub stop: *mut Object,
    pub step: *mut Object,
}

#[repr(C)]
pub struct Buffer {
    pub buf: *mut c_void,
    pub obj: *mut Object,
    pub len: isize,
    pub itemsize: isize,
    pub readonly: c_int,
    pub ndim: c_int,
    pub format: *mut c_char,
    pub shape: *mut isize,
    pub strides: *mut isize,
    pub suboffsets: *mut isize,
    pub internal: *mut c_void,
}

#[repr(transparent)]
pub struct Shared<T>(pub UnsafeCell<T>);

unsafe impl<T> Sync for Shared<T> {}

impl<T> Shared<T> {
    pub const fn get(&self) -> *mut T {
        self.0.get()
    }
}

pub const IMMORTAL: isize = if size_of::<isize>() > size_of::<u32>() {
    u32::MAX as isize
} else {
    (u32::MAX >> 2) as isize
};

pub const TPFLAGS_HEAPTYPE: c_ulong = 1 << 9;
pub const TPFLAGS_BASETYPE: c_ulong = 1 << 10;
pub const TPFLAGS_READY: c_ulong = 1 << 12;
pub const TPFLAGS_LONG_SUBCLASS: c_ulong = 1 << 24;
pub const TPFLAGS_LIST_SUBCLASS: c_ulong = 1 << 25;
pub const TPFLAGS_TUPLE_SUBCLASS: c_ulong = 1 << 26;
pub const TPFLAGS_BYTES_SUBCLASS: c_ulong = 1 << 27;
pub const TPFLAGS_UNICODE_SUBCLASS: c_ulong = 1 << 28;
pub const TPFLAGS_DICT_SUBCLASS: c_ulong = 1 << 29;
pub const TPFLAGS_BASE_EXC_SUBCLASS: c_ulong = 1 << 30;
pub const TPFLAGS_TYPE_SUBCLASS: c_ulong = 1 << 31;

pub const KIND_SHIFT: u32 = 2;
pub const KIND_MASK: u32 = 7 << KIND_SHIFT;
pub const COMPACT: u32 = 1 << 5;
pub const ASCII: u32 = 1 << 6;
pub const MAX_UNICODE: u32 = 0x10_ffff;

pub const DIGIT_BITS: u32 = 30;
pub const LONG_NON_SIZE_BITS: u32 = 3;
pub const LONG_SIGN_ZERO: usize = 1;
pub const LONG_SIGN_NEGATIVE: usize = 2;

pub const METH_VARARGS: c_int = 0x1;
pub const METH_KEYWORDS: c_int = 0x2;
pub const METH_NOARGS: c_int = 0x4;
pub const METH_O: c_int = 0x8;
pub const METH_CLASS: c_int = 0x10;
pub const METH_STATIC: c_int = 0x20;
pub const METH_FASTCALL: c_int = 0x80;
pub const METH_METHOD: c_int = 0x200;

pub const MOD_CREATE: c_int = 1;
pub const MOD_EXEC: c_int = 2;
pub const MOD_MULTIPLE_INTERPRETERS: c_int = 3;
pub const MOD_GIL: c_int = 4;

pub const T_SHORT: c_int = 0;
pub const T_INT: c_int = 1;
pub const T_LONG: c_int = 2;
pub const T_FLOAT: c_int = 3;
pub const T_DOUBLE: c_int = 4;
pub const T_STRING: c_int = 5;
pub const T_OBJECT: c_int = 6;
pub const T_CHAR: c_int = 7;
pub const T_BYTE: c_int = 8;
pub const T_UBYTE: c_int = 9;
pub const T_USHORT: c_int = 10;
pub const T_UINT: c_int = 11;
pub const T_ULONG: c_int = 12;
pub const T_BOOL: c_int = 14;
pub const T_OBJECT_EX: c_int = 16;
pub const T_LONGLONG: c_int = 17;
pub const T_ULONGLONG: c_int = 18;
pub const T_PYSSIZET: c_int = 19;
pub const READONLY: c_int = 1;

pub const LT: c_int = 0;
pub const LE: c_int = 1;
pub const EQ: c_int = 2;
pub const NE: c_int = 3;
pub const GT: c_int = 4;
pub const GE: c_int = 5;

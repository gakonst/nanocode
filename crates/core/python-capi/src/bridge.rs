use std::{
    cell::{Cell, RefCell, UnsafeCell},
    collections::{HashMap, HashSet},
    ffi::{CStr, CString, c_char, c_int, c_ulong},
    mem::{offset_of, size_of},
    ptr::{NonNull, addr_of, null, null_mut},
    slice,
    sync::atomic::{AtomicPtr, Ordering},
};

use rustpython_vm::{
    AsObject, Py, PyObject, PyObjectRef, PyPayload, PyRef, PyResult, VirtualMachine,
    builtins::{
        PyBaseExceptionRef, PyBytes, PyDict, PyFloat, PyInt, PyList, PySlice, PyStr, PyTuple,
        PyType, PyTypeRef,
    },
    class::PyClassImpl,
    common::wtf8::{CodePoint, Wtf8Buf},
    pyclass,
    types::PyTypeFlags,
};

use crate::{
    abi::*,
    heap::{self, Kind},
};

const fn type_object(name: &'static CStr, basicsize: usize, flags: c_ulong) -> Type {
    Type {
        base: VarObject {
            base: Object {
                refcnt: IMMORTAL,
                ty: addr_of!(PyType_Type).cast::<Type>().cast_mut(),
            },
            size: 0,
        },
        name: name.as_ptr(),
        basicsize: basicsize as isize,
        itemsize: 0,
        dealloc: Some(dealloc),
        vectorcall_offset: 0,
        getattr: null(),
        setattr: null(),
        as_async: null(),
        repr: None,
        as_number: null_mut(),
        as_sequence: null_mut(),
        as_mapping: null_mut(),
        hash: None,
        call: None,
        str: None,
        getattro: None,
        setattro: None,
        as_buffer: null(),
        flags: flags | TPFLAGS_READY,
        doc: null(),
        traverse: null(),
        clear: null(),
        richcompare: None,
        weaklistoffset: 0,
        iter: None,
        iternext: None,
        methods: null_mut(),
        members: null_mut(),
        getset: null_mut(),
        base_type: null_mut(),
        dict: null_mut(),
        descr_get: null(),
        descr_set: null(),
        dictoffset: 0,
        init: None,
        alloc: Some(crate::types::PyType_GenericAlloc),
        new: None,
        free: Some(heap::PyObject_Free),
        is_gc: null(),
        bases: null_mut(),
        mro: null_mut(),
        cache: null_mut(),
        subclasses: null_mut(),
        weaklist: null_mut(),
        del: None,
        version_tag: 0,
        finalize: None,
        vectorcall: null(),
        watched: 0,
        versions_used: 0,
    }
}

macro_rules! static_types {
    ($($name:ident = $text:literal, $layout:ty, $flags:expr, $field:ident;)*) => {
        $(
            #[unsafe(no_mangle)]
            pub static $name: Shared<Type> = Shared(UnsafeCell::new(type_object($text, size_of::<$layout>(), $flags)));
        )*
        fn register_types(vm: &VirtualMachine) {
            $(
                register_type($name.get(), vm.ctx.types.$field);
            )*
        }
    };
}

static_types! {
    PyType_Type = c"type", Type, TPFLAGS_TYPE_SUBCLASS | TPFLAGS_BASETYPE, type_type;
    PyBaseObject_Type = c"object", Object, TPFLAGS_BASETYPE, object_type;
    PyLong_Type = c"int", Long, TPFLAGS_LONG_SUBCLASS | TPFLAGS_BASETYPE, int_type;
    PyBool_Type = c"bool", Long, TPFLAGS_LONG_SUBCLASS, bool_type;
    PyFloat_Type = c"float", Float, TPFLAGS_BASETYPE, float_type;
    PyComplex_Type = c"complex", Object, TPFLAGS_BASETYPE, complex_type;
    PyUnicode_Type = c"str", Compact, TPFLAGS_UNICODE_SUBCLASS | TPFLAGS_BASETYPE, str_type;
    PyBytes_Type = c"bytes", Bytes, TPFLAGS_BYTES_SUBCLASS | TPFLAGS_BASETYPE, bytes_type;
    PyByteArray_Type = c"bytearray", Object, TPFLAGS_BASETYPE, bytearray_type;
    PyMemoryView_Type = c"memoryview", Object, 0, memoryview_type;
    PyTuple_Type = c"tuple", Tuple, TPFLAGS_TUPLE_SUBCLASS | TPFLAGS_BASETYPE, tuple_type;
    PyList_Type = c"list", List, TPFLAGS_LIST_SUBCLASS | TPFLAGS_BASETYPE, list_type;
    PyDict_Type = c"dict", Dict, TPFLAGS_DICT_SUBCLASS | TPFLAGS_BASETYPE, dict_type;
    PySet_Type = c"set", Object, TPFLAGS_BASETYPE, set_type;
    PyFrozenSet_Type = c"frozenset", Object, TPFLAGS_BASETYPE, frozenset_type;
    PySlice_Type = c"slice", SliceObject, 0, slice_type;
    PyRange_Type = c"range", Object, 0, range_type;
    PyModule_Type = c"module", Object, TPFLAGS_BASETYPE, module_type;
    PyProperty_Type = c"property", Object, TPFLAGS_BASETYPE, property_type;
    PyCapsule_Type = c"PyCapsule", Object, 0, capsule_type;
    _PyNone_Type = c"NoneType", Object, 0, none_type;
    PyEllipsis_Type = c"ellipsis", Object, 0, ellipsis_type;
    _PyNotImplemented_Type = c"NotImplementedType", Object, 0, not_implemented_type;
}

#[unsafe(no_mangle)]
pub static PyModuleDef_Type: Shared<Type> = Shared(UnsafeCell::new(type_object(
    c"moduledef",
    size_of::<ModuleDef>(),
    0,
)));

const fn singleton(ty: *const Shared<Type>) -> Object {
    Object {
        refcnt: IMMORTAL,
        ty: ty.cast::<Type>().cast_mut(),
    }
}

const fn boolean(value: bool) -> Long {
    Long {
        base: singleton(addr_of!(PyBool_Type)),
        tag: if value {
            1 << LONG_NON_SIZE_BITS
        } else {
            LONG_SIGN_ZERO
        },
        digits: [value as u32],
    }
}

#[unsafe(no_mangle)]
pub static _Py_NoneStruct: Shared<Object> =
    Shared(UnsafeCell::new(singleton(addr_of!(_PyNone_Type))));
#[unsafe(no_mangle)]
pub static _Py_EllipsisObject: Shared<Object> =
    Shared(UnsafeCell::new(singleton(addr_of!(PyEllipsis_Type))));
#[unsafe(no_mangle)]
pub static _Py_NotImplementedStruct: Shared<Object> =
    Shared(UnsafeCell::new(singleton(addr_of!(_PyNotImplemented_Type))));
#[unsafe(no_mangle)]
pub static _Py_TrueStruct: Shared<Long> = Shared(UnsafeCell::new(boolean(true)));
#[unsafe(no_mangle)]
pub static _Py_FalseStruct: Shared<Long> = Shared(UnsafeCell::new(boolean(false)));

macro_rules! exceptions {
    ($($name:ident = $field:ident,)*) => {
        $(
            #[unsafe(no_mangle)]
            pub static $name: Shared<*mut Object> = Shared(UnsafeCell::new(null_mut()));
        )*
        fn register_exceptions(vm: &VirtualMachine) {
            $(
                unsafe { *$name.get() = type_pointer(vm, vm.ctx.exceptions.$field).cast() };
            )*
        }
    };
}

exceptions! {
    PyExc_BaseException = base_exception_type,
    PyExc_Exception = exception_type,
    PyExc_SystemExit = system_exit,
    PyExc_KeyboardInterrupt = keyboard_interrupt,
    PyExc_GeneratorExit = generator_exit,
    PyExc_StopIteration = stop_iteration,
    PyExc_StopAsyncIteration = stop_async_iteration,
    PyExc_ArithmeticError = arithmetic_error,
    PyExc_FloatingPointError = floating_point_error,
    PyExc_OverflowError = overflow_error,
    PyExc_ZeroDivisionError = zero_division_error,
    PyExc_AssertionError = assertion_error,
    PyExc_AttributeError = attribute_error,
    PyExc_BufferError = buffer_error,
    PyExc_EOFError = eof_error,
    PyExc_ImportError = import_error,
    PyExc_ModuleNotFoundError = module_not_found_error,
    PyExc_LookupError = lookup_error,
    PyExc_IndexError = index_error,
    PyExc_KeyError = key_error,
    PyExc_MemoryError = memory_error,
    PyExc_NameError = name_error,
    PyExc_UnboundLocalError = unbound_local_error,
    PyExc_OSError = os_error,
    PyExc_EnvironmentError = os_error,
    PyExc_IOError = os_error,
    PyExc_BlockingIOError = blocking_io_error,
    PyExc_ChildProcessError = child_process_error,
    PyExc_ConnectionError = connection_error,
    PyExc_BrokenPipeError = broken_pipe_error,
    PyExc_FileExistsError = file_exists_error,
    PyExc_FileNotFoundError = file_not_found_error,
    PyExc_InterruptedError = interrupted_error,
    PyExc_IsADirectoryError = is_a_directory_error,
    PyExc_NotADirectoryError = not_a_directory_error,
    PyExc_PermissionError = permission_error,
    PyExc_ProcessLookupError = process_lookup_error,
    PyExc_TimeoutError = timeout_error,
    PyExc_ReferenceError = reference_error,
    PyExc_RuntimeError = runtime_error,
    PyExc_NotImplementedError = not_implemented_error,
    PyExc_RecursionError = recursion_error,
    PyExc_SyntaxError = syntax_error,
    PyExc_IndentationError = indentation_error,
    PyExc_TabError = tab_error,
    PyExc_SystemError = system_error,
    PyExc_TypeError = type_error,
    PyExc_ValueError = value_error,
    PyExc_UnicodeError = unicode_error,
    PyExc_UnicodeDecodeError = unicode_decode_error,
    PyExc_UnicodeEncodeError = unicode_encode_error,
    PyExc_UnicodeTranslateError = unicode_translate_error,
    PyExc_Warning = warning,
    PyExc_DeprecationWarning = deprecation_warning,
    PyExc_PendingDeprecationWarning = pending_deprecation_warning,
    PyExc_RuntimeWarning = runtime_warning,
    PyExc_SyntaxWarning = syntax_warning,
    PyExc_UserWarning = user_warning,
    PyExc_FutureWarning = future_warning,
    PyExc_ImportWarning = import_warning,
    PyExc_UnicodeWarning = unicode_warning,
    PyExc_BytesWarning = bytes_warning,
    PyExc_ResourceWarning = resource_warning,
    PyExc_EncodingWarning = encoding_warning,
}

thread_local! {
    static MACHINE: Cell<*const VirtualMachine> = const { Cell::new(null()) };
    static DEPTH: Cell<usize> = const { Cell::new(0) };
    static READY: Cell<bool> = const { Cell::new(false) };
    static ERROR: RefCell<Option<PyBaseExceptionRef>> = const { RefCell::new(None) };
    static PROXIES: RefCell<HashMap<usize, usize>> = RefCell::new(HashMap::new());
    static STATIC_TYPES: RefCell<HashMap<usize, PyTypeRef>> = RefCell::new(HashMap::new());
    static C_TYPES: RefCell<HashMap<usize, usize>> = RefCell::new(HashMap::new());
    static PROXY_TYPES: RefCell<HashSet<usize>> = RefCell::new(HashSet::new());
    static QUEUE: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
}

fn register_type(pointer: *mut Type, class: &'static Py<PyType>) {
    STATIC_TYPES.with_borrow_mut(|types| types.insert(pointer as usize, class.to_owned()));
    C_TYPES.with_borrow_mut(|types| {
        types.insert(class as *const Py<PyType> as usize, pointer as usize)
    });
}

pub fn register_c_type(pointer: *mut Type, class: &PyTypeRef) {
    STATIC_TYPES.with_borrow_mut(|types| types.insert(pointer as usize, class.clone()));
    C_TYPES.with_borrow_mut(|types| {
        types.insert(
            class.as_object() as *const PyObject as usize,
            pointer as usize,
        )
    });
}

pub fn static_type(pointer: *mut Type) -> Option<PyTypeRef> {
    STATIC_TYPES.with_borrow(|types| types.get(&(pointer as usize)).cloned())
}

fn ready(vm: &VirtualMachine) {
    if READY.replace(true) {
        return;
    }
    let _ = CObject::make_static_type();
    register_types(vm);
    let object = PyBaseObject_Type.get();
    for pointer in STATIC_TYPES.with_borrow(|types| types.keys().copied().collect::<Vec<_>>()) {
        let ty = pointer as *mut Type;
        if ty != object {
            unsafe {
                (*ty).base_type = if ty == PyBool_Type.get() {
                    PyLong_Type.get()
                } else {
                    object
                }
            };
        }
    }
    register_exceptions(vm);
}

pub fn machine<'a>() -> Option<&'a VirtualMachine> {
    let pointer = MACHINE.get();
    (!pointer.is_null()).then(|| unsafe { &*pointer })
}

pub fn enter<R>(vm: &VirtualMachine, f: impl FnOnce() -> R) -> R {
    ready(vm);
    let previous = MACHINE.replace(vm);
    DEPTH.set(DEPTH.get() + 1);
    let result = f();
    DEPTH.set(DEPTH.get() - 1);
    MACHINE.set(previous);
    if DEPTH.get() == 0 {
        sweep();
    }
    result
}

pub fn set_error(error: PyBaseExceptionRef) {
    ERROR.with_borrow_mut(|slot| *slot = Some(error));
}

pub fn take_error() -> Option<PyBaseExceptionRef> {
    ERROR.with_borrow_mut(Option::take)
}

pub fn current_error() -> Option<PyBaseExceptionRef> {
    ERROR.with_borrow(Clone::clone)
}

pub fn guard<T>(failed: T, f: impl FnOnce(&VirtualMachine) -> PyResult<T>) -> T {
    let Some(vm) = machine() else {
        return failed;
    };
    match f(vm) {
        Ok(value) => value,
        Err(error) => {
            set_error(error);
            failed
        }
    }
}

pub fn object(f: impl FnOnce(&VirtualMachine) -> PyResult<PyObjectRef>) -> *mut Object {
    guard(null_mut(), |vm| f(vm).map(|value| new_ref(vm, &value)))
}

pub fn borrowed_object(f: impl FnOnce(&VirtualMachine) -> PyResult<PyObjectRef>) -> *mut Object {
    guard(null_mut(), |vm| f(vm).map(|value| borrowed(vm, &value)))
}

pub fn status(f: impl FnOnce(&VirtualMachine) -> PyResult<()>) -> c_int {
    guard(-1, |vm| f(vm).map(|()| 0))
}

pub fn truth(f: impl FnOnce(&VirtualMachine) -> PyResult<bool>) -> c_int {
    guard(-1, |vm| f(vm).map(c_int::from))
}

pub fn size(f: impl FnOnce(&VirtualMachine) -> PyResult<usize>) -> isize {
    guard(-1, |vm| f(vm).map(|value| value as isize))
}

fn immortal(pointer: *mut Object) -> bool {
    let count = unsafe { (*pointer).refcnt };
    if size_of::<isize>() > size_of::<u32>() {
        (count as i32) < 0
    } else {
        count == IMMORTAL
    }
}

pub unsafe fn incref(pointer: *mut Object) {
    if !immortal(pointer) {
        unsafe { (*pointer).refcnt += 1 };
    }
}

pub unsafe fn decref(pointer: *mut Object) {
    if immortal(pointer) {
        return;
    }
    unsafe {
        (*pointer).refcnt -= 1;
        if (*pointer).refcnt == 0 {
            crate::api::_Py_Dealloc(pointer);
        }
    }
}

pub unsafe fn xdecref(pointer: *mut Object) {
    if !pointer.is_null() {
        unsafe { decref(pointer) };
    }
}

pub unsafe fn flags(pointer: *mut Object) -> c_ulong {
    unsafe {
        let ty = (*pointer).ty;
        if ty.is_null() { 0 } else { (*ty).flags }
    }
}

pub fn none() -> *mut Object {
    _Py_NoneStruct.get()
}

fn truth_pointer(value: bool) -> *mut Object {
    if value {
        _Py_TrueStruct.get().cast()
    } else {
        _Py_FalseStruct.get().cast()
    }
}

pub fn arg(vm: &VirtualMachine, pointer: *mut Object) -> PyResult<PyObjectRef> {
    if pointer.is_null() {
        return Err(vm.new_system_error("NULL object passed to the C API".to_owned()));
    }
    unsafe { to_rust(vm, pointer) }
}

fn strong(link: *const PyObject) -> PyObjectRef {
    unsafe { &*link }.to_owned()
}

fn own(object: &PyObject) -> *const PyObject {
    object.to_owned().into_raw().as_ptr()
}

unsafe fn disown(link: *const PyObject) {
    if let Some(pointer) = NonNull::new(link.cast_mut()) {
        drop(unsafe { PyObjectRef::from_raw(pointer) });
    }
}

pub unsafe fn to_rust(vm: &VirtualMachine, pointer: *mut Object) -> PyResult<PyObjectRef> {
    if pointer == none() {
        return Ok(vm.ctx.none());
    }
    if pointer == _Py_TrueStruct.get().cast() {
        return Ok(vm.ctx.new_bool(true).into());
    }
    if pointer == _Py_FalseStruct.get().cast() {
        return Ok(vm.ctx.new_bool(false).into());
    }
    if pointer == _Py_EllipsisObject.get() {
        return Ok(vm.ctx.ellipsis.clone().into());
    }
    if pointer == _Py_NotImplementedStruct.get() {
        return Ok(vm.ctx.not_implemented.clone().into());
    }
    if unsafe { flags(pointer) } & TPFLAGS_TYPE_SUBCLASS != 0 {
        if let Some(class) = static_type(pointer.cast()) {
            return Ok(class.into());
        }
        if !PROXY_TYPES.with_borrow(|types| types.contains(&(pointer as usize))) {
            return crate::types::c_type(vm, pointer.cast()).map(Into::into);
        }
    }
    let header = unsafe { heap::header(pointer) };
    match header.kind.get() {
        Kind::Proxy => Ok(strong(header.link.get())),
        Kind::Pending => unsafe { materialize(vm, pointer) },
        Kind::Plain | Kind::Instance => instance(vm, pointer),
    }
}

fn instance(vm: &VirtualMachine, pointer: *mut Object) -> PyResult<PyObjectRef> {
    let header = unsafe { heap::header(pointer) };
    if header.kind.get() == Kind::Instance && !header.link.get().is_null() {
        return Ok(strong(header.link.get()));
    }
    let class = crate::types::c_type(vm, unsafe { (*pointer).ty })?;
    let dict = class
        .slots
        .flags
        .has_feature(PyTypeFlags::HAS_DICT)
        .then(|| vm.ctx.new_dict());
    let wrapper = PyRef::new_ref(
        CObject {
            pointer: AtomicPtr::new(pointer),
        },
        class,
        dict,
    );
    unsafe { incref(pointer) };
    header.kind.set(Kind::Instance);
    header.link.set(wrapper.as_object() as *const PyObject);
    Ok(wrapper.into())
}

fn attach(pointer: *mut Object, object: &PyObject) {
    let header = unsafe { heap::header(pointer) };
    header.kind.set(Kind::Proxy);
    header.link.set(own(object));
    PROXIES.with_borrow_mut(|proxies| {
        proxies.insert(object as *const PyObject as usize, pointer as usize)
    });
}

pub fn units(vm: &VirtualMachine, pointer: *mut Object) -> PyResult<Wtf8Buf> {
    let ascii = pointer.cast::<Ascii>();
    let (length, state) = unsafe { ((*ascii).length.unsigned_abs(), (*ascii).state) };
    if state & COMPACT == 0 {
        return Err(vm.new_system_error("the C API reads only compact str objects".to_owned()));
    }
    let data = unsafe { pointer.cast::<u8>().add(unicode_header(state & ASCII != 0)) };
    let mut out = Wtf8Buf::with_capacity(length);
    match (state & KIND_MASK) >> KIND_SHIFT {
        1 if state & ASCII != 0 => out.push_str(&String::from_utf8_lossy(unsafe {
            slice::from_raw_parts(data, length)
        })),
        1 => unsafe { slice::from_raw_parts(data, length) }
            .iter()
            .for_each(|&unit| out.push_char(char::from(unit))),
        2 => unsafe { slice::from_raw_parts(data.cast::<u16>(), length) }
            .iter()
            .for_each(|&unit| out.push(code_point(u32::from(unit)))),
        _ => unsafe { slice::from_raw_parts(data.cast::<u32>(), length) }
            .iter()
            .for_each(|&unit| out.push(code_point(unit))),
    }
    Ok(out)
}

pub fn code_point(value: u32) -> CodePoint {
    CodePoint::from_u32(value).unwrap_or(CodePoint::from_char(char::REPLACEMENT_CHARACTER))
}

unsafe fn materialize(vm: &VirtualMachine, pointer: *mut Object) -> PyResult<PyObjectRef> {
    let flags = unsafe { flags(pointer) };
    let object: PyObjectRef = if flags & TPFLAGS_UNICODE_SUBCLASS != 0 {
        vm.ctx.new_str(units(vm, pointer)?).into()
    } else if flags & TPFLAGS_BYTES_SUBCLASS != 0 {
        let bytes = pointer.cast::<Bytes>();
        let length = unsafe { (*bytes).base.size.unsigned_abs() };
        let data = unsafe { slice::from_raw_parts((*bytes).data.as_ptr().cast::<u8>(), length) };
        vm.ctx.new_bytes(data.to_vec()).into()
    } else if flags & TPFLAGS_TUPLE_SUBCLASS != 0 {
        let tuple = pointer.cast::<Tuple>();
        let length = unsafe { (*tuple).base.size.unsigned_abs() };
        let items = unsafe { (*tuple).items.as_ptr() };
        vm.ctx.new_tuple(collect(vm, items, length)?).into()
    } else if flags & TPFLAGS_LIST_SUBCLASS != 0 {
        let list = pointer.cast::<List>();
        let length = unsafe { (*list).base.size.unsigned_abs() };
        vm.ctx
            .new_list(collect(vm, unsafe { (*list).items }, length)?)
            .into()
    } else {
        return Err(vm.new_system_error("the C API cannot read this object yet".to_owned()));
    };
    attach(pointer, &object);
    Ok(object)
}

fn collect(
    vm: &VirtualMachine,
    items: *const *mut Object,
    length: usize,
) -> PyResult<Vec<PyObjectRef>> {
    (0..length)
        .map(|index| arg(vm, unsafe { *items.add(index) }))
        .collect()
}

pub fn borrowed(vm: &VirtualMachine, object: &PyObject) -> *mut Object {
    if vm.is_none(object) {
        return none();
    }
    if object.class().is(vm.ctx.types.bool_type) {
        return truth_pointer(object.is(&vm.ctx.true_value));
    }
    if object.is(&vm.ctx.ellipsis) {
        return _Py_EllipsisObject.get();
    }
    if object.is(&vm.ctx.not_implemented) {
        return _Py_NotImplementedStruct.get();
    }
    if let Some(wrapper) = object.downcast_ref::<CObject>() {
        return wrapper.pointer();
    }
    if let Some(class) = object.downcast_ref::<PyType>() {
        return type_pointer(vm, class).cast();
    }
    let key = object as *const PyObject as usize;
    if let Some(pointer) = PROXIES.with_borrow(|proxies| proxies.get(&key).copied()) {
        let pointer = pointer as *mut Object;
        refresh(vm, object, pointer);
        return pointer;
    }
    let pointer = proxy(vm, object);
    attach(pointer, object);
    queue(pointer);
    pointer
}

pub fn new_ref(vm: &VirtualMachine, object: &PyObject) -> *mut Object {
    let pointer = borrowed(vm, object);
    unsafe { incref(pointer) };
    pointer
}

fn allocate_object(size: usize, ty: *mut Type, refcnt: isize) -> *mut Object {
    let pointer = heap::allocate(size, true).cast::<Object>();
    if pointer.is_null() {
        panic!("the C API could not allocate {size} bytes");
    }
    unsafe { pointer.write(Object { refcnt, ty }) };
    pointer
}

pub const fn unicode_header(ascii: bool) -> usize {
    if ascii {
        size_of::<Ascii>()
    } else {
        size_of::<Compact>()
    }
}

pub fn unicode_kind(max: u32) -> (usize, bool) {
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

pub fn new_unicode(ty: *mut Type, length: usize, max: u32, refcnt: isize) -> *mut Object {
    let (kind, ascii) = unicode_kind(max);
    let pointer = allocate_object(unicode_header(ascii) + (length + 1) * kind, ty, refcnt);
    unsafe {
        let head = pointer.cast::<Ascii>();
        (*head).length = length as isize;
        (*head).hash = -1;
        (*head).state = ((kind as u32) << KIND_SHIFT) | COMPACT | if ascii { ASCII } else { 0 };
    }
    pointer
}

pub fn unicode_data(pointer: *mut Object) -> *mut u8 {
    let state = unsafe { (*pointer.cast::<Ascii>()).state };
    unsafe { pointer.cast::<u8>().add(unicode_header(state & ASCII != 0)) }
}

pub fn fill_unicode(pointer: *mut Object, points: impl Iterator<Item = u32>) {
    let state = unsafe { (*pointer.cast::<Ascii>()).state };
    let data = unicode_data(pointer);
    for (index, point) in points.enumerate() {
        unsafe {
            match (state & KIND_MASK) >> KIND_SHIFT {
                1 => data.add(index).write(point as u8),
                2 => data.cast::<u16>().add(index).write(point as u16),
                _ => data.cast::<u32>().add(index).write(point),
            }
        }
    }
}

pub fn str_object(ty: *mut Type, text: &PyStr, refcnt: isize) -> *mut Object {
    let bytes = text.as_bytes();
    if bytes.is_ascii() {
        let pointer = new_unicode(ty, bytes.len(), 0, refcnt);
        unsafe { unicode_data(pointer).copy_from_nonoverlapping(bytes.as_ptr(), bytes.len()) };
        return pointer;
    }
    let points = || text.as_wtf8().code_points().map(CodePoint::to_u32);
    let (length, max) = points().fold((0, 0), |(length, max), point| (length + 1, max.max(point)));
    let pointer = new_unicode(ty, length, max, refcnt);
    fill_unicode(pointer, points());
    pointer
}

pub fn bytes_object(ty: *mut Type, data: &[u8], refcnt: isize) -> *mut Object {
    let pointer = allocate_object(offset_of!(Bytes, data) + data.len() + 1, ty, refcnt);
    unsafe {
        let bytes = pointer.cast::<Bytes>();
        (*bytes).base.size = data.len() as isize;
        (*bytes).hash = -1;
        (*bytes)
            .data
            .as_mut_ptr()
            .cast::<u8>()
            .copy_from_nonoverlapping(data.as_ptr(), data.len());
    }
    pointer
}

pub fn tuple_object(ty: *mut Type, length: usize, refcnt: isize) -> *mut Object {
    let pointer = allocate_object(
        offset_of!(Tuple, items) + length * size_of::<*mut Object>(),
        ty,
        refcnt,
    );
    unsafe { (*pointer.cast::<Tuple>()).base.size = length as isize };
    pointer
}

pub fn list_object(ty: *mut Type, length: usize, refcnt: isize) -> *mut Object {
    let pointer = allocate_object(size_of::<List>(), ty, refcnt);
    let items = heap::allocate(length * size_of::<*mut Object>(), true).cast::<*mut Object>();
    unsafe {
        let list = pointer.cast::<List>();
        (*list).base.size = length as isize;
        (*list).items = items;
        (*list).allocated = length as isize;
    }
    pointer
}

pub unsafe fn list_push(pointer: *mut Object, item: *mut Object) {
    let list = pointer.cast::<List>();
    unsafe {
        let length = (*list).base.size.unsigned_abs();
        if length as isize == (*list).allocated {
            let capacity = (length * 2).max(length + 1);
            (*list).items =
                heap::resize((*list).items.cast(), capacity * size_of::<*mut Object>()).cast();
            (*list).allocated = capacity as isize;
        }
        *(*list).items.add(length) = item;
        (*list).base.size = (length + 1) as isize;
    }
}

fn long_digits(value: &nanocode_numeric::BigInt) -> (usize, Vec<u32>) {
    let (sign, words) = value.to_u32_digits();
    let mask = (1u64 << DIGIT_BITS) - 1;
    let mut digits = Vec::new();
    let mut acc = 0u64;
    let mut have = 0;
    for word in words {
        acc |= u64::from(word) << have;
        have += u32::BITS;
        while have >= DIGIT_BITS {
            digits.push((acc & mask) as u32);
            acc >>= DIGIT_BITS;
            have -= DIGIT_BITS;
        }
    }
    if acc != 0 {
        digits.push(acc as u32);
    }
    while digits.last() == Some(&0) {
        digits.pop();
    }
    let sign = match sign {
        _ if digits.is_empty() => LONG_SIGN_ZERO,
        nanocode_numeric::Sign::Minus => LONG_SIGN_NEGATIVE,
        _ => 0,
    };
    (sign, digits)
}

fn proxy(vm: &VirtualMachine, object: &PyObject) -> *mut Object {
    let ty = type_pointer(vm, object.class());
    if let Some(text) = object.downcast_ref::<PyStr>() {
        return str_object(ty, text, 0);
    }
    if let Some(bytes) = object.downcast_ref::<PyBytes>() {
        return bytes_object(ty, bytes.as_bytes(), 0);
    }
    if let Some(int) = object.downcast_ref::<PyInt>() {
        let (sign, digits) = long_digits(int.as_bigint());
        let pointer = allocate_object(
            offset_of!(Long, digits) + digits.len().max(1) * size_of::<u32>(),
            ty,
            0,
        );
        unsafe {
            let long = pointer.cast::<Long>();
            (*long).tag = (digits.len() << LONG_NON_SIZE_BITS) | sign;
            (*long)
                .digits
                .as_mut_ptr()
                .copy_from_nonoverlapping(digits.as_ptr(), digits.len());
        }
        return pointer;
    }
    if let Some(float) = object.downcast_ref::<PyFloat>() {
        let pointer = allocate_object(size_of::<Float>(), ty, 0);
        unsafe { (*pointer.cast::<Float>()).value = float.to_f64() };
        return pointer;
    }
    if let Some(tuple) = object.downcast_ref::<PyTuple>() {
        let items = tuple.as_slice();
        let pointer = tuple_object(ty, items.len(), 0);
        for (index, item) in items.iter().enumerate() {
            unsafe {
                *(*pointer.cast::<Tuple>()).items.as_mut_ptr().add(index) = new_ref(vm, item)
            };
        }
        return pointer;
    }
    if let Some(list) = object.downcast_ref::<PyList>() {
        let items: Vec<PyObjectRef> = list.borrow_vec().to_vec();
        let pointer = list_object(ty, 0, 0);
        for item in &items {
            unsafe { list_push(pointer, new_ref(vm, item)) };
        }
        return pointer;
    }
    if let Some(dict) = object.downcast_ref::<PyDict>() {
        let pointer = allocate_object(size_of::<Dict>(), ty, 0);
        unsafe { (*pointer.cast::<Dict>()).used = dict.__len__() as isize };
        return pointer;
    }
    if let Some(slice) = object.downcast_ref::<PySlice>() {
        let pointer = allocate_object(size_of::<SliceObject>(), ty, 0);
        let part = |value: Option<&PyObjectRef>| {
            value.map_or_else(
                || {
                    unsafe { incref(none()) };
                    none()
                },
                |value| new_ref(vm, value),
            )
        };
        unsafe {
            let target = pointer.cast::<SliceObject>();
            (*target).start = part(slice.start.as_ref());
            (*target).stop = part(Some(&slice.stop));
            (*target).step = part(slice.step.as_ref());
        }
        return pointer;
    }
    allocate_object(size_of::<Object>(), ty, 0)
}

pub fn refresh(vm: &VirtualMachine, object: &PyObject, pointer: *mut Object) {
    if let Some(list) = object.downcast_ref::<PyList>() {
        let items: Vec<PyObjectRef> = list.borrow_vec().to_vec();
        let target = pointer.cast::<List>();
        unsafe {
            let length = (*target).base.size.unsigned_abs();
            let old: Vec<*mut Object> = (0..length)
                .map(|index| *(*target).items.add(index))
                .collect();
            (*target).base.size = 0;
            for item in &items {
                list_push(pointer, new_ref(vm, item));
            }
            old.into_iter().for_each(|item| xdecref(item));
        }
    } else if let Some(dict) = object.downcast_ref::<PyDict>() {
        unsafe { (*pointer.cast::<Dict>()).used = dict.__len__() as isize };
    }
}

fn subclass_flags(vm: &VirtualMachine, class: &Py<PyType>) -> c_ulong {
    let types = &vm.ctx.types;
    [
        (types.int_type, TPFLAGS_LONG_SUBCLASS),
        (types.list_type, TPFLAGS_LIST_SUBCLASS),
        (types.tuple_type, TPFLAGS_TUPLE_SUBCLASS),
        (types.bytes_type, TPFLAGS_BYTES_SUBCLASS),
        (types.str_type, TPFLAGS_UNICODE_SUBCLASS),
        (types.dict_type, TPFLAGS_DICT_SUBCLASS),
        (
            vm.ctx.exceptions.base_exception_type,
            TPFLAGS_BASE_EXC_SUBCLASS,
        ),
        (types.type_type, TPFLAGS_TYPE_SUBCLASS),
    ]
    .into_iter()
    .filter(|(base, _)| class.fast_issubclass(*base))
    .fold(TPFLAGS_BASETYPE | TPFLAGS_HEAPTYPE, |flags, (_, flag)| {
        flags | flag
    })
}

pub fn type_pointer(vm: &VirtualMachine, class: &Py<PyType>) -> *mut Type {
    ready(vm);
    let key = class.as_object() as *const PyObject as usize;
    if let Some(pointer) = C_TYPES.with_borrow(|types| types.get(&key).copied()) {
        return pointer as *mut Type;
    }
    let base = class
        .base
        .deref()
        .map_or(null_mut(), |base| type_pointer(vm, base));
    let name = CString::new(class.name().to_string())
        .unwrap_or_default()
        .into_raw();
    let pointer = heap::allocate(size_of::<Type>(), true).cast::<Type>();
    let mut template = type_object(c"", size_of::<Object>(), subclass_flags(vm, class));
    template.name = name;
    template.base_type = base;
    unsafe { pointer.write(template) };
    attach(pointer.cast(), class.as_object());
    PROXIES.with_borrow_mut(|proxies| proxies.remove(&key));
    C_TYPES.with_borrow_mut(|types| types.insert(key, pointer as usize));
    PROXY_TYPES.with_borrow_mut(|types| types.insert(pointer as usize));
    pointer
}

pub fn text(pointer: *const c_char) -> Option<&'static str> {
    if pointer.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(pointer) }.to_str().ok()
}

fn queue(pointer: *mut Object) {
    if !unsafe { heap::header(pointer) }.queued.replace(true) {
        QUEUE.with_borrow_mut(|queue| queue.push(pointer as usize));
    }
}

pub unsafe extern "C" fn dealloc(pointer: *mut Object) {
    match unsafe { heap::header(pointer) }.kind.get() {
        Kind::Proxy => queue(pointer),
        _ => unsafe { destroy(pointer) },
    }
}

unsafe fn destroy(pointer: *mut Object) {
    let flags = unsafe { flags(pointer) };
    unsafe {
        if flags & TPFLAGS_TUPLE_SUBCLASS != 0 {
            let tuple = pointer.cast::<Tuple>();
            for index in 0..(*tuple).base.size.unsigned_abs() {
                xdecref(*(*tuple).items.as_ptr().add(index));
            }
        } else if flags & TPFLAGS_LIST_SUBCLASS != 0 {
            let list = pointer.cast::<List>();
            for index in 0..(*list).base.size.unsigned_abs() {
                xdecref(*(*list).items.add(index));
            }
            heap::release((*list).items.cast());
        } else if flags & TPFLAGS_UNICODE_SUBCLASS != 0 {
            let state = (*pointer.cast::<Ascii>()).state;
            if state & ASCII == 0 {
                heap::release((*pointer.cast::<Compact>()).utf8.cast());
            }
        } else if (*pointer).ty == PySlice_Type.get() {
            let target = pointer.cast::<SliceObject>();
            xdecref((*target).start);
            xdecref((*target).stop);
            xdecref((*target).step);
        }
        heap::release(pointer.cast());
    }
}

fn sweep() {
    loop {
        let batch = QUEUE.with_borrow_mut(std::mem::take);
        if batch.is_empty() {
            return;
        }
        let mut links = Vec::new();
        for pointer in batch {
            let pointer = pointer as *mut Object;
            let header = unsafe { heap::header(pointer) };
            header.queued.set(false);
            if unsafe { (*pointer).refcnt } != 0 || header.kind.get() != Kind::Proxy {
                continue;
            }
            let link = header.link.replace(null());
            PROXIES.with_borrow_mut(|proxies| proxies.remove(&(link as usize)));
            links.push(link);
            unsafe { destroy(pointer) };
        }
        for link in links {
            unsafe { disown(link) };
        }
    }
}

pub fn finish(vm: &VirtualMachine, result: *mut Object) -> PyResult {
    if result.is_null() {
        return Err(take_error().unwrap_or_else(|| {
            vm.new_system_error("error return without exception set".to_owned())
        }));
    }
    let object = unsafe { to_rust(vm, result) };
    unsafe { decref(result) };
    take_error();
    object
}

pub fn check(_vm: &VirtualMachine, result: isize) -> PyResult<isize> {
    if result == -1
        && let Some(error) = take_error()
    {
        return Err(error);
    }
    Ok(result)
}

#[pyclass(module = "_capi", name = "object")]
#[derive(Debug, PyPayload)]
pub struct CObject {
    pointer: AtomicPtr<Object>,
}

#[pyclass(flags(BASETYPE))]
impl CObject {}

impl CObject {
    pub fn pointer(&self) -> *mut Object {
        self.pointer.load(Ordering::Relaxed)
    }
}

impl Drop for CObject {
    fn drop(&mut self) {
        let pointer = self.pointer();
        let header = unsafe { heap::header(pointer) };
        header.link.set(null());
        header.kind.set(Kind::Plain);
        unsafe { decref(pointer) };
    }
}

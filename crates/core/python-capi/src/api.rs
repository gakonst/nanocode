use std::{
    cell::RefCell,
    collections::HashMap,
    ffi::{CStr, c_char, c_int, c_long, c_longlong, c_ulong, c_ulonglong, c_void},
    mem::{offset_of, size_of},
    ptr::{null, null_mut},
    slice,
};

use rustpython_vm::{
    AsObject, PyObject, PyObjectRef, PyPayload, PyRef, PyResult, TryFromBorrowedObject,
    VirtualMachine,
    builtins::{
        PyBaseException, PyBaseExceptionRef, PyDict, PyIntRef, PyList, PySlice, PyStr, PyTuple,
        PyType,
    },
    function::FuncArgs,
    protocol::PyBuffer,
};

use crate::{
    abi::*,
    bridge::{
        self, arg, borrowed, current_error, decref, guard, incref, new_ref, none, object,
        set_error, size, status, take_error, text, truth, xdecref,
    },
    heap::{self, Kind},
};

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _Py_Dealloc(pointer: *mut Object) {
    unsafe {
        let ty = (*pointer).ty;
        match (*ty).dealloc {
            Some(dealloc) => crate::cast::call1(dealloc, pointer),
            None => heap::release(pointer.cast()),
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn Py_IncRef(pointer: *mut Object) {
    if !pointer.is_null() {
        unsafe { incref(pointer) };
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn Py_DecRef(pointer: *mut Object) {
    unsafe { xdecref(pointer) };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _Py_IncRef(pointer: *mut Object) {
    unsafe { Py_IncRef(pointer) };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _Py_DecRef(pointer: *mut Object) {
    unsafe { xdecref(pointer) };
}

fn exception(
    vm: &VirtualMachine,
    ty: PyObjectRef,
    value: Option<PyObjectRef>,
) -> PyResult<PyBaseExceptionRef> {
    let class = ty.downcast::<PyType>().map_err(|_| {
        vm.new_type_error("exceptions must be classes deriving from BaseException".to_owned())
    })?;
    if !class.fast_issubclass(vm.ctx.exceptions.base_exception_type) {
        return Err(vm.new_type_error(format!(
            "exceptions must derive from BaseException, not {}",
            class.name()
        )));
    }
    if let Some(value) = &value
        && value.class().fast_issubclass(&class)
        && let Ok(error) = value.clone().downcast::<PyBaseException>()
    {
        return Ok(error);
    }
    let args: Vec<PyObjectRef> = match value {
        None => Vec::new(),
        Some(value) if vm.is_none(&value) => Vec::new(),
        Some(value) => match value.downcast_ref::<PyTuple>() {
            Some(tuple) => tuple.as_slice().to_vec(),
            None => vec![value],
        },
    };
    let made = class.as_object().call(FuncArgs::from(args), vm)?;
    made.downcast::<PyBaseException>().map_err(|_| {
        vm.new_type_error("calling an exception class did not return an exception".to_owned())
    })
}

fn raise(vm: &VirtualMachine, ty: *mut Object, value: Option<PyObjectRef>) -> PyResult<()> {
    let error = exception(vm, arg(vm, ty)?, value)?;
    set_error(error);
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn PyErr_SetString(ty: *mut Object, message: *const c_char) {
    guard((), |vm| {
        let message = unsafe { CStr::from_ptr(message) }
            .to_string_lossy()
            .into_owned();
        raise(vm, ty, Some(vm.ctx.new_str(message).into()))
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn PyErr_SetObject(ty: *mut Object, value: *mut Object) {
    guard((), |vm| {
        let value = if value.is_null() {
            None
        } else {
            Some(arg(vm, value)?)
        };
        raise(vm, ty, value)
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn PyErr_SetNone(ty: *mut Object) {
    guard((), |vm| raise(vm, ty, None));
}

#[unsafe(no_mangle)]
pub extern "C" fn PyErr_Occurred() -> *mut Object {
    match (current_error(), bridge::machine()) {
        (Some(error), Some(vm)) => borrowed(vm, error.class().as_object()),
        _ => null_mut(),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn PyErr_Clear() {
    take_error();
}

fn matches(given: &PyObject, expected: &PyObject) -> bool {
    if let Some(tuple) = expected.downcast_ref::<PyTuple>() {
        return tuple.as_slice().iter().any(|item| matches(given, item));
    }
    let given = given
        .downcast_ref::<PyType>()
        .map_or_else(|| given.class().to_owned(), |class| class.to_owned());
    expected
        .downcast_ref::<PyType>()
        .is_some_and(|expected| given.fast_issubclass(expected))
}

#[unsafe(no_mangle)]
pub extern "C" fn PyErr_GivenExceptionMatches(given: *mut Object, expected: *mut Object) -> c_int {
    if given.is_null() || expected.is_null() {
        return 0;
    }
    guard(0, |vm| {
        Ok(c_int::from(matches(
            &*arg(vm, given)?,
            &*arg(vm, expected)?,
        )))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyErr_ExceptionMatches(expected: *mut Object) -> c_int {
    let Some(error) = current_error() else {
        return 0;
    };
    guard(0, |vm| {
        Ok(c_int::from(matches(
            error.as_object(),
            &*arg(vm, expected)?,
        )))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyErr_NoMemory() -> *mut Object {
    guard((), |vm| {
        set_error(vm.new_memory_error(String::new()));
        Ok(())
    });
    null_mut()
}

#[unsafe(no_mangle)]
pub extern "C" fn PyErr_BadArgument() -> c_int {
    guard((), |vm| {
        set_error(vm.new_type_error("bad argument type for built-in operation".to_owned()));
        Ok(())
    });
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn PyErr_BadInternalCall() {
    guard((), |vm| {
        set_error(vm.new_system_error("bad argument to internal function".to_owned()));
        Ok(())
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn _PyErr_BadInternalCall(_file: *const c_char, _line: c_int) {
    PyErr_BadInternalCall();
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_Fetch(
    ty: *mut *mut Object,
    value: *mut *mut Object,
    traceback: *mut *mut Object,
) {
    let error = take_error();
    let (class, error) = match (error, bridge::machine()) {
        (Some(error), Some(vm)) => (
            new_ref(vm, error.class().as_object()),
            new_ref(vm, error.as_object()),
        ),
        _ => (null_mut(), null_mut()),
    };
    unsafe {
        *ty = class;
        *value = error;
        *traceback = null_mut();
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_Restore(
    ty: *mut Object,
    value: *mut Object,
    traceback: *mut Object,
) {
    take_error();
    if !ty.is_null() {
        guard((), |vm| {
            let value = if value.is_null() {
                None
            } else {
                Some(arg(vm, value)?)
            };
            raise(vm, ty, value)
        });
    }
    unsafe {
        xdecref(ty);
        xdecref(value);
        xdecref(traceback);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_NormalizeException(
    _ty: *mut *mut Object,
    _value: *mut *mut Object,
    _traceback: *mut *mut Object,
) {
}

#[unsafe(no_mangle)]
pub extern "C" fn PyErr_GetRaisedException() -> *mut Object {
    match (take_error(), bridge::machine()) {
        (Some(error), Some(vm)) => new_ref(vm, error.as_object()),
        _ => null_mut(),
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_SetRaisedException(value: *mut Object) {
    take_error();
    if value.is_null() {
        return;
    }
    guard((), |vm| {
        let value = arg(vm, value)?;
        raise(vm, borrowed(vm, value.class().as_object()), Some(value))
    });
    unsafe { decref(value) };
}

#[unsafe(no_mangle)]
pub extern "C" fn PyErr_NewExceptionWithDoc(
    name: *const c_char,
    doc: *const c_char,
    base: *mut Object,
    dict: *mut Object,
) -> *mut Object {
    object(|vm| {
        let full = text(name)
            .ok_or_else(|| vm.new_system_error("PyErr_NewException() needs a name".to_owned()))?;
        let (module, short) = full.rsplit_once('.').ok_or_else(|| {
            vm.new_system_error("PyErr_NewException(): name must be module.class".to_owned())
        })?;
        let bases: PyObjectRef = if base.is_null() {
            vm.ctx
                .new_tuple(vec![vm.ctx.exceptions.exception_type.to_owned().into()])
                .into()
        } else {
            let base = arg(vm, base)?;
            if base.downcast_ref::<PyTuple>().is_some() {
                base
            } else {
                vm.ctx.new_tuple(vec![base]).into()
            }
        };
        let namespace = if dict.is_null() {
            vm.ctx.new_dict()
        } else {
            arg(vm, dict)?
                .downcast::<PyDict>()
                .map_err(|_| vm.new_type_error("dict expected".to_owned()))?
        };
        namespace.set_item("__module__", vm.ctx.new_str(module).into(), vm)?;
        if let Some(doc) = text(doc) {
            namespace.set_item("__doc__", vm.ctx.new_str(doc).into(), vm)?;
        }
        vm.ctx
            .types
            .type_type
            .as_object()
            .call((vm.ctx.new_str(short), bases, namespace), vm)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyErr_NewException(
    name: *const c_char,
    base: *mut Object,
    dict: *mut Object,
) -> *mut Object {
    PyErr_NewExceptionWithDoc(name, null(), base, dict)
}

#[unsafe(no_mangle)]
pub extern "C" fn PyErr_CheckSignals() -> c_int {
    status(|vm| vm.check_signals())
}

#[unsafe(no_mangle)]
pub extern "C" fn PyErr_WarnEx(
    _category: *mut Object,
    _message: *const c_char,
    _level: isize,
) -> c_int {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn PyErr_Print() {
    take_error();
}

#[unsafe(no_mangle)]
pub extern "C" fn PyErr_WriteUnraisable(_object: *mut Object) {
    take_error();
}

fn name(vm: &VirtualMachine, pointer: *const c_char) -> PyResult<&'static str> {
    text(pointer)
        .ok_or_else(|| vm.new_system_error("the C API got a name that is not UTF-8".to_owned()))
}

#[unsafe(no_mangle)]
pub extern "C" fn PyObject_Repr(pointer: *mut Object) -> *mut Object {
    object(|vm| Ok(arg(vm, pointer)?.repr(vm)?.into()))
}

#[unsafe(no_mangle)]
pub extern "C" fn PyObject_Str(pointer: *mut Object) -> *mut Object {
    object(|vm| Ok(arg(vm, pointer)?.str(vm)?.into()))
}

#[unsafe(no_mangle)]
pub extern "C" fn PyObject_ASCII(pointer: *mut Object) -> *mut Object {
    object(|vm| Ok(arg(vm, pointer)?.ascii(vm)?.into()))
}

#[unsafe(no_mangle)]
pub extern "C" fn PyObject_Bytes(pointer: *mut Object) -> *mut Object {
    object(|vm| {
        vm.ctx
            .types
            .bytes_type
            .as_object()
            .call((arg(vm, pointer)?,), vm)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyObject_Format(pointer: *mut Object, spec: *mut Object) -> *mut Object {
    object(|vm| {
        let spec = if spec.is_null() {
            vm.ctx.new_str("").into()
        } else {
            arg(vm, spec)?
        };
        vm.call_method(&*arg(vm, pointer)?, "__format__", (spec,))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyObject_GetAttr(pointer: *mut Object, attribute: *mut Object) -> *mut Object {
    object(|vm| {
        let attribute = arg(vm, attribute)?
            .downcast::<PyStr>()
            .map_err(|_| vm.new_type_error("attribute name must be string".to_owned()))?;
        arg(vm, pointer)?.get_attr(&attribute, vm)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyObject_GenericGetAttr(
    pointer: *mut Object,
    attribute: *mut Object,
) -> *mut Object {
    PyObject_GetAttr(pointer, attribute)
}

#[unsafe(no_mangle)]
pub extern "C" fn PyObject_GetAttrString(
    pointer: *mut Object,
    attribute: *const c_char,
) -> *mut Object {
    object(|vm| arg(vm, pointer)?.get_attr(vm.ctx.intern_str(name(vm, attribute)?), vm))
}

#[unsafe(no_mangle)]
pub extern "C" fn PyObject_SetAttr(
    pointer: *mut Object,
    attribute: *mut Object,
    value: *mut Object,
) -> c_int {
    status(|vm| {
        let target = arg(vm, pointer)?;
        let attribute = arg(vm, attribute)?
            .downcast::<PyStr>()
            .map_err(|_| vm.new_type_error("attribute name must be string".to_owned()))?;
        if value.is_null() {
            target.del_attr(&attribute, vm)
        } else {
            target.set_attr(&attribute, arg(vm, value)?, vm)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyObject_GenericSetAttr(
    pointer: *mut Object,
    attribute: *mut Object,
    value: *mut Object,
) -> c_int {
    PyObject_SetAttr(pointer, attribute, value)
}

#[unsafe(no_mangle)]
pub extern "C" fn PyObject_SetAttrString(
    pointer: *mut Object,
    attribute: *const c_char,
    value: *mut Object,
) -> c_int {
    status(|vm| {
        let target = arg(vm, pointer)?;
        let attribute = vm.ctx.intern_str(name(vm, attribute)?);
        if value.is_null() {
            target.del_attr(attribute, vm)
        } else {
            target.set_attr(attribute, arg(vm, value)?, vm)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyObject_HasAttr(pointer: *mut Object, attribute: *mut Object) -> c_int {
    let result = PyObject_GetAttr(pointer, attribute);
    take_error();
    if result.is_null() {
        return 0;
    }
    unsafe { decref(result) };
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn PyObject_HasAttrString(pointer: *mut Object, attribute: *const c_char) -> c_int {
    let result = PyObject_GetAttrString(pointer, attribute);
    take_error();
    if result.is_null() {
        return 0;
    }
    unsafe { decref(result) };
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn PyObject_GetItem(pointer: *mut Object, key: *mut Object) -> *mut Object {
    object(|vm| arg(vm, pointer)?.get_item(&*arg(vm, key)?, vm))
}

#[unsafe(no_mangle)]
pub extern "C" fn PyObject_SetItem(
    pointer: *mut Object,
    key: *mut Object,
    value: *mut Object,
) -> c_int {
    status(|vm| arg(vm, pointer)?.set_item(&*arg(vm, key)?, arg(vm, value)?, vm))
}

#[unsafe(no_mangle)]
pub extern "C" fn PyObject_DelItem(pointer: *mut Object, key: *mut Object) -> c_int {
    status(|vm| arg(vm, pointer)?.del_item(&*arg(vm, key)?, vm))
}

#[unsafe(no_mangle)]
pub extern "C" fn PyObject_Size(pointer: *mut Object) -> isize {
    size(|vm| arg(vm, pointer)?.length(vm))
}

#[unsafe(no_mangle)]
pub extern "C" fn PyObject_Length(pointer: *mut Object) -> isize {
    PyObject_Size(pointer)
}

#[unsafe(no_mangle)]
pub extern "C" fn PyObject_Hash(pointer: *mut Object) -> isize {
    guard(-1, |vm| Ok(arg(vm, pointer)?.hash(vm)? as isize))
}

#[unsafe(no_mangle)]
pub extern "C" fn PyObject_HashNotImplemented(pointer: *mut Object) -> isize {
    guard(-1, |vm| {
        Err(vm.new_type_error(format!(
            "unhashable type: '{}'",
            arg(vm, pointer)?.class().name()
        )))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyObject_IsTrue(pointer: *mut Object) -> c_int {
    if pointer == bridge::_Py_TrueStruct.get().cast() {
        return 1;
    }
    if pointer == bridge::_Py_FalseStruct.get().cast() || pointer == none() {
        return 0;
    }
    truth(|vm| arg(vm, pointer)?.is_true(vm))
}

#[unsafe(no_mangle)]
pub extern "C" fn PyObject_Not(pointer: *mut Object) -> c_int {
    match PyObject_IsTrue(pointer) {
        -1 => -1,
        value => c_int::from(value == 0),
    }
}

fn comparison(vm: &VirtualMachine, op: c_int) -> PyResult<rustpython_vm::types::PyComparisonOp> {
    use rustpython_vm::types::PyComparisonOp;
    Ok(match op {
        LT => PyComparisonOp::Lt,
        LE => PyComparisonOp::Le,
        EQ => PyComparisonOp::Eq,
        NE => PyComparisonOp::Ne,
        GT => PyComparisonOp::Gt,
        GE => PyComparisonOp::Ge,
        _ => return Err(vm.new_system_error(format!("bad comparison operator {op}"))),
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyObject_RichCompare(a: *mut Object, b: *mut Object, op: c_int) -> *mut Object {
    object(|vm| arg(vm, a)?.rich_compare(arg(vm, b)?, comparison(vm, op)?, vm))
}

#[unsafe(no_mangle)]
pub extern "C" fn PyObject_RichCompareBool(a: *mut Object, b: *mut Object, op: c_int) -> c_int {
    if a == b && (op == EQ || op == NE) {
        return c_int::from(op == EQ);
    }
    truth(|vm| arg(vm, a)?.rich_compare_bool(&*arg(vm, b)?, comparison(vm, op)?, vm))
}

#[unsafe(no_mangle)]
pub extern "C" fn PyObject_IsInstance(pointer: *mut Object, class: *mut Object) -> c_int {
    truth(|vm| arg(vm, pointer)?.is_instance(&*arg(vm, class)?, vm))
}

#[unsafe(no_mangle)]
pub extern "C" fn PyObject_IsSubclass(pointer: *mut Object, class: *mut Object) -> c_int {
    truth(|vm| arg(vm, pointer)?.is_subclass(&*arg(vm, class)?, vm))
}

fn call(
    vm: &VirtualMachine,
    callable: *mut Object,
    args: Vec<PyObjectRef>,
    kwargs: *mut Object,
) -> PyResult {
    let callable = arg(vm, callable)?;
    let mut call = FuncArgs::from(args);
    if !kwargs.is_null() {
        let kwargs = arg(vm, kwargs)?
            .downcast::<PyDict>()
            .map_err(|_| vm.new_type_error("keyword arguments must be a dict".to_owned()))?;
        for (key, value) in kwargs.items_vec() {
            let key = key
                .downcast::<PyStr>()
                .map_err(|_| vm.new_type_error("keywords must be strings".to_owned()))?;
            call.kwargs.insert(key.as_wtf8().to_owned(), value);
        }
    }
    callable.call(call, vm)
}

fn tuple_args(vm: &VirtualMachine, args: *mut Object) -> PyResult<Vec<PyObjectRef>> {
    if args.is_null() {
        return Ok(Vec::new());
    }
    let args = arg(vm, args)?;
    args.downcast_ref::<PyTuple>()
        .map(|tuple| tuple.as_slice().to_vec())
        .ok_or_else(|| vm.new_type_error("argument list must be a tuple".to_owned()))
}

#[unsafe(no_mangle)]
pub extern "C" fn PyObject_Call(
    callable: *mut Object,
    args: *mut Object,
    kwargs: *mut Object,
) -> *mut Object {
    object(|vm| call(vm, callable, tuple_args(vm, args)?, kwargs))
}

#[unsafe(no_mangle)]
pub extern "C" fn PyObject_CallObject(callable: *mut Object, args: *mut Object) -> *mut Object {
    object(|vm| call(vm, callable, tuple_args(vm, args)?, null_mut()))
}

#[unsafe(no_mangle)]
pub extern "C" fn PyObject_CallNoArgs(callable: *mut Object) -> *mut Object {
    object(|vm| call(vm, callable, Vec::new(), null_mut()))
}

#[unsafe(no_mangle)]
pub extern "C" fn PyObject_CallOneArg(callable: *mut Object, value: *mut Object) -> *mut Object {
    object(|vm| call(vm, callable, vec![arg(vm, value)?], null_mut()))
}

const VECTORCALL_OFFSET: usize = 1 << (usize::BITS - 1);

fn vector(
    vm: &VirtualMachine,
    args: *const *mut Object,
    count: usize,
    names: *mut Object,
) -> PyResult<FuncArgs> {
    let items: Vec<PyObjectRef> = (0..count)
        .map(|index| arg(vm, unsafe { *args.add(index) }))
        .collect::<PyResult<_>>()?;
    let names = if names.is_null() {
        Vec::new()
    } else {
        tuple_args(vm, names)?
    };
    let positional = count - names.len();
    let mut call = FuncArgs::from(items[..positional].to_vec());
    for (key, value) in names.iter().zip(&items[positional..]) {
        let key = key
            .downcast_ref::<PyStr>()
            .ok_or_else(|| vm.new_type_error("keywords must be strings".to_owned()))?;
        call.kwargs.insert(key.as_wtf8().to_owned(), value.clone());
    }
    Ok(call)
}

#[unsafe(no_mangle)]
pub extern "C" fn PyObject_Vectorcall(
    callable: *mut Object,
    args: *const *mut Object,
    count: usize,
    names: *mut Object,
) -> *mut Object {
    object(|vm| {
        let names_count = if names.is_null() {
            0
        } else {
            tuple_args(vm, names)?.len()
        };
        let call = vector(vm, args, (count & !VECTORCALL_OFFSET) + names_count, names)?;
        arg(vm, callable)?.call(call, vm)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyObject_VectorcallMethod(
    method: *mut Object,
    args: *const *mut Object,
    count: usize,
    names: *mut Object,
) -> *mut Object {
    object(|vm| {
        let names_count = if names.is_null() {
            0
        } else {
            tuple_args(vm, names)?.len()
        };
        let mut call = vector(vm, args, (count & !VECTORCALL_OFFSET) + names_count, names)?;
        if call.args.is_empty() {
            return Err(vm.new_type_error("a method call needs its object".to_owned()));
        }
        let zelf = call.args.remove(0);
        let method = arg(vm, method)?
            .downcast::<PyStr>()
            .map_err(|_| vm.new_type_error("method name must be a string".to_owned()))?;
        zelf.get_attr(&method, vm)?.call(call, vm)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyCallable_Check(pointer: *mut Object) -> c_int {
    guard(0, |vm| Ok(c_int::from(arg(vm, pointer)?.is_callable())))
}

#[unsafe(no_mangle)]
pub extern "C" fn PyObject_GetIter(pointer: *mut Object) -> *mut Object {
    object(|vm| Ok(arg(vm, pointer)?.get_iter(vm)?.into()))
}

#[unsafe(no_mangle)]
pub extern "C" fn PyObject_SelfIter(pointer: *mut Object) -> *mut Object {
    unsafe { incref(pointer) };
    pointer
}

#[unsafe(no_mangle)]
pub extern "C" fn PyIter_Next(pointer: *mut Object) -> *mut Object {
    guard(null_mut(), |vm| {
        let iterator = arg(vm, pointer)?;
        match vm.call_method(&iterator, "__next__", ()) {
            Ok(value) => Ok(new_ref(vm, &value)),
            Err(error)
                if error
                    .class()
                    .fast_issubclass(vm.ctx.exceptions.stop_iteration) =>
            {
                Ok(null_mut())
            }
            Err(error) => Err(error),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyIter_Check(pointer: *mut Object) -> c_int {
    guard(0, |vm| {
        Ok(c_int::from(
            arg(vm, pointer)?
                .class()
                .get_attr(vm.ctx.intern_str("__next__"))
                .is_some(),
        ))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyObject_Type(pointer: *mut Object) -> *mut Object {
    object(|vm| Ok(arg(vm, pointer)?.class().to_owned().into()))
}

#[unsafe(no_mangle)]
pub extern "C" fn PyObject_Dir(pointer: *mut Object) -> *mut Object {
    object(|vm| {
        vm.builtins
            .get_attr("dir", vm)?
            .call((arg(vm, pointer)?,), vm)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyObject_ClearWeakRefs(_pointer: *mut Object) {}

fn index(vm: &VirtualMachine, pointer: *mut Object) -> PyResult<PyIntRef> {
    arg(vm, pointer)?.try_index(vm)
}

#[unsafe(no_mangle)]
pub extern "C" fn PyLong_FromLong(value: c_long) -> *mut Object {
    object(|vm| Ok(vm.ctx.new_int(value).into()))
}

#[unsafe(no_mangle)]
pub extern "C" fn PyLong_FromUnsignedLong(value: c_ulong) -> *mut Object {
    object(|vm| Ok(vm.ctx.new_int(value).into()))
}

#[unsafe(no_mangle)]
pub extern "C" fn PyLong_FromLongLong(value: c_longlong) -> *mut Object {
    object(|vm| Ok(vm.ctx.new_int(value).into()))
}

#[unsafe(no_mangle)]
pub extern "C" fn PyLong_FromUnsignedLongLong(value: c_ulonglong) -> *mut Object {
    object(|vm| Ok(vm.ctx.new_int(value).into()))
}

#[unsafe(no_mangle)]
pub extern "C" fn PyLong_FromSsize_t(value: isize) -> *mut Object {
    object(|vm| Ok(vm.ctx.new_int(value).into()))
}

#[unsafe(no_mangle)]
pub extern "C" fn PyLong_FromSize_t(value: usize) -> *mut Object {
    object(|vm| Ok(vm.ctx.new_int(value).into()))
}

#[unsafe(no_mangle)]
pub extern "C" fn PyLong_FromDouble(value: f64) -> *mut Object {
    object(|vm| {
        vm.ctx
            .types
            .int_type
            .as_object()
            .call((vm.ctx.new_float(value),), vm)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_FromString(
    text: *const c_char,
    end: *mut *mut c_char,
    base: c_int,
) -> *mut Object {
    let source = unsafe { CStr::from_ptr(text) };
    if !end.is_null() {
        unsafe { *end = text.add(source.to_bytes().len()).cast_mut() };
    }
    object(|vm| {
        vm.ctx.types.int_type.as_object().call(
            (vm.ctx.new_str(source.to_string_lossy().into_owned()), base),
            vm,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyLong_FromUnicodeObject(value: *mut Object, base: c_int) -> *mut Object {
    object(|vm| {
        vm.ctx
            .types
            .int_type
            .as_object()
            .call((arg(vm, value)?, base), vm)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyLong_AsLong(pointer: *mut Object) -> c_long {
    guard(-1, |vm| index(vm, pointer)?.try_to_primitive(vm))
}

#[unsafe(no_mangle)]
pub extern "C" fn PyLong_AsInt(pointer: *mut Object) -> c_int {
    guard(-1, |vm| index(vm, pointer)?.try_to_primitive(vm))
}

#[unsafe(no_mangle)]
pub extern "C" fn PyLong_AsLongLong(pointer: *mut Object) -> c_longlong {
    guard(-1, |vm| index(vm, pointer)?.try_to_primitive(vm))
}

#[unsafe(no_mangle)]
pub extern "C" fn PyLong_AsSsize_t(pointer: *mut Object) -> isize {
    guard(-1, |vm| index(vm, pointer)?.try_to_primitive(vm))
}

#[unsafe(no_mangle)]
pub extern "C" fn PyLong_AsSize_t(pointer: *mut Object) -> usize {
    guard(usize::MAX, |vm| index(vm, pointer)?.try_to_primitive(vm))
}

#[unsafe(no_mangle)]
pub extern "C" fn PyLong_AsUnsignedLong(pointer: *mut Object) -> c_ulong {
    guard(c_ulong::MAX, |vm| index(vm, pointer)?.try_to_primitive(vm))
}

#[unsafe(no_mangle)]
pub extern "C" fn PyLong_AsUnsignedLongLong(pointer: *mut Object) -> c_ulonglong {
    guard(c_ulonglong::MAX, |vm| {
        index(vm, pointer)?.try_to_primitive(vm)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyLong_AsUnsignedLongMask(pointer: *mut Object) -> c_ulong {
    guard(c_ulong::MAX, |vm| {
        Ok(index(vm, pointer)?.as_u64_mask() as c_ulong)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyLong_AsUnsignedLongLongMask(pointer: *mut Object) -> c_ulonglong {
    guard(c_ulonglong::MAX, |vm| Ok(index(vm, pointer)?.as_u64_mask()))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_AsLongAndOverflow(
    pointer: *mut Object,
    overflow: *mut c_int,
) -> c_long {
    unsafe { *overflow = 0 };
    guard(-1, |vm| {
        let value = index(vm, pointer)?;
        match value.try_to_primitive::<c_long>(vm) {
            Ok(value) => Ok(value),
            Err(_) => {
                unsafe {
                    *overflow = if value.as_bigint().sign() == nanocode_numeric::Sign::Minus {
                        -1
                    } else {
                        1
                    }
                };
                Ok(-1)
            }
        }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_AsLongLongAndOverflow(
    pointer: *mut Object,
    overflow: *mut c_int,
) -> c_longlong {
    unsafe { *overflow = 0 };
    guard(-1, |vm| {
        let value = index(vm, pointer)?;
        match value.try_to_primitive::<c_longlong>(vm) {
            Ok(value) => Ok(value),
            Err(_) => {
                unsafe {
                    *overflow = if value.as_bigint().sign() == nanocode_numeric::Sign::Minus {
                        -1
                    } else {
                        1
                    }
                };
                Ok(-1)
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyLong_AsDouble(pointer: *mut Object) -> f64 {
    guard(-1.0, |vm| {
        Ok(arg(vm, pointer)?
            .try_int(vm)?
            .as_object()
            .try_float(vm)?
            .to_f64())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyFloat_FromDouble(value: f64) -> *mut Object {
    object(|vm| Ok(vm.ctx.new_float(value).into()))
}

#[unsafe(no_mangle)]
pub extern "C" fn PyFloat_AsDouble(pointer: *mut Object) -> f64 {
    guard(-1.0, |vm| Ok(arg(vm, pointer)?.try_float(vm)?.to_f64()))
}

#[unsafe(no_mangle)]
pub extern "C" fn PyBool_FromLong(value: c_long) -> *mut Object {
    let pointer: *mut Object = if value != 0 {
        bridge::_Py_TrueStruct.get().cast()
    } else {
        bridge::_Py_FalseStruct.get().cast()
    };
    unsafe { incref(pointer) };
    pointer
}

#[unsafe(no_mangle)]
pub extern "C" fn PyNumber_Index(pointer: *mut Object) -> *mut Object {
    object(|vm| Ok(index(vm, pointer)?.into()))
}

#[unsafe(no_mangle)]
pub extern "C" fn PyNumber_Long(pointer: *mut Object) -> *mut Object {
    object(|vm| Ok(arg(vm, pointer)?.try_int(vm)?.into()))
}

#[unsafe(no_mangle)]
pub extern "C" fn PyNumber_Float(pointer: *mut Object) -> *mut Object {
    object(|vm| Ok(arg(vm, pointer)?.try_float(vm)?.into()))
}

#[unsafe(no_mangle)]
pub extern "C" fn PyNumber_AsSsize_t(pointer: *mut Object, overflow: *mut Object) -> isize {
    guard(-1, |vm| {
        let value = index(vm, pointer)?;
        match value.try_to_primitive::<isize>(vm) {
            Ok(value) => Ok(value),
            Err(error) if overflow.is_null() => Ok(
                if value.as_bigint().sign() == nanocode_numeric::Sign::Minus {
                    isize::MIN
                } else {
                    let _ = error;
                    isize::MAX
                },
            ),
            Err(_) => Err(exception(
                vm,
                arg(vm, overflow)?,
                Some(
                    vm.ctx
                        .new_str(format!(
                            "cannot fit '{}' into an index-sized integer",
                            value.class().name()
                        ))
                        .into(),
                ),
            )?),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyNumber_ToBase(pointer: *mut Object, base: c_int) -> *mut Object {
    object(|vm| {
        let function = match base {
            2 => "bin",
            8 => "oct",
            10 => return Ok(index(vm, pointer)?.as_object().str(vm)?.into()),
            16 => "hex",
            _ => {
                return Err(
                    vm.new_system_error("PyNumber_ToBase: base must be 2, 8, 10 or 16".to_owned())
                );
            }
        };
        vm.builtins
            .get_attr(function, vm)?
            .call((arg(vm, pointer)?,), vm)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyNumber_Check(pointer: *mut Object) -> c_int {
    guard(0, |vm| {
        let value = arg(vm, pointer)?;
        Ok(c_int::from(
            value.try_index_opt(vm).is_some() || value.try_float_opt(vm).is_some(),
        ))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyIndex_Check(pointer: *mut Object) -> c_int {
    guard(0, |vm| {
        Ok(c_int::from(
            arg(vm, pointer)?
                .class()
                .get_attr(vm.ctx.intern_str("__index__"))
                .is_some(),
        ))
    })
}

fn string(vm: &VirtualMachine, pointer: *mut Object) -> PyResult<PyRef<PyStr>> {
    arg(vm, pointer)?
        .downcast::<PyStr>()
        .map_err(|object| vm.new_type_error(format!("expected str, got {}", object.class().name())))
}

#[unsafe(no_mangle)]
pub extern "C" fn PyUnicode_New(length: isize, max: u32) -> *mut Object {
    if max > MAX_UNICODE || length < 0 {
        guard((), |vm| {
            set_error(
                vm.new_system_error("invalid maximum character passed to PyUnicode_New".to_owned()),
            );
            Ok(())
        });
        return null_mut();
    }
    let pointer = bridge::new_unicode(bridge::PyUnicode_Type.get(), length.unsigned_abs(), max, 1);
    unsafe { heap::header(pointer) }.kind.set(Kind::Pending);
    pointer
}

fn decode(vm: &VirtualMachine, data: &[u8], errors: *const c_char) -> PyResult {
    match std::str::from_utf8(data) {
        Ok(valid) => Ok(vm.ctx.new_str(valid).into()),
        Err(_) => {
            let errors = text(errors).unwrap_or("strict");
            vm.call_method(
                vm.ctx.new_bytes(data.to_vec()).as_object(),
                "decode",
                ("utf-8", errors),
            )
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn PyUnicode_FromString(data: *const c_char) -> *mut Object {
    object(|vm| decode(vm, unsafe { CStr::from_ptr(data) }.to_bytes(), null()))
}

#[unsafe(no_mangle)]
pub extern "C" fn PyUnicode_FromStringAndSize(data: *const c_char, length: isize) -> *mut Object {
    if data.is_null() {
        return PyUnicode_New(length, u32::from(u8::MAX >> 1));
    }
    object(|vm| {
        decode(
            vm,
            unsafe { slice::from_raw_parts(data.cast::<u8>(), length.unsigned_abs()) },
            null(),
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyUnicode_DecodeUTF8(
    data: *const c_char,
    length: isize,
    errors: *const c_char,
) -> *mut Object {
    object(|vm| {
        decode(
            vm,
            unsafe { slice::from_raw_parts(data.cast::<u8>(), length.unsigned_abs()) },
            errors,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyUnicode_DecodeLatin1(
    data: *const c_char,
    length: isize,
    _errors: *const c_char,
) -> *mut Object {
    object(|vm| {
        Ok(vm
            .ctx
            .new_str(
                unsafe { slice::from_raw_parts(data.cast::<u8>(), length.unsigned_abs()) }
                    .iter()
                    .map(|&byte| char::from(byte))
                    .collect::<String>(),
            )
            .into())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyUnicode_FromKindAndData(
    kind: c_int,
    buffer: *const c_void,
    length: isize,
) -> *mut Object {
    object(|vm| {
        let length = length.unsigned_abs();
        let mut out = rustpython_vm::common::wtf8::Wtf8Buf::with_capacity(length);
        match kind {
            1 => unsafe { slice::from_raw_parts(buffer.cast::<u8>(), length) }
                .iter()
                .for_each(|&unit| out.push_char(char::from(unit))),
            2 => unsafe { slice::from_raw_parts(buffer.cast::<u16>(), length) }
                .iter()
                .for_each(|&unit| out.push(bridge::code_point(u32::from(unit)))),
            4 => unsafe { slice::from_raw_parts(buffer.cast::<u32>(), length) }
                .iter()
                .for_each(|&unit| out.push(bridge::code_point(unit))),
            _ => return Err(vm.new_system_error("invalid kind".to_owned())),
        }
        Ok(vm.ctx.new_str(out).into())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyUnicode_FromOrdinal(ordinal: c_int) -> *mut Object {
    object(|vm| {
        let point = u32::try_from(ordinal)
            .ok()
            .filter(|&point| point <= MAX_UNICODE)
            .ok_or_else(|| vm.new_value_error("chr() arg not in range(0x110000)".to_owned()))?;
        let mut out = rustpython_vm::common::wtf8::Wtf8Buf::new();
        out.push(bridge::code_point(point));
        Ok(vm.ctx.new_str(out).into())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyUnicode_FromObject(pointer: *mut Object) -> *mut Object {
    object(|vm| {
        let value = arg(vm, pointer)?;
        if value.class().is(vm.ctx.types.str_type) {
            return Ok(value);
        }
        let text = value.downcast::<PyStr>().map_err(|value| {
            vm.new_type_error(format!(
                "Can't convert '{}' object to str implicitly",
                value.class().name()
            ))
        })?;
        Ok(vm.ctx.new_str(text.as_wtf8().to_owned()).into())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyUnicode_GetLength(pointer: *mut Object) -> isize {
    size(|vm| Ok(string(vm, pointer)?.char_len()))
}

#[unsafe(no_mangle)]
pub extern "C" fn PyUnicode_ReadChar(pointer: *mut Object, at: isize) -> u32 {
    guard(u32::MAX, |vm| {
        let text = string(vm, pointer)?;
        let at = usize::try_from(at)
            .map_err(|_| vm.new_index_error("string index out of range".to_owned()))?;
        text.as_wtf8()
            .code_points()
            .nth(at)
            .map(|point| point.to_u32())
            .ok_or_else(|| vm.new_index_error("string index out of range".to_owned()))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyUnicode_Substring(
    pointer: *mut Object,
    start: isize,
    end: isize,
) -> *mut Object {
    if pointer.is_null() || unsafe { (*(*pointer).ty).flags } & TPFLAGS_UNICODE_SUBCLASS == 0 {
        return guard(null_mut(), |vm| {
            Err(vm.new_type_error("PyUnicode_Substring needs a str".to_owned()))
        });
    }
    let head = unsafe { &*pointer.cast::<Ascii>() };
    let start = start.clamp(0, head.length);
    let end = end.clamp(start, head.length);
    let kind = ((head.state & KIND_MASK) >> KIND_SHIFT) as usize;
    let data = bridge::unicode_data(pointer);
    if head.state & ASCII != 0 {
        return object(|vm| {
            let bytes = unsafe {
                slice::from_raw_parts(data.add(start.unsigned_abs()), (end - start).unsigned_abs())
            };
            let text = core::str::from_utf8(bytes)
                .map_err(|_| vm.new_system_error("an ASCII str holds non-ASCII data".to_owned()))?;
            Ok(vm.ctx.new_str(text).into())
        });
    }
    PyUnicode_FromKindAndData(
        kind as c_int,
        unsafe { data.add(start.unsigned_abs() * kind) }.cast(),
        end - start,
    )
}

#[unsafe(no_mangle)]
pub extern "C" fn PyUnicode_Join(separator: *mut Object, items: *mut Object) -> *mut Object {
    object(|vm| vm.call_method(&*arg(vm, separator)?, "join", (arg(vm, items)?,)))
}

#[unsafe(no_mangle)]
pub extern "C" fn PyUnicode_Concat(a: *mut Object, b: *mut Object) -> *mut Object {
    object(|vm| vm._add(&*arg(vm, a)?, &*arg(vm, b)?))
}

#[unsafe(no_mangle)]
pub extern "C" fn PyUnicode_Contains(container: *mut Object, element: *mut Object) -> c_int {
    truth(|vm| {
        vm.call_method(&*arg(vm, container)?, "__contains__", (arg(vm, element)?,))?
            .is_true(vm)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyUnicode_Compare(a: *mut Object, b: *mut Object) -> c_int {
    guard(-1, |vm| {
        let (a, b) = (string(vm, a)?, string(vm, b)?);
        Ok(match a.as_wtf8().cmp(b.as_wtf8()) {
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => 1,
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyUnicode_CompareWithASCIIString(a: *mut Object, b: *const c_char) -> c_int {
    guard(-1, |vm| {
        let a = string(vm, a)?;
        let b = unsafe { CStr::from_ptr(b) }.to_bytes();
        Ok(match a.as_bytes().cmp(b) {
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => 1,
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyUnicode_EqualToUTF8(a: *mut Object, b: *const c_char) -> c_int {
    guard(0, |vm| {
        Ok(c_int::from(
            string(vm, a)?.as_bytes() == unsafe { CStr::from_ptr(b) }.to_bytes(),
        ))
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_AsUTF8AndSize(
    pointer: *mut Object,
    length: *mut isize,
) -> *const c_char {
    guard(null(), |vm| {
        let text = string(vm, pointer)?;
        let state = unsafe { (*pointer.cast::<Ascii>()).state };
        let count = unsafe { (*pointer.cast::<Ascii>()).length };
        if state & ASCII != 0 {
            if !length.is_null() {
                unsafe { *length = count };
            }
            return Ok(bridge::unicode_data(pointer).cast_const().cast());
        }
        let compact = pointer.cast::<Compact>();
        if unsafe { (*compact).utf8 }.is_null() {
            let utf8 = text.to_str().ok_or_else(|| {
                vm.new_exception_msg(
                    vm.ctx.exceptions.unicode_encode_error.to_owned(),
                    "'utf-8' codec can't encode characters: surrogates not allowed".into(),
                )
            })?;
            let buffer = heap::allocate(utf8.len() + 1, true).cast::<u8>();
            unsafe {
                buffer.copy_from_nonoverlapping(utf8.as_ptr(), utf8.len());
                (*compact).utf8 = buffer.cast();
                (*compact).utf8_length = utf8.len() as isize;
            }
        }
        if !length.is_null() {
            unsafe { *length = (*compact).utf8_length };
        }
        Ok(unsafe { (*compact).utf8 }.cast_const())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_AsUTF8(pointer: *mut Object) -> *const c_char {
    unsafe { PyUnicode_AsUTF8AndSize(pointer, null_mut()) }
}

#[unsafe(no_mangle)]
pub extern "C" fn PyUnicode_AsEncodedString(
    pointer: *mut Object,
    encoding: *const c_char,
    errors: *const c_char,
) -> *mut Object {
    object(|vm| {
        vm.call_method(
            &*arg(vm, pointer)?,
            "encode",
            (
                text(encoding).unwrap_or("utf-8"),
                text(errors).unwrap_or("strict"),
            ),
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyUnicode_AsUTF8String(pointer: *mut Object) -> *mut Object {
    object(|vm| vm.call_method(&*arg(vm, pointer)?, "encode", ("utf-8",)))
}

#[unsafe(no_mangle)]
pub extern "C" fn PyUnicode_InternFromString(data: *const c_char) -> *mut Object {
    PyUnicode_FromString(data)
}

#[unsafe(no_mangle)]
pub extern "C" fn PyUnicode_InternInPlace(_pointer: *mut *mut Object) {}

#[unsafe(no_mangle)]
pub extern "C" fn _PyUnicode_ToLowercase(point: u32) -> u32 {
    char::from_u32(point).map_or(point, |c| {
        let mut lower = c.to_lowercase();
        match (lower.next(), lower.next()) {
            (Some(single), None) => u32::from(single),
            _ => point,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn _PyUnicode_ToUppercase(point: u32) -> u32 {
    char::from_u32(point).map_or(point, |c| {
        let mut upper = c.to_uppercase();
        match (upper.next(), upper.next()) {
            (Some(single), None) => u32::from(single),
            _ => point,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyBytes_FromStringAndSize(data: *const c_char, length: isize) -> *mut Object {
    let length = length.unsigned_abs();
    let pointer = if data.is_null() {
        bridge::bytes_object(bridge::PyBytes_Type.get(), &vec![0; length], 1)
    } else {
        bridge::bytes_object(
            bridge::PyBytes_Type.get(),
            unsafe { slice::from_raw_parts(data.cast::<u8>(), length) },
            1,
        )
    };
    unsafe { heap::header(pointer) }.kind.set(Kind::Pending);
    pointer
}

#[unsafe(no_mangle)]
pub extern "C" fn PyBytes_FromString(data: *const c_char) -> *mut Object {
    PyBytes_FromStringAndSize(
        data,
        unsafe { CStr::from_ptr(data) }.to_bytes().len() as isize,
    )
}

fn bytes_pointer(vm: &VirtualMachine, pointer: *mut Object) -> PyResult<*mut Bytes> {
    if pointer.is_null() || unsafe { bridge::flags(pointer) } & TPFLAGS_BYTES_SUBCLASS == 0 {
        let name = if pointer.is_null() {
            "NULL".to_owned()
        } else {
            arg(vm, pointer)?.class().name().to_string()
        };
        return Err(vm.new_type_error(format!("expected bytes, {name} found")));
    }
    Ok(pointer.cast())
}

#[unsafe(no_mangle)]
pub extern "C" fn PyBytes_AsString(pointer: *mut Object) -> *mut c_char {
    guard(null_mut(), |vm| {
        Ok(unsafe { (*bytes_pointer(vm, pointer)?).data.as_mut_ptr() })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyBytes_Size(pointer: *mut Object) -> isize {
    guard(-1, |vm| {
        Ok(unsafe { (*bytes_pointer(vm, pointer)?).base.size })
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyBytes_AsStringAndSize(
    pointer: *mut Object,
    data: *mut *mut c_char,
    length: *mut isize,
) -> c_int {
    guard(-1, |vm| {
        let bytes = bytes_pointer(vm, pointer)?;
        unsafe {
            *data = (*bytes).data.as_mut_ptr();
            if !length.is_null() {
                *length = (*bytes).base.size;
            }
        }
        Ok(0)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyBytes_FromObject(pointer: *mut Object) -> *mut Object {
    object(|vm| {
        vm.ctx
            .types
            .bytes_type
            .as_object()
            .call((arg(vm, pointer)?,), vm)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _PyBytes_Resize(slot: *mut *mut Object, length: isize) -> c_int {
    let pointer = unsafe { *slot };
    if pointer.is_null()
        || unsafe { heap::header(pointer) }.kind.get() != Kind::Pending
        || length < 0
    {
        guard((), |vm| {
            set_error(vm.new_system_error("_PyBytes_Resize() works only on new bytes".to_owned()));
            Ok(())
        });
        return -1;
    }
    let resized = unsafe {
        heap::resize(
            pointer.cast(),
            offset_of!(Bytes, data) + length.unsigned_abs() + 1,
        )
    }
    .cast::<Bytes>();
    if resized.is_null() {
        PyErr_NoMemory();
        return -1;
    }
    unsafe {
        (*resized).base.size = length;
        *(*resized).data.as_mut_ptr().add(length.unsigned_abs()) = 0;
        *slot = resized.cast();
    }
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn PyTuple_New(length: isize) -> *mut Object {
    let pointer = bridge::tuple_object(bridge::PyTuple_Type.get(), length.unsigned_abs(), 1);
    unsafe { heap::header(pointer) }.kind.set(Kind::Pending);
    pointer
}

fn tuple_pointer(vm: &VirtualMachine, pointer: *mut Object) -> PyResult<*mut Tuple> {
    if pointer.is_null() || unsafe { bridge::flags(pointer) } & TPFLAGS_TUPLE_SUBCLASS == 0 {
        return Err(
            vm.new_system_error("bad argument to internal function: expected a tuple".to_owned())
        );
    }
    Ok(pointer.cast())
}

#[unsafe(no_mangle)]
pub extern "C" fn PyTuple_Size(pointer: *mut Object) -> isize {
    guard(-1, |vm| {
        Ok(unsafe { (*tuple_pointer(vm, pointer)?).base.size })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyTuple_GetItem(pointer: *mut Object, at: isize) -> *mut Object {
    guard(null_mut(), |vm| {
        let tuple = tuple_pointer(vm, pointer)?;
        if at < 0 || at >= unsafe { (*tuple).base.size } {
            return Err(vm.new_index_error("tuple index out of range".to_owned()));
        }
        Ok(unsafe { *(*tuple).items.as_ptr().add(at.unsigned_abs()) })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyTuple_SetItem(pointer: *mut Object, at: isize, item: *mut Object) -> c_int {
    guard(-1, |vm| {
        let tuple = tuple_pointer(vm, pointer)?;
        if at < 0 || at >= unsafe { (*tuple).base.size } {
            unsafe { xdecref(item) };
            return Err(vm.new_index_error("tuple assignment index out of range".to_owned()));
        }
        unsafe {
            let slot = (*tuple).items.as_mut_ptr().add(at.unsigned_abs());
            let old = *slot;
            *slot = item;
            xdecref(old);
        }
        Ok(0)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyTuple_GetSlice(pointer: *mut Object, low: isize, high: isize) -> *mut Object {
    object(|vm| {
        let tuple = arg(vm, pointer)?
            .downcast::<PyTuple>()
            .map_err(|_| vm.new_system_error("expected a tuple".to_owned()))?;
        let items = tuple.as_slice();
        let low = low.clamp(0, items.len() as isize).unsigned_abs();
        let high = high
            .clamp(low as isize, items.len() as isize)
            .unsigned_abs();
        Ok(vm.ctx.new_tuple(items[low..high].to_vec()).into())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyList_New(length: isize) -> *mut Object {
    let pointer = bridge::list_object(bridge::PyList_Type.get(), length.unsigned_abs(), 1);
    unsafe { heap::header(pointer) }.kind.set(Kind::Pending);
    pointer
}

fn list_pointer(vm: &VirtualMachine, pointer: *mut Object) -> PyResult<*mut List> {
    if pointer.is_null() || unsafe { bridge::flags(pointer) } & TPFLAGS_LIST_SUBCLASS == 0 {
        return Err(
            vm.new_system_error("bad argument to internal function: expected a list".to_owned())
        );
    }
    Ok(pointer.cast())
}

fn rust_list(vm: &VirtualMachine, pointer: *mut Object) -> PyResult<Option<PyRef<PyList>>> {
    if unsafe { heap::header(pointer) }.kind.get() != Kind::Proxy {
        return Ok(None);
    }
    Ok(arg(vm, pointer)?.downcast::<PyList>().ok())
}

#[unsafe(no_mangle)]
pub extern "C" fn PyList_Size(pointer: *mut Object) -> isize {
    guard(-1, |vm| {
        Ok(unsafe { (*list_pointer(vm, pointer)?).base.size })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyList_GetItem(pointer: *mut Object, at: isize) -> *mut Object {
    guard(null_mut(), |vm| {
        let list = list_pointer(vm, pointer)?;
        if at < 0 || at >= unsafe { (*list).base.size } {
            return Err(vm.new_index_error("list index out of range".to_owned()));
        }
        Ok(unsafe { *(*list).items.add(at.unsigned_abs()) })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyList_SetItem(pointer: *mut Object, at: isize, item: *mut Object) -> c_int {
    guard(-1, |vm| {
        let list = list_pointer(vm, pointer)?;
        if at < 0 || at >= unsafe { (*list).base.size } {
            unsafe { xdecref(item) };
            return Err(vm.new_index_error("list assignment index out of range".to_owned()));
        }
        if let Some(rust) = rust_list(vm, pointer)? {
            rust.borrow_vec_mut()[at.unsigned_abs()] = arg(vm, item)?;
        }
        unsafe {
            let slot = (*list).items.add(at.unsigned_abs());
            let old = *slot;
            *slot = item;
            xdecref(old);
        }
        Ok(0)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyList_Append(pointer: *mut Object, item: *mut Object) -> c_int {
    guard(-1, |vm| {
        list_pointer(vm, pointer)?;
        if item.is_null() {
            return Err(vm.new_system_error("PyList_Append() got NULL".to_owned()));
        }
        if let Some(rust) = rust_list(vm, pointer)? {
            rust.borrow_vec_mut().push(arg(vm, item)?);
        }
        unsafe {
            incref(item);
            bridge::list_push(pointer, item);
        }
        Ok(0)
    })
}

fn list_method(
    pointer: *mut Object,
    f: impl FnOnce(&VirtualMachine, &PyObjectRef) -> PyResult<()>,
) -> c_int {
    status(|vm| {
        list_pointer(vm, pointer)?;
        let list = arg(vm, pointer)?;
        f(vm, &list)?;
        bridge::refresh(vm, &list, pointer);
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyList_Insert(pointer: *mut Object, at: isize, item: *mut Object) -> c_int {
    list_method(pointer, |vm, list| {
        vm.call_method(list, "insert", (at, arg(vm, item)?))
            .map(drop)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyList_Reverse(pointer: *mut Object) -> c_int {
    list_method(pointer, |vm, list| {
        vm.call_method(list, "reverse", ()).map(drop)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyList_Sort(pointer: *mut Object) -> c_int {
    list_method(pointer, |vm, list| {
        vm.call_method(list, "sort", ()).map(drop)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyList_AsTuple(pointer: *mut Object) -> *mut Object {
    object(|vm| {
        list_pointer(vm, pointer)?;
        let list = arg(vm, pointer)?
            .downcast::<PyList>()
            .map_err(|_| vm.new_system_error("expected a list".to_owned()))?;
        let items = list.borrow_vec().to_vec();
        Ok(vm.ctx.new_tuple(items).into())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyList_GetSlice(pointer: *mut Object, low: isize, high: isize) -> *mut Object {
    object(|vm| {
        let list = arg(vm, pointer)?
            .downcast::<PyList>()
            .map_err(|_| vm.new_system_error("expected a list".to_owned()))?;
        let items = list.borrow_vec().to_vec();
        let low = low.clamp(0, items.len() as isize).unsigned_abs();
        let high = high
            .clamp(low as isize, items.len() as isize)
            .unsigned_abs();
        Ok(vm.ctx.new_list(items[low..high].to_vec()).into())
    })
}

fn dict(vm: &VirtualMachine, pointer: *mut Object) -> PyResult<PyRef<PyDict>> {
    arg(vm, pointer)?.downcast::<PyDict>().map_err(|_| {
        vm.new_system_error("bad argument to internal function: expected a dict".to_owned())
    })
}

fn dict_changed(pointer: *mut Object, dict: &PyDict) {
    if unsafe { bridge::flags(pointer) } & TPFLAGS_DICT_SUBCLASS != 0
        && unsafe { heap::header(pointer) }.kind.get() == Kind::Proxy
    {
        unsafe { (*pointer.cast::<Dict>()).used = dict.__len__() as isize };
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn PyDict_New() -> *mut Object {
    object(|vm| Ok(vm.ctx.new_dict().into()))
}

#[unsafe(no_mangle)]
pub extern "C" fn PyDict_GetItemWithError(pointer: *mut Object, key: *mut Object) -> *mut Object {
    borrowed_lookup(pointer, key)
}

fn borrowed_lookup(pointer: *mut Object, key: *mut Object) -> *mut Object {
    guard(null_mut(), |vm| {
        let found = dict(vm, pointer)?.get_item_opt(&*arg(vm, key)?, vm)?;
        Ok(found.map_or(null_mut(), |value| borrowed(vm, &value)))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyDict_GetItem(pointer: *mut Object, key: *mut Object) -> *mut Object {
    let saved = take_error();
    let found = borrowed_lookup(pointer, key);
    take_error();
    if let Some(saved) = saved {
        set_error(saved);
    }
    found
}

#[unsafe(no_mangle)]
pub extern "C" fn PyDict_GetItemString(pointer: *mut Object, key: *const c_char) -> *mut Object {
    let saved = take_error();
    let found = guard(null_mut(), |vm| {
        let found = dict(vm, pointer)?.get_item_opt(name(vm, key)?, vm)?;
        Ok(found.map_or(null_mut(), |value| borrowed(vm, &value)))
    });
    take_error();
    if let Some(saved) = saved {
        set_error(saved);
    }
    found
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_GetItemRef(
    pointer: *mut Object,
    key: *mut Object,
    result: *mut *mut Object,
) -> c_int {
    unsafe { *result = null_mut() };
    guard(-1, |vm| {
        let found = dict(vm, pointer)?.get_item_opt(&*arg(vm, key)?, vm)?;
        Ok(match found {
            Some(value) => {
                unsafe { *result = new_ref(vm, &value) };
                1
            }
            None => 0,
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyDict_SetItem(
    pointer: *mut Object,
    key: *mut Object,
    value: *mut Object,
) -> c_int {
    status(|vm| {
        let target = dict(vm, pointer)?;
        target.set_item(&*arg(vm, key)?, arg(vm, value)?, vm)?;
        dict_changed(pointer, &target);
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyDict_SetItemString(
    pointer: *mut Object,
    key: *const c_char,
    value: *mut Object,
) -> c_int {
    status(|vm| {
        let target = dict(vm, pointer)?;
        target.set_item(name(vm, key)?, arg(vm, value)?, vm)?;
        dict_changed(pointer, &target);
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyDict_DelItem(pointer: *mut Object, key: *mut Object) -> c_int {
    status(|vm| {
        let target = dict(vm, pointer)?;
        target.del_item(&*arg(vm, key)?, vm)?;
        dict_changed(pointer, &target);
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyDict_DelItemString(pointer: *mut Object, key: *const c_char) -> c_int {
    status(|vm| {
        let target = dict(vm, pointer)?;
        target.del_item(name(vm, key)?, vm)?;
        dict_changed(pointer, &target);
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyDict_Contains(pointer: *mut Object, key: *mut Object) -> c_int {
    truth(|vm| {
        Ok(dict(vm, pointer)?
            .get_item_opt(&*arg(vm, key)?, vm)?
            .is_some())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyDict_Copy(pointer: *mut Object) -> *mut Object {
    object(|vm| Ok(dict(vm, pointer)?.copy().into_ref(&vm.ctx).into()))
}

#[unsafe(no_mangle)]
pub extern "C" fn PyDict_Size(pointer: *mut Object) -> isize {
    size(|vm| Ok(dict(vm, pointer)?.__len__()))
}

#[unsafe(no_mangle)]
pub extern "C" fn PyDict_Clear(pointer: *mut Object) {
    guard((), |vm| {
        let target = dict(vm, pointer)?;
        target.clear();
        dict_changed(pointer, &target);
        Ok(())
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn PyDict_Keys(pointer: *mut Object) -> *mut Object {
    object(|vm| Ok(vm.ctx.new_list(dict(vm, pointer)?.keys_vec()).into()))
}

#[unsafe(no_mangle)]
pub extern "C" fn PyDict_Values(pointer: *mut Object) -> *mut Object {
    object(|vm| Ok(vm.ctx.new_list(dict(vm, pointer)?.values_vec()).into()))
}

#[unsafe(no_mangle)]
pub extern "C" fn PyDict_Items(pointer: *mut Object) -> *mut Object {
    object(|vm| {
        Ok(vm
            .ctx
            .new_list(
                dict(vm, pointer)?
                    .items_vec()
                    .into_iter()
                    .map(|(key, value)| vm.ctx.new_tuple(vec![key, value]).into())
                    .collect(),
            )
            .into())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyDict_Update(pointer: *mut Object, other: *mut Object) -> c_int {
    status(|vm| {
        let target = dict(vm, pointer)?;
        vm.call_method(target.as_object(), "update", (arg(vm, other)?,))?;
        dict_changed(pointer, &target);
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyDict_Merge(pointer: *mut Object, other: *mut Object, replace: c_int) -> c_int {
    status(|vm| {
        let target = dict(vm, pointer)?;
        let source = arg(vm, other)?;
        let keys = vm.call_method(&source, "keys", ())?;
        for key in keys.try_to_value::<Vec<PyObjectRef>>(vm)? {
            if replace != 0 || target.get_item_opt(&*key, vm)?.is_none() {
                let value = source.get_item(&*key, vm)?;
                target.set_item(&*key, value, vm)?;
            }
        }
        dict_changed(pointer, &target);
        Ok(())
    })
}

thread_local! {
    static SNAPSHOTS: RefCell<HashMap<usize, Vec<(PyObjectRef, PyObjectRef)>>> = RefCell::new(HashMap::new());
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_Next(
    pointer: *mut Object,
    position: *mut isize,
    key: *mut *mut Object,
    value: *mut *mut Object,
) -> c_int {
    guard(0, |vm| {
        let at = unsafe { *position };
        if at == 0 {
            let items = dict(vm, pointer)?.items_vec();
            SNAPSHOTS.with_borrow_mut(|snapshots| snapshots.insert(pointer as usize, items));
        }
        let entry = SNAPSHOTS.with_borrow(|snapshots| {
            snapshots
                .get(&(pointer as usize))
                .and_then(|items| items.get(at.unsigned_abs()).cloned())
        });
        let Some((found_key, found_value)) = entry else {
            SNAPSHOTS.with_borrow_mut(|snapshots| snapshots.remove(&(pointer as usize)));
            return Ok(0);
        };
        unsafe {
            *position = at + 1;
            if !key.is_null() {
                *key = borrowed(vm, &found_key);
            }
            if !value.is_null() {
                *value = borrowed(vm, &found_value);
            }
        }
        Ok(1)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PySlice_New(
    start: *mut Object,
    stop: *mut Object,
    step: *mut Object,
) -> *mut Object {
    object(|vm| {
        let part = |pointer: *mut Object| {
            if pointer.is_null() {
                Ok(vm.ctx.none())
            } else {
                arg(vm, pointer)
            }
        };
        vm.ctx
            .types
            .slice_type
            .as_object()
            .call((part(start)?, part(stop)?, part(step)?), vm)
    })
}

fn slice_index(vm: &VirtualMachine, value: &PyObjectRef) -> PyResult<isize> {
    let value = value.try_index(vm)?;
    Ok(value.try_to_primitive::<isize>(vm).unwrap_or(
        if value.as_bigint().sign() == nanocode_numeric::Sign::Minus {
            isize::MIN
        } else {
            isize::MAX
        },
    ))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySlice_Unpack(
    pointer: *mut Object,
    start: *mut isize,
    stop: *mut isize,
    step: *mut isize,
) -> c_int {
    status(|vm| {
        let target = arg(vm, pointer)?
            .downcast::<PySlice>()
            .map_err(|_| vm.new_type_error("expected a slice".to_owned()))?;
        let mut stride = match &target.step {
            Some(value) if !vm.is_none(value) => slice_index(vm, value)?,
            _ => 1,
        };
        if stride == 0 {
            return Err(vm.new_value_error("slice step cannot be zero".to_owned()));
        }
        stride = stride.max(-isize::MAX);
        let first = match &target.start {
            Some(value) if !vm.is_none(value) => slice_index(vm, value)?,
            _ => {
                if stride < 0 {
                    isize::MAX
                } else {
                    0
                }
            }
        };
        let last = if vm.is_none(&target.stop) {
            if stride < 0 { isize::MIN } else { isize::MAX }
        } else {
            slice_index(vm, &target.stop)?
        };
        unsafe {
            *start = first;
            *stop = last;
            *step = stride;
        }
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySlice_AdjustIndices(
    length: isize,
    start: *mut isize,
    stop: *mut isize,
    step: isize,
) -> isize {
    let adjust = |value: &mut isize| {
        if *value < 0 {
            *value += length;
            if *value < 0 {
                *value = if step < 0 { -1 } else { 0 };
            }
        } else if *value >= length {
            *value = if step < 0 { length - 1 } else { length };
        }
    };
    unsafe {
        adjust(&mut *start);
        adjust(&mut *stop);
        if step < 0 {
            if *stop < *start {
                return (*start - *stop - 1) / (-step) + 1;
            }
        } else if *start < *stop {
            return (*stop - *start - 1) / step + 1;
        }
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySlice_GetIndicesEx(
    pointer: *mut Object,
    length: isize,
    start: *mut isize,
    stop: *mut isize,
    step: *mut isize,
    count: *mut isize,
) -> c_int {
    unsafe {
        if PySlice_Unpack(pointer, start, stop, step) < 0 {
            return -1;
        }
        *count = PySlice_AdjustIndices(length, start, stop, *step);
    }
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn PySequence_Check(pointer: *mut Object) -> c_int {
    guard(0, |vm| {
        let value = arg(vm, pointer)?;
        Ok(c_int::from(
            value.downcast_ref::<PyDict>().is_none()
                && value
                    .class()
                    .get_attr(vm.ctx.intern_str("__getitem__"))
                    .is_some(),
        ))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PySequence_Size(pointer: *mut Object) -> isize {
    PyObject_Size(pointer)
}

#[unsafe(no_mangle)]
pub extern "C" fn PySequence_Length(pointer: *mut Object) -> isize {
    PyObject_Size(pointer)
}

#[unsafe(no_mangle)]
pub extern "C" fn PySequence_GetItem(pointer: *mut Object, at: isize) -> *mut Object {
    object(|vm| arg(vm, pointer)?.get_item(vm.ctx.new_int(at).as_object(), vm))
}

#[unsafe(no_mangle)]
pub extern "C" fn PySequence_SetItem(pointer: *mut Object, at: isize, value: *mut Object) -> c_int {
    status(|vm| arg(vm, pointer)?.set_item(vm.ctx.new_int(at).as_object(), arg(vm, value)?, vm))
}

#[unsafe(no_mangle)]
pub extern "C" fn PySequence_GetSlice(
    pointer: *mut Object,
    low: isize,
    high: isize,
) -> *mut Object {
    object(|vm| {
        let range = vm.ctx.types.slice_type.as_object().call((low, high), vm)?;
        arg(vm, pointer)?.get_item(&*range, vm)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PySequence_Contains(pointer: *mut Object, value: *mut Object) -> c_int {
    truth(|vm| {
        vm.call_method(&*arg(vm, pointer)?, "__contains__", (arg(vm, value)?,))?
            .is_true(vm)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PySequence_List(pointer: *mut Object) -> *mut Object {
    object(|vm| {
        vm.ctx
            .types
            .list_type
            .as_object()
            .call((arg(vm, pointer)?,), vm)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PySequence_Tuple(pointer: *mut Object) -> *mut Object {
    object(|vm| {
        vm.ctx
            .types
            .tuple_type
            .as_object()
            .call((arg(vm, pointer)?,), vm)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PySequence_Fast(pointer: *mut Object, message: *const c_char) -> *mut Object {
    object(|vm| {
        let value = arg(vm, pointer)?;
        if value.downcast_ref::<PyList>().is_some() || value.downcast_ref::<PyTuple>().is_some() {
            return Ok(value);
        }
        vm.ctx
            .types
            .list_type
            .as_object()
            .call((value,), vm)
            .map_err(|_| {
                vm.new_type_error(text(message).unwrap_or("expected a sequence").to_owned())
            })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyMapping_Check(pointer: *mut Object) -> c_int {
    guard(0, |vm| {
        Ok(c_int::from(
            arg(vm, pointer)?
                .class()
                .get_attr(vm.ctx.intern_str("__getitem__"))
                .is_some(),
        ))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyMapping_Size(pointer: *mut Object) -> isize {
    PyObject_Size(pointer)
}

#[unsafe(no_mangle)]
pub extern "C" fn PyMapping_Length(pointer: *mut Object) -> isize {
    PyObject_Size(pointer)
}

fn mapping_list(pointer: *mut Object, method: &'static str) -> *mut Object {
    object(|vm| {
        let view = vm.call_method(&*arg(vm, pointer)?, method, ())?;
        vm.ctx.types.list_type.as_object().call((view,), vm)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyMapping_Keys(pointer: *mut Object) -> *mut Object {
    mapping_list(pointer, "keys")
}

#[unsafe(no_mangle)]
pub extern "C" fn PyMapping_Values(pointer: *mut Object) -> *mut Object {
    mapping_list(pointer, "values")
}

#[unsafe(no_mangle)]
pub extern "C" fn PyMapping_Items(pointer: *mut Object) -> *mut Object {
    mapping_list(pointer, "items")
}

#[unsafe(no_mangle)]
pub extern "C" fn PyMapping_GetItemString(pointer: *mut Object, key: *const c_char) -> *mut Object {
    object(|vm| arg(vm, pointer)?.get_item(name(vm, key)?, vm))
}

#[unsafe(no_mangle)]
pub extern "C" fn PyImport_ImportModule(module: *const c_char) -> *mut Object {
    object(|vm| import(vm, name(vm, module)?))
}

fn import(vm: &VirtualMachine, module: &str) -> PyResult {
    vm.import(&vm.ctx.new_str(module), 0)?;
    vm.sys_module.get_attr("modules", vm)?.get_item(module, vm)
}

#[unsafe(no_mangle)]
pub extern "C" fn PyImport_Import(module: *mut Object) -> *mut Object {
    object(|vm| {
        let module = string(vm, module)?;
        import(vm, &module.to_string_lossy())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyImport_ImportModuleLevel(
    module: *const c_char,
    globals: *mut Object,
    locals: *mut Object,
    fromlist: *mut Object,
    level: c_int,
) -> *mut Object {
    object(|vm| {
        let part = |pointer: *mut Object| {
            if pointer.is_null() {
                Ok(vm.ctx.none())
            } else {
                arg(vm, pointer)
            }
        };
        vm.builtins.get_attr("__import__", vm)?.call(
            (
                vm.ctx.new_str(name(vm, module)?),
                part(globals)?,
                part(locals)?,
                part(fromlist)?,
                level,
            ),
            vm,
        )
    })
}

const BUFFER_WRITABLE: c_int = 0x1;
const BUFFER_FORMAT: c_int = 0x4;
const BUFFER_ND: c_int = 0x8;
const BUFFER_STRIDES: c_int = 0x10 | BUFFER_ND;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyBuffer_FillInfo(
    view: *mut Buffer,
    owner: *mut Object,
    data: *mut c_void,
    length: isize,
    readonly: c_int,
    flags: c_int,
) -> c_int {
    if flags & BUFFER_WRITABLE != 0 && readonly != 0 {
        guard((), |vm| {
            set_error(vm.new_buffer_error("Object is not writable.".to_owned()));
            Ok(())
        });
        return -1;
    }
    if !owner.is_null() {
        unsafe { incref(owner) };
    }
    unsafe {
        view.write(Buffer {
            buf: data,
            obj: owner,
            len: length,
            itemsize: 1,
            readonly,
            ndim: 1,
            format: if flags & BUFFER_FORMAT != 0 {
                c"B".as_ptr().cast_mut()
            } else {
                null_mut()
            },
            shape: if flags & BUFFER_ND == BUFFER_ND {
                &raw mut (*view).len
            } else {
                null_mut()
            },
            strides: if flags & BUFFER_STRIDES == BUFFER_STRIDES {
                &raw mut (*view).itemsize
            } else {
                null_mut()
            },
            suboffsets: null_mut(),
            internal: null_mut(),
        });
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_GetBuffer(
    pointer: *mut Object,
    view: *mut Buffer,
    flags: c_int,
) -> c_int {
    if !pointer.is_null() && unsafe { bridge::flags(pointer) } & TPFLAGS_BYTES_SUBCLASS != 0 {
        let bytes = pointer.cast::<Bytes>();
        return unsafe {
            PyBuffer_FillInfo(
                view,
                pointer,
                (*bytes).data.as_mut_ptr().cast(),
                (*bytes).base.size,
                1,
                flags,
            )
        };
    }
    if !pointer.is_null()
        && let Some(getbuffer) =
            crate::buffer::procs(unsafe { (*pointer).ty }).and_then(|procs| procs.getbuffer)
    {
        return unsafe { getbuffer(pointer, view, flags) };
    }
    guard(-1, |vm| {
        let value = arg(vm, pointer)?;
        let buffer = PyBuffer::try_from_borrowed_object(vm, &value)?;
        if flags & BUFFER_WRITABLE != 0 {
            return Err(vm.new_buffer_error(
                "the C API copies this buffer, so it cannot be written".to_owned(),
            ));
        }
        let (copy, length) = buffer.contiguous_or_collect(|data| {
            let copy = heap::allocate(data.len(), false).cast::<u8>();
            unsafe { copy.copy_from_nonoverlapping(data.as_ptr(), data.len()) };
            (copy, data.len())
        });
        if unsafe { PyBuffer_FillInfo(view, pointer, copy.cast(), length as isize, 1, flags) } < 0 {
            unsafe { heap::release(copy.cast()) };
            return Ok(-1);
        }
        unsafe { (*view).internal = copy.cast() };
        Ok(0)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyBuffer_Release(view: *mut Buffer) {
    unsafe {
        let owner = (*view).obj;
        match crate::buffer::procs(if owner.is_null() {
            null_mut()
        } else {
            (*owner).ty
        }) {
            Some(procs) => {
                if let Some(release) = procs.release {
                    release(owner, view);
                }
            }
            None => heap::release((*view).internal),
        }
        (*view).internal = null_mut();
        (*view).obj = null_mut();
        xdecref(owner);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn PyObject_CheckBuffer(pointer: *mut Object) -> c_int {
    guard(0, |vm| {
        Ok(c_int::from(
            PyBuffer::try_from_borrowed_object(vm, &*arg(vm, pointer)?).is_ok(),
        ))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn Py_GenericAlias(origin: *mut Object, args: *mut Object) -> *mut Object {
    object(|vm| {
        vm.ctx
            .types
            .generic_alias_type
            .as_object()
            .call((arg(vm, origin)?, arg(vm, args)?), vm)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyThread_allocate_lock() -> *mut c_void {
    heap::allocate(size_of::<usize>(), true)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyThread_free_lock(lock: *mut c_void) {
    unsafe { heap::release(lock) };
}

#[unsafe(no_mangle)]
pub extern "C" fn PyThread_acquire_lock(_lock: *mut c_void, _wait: c_int) -> c_int {
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn PyThread_release_lock(_lock: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn PyEval_SaveThread() -> *mut c_void {
    null_mut()
}

#[unsafe(no_mangle)]
pub extern "C" fn PyEval_RestoreThread(_state: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn PyGILState_Ensure() -> c_int {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn PyGILState_Release(_state: c_int) {}

#[unsafe(no_mangle)]
pub extern "C" fn PyGILState_Check() -> c_int {
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn Py_IsInitialized() -> c_int {
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn Py_EnterRecursiveCall(_location: *const c_char) -> c_int {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn Py_LeaveRecursiveCall() {}

fn optional(
    vm: &VirtualMachine,
    pointer: *mut Object,
    attribute: &str,
) -> PyResult<Option<PyObjectRef>> {
    let value = arg(vm, pointer)?;
    vm.get_attribute_opt(&value, vm.ctx.intern_str(attribute))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_GetOptionalAttr(
    pointer: *mut Object,
    attribute: *mut Object,
    result: *mut *mut Object,
) -> c_int {
    unsafe { *result = null_mut() };
    guard(-1, |vm| {
        let attribute = string(vm, attribute)?;
        Ok(match optional(vm, pointer, &attribute.to_string_lossy())? {
            Some(value) => {
                unsafe { *result = new_ref(vm, &value) };
                1
            }
            None => 0,
        })
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_GetOptionalAttrString(
    pointer: *mut Object,
    attribute: *const c_char,
    result: *mut *mut Object,
) -> c_int {
    unsafe { *result = null_mut() };
    guard(-1, |vm| {
        Ok(match optional(vm, pointer, name(vm, attribute)?)? {
            Some(value) => {
                unsafe { *result = new_ref(vm, &value) };
                1
            }
            None => 0,
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyObject_HasAttrWithError(pointer: *mut Object, attribute: *mut Object) -> c_int {
    guard(-1, |vm| {
        let attribute = string(vm, attribute)?;
        Ok(c_int::from(
            optional(vm, pointer, &attribute.to_string_lossy())?.is_some(),
        ))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyObject_GenericGetDict(
    pointer: *mut Object,
    _context: *mut c_void,
) -> *mut Object {
    object(|vm| arg(vm, pointer)?.get_attr("__dict__", vm))
}

#[unsafe(no_mangle)]
pub extern "C" fn PyObject_GenericSetDict(
    pointer: *mut Object,
    value: *mut Object,
    _context: *mut c_void,
) -> c_int {
    status(|vm| arg(vm, pointer)?.set_attr("__dict__", arg(vm, value)?, vm))
}

macro_rules! binary_numbers {
    ($($name:ident => $method:ident),* $(,)?) => {
        $(
            #[unsafe(no_mangle)]
            pub extern "C" fn $name(a: *mut Object, b: *mut Object) -> *mut Object {
                object(|vm| vm.$method(&*arg(vm, a)?, &*arg(vm, b)?))
            }
        )*
    };
}

binary_numbers! {
    PyNumber_Add => _add,
    PyNumber_Subtract => _sub,
    PyNumber_Multiply => _mul,
    PyNumber_Remainder => _mod,
    PyNumber_TrueDivide => _truediv,
    PyNumber_FloorDivide => _floordiv,
    PyNumber_Or => _or,
    PyNumber_And => _and,
    PyNumber_Xor => _xor,
    PyNumber_Lshift => _lshift,
    PyNumber_Rshift => _rshift,
    PyNumber_InPlaceAdd => _iadd,
    PyNumber_InPlaceSubtract => _isub,
    PyNumber_InPlaceMultiply => _imul,
    PyUnicode_Format => _mod,
}

#[unsafe(no_mangle)]
pub extern "C" fn PySequence_Repeat(pointer: *mut Object, count: isize) -> *mut Object {
    object(|vm| vm._mul(&*arg(vm, pointer)?, vm.ctx.new_int(count).as_object()))
}

#[unsafe(no_mangle)]
pub extern "C" fn PyList_GetItemRef(pointer: *mut Object, at: isize) -> *mut Object {
    object(|vm| {
        let list = arg(vm, pointer)?
            .downcast::<PyList>()
            .map_err(|_| vm.new_type_error("expected a list".to_owned()))?;
        let items = list.borrow_vec();
        usize::try_from(at)
            .ok()
            .and_then(|at| items.get(at).cloned())
            .ok_or_else(|| vm.new_index_error("list index out of range".to_owned()))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyArg_ValidateKeywordArguments(keywords: *mut Object) -> c_int {
    guard(0, |vm| {
        let keywords = dict(vm, keywords)?;
        if keywords
            .keys_vec()
            .iter()
            .any(|key| !key.fast_isinstance(vm.ctx.types.str_type))
        {
            return Err(vm.new_type_error("keywords must be strings".to_owned()));
        }
        Ok(1)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyUnicode_AsASCIIString(pointer: *mut Object) -> *mut Object {
    object(|vm| vm.call_method(&*arg(vm, pointer)?, "encode", ("ascii",)))
}

#[unsafe(no_mangle)]
pub extern "C" fn PyUnicode_DecodeASCII(
    data: *const c_char,
    length: isize,
    errors: *const c_char,
) -> *mut Object {
    object(|vm| {
        let bytes = unsafe { slice::from_raw_parts(data.cast::<u8>(), length.unsigned_abs()) };
        let errors = text(errors).unwrap_or("strict");
        vm.call_method(
            vm.ctx.new_bytes(bytes.to_vec()).as_object(),
            "decode",
            ("ascii", errors),
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyUnicode_FindChar(
    pointer: *mut Object,
    character: u32,
    start: isize,
    end: isize,
    direction: c_int,
) -> isize {
    guard(-2, |vm| {
        let text = string(vm, pointer)?;
        let length = text.char_len() as isize;
        let start = start.clamp(0, length).unsigned_abs();
        let end = end.clamp(0, length).unsigned_abs();
        let points: Vec<u32> = text
            .as_wtf8()
            .code_points()
            .skip(start)
            .take(end.saturating_sub(start))
            .map(|point| point.to_u32())
            .collect();
        let found = if direction > 0 {
            points.iter().position(|&point| point == character)
        } else {
            points.iter().rposition(|&point| point == character)
        };
        Ok(found.map_or(-1, |at| (start + at) as isize))
    })
}

#[unsafe(no_mangle)]
pub static _PyByteArray_empty_string: [c_char; 1] = [0];

#[unsafe(no_mangle)]
pub extern "C" fn PyByteArray_Size(pointer: *mut Object) -> isize {
    size(|vm| arg(vm, pointer)?.length(vm))
}

#[unsafe(no_mangle)]
pub extern "C" fn PyByteArray_AsString(pointer: *mut Object) -> *mut c_char {
    guard(null_mut(), |vm| {
        let value = arg(vm, pointer)?;
        let array = value
            .downcast_ref::<rustpython_vm::builtins::PyByteArray>()
            .ok_or_else(|| vm.new_type_error("expected a bytearray".to_owned()))?;
        let data = array.borrow_buf();
        Ok(if data.is_empty() {
            _PyByteArray_empty_string.as_ptr().cast_mut()
        } else {
            data.as_ptr().cast::<c_char>().cast_mut()
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyMemoryView_FromObject(pointer: *mut Object) -> *mut Object {
    object(|vm| {
        vm.ctx
            .types
            .memoryview_type
            .as_object()
            .call((arg(vm, pointer)?,), vm)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyMemoryView_GetContiguous(
    pointer: *mut Object,
    _buffer_type: c_int,
    order: c_char,
) -> *mut Object {
    object(|vm| {
        let view = vm
            .ctx
            .types
            .memoryview_type
            .as_object()
            .call((arg(vm, pointer)?,), vm)?;
        let attribute = if order as u8 == b'F' {
            "f_contiguous"
        } else {
            "c_contiguous"
        };
        if view.get_attr(attribute, vm)?.try_to_bool(vm)? {
            return Ok(view);
        }
        let order = char::from(order as u8).to_string();
        let bytes = vm.call_method(&view, "tobytes", (order,))?;
        vm.ctx.types.memoryview_type.as_object().call((bytes,), vm)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyBuffer_IsContiguous(view: *const Buffer, order: c_char) -> c_int {
    let view = unsafe { &*view };
    if !view.suboffsets.is_null() {
        return 0;
    }
    if view.strides.is_null() {
        return 1;
    }
    let dimensions = view.ndim.unsigned_abs() as usize;
    let shape = unsafe { slice::from_raw_parts(view.shape, dimensions) };
    let strides = unsafe { slice::from_raw_parts(view.strides, dimensions) };
    let walk = |order: &mut dyn Iterator<Item = usize>| {
        let mut expected = view.itemsize;
        for at in order {
            if shape[at] > 1 && strides[at] != expected {
                return false;
            }
            expected *= shape[at];
        }
        true
    };
    let c = walk(&mut (0..dimensions).rev());
    let f = walk(&mut (0..dimensions));
    c_int::from(match order as u8 {
        b'C' => c,
        b'F' => f,
        _ => c || f,
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyFloat_Pack8(value: f64, data: *mut c_char, little: c_int) -> c_int {
    let bytes = if little != 0 {
        value.to_le_bytes()
    } else {
        value.to_be_bytes()
    };
    unsafe {
        data.cast::<u8>()
            .copy_from_nonoverlapping(bytes.as_ptr(), bytes.len())
    };
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn PyFloat_Pack4(value: f64, data: *mut c_char, little: c_int) -> c_int {
    let single = value as f32;
    if single.is_infinite() && value.is_finite() {
        return guard(-1, |vm| {
            Err(vm.new_overflow_error("float too large to pack with f format".to_owned()))
        });
    }
    let bytes = if little != 0 {
        single.to_le_bytes()
    } else {
        single.to_be_bytes()
    };
    unsafe {
        data.cast::<u8>()
            .copy_from_nonoverlapping(bytes.as_ptr(), bytes.len())
    };
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn PyFloat_Unpack8(data: *const c_char, little: c_int) -> f64 {
    let mut bytes = [0; size_of::<f64>()];
    unsafe {
        bytes
            .as_mut_ptr()
            .copy_from_nonoverlapping(data.cast::<u8>(), bytes.len())
    };
    if little != 0 {
        f64::from_le_bytes(bytes)
    } else {
        f64::from_be_bytes(bytes)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn PyFloat_Unpack4(data: *const c_char, little: c_int) -> f64 {
    let mut bytes = [0; size_of::<f32>()];
    unsafe {
        bytes
            .as_mut_ptr()
            .copy_from_nonoverlapping(data.cast::<u8>(), bytes.len())
    };
    f64::from(if little != 0 {
        f32::from_le_bytes(bytes)
    } else {
        f32::from_be_bytes(bytes)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_GetExcInfo(
    ty: *mut *mut Object,
    value: *mut *mut Object,
    traceback: *mut *mut Object,
) {
    let (class, error, trace) = bridge::machine()
        .and_then(|vm| vm.current_exception().map(|error| (vm, error)))
        .map_or((null_mut(), null_mut(), null_mut()), |(vm, error)| {
            let trace = error
                .__traceback__()
                .map_or(null_mut(), |trace| new_ref(vm, trace.as_object()));
            (
                new_ref(vm, error.class().as_object()),
                new_ref(vm, error.as_object()),
                trace,
            )
        });
    unsafe {
        *ty = class;
        *value = error;
        *traceback = trace;
    }
}

fn handled(vm: &VirtualMachine, value: *mut Object) -> PyResult<Option<PyBaseExceptionRef>> {
    if value.is_null() || value == none() {
        return Ok(None);
    }
    arg(vm, value)?
        .downcast::<PyBaseException>()
        .map(Some)
        .map_err(|_| vm.new_type_error("the handled exception must be an exception".to_owned()))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_SetExcInfo(
    ty: *mut Object,
    value: *mut Object,
    traceback: *mut Object,
) {
    guard((), |vm| {
        vm.set_exception(handled(vm, value)?);
        Ok(())
    });
    unsafe {
        xdecref(ty);
        xdecref(value);
        xdecref(traceback);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn PyErr_GetHandledException() -> *mut Object {
    bridge::machine()
        .and_then(|vm| {
            vm.current_exception()
                .map(|error| new_ref(vm, error.as_object()))
        })
        .unwrap_or(null_mut())
}

#[unsafe(no_mangle)]
pub extern "C" fn PyErr_SetHandledException(value: *mut Object) {
    guard((), |vm| {
        vm.set_exception(handled(vm, value)?);
        Ok(())
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn PyException_GetTraceback(pointer: *mut Object) -> *mut Object {
    guard(null_mut(), |vm| {
        let value = arg(vm, pointer)?.get_attr("__traceback__", vm)?;
        Ok(if vm.is_none(&value) {
            null_mut()
        } else {
            new_ref(vm, &value)
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyTraceBack_Here(_frame: *mut Object) -> c_int {
    guard(-1, |vm| {
        let (Some(error), Some(frame)) = (bridge::current_error(), vm.current_frame()) else {
            return Ok(0);
        };
        let next = error.as_object().get_attr("__traceback__", vm)?;
        let lasti = frame.as_object().get_attr("f_lasti", vm)?;
        let lineno = frame.as_object().get_attr("f_lineno", vm)?;
        let lineno = if vm.is_none(&lineno) {
            frame
                .as_object()
                .get_attr("f_code", vm)?
                .get_attr("co_firstlineno", vm)?
        } else {
            lineno
        };
        let traceback = vm
            .ctx
            .types
            .traceback_type
            .as_object()
            .call((next, frame, lasti, lineno), vm)?;
        error.as_object().set_attr("__traceback__", traceback, vm)?;
        Ok(0)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyErr_PrintEx(_set_last: c_int) {
    PyErr_Print();
}

#[unsafe(no_mangle)]
pub extern "C" fn PySys_GetObject(attribute: *const c_char) -> *mut Object {
    guard(null_mut(), |vm| {
        Ok(vm
            .get_attribute_opt(
                vm.sys_module.as_object(),
                vm.ctx.intern_str(name(vm, attribute)?),
            )
            .ok()
            .flatten()
            .map_or(null_mut(), |value| borrowed(vm, &value)))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyEval_GetBuiltins() -> *mut Object {
    bridge::borrowed_object(|vm| Ok(vm.builtins.dict().into()))
}

fn modules(vm: &VirtualMachine) -> PyResult {
    vm.sys_module.get_attr("modules", vm)
}

#[unsafe(no_mangle)]
pub extern "C" fn PyImport_GetModuleDict() -> *mut Object {
    bridge::borrowed_object(modules)
}

#[unsafe(no_mangle)]
pub extern "C" fn PyImport_GetModule(module: *mut Object) -> *mut Object {
    guard(null_mut(), |vm| {
        Ok(modules(vm)?
            .get_item(&*arg(vm, module)?, vm)
            .map_or(null_mut(), |module| new_ref(vm, &module)))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyImport_AddModuleRef(module: *const c_char) -> *mut Object {
    object(|vm| {
        let module = name(vm, module)?;
        let modules = modules(vm)?;
        if let Ok(found) = modules.get_item(module, vm) {
            return Ok(found);
        }
        let created: PyObjectRef = vm.new_module(module, vm.ctx.new_dict(), None).into();
        modules.set_item(module, created.clone(), vm)?;
        Ok(created)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyImport_ImportModuleLevelObject(
    module: *mut Object,
    globals: *mut Object,
    locals: *mut Object,
    fromlist: *mut Object,
    level: c_int,
) -> *mut Object {
    object(|vm| {
        let part = |pointer: *mut Object| {
            if pointer.is_null() {
                Ok(vm.ctx.none())
            } else {
                arg(vm, pointer)
            }
        };
        vm.builtins.get_attr("__import__", vm)?.call(
            (
                arg(vm, module)?,
                part(globals)?,
                part(locals)?,
                part(fromlist)?,
                level,
            ),
            vm,
        )
    })
}

static INTERPRETER: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

#[unsafe(no_mangle)]
pub extern "C" fn PyInterpreterState_Get() -> *mut c_void {
    INTERPRETER.as_ptr().cast()
}

#[unsafe(no_mangle)]
pub extern "C" fn PyInterpreterState_GetID(_interpreter: *mut c_void) -> i64 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn PyObject_GC_Track(_pointer: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn PyObject_GC_UnTrack(_pointer: *mut c_void) {}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_GC_Del(pointer: *mut c_void) {
    unsafe { heap::PyObject_Free(pointer) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _PyObject_GC_New(ty: *mut Type) -> *mut Object {
    unsafe { crate::types::_PyObject_New(ty) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _PyObject_GC_NewVar(ty: *mut Type, items: isize) -> *mut Object {
    unsafe { crate::types::_PyObject_NewVar(ty, items) }
}

fn capsule(
    vm: &VirtualMachine,
    pointer: *mut Object,
) -> PyResult<PyRef<rustpython_vm::builtins::PyCapsule>> {
    arg(vm, pointer)?
        .downcast::<rustpython_vm::builtins::PyCapsule>()
        .map_err(|_| vm.new_value_error("the object is not a capsule".to_owned()))
}

fn same_name(left: Option<&CStr>, right: *const c_char) -> bool {
    match (left, right.is_null()) {
        (None, true) => true,
        (Some(left), false) => left == unsafe { CStr::from_ptr(right) },
        _ => false,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn PyCapsule_New(
    pointer: *mut c_void,
    capsule_name: *const c_char,
    _destructor: *mut c_void,
) -> *mut Object {
    object(|vm| {
        if pointer.is_null() {
            return Err(vm.new_value_error("PyCapsule_New called with null pointer".to_owned()));
        }
        let capsule_name =
            (!capsule_name.is_null()).then(|| unsafe { CStr::from_ptr(capsule_name) });
        Ok(
            rustpython_vm::builtins::PyCapsule::new(pointer, capsule_name, None)
                .into_ref(&vm.ctx)
                .into(),
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyCapsule_GetPointer(
    pointer: *mut Object,
    capsule_name: *const c_char,
) -> *mut c_void {
    guard(null_mut(), |vm| {
        let capsule = capsule(vm, pointer)?;
        if !same_name(capsule.name(), capsule_name) {
            return Err(
                vm.new_value_error("PyCapsule_GetPointer called with incorrect name".to_owned())
            );
        }
        Ok(capsule.pointer())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyCapsule_GetName(pointer: *mut Object) -> *const c_char {
    guard(null(), |vm| {
        Ok(capsule(vm, pointer)?.name().map_or(null(), CStr::as_ptr))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyCapsule_GetContext(pointer: *mut Object) -> *mut c_void {
    guard(null_mut(), |vm| Ok(capsule(vm, pointer)?.context()))
}

#[unsafe(no_mangle)]
pub extern "C" fn PyCapsule_SetContext(pointer: *mut Object, context: *mut c_void) -> c_int {
    status(|vm| {
        capsule(vm, pointer)?.set_context(context);
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyCapsule_SetPointer(pointer: *mut Object, value: *mut c_void) -> c_int {
    status(|vm| {
        capsule(vm, pointer)?.set_pointer(value);
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyCapsule_IsValid(pointer: *mut Object, capsule_name: *const c_char) -> c_int {
    let Some(vm) = bridge::machine() else {
        return 0;
    };
    if pointer.is_null() {
        return 0;
    }
    let Ok(value) = arg(vm, pointer) else {
        return 0;
    };
    value
        .downcast_ref::<rustpython_vm::builtins::PyCapsule>()
        .map_or(0, |capsule| {
            c_int::from(!capsule.pointer().is_null() && same_name(capsule.name(), capsule_name))
        })
}

thread_local! {
    static FUNCTIONS: RefCell<HashMap<usize, (usize, usize)>> = RefCell::new(HashMap::new());
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyCMethod_New(
    def: *mut MethodDef,
    zelf: *mut Object,
    module: *mut Object,
    _class: *mut Type,
) -> *mut Object {
    object(|vm| {
        let function_name = text(unsafe { (*def).name }).unwrap_or("?");
        if !zelf.is_null() {
            unsafe { incref(zelf) };
        }
        let address = def as usize;
        let owner = zelf as usize;
        let f = crate::types::function(vm, function_name, move |vm, args| {
            crate::types::invoke(
                vm,
                address as *const MethodDef,
                &|_| owner as *mut Object,
                args,
            )
        });
        if !module.is_null() {
            let module = arg(vm, module)?;
            if let Ok(module) = module.get_attr("__name__", vm) {
                let _ = f.set_attr("__module__", module, vm);
            }
        }
        FUNCTIONS.with_borrow_mut(|functions| {
            functions.insert(f.as_object() as *const PyObject as usize, (address, owner))
        });
        Ok(f)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyCFunction_NewEx(
    def: *mut MethodDef,
    zelf: *mut Object,
    module: *mut Object,
) -> *mut Object {
    unsafe { PyCMethod_New(def, zelf, module, null_mut()) }
}

fn native(pointer: *mut Object) -> Option<(usize, usize)> {
    let vm = bridge::machine()?;
    let value = arg(vm, pointer).ok()?;
    FUNCTIONS.with_borrow(|functions| {
        functions
            .get(&(value.as_object() as *const PyObject as usize))
            .copied()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyCFunction_GetFunction(pointer: *mut Object) -> Option<unsafe extern "C" fn()> {
    native(pointer).and_then(|(def, _)| unsafe { (*(def as *const MethodDef)).meth })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyCFunction_GetSelf(pointer: *mut Object) -> *mut Object {
    native(pointer).map_or(null_mut(), |(_, owner)| owner as *mut Object)
}

#[unsafe(no_mangle)]
pub extern "C" fn PyCFunction_GetFlags(pointer: *mut Object) -> c_int {
    native(pointer).map_or(0, |(def, _)| unsafe { (*(def as *const MethodDef)).flags })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyMethod_New(function: *mut Object, zelf: *mut Object) -> *mut Object {
    object(|vm| {
        vm.ctx
            .types
            .bound_method_type
            .as_object()
            .call((arg(vm, function)?, arg(vm, zelf)?), vm)
    })
}

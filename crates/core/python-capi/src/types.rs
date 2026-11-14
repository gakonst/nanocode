use std::{
    ffi::{c_char, c_int, c_void},
    mem::transmute,
    ptr::null_mut,
};

use rustpython_vm::{
    AsObject, Py, PyObject, PyObjectRef, PyPayload, PyRef, PyResult, VirtualMachine,
    builtins::{PyType, PyTypeRef},
    function::{Callee, FuncArgs, ItemDoc, PyMethodDef, PyMethodFlags},
    types::{PyTypeFlags, PyTypeSlots},
};

use crate::{
    abi::*,
    bridge::{self, CObject, arg, borrowed, check, enter, finish, incref, new_ref, text, xdecref},
    cast::{call1, call2, call3, call4},
    heap,
};

pub fn builtin(
    name: &'static str,
    doc: Option<&'static str>,
    flags: PyMethodFlags,
    f: impl Fn(&VirtualMachine, FuncArgs) -> PyResult + 'static,
) -> &'static PyMethodDef {
    let call = move |vm: &VirtualMachine, args: FuncArgs, _: Callee| f(vm, args);
    Box::leak(Box::new(PyMethodDef::new_dyn(
        name,
        Box::leak(Box::new(call)),
        flags,
        ItemDoc {
            text: doc,
            ..ItemDoc::NONE
        },
    )))
}

pub fn function(
    vm: &VirtualMachine,
    name: &'static str,
    f: impl Fn(&VirtualMachine, FuncArgs) -> PyResult + 'static,
) -> PyObjectRef {
    builtin(name, None, PyMethodFlags::empty(), f)
        .build_function(&vm.ctx)
        .into()
}

pub fn module_function(
    vm: &VirtualMachine,
    module: &PyObject,
    name: &'static str,
    f: impl Fn(&VirtualMachine, FuncArgs) -> PyResult + 'static,
) -> PyResult<PyObjectRef> {
    let owner = module.get_attr("__name__", vm)?.str(vm)?;
    Ok(builtin(name, None, PyMethodFlags::empty(), f)
        .to_function()
        .with_module(vm.ctx.intern_str(owner.as_wtf8()))
        .with_module_object(module.to_owned())
        .into_ref(&vm.ctx)
        .into())
}

type Call2 = unsafe extern "C" fn(*mut Object, *mut Object) -> *mut Object;
type Call3 = unsafe extern "C" fn(*mut Object, *mut Object, *mut Object) -> *mut Object;
type Fast = unsafe extern "C" fn(*mut Object, *const *mut Object, isize) -> *mut Object;
type FastKeywords =
    unsafe extern "C" fn(*mut Object, *const *mut Object, isize, *mut Object) -> *mut Object;

pub fn invoke(
    vm: &VirtualMachine,
    def: *const MethodDef,
    zelf: &dyn Fn(&VirtualMachine) -> *mut Object,
    args: FuncArgs,
) -> PyResult {
    let (name, flags, meth) =
        unsafe { (text((*def).name).unwrap_or("?"), (*def).flags, (*def).meth) };
    let Some(meth) = meth else {
        return Err(vm.new_system_error(format!("{name}() has no C function")));
    };
    let positional = args.args.len();
    let keywords = args.kwargs.len();
    let calls =
        flags & (METH_VARARGS | METH_KEYWORDS | METH_NOARGS | METH_O | METH_FASTCALL | METH_METHOD);
    if keywords > 0 && calls & METH_KEYWORDS == 0 {
        return Err(vm.new_type_error(format!("{name}() takes no keyword arguments")));
    }
    match calls {
        METH_NOARGS if positional != 0 => {
            return Err(
                vm.new_type_error(format!("{name}() takes no arguments ({positional} given)"))
            );
        }
        METH_O if positional != 1 => {
            return Err(vm.new_type_error(format!(
                "{name}() takes exactly one argument ({positional} given)"
            )));
        }
        _ => {}
    }
    enter(vm, || {
        let zelf = zelf(vm);
        let result = unsafe {
            match calls {
                METH_NOARGS => call2(
                    transmute::<unsafe extern "C" fn(), Call2>(meth),
                    zelf,
                    null_mut(),
                ),
                METH_O => call2(
                    transmute::<unsafe extern "C" fn(), Call2>(meth),
                    zelf,
                    borrowed(vm, &args.args[0]),
                ),
                METH_VARARGS => {
                    let tuple = vm.ctx.new_tuple(args.args.clone());
                    call2(
                        transmute::<unsafe extern "C" fn(), Call2>(meth),
                        zelf,
                        borrowed(vm, tuple.as_object()),
                    )
                }
                c if c == METH_VARARGS | METH_KEYWORDS => {
                    let tuple = vm.ctx.new_tuple(args.args.clone());
                    let kwargs = if keywords == 0 {
                        null_mut()
                    } else {
                        let dict = vm.ctx.new_dict();
                        for (key, value) in &args.kwargs {
                            dict.set_item(
                                vm.ctx.new_str(key.clone()).as_object(),
                                value.clone(),
                                vm,
                            )?;
                        }
                        borrowed(vm, dict.as_object())
                    };
                    call3(
                        transmute::<unsafe extern "C" fn(), Call3>(meth),
                        zelf,
                        borrowed(vm, tuple.as_object()),
                        kwargs,
                    )
                }
                METH_FASTCALL => {
                    let items: Vec<*mut Object> =
                        args.args.iter().map(|item| borrowed(vm, item)).collect();
                    call3(
                        transmute::<unsafe extern "C" fn(), Fast>(meth),
                        zelf,
                        items.as_ptr(),
                        positional as isize,
                    )
                }
                c if c == METH_FASTCALL | METH_KEYWORDS => {
                    let mut items: Vec<*mut Object> =
                        args.args.iter().map(|item| borrowed(vm, item)).collect();
                    let names = if keywords == 0 {
                        null_mut()
                    } else {
                        let mut names = Vec::new();
                        for (key, value) in &args.kwargs {
                            items.push(borrowed(vm, value));
                            names.push(vm.ctx.new_str(key.clone()).into());
                        }
                        borrowed(vm, vm.ctx.new_tuple(names).as_object())
                    };
                    call4(
                        transmute::<unsafe extern "C" fn(), FastKeywords>(meth),
                        zelf,
                        items.as_ptr(),
                        positional as isize,
                        names,
                    )
                }
                other => return Err(vm.new_system_error(format!(
                    "{name}() uses calling convention {other:#x}, which the C API does not run yet"
                ))),
            }
        };
        finish(vm, result)
    })
}

fn pointer_of(vm: &VirtualMachine, object: &PyObject) -> PyResult<*mut Object> {
    object
        .downcast_ref::<CObject>()
        .map(|wrapper| wrapper.pointer())
        .ok_or_else(|| {
            vm.new_type_error(format!(
                "expected a C extension object, got '{}'",
                object.class().name()
            ))
        })
}

fn first(vm: &VirtualMachine, args: &mut FuncArgs) -> PyResult<PyObjectRef> {
    if args.args.is_empty() {
        return Err(vm.new_type_error("the method needs an instance".to_owned()));
    }
    Ok(args.args.remove(0))
}

fn method(
    vm: &VirtualMachine,
    class: &'static Py<PyType>,
    name: &'static str,
    f: impl Fn(&VirtualMachine, PyObjectRef, FuncArgs) -> PyResult + 'static,
) -> PyObjectRef {
    builtin(name, None, PyMethodFlags::METHOD, move |vm, mut args| {
        let zelf = first(vm, &mut args)?;
        f(vm, zelf, args)
    })
    .build_method(&vm.ctx, class)
    .into()
}

fn member(vm: &VirtualMachine, def: &MemberDef) -> PyResult<PyObjectRef> {
    let name = text(def.name).unwrap_or("?");
    let kind = def.kind;
    let offset = def.offset as usize;
    let getter = function(vm, name, move |vm, args| {
        let zelf = args
            .args
            .first()
            .cloned()
            .ok_or_else(|| vm.new_type_error("the getter needs an instance".to_owned()))?;
        let at = unsafe { pointer_of(vm, &zelf)?.cast::<u8>().add(offset) };
        read_member(vm, kind, at, name)
    });
    let setter = if def.flags & READONLY != 0 {
        vm.ctx.none()
    } else {
        function(vm, name, move |vm, args| {
            let (Some(zelf), Some(value)) = (args.args.first(), args.args.get(1)) else {
                return Err(
                    vm.new_type_error("the setter needs an instance and a value".to_owned())
                );
            };
            let at = unsafe { pointer_of(vm, zelf)?.cast::<u8>().add(offset) };
            write_member(vm, kind, at, value)?;
            Ok(vm.ctx.none())
        })
    };
    let doc = text(def.doc).map_or_else(|| vm.ctx.none(), |doc| vm.ctx.new_str(doc).into());
    vm.ctx
        .types
        .property_type
        .as_object()
        .call((getter, setter, vm.ctx.none(), doc), vm)
}

fn integer<T: Copy>(at: *mut u8) -> T {
    unsafe { at.cast::<T>().read_unaligned() }
}

fn read_member(vm: &VirtualMachine, kind: c_int, at: *mut u8, name: &str) -> PyResult {
    Ok(match kind {
        T_OBJECT | T_OBJECT_EX => {
            let pointer = integer::<*mut Object>(at);
            if pointer.is_null() {
                if kind == T_OBJECT_EX {
                    return Err(vm.new_attribute_error(format!("'{name}' is not set")));
                }
                return Ok(vm.ctx.none());
            }
            enter(vm, || unsafe { bridge::to_rust(vm, pointer) })?
        }
        T_PYSSIZET => vm.ctx.new_int(integer::<isize>(at)).into(),
        T_INT => vm.ctx.new_int(integer::<c_int>(at)).into(),
        T_UINT => vm.ctx.new_int(integer::<u32>(at)).into(),
        T_LONG => vm.ctx.new_int(integer::<std::ffi::c_long>(at)).into(),
        T_ULONG => vm.ctx.new_int(integer::<std::ffi::c_ulong>(at)).into(),
        T_LONGLONG => vm.ctx.new_int(integer::<i64>(at)).into(),
        T_ULONGLONG => vm.ctx.new_int(integer::<u64>(at)).into(),
        T_SHORT => vm.ctx.new_int(integer::<i16>(at)).into(),
        T_USHORT => vm.ctx.new_int(integer::<u16>(at)).into(),
        T_BYTE => vm.ctx.new_int(integer::<i8>(at)).into(),
        T_UBYTE => vm.ctx.new_int(integer::<u8>(at)).into(),
        T_BOOL => vm.ctx.new_bool(integer::<u8>(at) != 0).into(),
        T_CHAR => vm
            .ctx
            .new_str(char::from(integer::<u8>(at)).to_string())
            .into(),
        T_DOUBLE => vm.ctx.new_float(integer::<f64>(at)).into(),
        T_FLOAT => vm.ctx.new_float(f64::from(integer::<f32>(at))).into(),
        T_STRING => text(integer::<*const c_char>(at))
            .map_or_else(|| vm.ctx.none(), |value| vm.ctx.new_str(value).into()),
        other => {
            return Err(vm.new_system_error(format!(
                "member '{name}' has type {other}, which the C API does not read yet"
            )));
        }
    })
}

fn write_member(
    vm: &VirtualMachine,
    kind: c_int,
    at: *mut u8,
    value: &PyObjectRef,
) -> PyResult<()> {
    fn store<T>(at: *mut u8, value: T) {
        unsafe { at.cast::<T>().write_unaligned(value) };
    }
    let index = |vm: &VirtualMachine| value.try_index(vm);
    match kind {
        T_OBJECT | T_OBJECT_EX => enter(vm, || {
            let new = new_ref(vm, value);
            let old = integer::<*mut Object>(at);
            store(at, new);
            unsafe { xdecref(old) };
        }),
        T_PYSSIZET => store(at, index(vm)?.try_to_primitive::<isize>(vm)?),
        T_INT => store(at, index(vm)?.try_to_primitive::<c_int>(vm)?),
        T_UINT => store(at, index(vm)?.try_to_primitive::<u32>(vm)?),
        T_LONG => store(at, index(vm)?.try_to_primitive::<std::ffi::c_long>(vm)?),
        T_ULONG => store(at, index(vm)?.try_to_primitive::<std::ffi::c_ulong>(vm)?),
        T_LONGLONG => store(at, index(vm)?.try_to_primitive::<i64>(vm)?),
        T_ULONGLONG => store(at, index(vm)?.try_to_primitive::<u64>(vm)?),
        T_SHORT => store(at, index(vm)?.try_to_primitive::<i16>(vm)?),
        T_USHORT => store(at, index(vm)?.try_to_primitive::<u16>(vm)?),
        T_BYTE => store(at, index(vm)?.try_to_primitive::<i8>(vm)?),
        T_UBYTE => store(at, index(vm)?.try_to_primitive::<u8>(vm)?),
        T_BOOL => store(at, u8::from(value.clone().is_true(vm)?)),
        T_DOUBLE => store(at, value.try_float(vm)?.to_f64()),
        T_FLOAT => store(at, value.try_float(vm)?.to_f64() as f32),
        other => {
            return Err(
                vm.new_type_error(format!("member type {other} cannot be set from Python yet"))
            );
        }
    }
    Ok(())
}

fn getset(vm: &VirtualMachine, def: &GetSetDef) -> PyResult<PyObjectRef> {
    let name = text(def.name).unwrap_or("?");
    let closure = def.closure as usize;
    let getter = def.get.map_or_else(
        || vm.ctx.none(),
        |get| {
            function(vm, name, move |vm, args| {
                let zelf =
                    args.args.first().cloned().ok_or_else(|| {
                        vm.new_type_error("the getter needs an instance".to_owned())
                    })?;
                enter(vm, || {
                    let pointer = borrowed(vm, &zelf);
                    finish(vm, unsafe { call2(get, pointer, closure as *mut c_void) })
                })
            })
        },
    );
    let setter = def.set.map_or_else(
        || vm.ctx.none(),
        |set| {
            function(vm, name, move |vm, args| {
                let (Some(zelf), Some(value)) = (args.args.first(), args.args.get(1)) else {
                    return Err(
                        vm.new_type_error("the setter needs an instance and a value".to_owned())
                    );
                };
                enter(vm, || {
                    let result = unsafe {
                        call3(
                            set,
                            borrowed(vm, zelf),
                            borrowed(vm, value),
                            closure as *mut c_void,
                        )
                    };
                    check(vm, result as isize).map(|_| vm.ctx.none())
                })
            })
        },
    );
    let doc = text(def.doc).map_or_else(|| vm.ctx.none(), |doc| vm.ctx.new_str(doc).into());
    vm.ctx
        .types
        .property_type
        .as_object()
        .call((getter, setter, vm.ctx.none(), doc), vm)
}

fn unary(
    vm: &VirtualMachine,
    zelf: &PyObjectRef,
    f: unsafe extern "C" fn(*mut Object) -> *mut Object,
) -> PyResult {
    enter(vm, || finish(vm, unsafe { call1(f, borrowed(vm, zelf)) }))
}

fn binary(
    vm: &VirtualMachine,
    zelf: &PyObjectRef,
    other: &PyObjectRef,
    f: unsafe extern "C" fn(*mut Object, *mut Object) -> *mut Object,
) -> PyResult {
    enter(vm, || {
        finish(vm, unsafe {
            call2(f, borrowed(vm, zelf), borrowed(vm, other))
        })
    })
}

fn one(vm: &VirtualMachine, args: &FuncArgs) -> PyResult<PyObjectRef> {
    args.args
        .first()
        .cloned()
        .ok_or_else(|| vm.new_type_error("expected one argument".to_owned()))
}

fn keywords(vm: &VirtualMachine, args: &FuncArgs) -> PyResult<*mut Object> {
    if args.kwargs.is_empty() {
        return Ok(null_mut());
    }
    let dict = vm.ctx.new_dict();
    for (key, value) in &args.kwargs {
        dict.set_item(vm.ctx.new_str(key.clone()).as_object(), value.clone(), vm)?;
    }
    Ok(borrowed(vm, dict.as_object()))
}

fn slots(
    vm: &VirtualMachine,
    ty: *mut Type,
    class: &'static Py<PyType>,
) -> Vec<(&'static str, PyObjectRef)> {
    let t = unsafe { &*ty };
    let mut out: Vec<(&'static str, PyObjectRef)> = Vec::new();
    if let Some(f) = t.repr {
        out.push((
            "__repr__",
            method(vm, class, "__repr__", move |vm, zelf, _| {
                unary(vm, &zelf, f)
            }),
        ));
    }
    if let Some(f) = t.str {
        out.push((
            "__str__",
            method(vm, class, "__str__", move |vm, zelf, _| unary(vm, &zelf, f)),
        ));
    }
    if let Some(f) = t.iter {
        out.push((
            "__iter__",
            method(vm, class, "__iter__", move |vm, zelf, _| {
                unary(vm, &zelf, f)
            }),
        ));
    }
    if let Some(f) = t.iternext {
        out.push((
            "__next__",
            method(vm, class, "__next__", move |vm, zelf, _| {
                enter(vm, || {
                    let result = unsafe { call1(f, borrowed(vm, &zelf)) };
                    if result.is_null() && bridge::current_error().is_none() {
                        return Err(vm.new_stop_iteration(None));
                    }
                    finish(vm, result)
                })
            }),
        ));
    }
    if let Some(f) = t.hash {
        out.push((
            "__hash__",
            method(vm, class, "__hash__", move |vm, zelf, _| {
                enter(vm, || {
                    check(vm, unsafe { call1(f, borrowed(vm, &zelf)) })
                        .map(|hash| vm.ctx.new_int(hash).into())
                })
            }),
        ));
    }
    if let Some(f) = t.call {
        out.push((
            "__call__",
            method(vm, class, "__call__", move |vm, zelf, args| {
                enter(vm, || {
                    let tuple = vm.ctx.new_tuple(args.args.clone());
                    let kwargs = keywords(vm, &args)?;
                    finish(vm, unsafe {
                        call3(
                            f,
                            borrowed(vm, &zelf),
                            borrowed(vm, tuple.as_object()),
                            kwargs,
                        )
                    })
                })
            }),
        ));
    }
    if let Some(f) = t.richcompare {
        for (name, op) in [
            ("__lt__", LT),
            ("__le__", LE),
            ("__eq__", EQ),
            ("__ne__", NE),
            ("__gt__", GT),
            ("__ge__", GE),
        ] {
            out.push((
                name,
                method(vm, class, name, move |vm, zelf, args| {
                    let other = one(vm, &args)?;
                    enter(vm, || {
                        finish(vm, unsafe {
                            call3(f, borrowed(vm, &zelf), borrowed(vm, &other), op)
                        })
                    })
                }),
            ));
        }
    }
    let mapping = if t.as_mapping.is_null() {
        None
    } else {
        Some(unsafe { &*t.as_mapping })
    };
    let sequence = if t.as_sequence.is_null() {
        None
    } else {
        Some(unsafe { &*t.as_sequence })
    };
    if let Some(f) = mapping.and_then(|mapping| mapping.subscript) {
        out.push((
            "__getitem__",
            method(vm, class, "__getitem__", move |vm, zelf, args| {
                binary(vm, &zelf, &one(vm, &args)?, f)
            }),
        ));
    } else if let Some(f) = sequence.and_then(|sequence| sequence.item) {
        out.push((
            "__getitem__",
            method(vm, class, "__getitem__", move |vm, zelf, args| {
                let index = one(vm, &args)?
                    .try_index(vm)?
                    .try_to_primitive::<isize>(vm)?;
                enter(vm, || {
                    finish(vm, unsafe { call2(f, borrowed(vm, &zelf), index) })
                })
            }),
        ));
    }
    if let Some(f) = mapping
        .and_then(|mapping| mapping.length)
        .or_else(|| sequence.and_then(|sequence| sequence.length))
    {
        out.push((
            "__len__",
            method(vm, class, "__len__", move |vm, zelf, _| {
                enter(vm, || {
                    check(vm, unsafe { call1(f, borrowed(vm, &zelf)) })
                        .map(|length| vm.ctx.new_int(length).into())
                })
            }),
        ));
    }
    if let Some(f) = mapping.and_then(|mapping| mapping.ass_subscript) {
        out.push((
            "__setitem__",
            method(vm, class, "__setitem__", move |vm, zelf, args| {
                let (Some(key), Some(value)) = (args.args.first(), args.args.get(1)) else {
                    return Err(
                        vm.new_type_error("__setitem__ expects a key and a value".to_owned())
                    );
                };
                enter(vm, || {
                    check(vm, unsafe {
                        call3(
                            f,
                            borrowed(vm, &zelf),
                            borrowed(vm, key),
                            borrowed(vm, value),
                        )
                    } as isize)
                    .map(|_| vm.ctx.none())
                })
            }),
        ));
        out.push((
            "__delitem__",
            method(vm, class, "__delitem__", move |vm, zelf, args| {
                let key = one(vm, &args)?;
                enter(vm, || {
                    check(vm, unsafe {
                        call3(f, borrowed(vm, &zelf), borrowed(vm, &key), null_mut())
                    } as isize)
                    .map(|_| vm.ctx.none())
                })
            }),
        ));
    }
    if let Some(f) = sequence.and_then(|sequence| sequence.contains) {
        out.push((
            "__contains__",
            method(vm, class, "__contains__", move |vm, zelf, args| {
                let item = one(vm, &args)?;
                enter(vm, || {
                    check(
                        vm,
                        unsafe { call2(f, borrowed(vm, &zelf), borrowed(vm, &item)) } as isize,
                    )
                    .map(|found| vm.ctx.new_bool(found != 0).into())
                })
            }),
        ));
    }
    if !t.as_number.is_null()
        && let Some(f) = unsafe { (*t.as_number).bool }
    {
        out.push((
            "__bool__",
            method(vm, class, "__bool__", move |vm, zelf, _| {
                enter(vm, || {
                    check(vm, unsafe { call1(f, borrowed(vm, &zelf)) } as isize)
                        .map(|value| vm.ctx.new_bool(value != 0).into())
                })
            }),
        ));
    }
    if let Some(f) = t.init {
        out.push((
            "__init__",
            method(vm, class, "__init__", move |vm, zelf, args| {
                enter(vm, || {
                    let tuple = vm.ctx.new_tuple(args.args.clone());
                    let kwargs = keywords(vm, &args)?;
                    check(vm, unsafe {
                        call3(
                            f,
                            borrowed(vm, &zelf),
                            borrowed(vm, tuple.as_object()),
                            kwargs,
                        )
                    } as isize)
                    .map(|_| vm.ctx.none())
                })
            }),
        ));
    }
    if let Some(f) = t.new {
        let ty = ty as usize;
        let new = function(vm, "__new__", move |vm, mut args| {
            let _cls = first(vm, &mut args)?;
            enter(vm, || {
                let tuple = vm.ctx.new_tuple(args.args.clone());
                let kwargs = keywords(vm, &args)?;
                finish(vm, unsafe {
                    call3(f, ty as *mut Type, borrowed(vm, tuple.as_object()), kwargs)
                })
            })
        });
        if let Ok(new) = vm.ctx.types.staticmethod_type.as_object().call((new,), vm) {
            out.push(("__new__", new));
        }
    }
    out
}

pub fn ready_type(ty: *mut Type) {
    unsafe {
        let t = &mut *ty;
        if t.base.base.ty.is_null() {
            t.base.base.ty = bridge::PyType_Type.get();
        }
        if t.base_type.is_null() && ty != bridge::PyBaseObject_Type.get() {
            t.base_type = bridge::PyBaseObject_Type.get();
        }
        if t.alloc.is_none() {
            t.alloc = Some(PyType_GenericAlloc);
        }
        if t.free.is_none() {
            t.free = Some(heap::PyObject_Free);
        }
        t.flags |= TPFLAGS_READY;
    }
}

pub fn c_type(vm: &VirtualMachine, ty: *mut Type) -> PyResult<PyTypeRef> {
    if let Some(class) = bridge::static_type(ty) {
        return Ok(class);
    }
    ready_type(ty);
    let full = text(unsafe { (*ty).name }).unwrap_or("?");
    let (module, name) = full.rsplit_once('.').unwrap_or(("builtins", full));
    let base_type = unsafe { (*ty).base_type };
    let base = if base_type.is_null() || base_type == bridge::PyBaseObject_Type.get() {
        CObject::class(&vm.ctx).to_owned()
    } else {
        let base = c_type(vm, base_type)?;
        if !base.fast_issubclass(CObject::class(&vm.ctx)) {
            return Err(vm.new_system_error(format!(
                "{full}: a C type based on {} is not supported yet",
                base.name()
            )));
        }
        base
    };
    let class = vm.ctx.new_class(Some(module), name, base, {
        let mut slots = PyTypeSlots::heap_default();
        if unsafe { (*ty).weaklistoffset } != 0 {
            slots.flags |=
                PyTypeFlags::from_slice(&[PyTypeFlags::HAS_WEAKREF, PyTypeFlags::MANAGED_WEAKREF]);
        }
        slots
    });
    bridge::register_c_type(ty, &class);
    let leaked: &'static Py<PyType> = PyRef::leak(class.clone());
    let mut attributes: Vec<(&'static str, PyObjectRef)> = Vec::new();
    if let Some(doc) = text(unsafe { (*ty).doc }) {
        attributes.push(("__doc__", vm.ctx.new_str(doc).into()));
    }
    let mut def = unsafe { (*ty).methods };
    while !def.is_null() && !unsafe { (*def).name }.is_null() {
        let name = text(unsafe { (*def).name }).unwrap_or("?");
        let flags = unsafe { (*def).flags };
        let doc = text(unsafe { (*def).doc });
        let address = def as usize;
        let value: PyObjectRef = if flags & METH_CLASS != 0 {
            builtin(name, doc, PyMethodFlags::CLASS, move |vm, mut args| {
                let cls = first(vm, &mut args)?;
                invoke(
                    vm,
                    address as *const MethodDef,
                    &|vm| borrowed(vm, &cls),
                    args,
                )
            })
            .build_classmethod(&vm.ctx, leaked)
            .into()
        } else if flags & METH_STATIC != 0 {
            let f = function(vm, name, move |vm, args| {
                invoke(vm, address as *const MethodDef, &|_| null_mut(), args)
            });
            vm.ctx.types.staticmethod_type.as_object().call((f,), vm)?
        } else {
            builtin(name, doc, PyMethodFlags::METHOD, move |vm, mut args| {
                let zelf = first(vm, &mut args)?;
                invoke(
                    vm,
                    address as *const MethodDef,
                    &|vm| borrowed(vm, &zelf),
                    args,
                )
            })
            .build_method(&vm.ctx, leaked)
            .into()
        };
        attributes.push((name, value));
        def = unsafe { def.add(1) };
    }
    let mut def = unsafe { (*ty).members };
    while !def.is_null() && !unsafe { (*def).name }.is_null() {
        attributes.push((
            text(unsafe { (*def).name }).unwrap_or("?"),
            member(vm, unsafe { &*def })?,
        ));
        def = unsafe { def.add(1) };
    }
    let mut def = unsafe { (*ty).getset };
    while !def.is_null() && !unsafe { (*def).name }.is_null() {
        attributes.push((
            text(unsafe { (*def).name }).unwrap_or("?"),
            getset(vm, unsafe { &*def })?,
        ));
        def = unsafe { def.add(1) };
    }
    attributes.extend(slots(vm, ty, leaked));
    for (name, value) in attributes {
        class
            .as_object()
            .set_attr(vm.ctx.intern_str(name), value, vm)?;
    }
    Ok(class)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_Ready(ty: *mut Type) -> c_int {
    ready_type(ty);
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_GenericAlloc(ty: *mut Type, items: isize) -> *mut Object {
    let size = unsafe {
        (*ty).basicsize.unsigned_abs() + items.unsigned_abs() * (*ty).itemsize.unsigned_abs()
    };
    let pointer = heap::allocate(size, true).cast::<Object>();
    if pointer.is_null() {
        return crate::api::PyErr_NoMemory();
    }
    unsafe {
        pointer.write(Object { refcnt: 1, ty });
        if (*ty).itemsize != 0 {
            (*pointer.cast::<VarObject>()).size = items;
        }
        if (*ty).flags & TPFLAGS_HEAPTYPE != 0 {
            incref(ty.cast());
        }
    }
    pointer
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_GenericNew(
    ty: *mut Type,
    _args: *mut Object,
    _kwargs: *mut Object,
) -> *mut Object {
    unsafe {
        match (*ty).alloc {
            Some(alloc) => alloc(ty, 0),
            None => PyType_GenericAlloc(ty, 0),
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _PyObject_New(ty: *mut Type) -> *mut Object {
    let pointer = heap::allocate(unsafe { (*ty).basicsize.unsigned_abs() }, false).cast::<Object>();
    if pointer.is_null() {
        return crate::api::PyErr_NoMemory();
    }
    unsafe { PyObject_Init(pointer, ty) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _PyObject_NewVar(ty: *mut Type, items: isize) -> *mut Object {
    let size = unsafe {
        (*ty).basicsize.unsigned_abs() + items.unsigned_abs() * (*ty).itemsize.unsigned_abs()
    };
    let pointer = heap::allocate(size, false).cast::<Object>();
    if pointer.is_null() {
        return crate::api::PyErr_NoMemory();
    }
    unsafe {
        (*pointer.cast::<VarObject>()).size = items;
        PyObject_Init(pointer, ty)
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_Init(pointer: *mut Object, ty: *mut Type) -> *mut Object {
    if pointer.is_null() {
        return pointer;
    }
    unsafe {
        (*pointer).refcnt = 1;
        (*pointer).ty = ty;
        if (*ty).flags & TPFLAGS_HEAPTYPE != 0 {
            incref(ty.cast());
        }
    }
    pointer
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_InitVar(
    pointer: *mut Object,
    ty: *mut Type,
    items: isize,
) -> *mut Object {
    if pointer.is_null() {
        return pointer;
    }
    unsafe {
        (*pointer.cast::<VarObject>()).size = items;
        PyObject_Init(pointer, ty)
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_IsSubtype(a: *mut Type, b: *mut Type) -> c_int {
    if a == b {
        return 1;
    }
    let Some(vm) = bridge::machine() else {
        let mut at = a;
        while !at.is_null() {
            if at == b {
                return 1;
            }
            at = unsafe { (*at).base_type };
        }
        return 0;
    };
    let (Ok(a), Ok(b)) = (arg(vm, a.cast()), arg(vm, b.cast())) else {
        return 0;
    };
    match (a.downcast_ref::<PyType>(), b.downcast_ref::<PyType>()) {
        (Some(a), Some(b)) => c_int::from(a.fast_issubclass(b)),
        _ => 0,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_GetFlags(ty: *mut Type) -> std::ffi::c_ulong {
    unsafe { (*ty).flags }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_Modified(_ty: *mut Type) {}

use std::{
    cell::RefCell,
    collections::HashMap,
    ffi::{c_char, c_int, c_void},
    ptr::null_mut,
};

use rustpython_vm::{
    AsObject, PyObject, PyRef, PyResult, VirtualMachine,
    builtins::{PyModule, PyModuleDef, PyModuleSlots},
};

use crate::{
    abi::*,
    bridge::{self, arg, enter, finish, guard, incref, new_ref, take_error, text},
    heap,
    types::{invoke, module_function},
};

pub struct Extension {
    pub name: &'static str,
    init: unsafe extern "C" fn() -> *mut Object,
}

include!(concat!(env!("OUT_DIR"), "/extensions.rs"));

thread_local! {
    static MODULES: RefCell<HashMap<usize, usize>> = RefCell::new(HashMap::new());
    static STATES: RefCell<HashMap<usize, usize>> = RefCell::new(HashMap::new());
    static CONTEXT: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) };
}

pub fn module_defs(ctx: &rustpython_vm::Context) -> Vec<&'static PyModuleDef> {
    EXTENSIONS
        .iter()
        .map(|extension| {
            &*Box::leak(Box::new(PyModuleDef {
                name: ctx.intern_str(extension.name),
                doc: None,
                methods: &[],
                slots: PyModuleSlots {
                    create: Some(create),
                    exec: None,
                },
            }))
        })
        .collect()
}

fn create(
    vm: &VirtualMachine,
    spec: &PyObject,
    def: &'static PyModuleDef,
) -> PyResult<PyRef<PyModule>> {
    let name = def.name.as_str();
    let extension = EXTENSIONS
        .iter()
        .find(|extension| extension.name == name)
        .ok_or_else(|| {
            vm.new_import_error(
                format!("the C extension {name} is not compiled in"),
                vm.ctx.new_str(name),
            )
        })?;
    let module = enter(vm, || {
        CONTEXT.set(Some(extension.name));
        let result = unsafe { (extension.init)() };
        CONTEXT.set(None);
        if result.is_null() {
            return Err(take_error().unwrap_or_else(|| {
                vm.new_system_error(format!(
                    "initialization of {name} failed without raising an exception"
                ))
            }));
        }
        if unsafe { (*result).ty } == bridge::PyModuleDef_Type.get() {
            multi_phase(vm, result.cast(), name, spec)
        } else {
            finish(vm, result)
        }
    })?;
    module.downcast::<PyModule>().map_err(|_| {
        vm.new_system_error(format!("initialization of {name} did not return a module"))
    })
}

fn multi_phase(vm: &VirtualMachine, def: *mut ModuleDef, name: &str, spec: &PyObject) -> PyResult {
    let mut creator = None;
    let mut slot = unsafe { (*def).slots };
    while !slot.is_null() && unsafe { (*slot).slot } != 0 {
        match unsafe { (*slot).slot } {
            MOD_EXEC | MOD_MULTIPLE_INTERPRETERS | MOD_GIL => {}
            MOD_CREATE => creator = Some(unsafe { (*slot).value }),
            other => {
                return Err(vm.new_import_error(
                    format!("{name} has module slot {other}, which the C API does not run yet"),
                    vm.ctx.new_str(name),
                ));
            }
        }
        slot = unsafe { slot.add(1) };
    }
    let pointer = match creator {
        Some(creator) => {
            let creator = unsafe {
                std::mem::transmute::<
                    *mut c_void,
                    unsafe extern "C" fn(*mut Object, *mut ModuleDef) -> *mut Object,
                >(creator)
            };
            let pointer = unsafe { crate::cast::call2(creator, bridge::borrowed(vm, spec), def) };
            if pointer.is_null() {
                return Err(take_error().unwrap_or_else(|| {
                    vm.new_system_error(format!(
                        "creation of {name} failed without raising an exception"
                    ))
                }));
            }
            adopt(vm, def, pointer)?
        }
        None => new_module(vm, def, name)?,
    };
    let module = arg(vm, pointer)?;
    let mut slot = unsafe { (*def).slots };
    while !slot.is_null() && unsafe { (*slot).slot } != 0 {
        if unsafe { (*slot).slot } == MOD_EXEC {
            let exec = unsafe {
                std::mem::transmute::<*mut c_void, unsafe extern "C" fn(*mut Object) -> c_int>(
                    (*slot).value,
                )
            };
            if unsafe { crate::cast::call1(exec, pointer) } != 0 {
                return Err(take_error().unwrap_or_else(|| {
                    vm.new_system_error(format!(
                        "execution of {name} failed without raising an exception"
                    ))
                }));
            }
        }
        slot = unsafe { slot.add(1) };
    }
    Ok(module)
}

fn new_module(vm: &VirtualMachine, def: *mut ModuleDef, name: &str) -> PyResult<*mut Object> {
    let doc = text(unsafe { (*def).doc }).map(|doc| vm.ctx.new_str(doc));
    let module = vm.new_module(name, vm.ctx.new_dict(), doc);
    let pointer = new_ref(vm, module.as_object());
    unsafe { incref(pointer) };
    adopt(vm, def, pointer)
}

fn adopt(vm: &VirtualMachine, def: *mut ModuleDef, pointer: *mut Object) -> PyResult<*mut Object> {
    let module = arg(vm, pointer)?;
    let address = pointer as usize;
    let mut method = unsafe { (*def).methods };
    while !method.is_null() && !unsafe { (*method).name }.is_null() {
        let key = text(unsafe { (*method).name }).unwrap_or("?");
        let entry = method as usize;
        let f = module_function(vm, module.as_object(), key, move |vm, args| {
            invoke(
                vm,
                entry as *const MethodDef,
                &|_| address as *mut Object,
                args,
            )
        })?;
        module.as_object().set_attr(vm.ctx.intern_str(key), f, vm)?;
        method = unsafe { method.add(1) };
    }
    let size = unsafe { (*def).size };
    if size > 0 {
        STATES.with_borrow_mut(|states| {
            states.insert(address, heap::allocate(size.unsigned_abs(), true) as usize)
        });
    }
    MODULES.with_borrow_mut(|modules| modules.insert(def as usize, address));
    Ok(pointer)
}

static DEFINITIONS: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyModuleDef_Init(def: *mut ModuleDef) -> *mut Object {
    unsafe {
        if (*def).base.index == 0 {
            (*def).base.base.ty = bridge::PyModuleDef_Type.get();
            (*def).base.base.refcnt = 1;
            (*def).base.index = DEFINITIONS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        }
    }
    def.cast()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyModule_Create2(def: *mut ModuleDef, _version: c_int) -> *mut Object {
    guard(null_mut(), |vm| {
        let name = CONTEXT
            .take()
            .or_else(|| text(unsafe { (*def).name }))
            .unwrap_or("?");
        new_module(vm, def, name)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyModule_FromDefAndSpec2(
    def: *mut ModuleDef,
    _spec: *mut Object,
    version: c_int,
) -> *mut Object {
    unsafe { PyModule_Create2(def, version) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyModule_ExecDef(_module: *mut Object, _def: *mut ModuleDef) -> c_int {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn PyModule_GetState(module: *mut Object) -> *mut c_void {
    STATES
        .with_borrow(|states| states.get(&(module as usize)).copied())
        .map_or(null_mut(), |state| state as *mut c_void)
}

#[unsafe(no_mangle)]
pub extern "C" fn PyState_FindModule(def: *mut ModuleDef) -> *mut Object {
    MODULES
        .with_borrow(|modules| modules.get(&(def as usize)).copied())
        .map_or(null_mut(), |module| module as *mut Object)
}

#[unsafe(no_mangle)]
pub extern "C" fn PyState_AddModule(module: *mut Object, def: *mut ModuleDef) -> c_int {
    MODULES.with_borrow_mut(|modules| modules.insert(def as usize, module as usize));
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn PyModule_GetDict(module: *mut Object) -> *mut Object {
    bridge::borrowed_object(|vm| {
        let module = arg(vm, module)?;
        Ok(module
            .dict()
            .map(Into::into)
            .unwrap_or_else(|| vm.ctx.new_dict().into()))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyModule_GetNameObject(module: *mut Object) -> *mut Object {
    bridge::object(|vm| arg(vm, module)?.get_attr("__name__", vm))
}

#[unsafe(no_mangle)]
pub extern "C" fn PyModule_AddObjectRef(
    module: *mut Object,
    name: *const c_char,
    value: *mut Object,
) -> c_int {
    bridge::status(|vm| {
        let module = arg(vm, module)?;
        let value = arg(vm, value)?;
        let name = text(name).ok_or_else(|| {
            vm.new_system_error("PyModule_AddObjectRef() needs a name".to_owned())
        })?;
        module.set_attr(vm.ctx.intern_str(name), value, vm)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyModule_AddObject(
    module: *mut Object,
    name: *const c_char,
    value: *mut Object,
) -> c_int {
    let result = PyModule_AddObjectRef(module, name, value);
    if result == 0 {
        unsafe { bridge::decref(value) };
    }
    result
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyModule_Add(
    module: *mut Object,
    name: *const c_char,
    value: *mut Object,
) -> c_int {
    let result = PyModule_AddObjectRef(module, name, value);
    unsafe { bridge::xdecref(value) };
    result
}

#[unsafe(no_mangle)]
pub extern "C" fn PyModule_AddIntConstant(
    module: *mut Object,
    name: *const c_char,
    value: std::ffi::c_long,
) -> c_int {
    bridge::status(|vm| {
        let module = arg(vm, module)?;
        let name = text(name).ok_or_else(|| {
            vm.new_system_error("PyModule_AddIntConstant() needs a name".to_owned())
        })?;
        module.set_attr(vm.ctx.intern_str(name), vm.ctx.new_int(value), vm)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyModule_AddStringConstant(
    module: *mut Object,
    name: *const c_char,
    value: *const c_char,
) -> c_int {
    bridge::status(|vm| {
        let module = arg(vm, module)?;
        let name = text(name).ok_or_else(|| {
            vm.new_system_error("PyModule_AddStringConstant() needs a name".to_owned())
        })?;
        module.set_attr(
            vm.ctx.intern_str(name),
            vm.ctx.new_str(text(value).unwrap_or_default()),
            vm,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyModule_AddType(module: *mut Object, ty: *mut Type) -> c_int {
    bridge::status(|vm| {
        let module = arg(vm, module)?;
        let class = arg(vm, ty.cast())?;
        let full = text(unsafe { (*ty).name }).unwrap_or("?");
        let name = full.rsplit('.').next().unwrap_or(full);
        module.set_attr(vm.ctx.intern_str(name), class, vm)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyModule_AddFunctions(module: *mut Object, methods: *mut MethodDef) -> c_int {
    bridge::status(|vm| {
        let target = arg(vm, module)?;
        let address = module as usize;
        let mut method = methods;
        while !method.is_null() && !unsafe { (*method).name }.is_null() {
            let key = text(unsafe { (*method).name }).unwrap_or("?");
            let entry = method as usize;
            let f = module_function(vm, &target, key, move |vm, args| {
                invoke(
                    vm,
                    entry as *const MethodDef,
                    &|_| address as *mut Object,
                    args,
                )
            })?;
            target.set_attr(vm.ctx.intern_str(key), f, vm)?;
            method = unsafe { method.add(1) };
        }
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyModule_GetDef(module: *mut Object) -> *mut ModuleDef {
    MODULES
        .with_borrow(|modules| {
            modules
                .iter()
                .find(|&(_, &address)| address == module as usize)
                .map(|(&def, _)| def)
        })
        .map_or(null_mut(), |def| def as *mut ModuleDef)
}

#[unsafe(no_mangle)]
pub extern "C" fn PyModule_GetName(module: *mut Object) -> *const c_char {
    guard(std::ptr::null(), |vm| {
        let name = arg(vm, module)?.get_attr("__name__", vm)?;
        Ok(unsafe { crate::api::PyUnicode_AsUTF8(bridge::borrowed(vm, &name)) })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn PyModule_NewObject(name: *mut Object) -> *mut Object {
    bridge::object(|vm| {
        let name = arg(vm, name)?.str(vm)?;
        Ok(vm
            .new_module(&name.to_string_lossy(), vm.ctx.new_dict(), None)
            .into())
    })
}

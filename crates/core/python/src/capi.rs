use nanocode_python_capi::{EXTENSIONS, Kind, Method, Owned, Units};
use rustpython_vm::{
    Context, PyObject, PyObjectRef, PyRef, PyResult, VirtualMachine,
    builtins::{PyModule, PyModuleDef, PyModuleSlots, PyStr},
    common::wtf8::{CodePoint, Wtf8Buf},
    function::{Callee, FuncArgs, ItemDoc, PyMethodDef, PyMethodFlags},
};

pub fn module_defs(ctx: &Context) -> Vec<&'static PyModuleDef> {
    EXTENSIONS
        .iter()
        .map(|extension| {
            let (methods, slots): (&'static [PyMethodDef], PyModuleSlots) =
                match extension.methods() {
                    Ok(methods) => (
                        Box::leak(methods.into_iter().map(def).collect()),
                        PyModuleSlots::default(),
                    ),
                    Err(_) => (
                        &[],
                        PyModuleSlots {
                            create: Some(refuse),
                            exec: None,
                        },
                    ),
                };
            &*Box::leak(Box::new(PyModuleDef {
                name: ctx.intern_str(extension.name),
                doc: None,
                methods,
                slots,
            }))
        })
        .collect()
}

fn refuse(
    vm: &VirtualMachine,
    _spec: &PyObject,
    def: &'static PyModuleDef,
) -> PyResult<PyRef<PyModule>> {
    let name = def.name.as_str();
    let reason = EXTENSIONS
        .iter()
        .find(|extension| extension.name == name)
        .and_then(|extension| extension.methods().err())
        .map_or_else(
            || "it is not compiled in".to_owned(),
            |refused| refused.to_string(),
        );
    Err(vm.new_import_error(
        format!("cannot load the C extension {name}: {reason}"),
        vm.ctx.new_str(name),
    ))
}

fn def(method: Method) -> PyMethodDef {
    let func = move |vm: &VirtualMachine, args: FuncArgs, _: Callee| call(vm, &method, args);
    PyMethodDef::new_dyn(
        method.name,
        Box::leak(Box::new(func)),
        PyMethodFlags::empty(),
        ItemDoc {
            text: method.doc,
            ..ItemDoc::NONE
        },
    )
}

fn call(vm: &VirtualMachine, method: &Method, args: FuncArgs) -> PyResult {
    let name = method.name;
    let given = args.args.len() + args.kwargs.len();
    match method.kind {
        Kind::One if args.kwargs.is_empty() && args.args.len() == 1 => {
            let object = &args.args[0];
            let arg = to_c(vm, object)?;
            let result = method.call(Some(&arg)).ok_or_else(|| null(vm, name))?;
            if result.is(&arg) {
                return Ok(object.clone());
            }
            from_c(vm, &result, name)
        }
        Kind::One => Err(vm.new_type_error(format!(
            "{name}() takes exactly one argument ({given} given)"
        ))),
        Kind::None if given == 0 => {
            let result = method.call(None).ok_or_else(|| null(vm, name))?;
            from_c(vm, &result, name)
        }
        Kind::None => {
            Err(vm.new_type_error(format!("{name}() takes no arguments ({given} given)")))
        }
        Kind::Other(flags) => Err(vm.new_system_error(format!(
            "{name}() uses calling convention {flags:#x}, which the C API does not run yet"
        ))),
    }
}

fn null(vm: &VirtualMachine, name: &str) -> rustpython_vm::builtins::PyBaseExceptionRef {
    vm.new_system_error(format!(
        "<built-in function {name}> returned NULL without setting an exception"
    ))
}

fn to_c(vm: &VirtualMachine, object: &PyObjectRef) -> PyResult<Owned> {
    let Some(text) = object.downcast_ref::<PyStr>() else {
        return Err(vm.new_type_error(format!(
            "the C API has no CPython object for '{}' yet",
            object.class().name()
        )));
    };
    let bytes = text.as_bytes();
    let owned = if bytes.is_ascii() {
        Owned::ascii(bytes)
    } else {
        Owned::str(text.as_wtf8().code_points().map(CodePoint::to_u32))
    };
    owned.ok_or_else(|| vm.new_memory_error(String::new()))
}

fn from_c(vm: &VirtualMachine, object: &Owned, name: &str) -> PyResult {
    let text = match object.units() {
        Some(Units::Latin1(bytes)) if bytes.is_ascii() => {
            vm.ctx.new_str(String::from_utf8_lossy(bytes).into_owned())
        }
        Some(Units::Latin1(bytes)) => vm
            .ctx
            .new_str(bytes.iter().copied().map(char::from).collect::<String>()),
        Some(Units::Ucs2(units)) => vm.ctx.new_str(wtf8(units.iter().copied().map(u32::from))),
        Some(Units::Ucs4(units)) => vm.ctx.new_str(wtf8(units.iter().copied())),
        None => {
            return Err(vm.new_system_error(format!(
                "{name}() returned an object the C API cannot read yet"
            )));
        }
    };
    Ok(text.into())
}

fn wtf8(points: impl Iterator<Item = u32>) -> Wtf8Buf {
    let mut out = Wtf8Buf::new();
    for point in points {
        out.push(
            CodePoint::from_u32(point).unwrap_or(CodePoint::from_char(char::REPLACEMENT_CHARACTER)),
        );
    }
    out
}

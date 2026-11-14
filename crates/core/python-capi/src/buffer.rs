use std::{
    ffi::c_int,
    ptr::null_mut,
    slice,
    sync::atomic::{AtomicPtr, Ordering},
};

use rustpython_vm::{
    PyObject, PyPayload, PyResult, VirtualMachine,
    common::borrow::{BorrowedValue, BorrowedValueMut},
    protocol::{BufferDescriptor, BufferFlags, BufferMethods, PyBuffer},
    pyclass,
};

use crate::{
    abi::*,
    bridge::{borrowed, take_error, xdecref},
};

#[pyclass(module = "_capi", name = "buffer")]
#[derive(Debug, PyPayload)]
pub struct CBuffer {
    view: AtomicPtr<Buffer>,
}

#[pyclass]
impl CBuffer {}

impl CBuffer {
    fn view(&self) -> *mut Buffer {
        self.view.load(Ordering::Relaxed)
    }
}

pub fn procs(ty: *mut Type) -> Option<&'static BufferProcs> {
    if ty.is_null() {
        return None;
    }
    let procs = unsafe { (*ty).as_buffer }.cast::<BufferProcs>();
    (!procs.is_null())
        .then(|| unsafe { &*procs })
        .filter(|procs| procs.getbuffer.is_some())
}

fn bytes(buffer: &PyBuffer) -> (*mut u8, usize) {
    let view = buffer.obj_as::<CBuffer>().view();
    if view.is_null() {
        return (null_mut(), 0);
    }
    unsafe { ((*view).buf.cast::<u8>(), (*view).len.unsigned_abs()) }
}

static METHODS: BufferMethods = BufferMethods {
    obj_bytes: |buffer| match bytes(buffer) {
        (data, length) if !data.is_null() => {
            BorrowedValue::Ref(unsafe { slice::from_raw_parts(data, length) })
        }
        _ => BorrowedValue::Ref(&[]),
    },
    obj_bytes_mut: |buffer| match bytes(buffer) {
        (data, length) if !data.is_null() => {
            BorrowedValueMut::RefMut(unsafe { slice::from_raw_parts_mut(data, length) })
        }
        _ => BorrowedValueMut::RefMut(&mut []),
    },
    release: |buffer| {
        let view = buffer
            .obj_as::<CBuffer>()
            .view
            .swap(null_mut(), Ordering::Relaxed);
        if view.is_null() {
            return;
        }
        let owner = unsafe { (*view).obj };
        if !owner.is_null()
            && let Some(release) = procs(unsafe { (*owner).ty }).and_then(|procs| procs.release)
        {
            unsafe { release(owner, view) };
        }
        unsafe {
            xdecref(owner);
            drop(Box::from_raw(view));
        }
    },
    retain: |_| {},
};

pub fn export(object: &PyObject, flags: BufferFlags, vm: &VirtualMachine) -> PyResult<PyBuffer> {
    let pointer = borrowed(vm, object);
    let Some(getbuffer) = procs(unsafe { (*pointer).ty }).and_then(|procs| procs.getbuffer) else {
        return Err(vm.new_type_error(format!(
            "a bytes-like object is required, not '{}'",
            object.class().name()
        )));
    };
    let view = Box::into_raw(Box::new(unsafe { std::mem::zeroed::<Buffer>() }));
    if unsafe { getbuffer(pointer, view, flags.bits() as c_int) } < 0 {
        unsafe { drop(Box::from_raw(view)) };
        return Err(take_error().unwrap_or_else(|| {
            vm.new_system_error("bf_getbuffer failed without an exception".to_owned())
        }));
    }
    let (length, readonly) = unsafe { ((*view).len.unsigned_abs(), (*view).readonly != 0) };
    let holder = CBuffer {
        view: AtomicPtr::new(view),
    }
    .into_pyobject(vm);
    Ok(PyBuffer::new(
        holder,
        BufferDescriptor::simple(length, readonly),
        &METHODS,
    ))
}

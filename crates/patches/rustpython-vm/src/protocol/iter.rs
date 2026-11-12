use crate::{
    AsObject, PyObject, PyObjectRef, PyPayload, PyResult, TryFromObject, VirtualMachine,
    builtins::iter::PySequenceIterator,
    convert::{ToPyObject, ToPyResult},
    object::{Traverse, TraverseFn},
};
use core::borrow::Borrow;
use core::ops::Deref;

/// [Iterator Protocol](https://docs.python.org/3/c-api/iter.html).
#[derive(Debug, Clone)]
#[repr(transparent)]
pub struct PyIter<O = PyObjectRef>(O)
where
    O: Borrow<PyObject>;

unsafe impl<O: Borrow<PyObject>> Traverse for PyIter<O> {
    fn traverse(&self, tracer_fn: &mut TraverseFn<'_>) {
        // Report the iterator itself, not its referents: an owner holding a
        // `PyIter` owns the iterator object, and reporting what the iterator
        // points at instead leaves the iterator's own reference unaccounted
        // for, so a cycle running through it is never collected.
        tracer_fn(self.0.borrow());
    }
}

impl PyIter<PyObjectRef> {
    pub fn check(obj: &PyObject) -> bool {
        obj.class().slots().iternext.load().is_some()
    }
}

impl<O> PyIter<O>
where
    O: Borrow<PyObject>,
{
    #[must_use]
    pub const fn new(obj: O) -> Self {
        Self(obj)
    }

    pub fn next(&self, vm: &VirtualMachine) -> PyResult<PyIterReturn> {
        // nanocode: a step of the budget (vm/limit.rs); Rust-driven iteration never
        // reaches the dispatch loop.
        if vm.spend() {
            vm.refuel()?;
        }
        let iternext = self
            .0
            .borrow()
            .class()
            .slots
            .iternext
            .load()
            .ok_or_else(|| {
                vm.new_type_error(format!(
                    "'{}' object is not an iterator",
                    self.0.borrow().class().slot_name()
                ))
            })?;
        iternext(self.0.borrow(), vm)
    }

    /// Walks the iterator without asking it how long it is. Almost nothing
    /// asks: a loop over an iterator takes no room up front, so what the
    /// object would have answered -- slowly, or by raising -- never runs.
    pub fn iter<'a, 'b, U>(
        &'b self,
        vm: &'a VirtualMachine,
    ) -> PyResult<PyIterIter<'a, U, &'b PyObject>> {
        Ok(PyIterIter::new(vm, self.0.borrow(), None))
    }
}

impl PyIter<PyObjectRef> {
    /// Returns an iterator over this sequence of objects. See [`Self::iter`]
    /// for why it does not ask how long the iterator is.
    pub fn into_iter<U>(self, vm: &VirtualMachine) -> PyIterIter<'_, U, PyObjectRef> {
        PyIterIter::new(vm, self.0, None)
    }

    /// [`Self::into_iter`] for a caller that fills a sized container from the
    /// iterator, the way `PySequence_Fast()` does. It asks how much room that
    /// takes and answers with whatever asking raised.
    pub fn into_iter_sized<U>(
        self,
        vm: &VirtualMachine,
    ) -> PyResult<PyIterIter<'_, U, PyObjectRef>> {
        let length_hint = vm.length_hint_opt(self.as_object().to_owned())?;
        Ok(PyIterIter::new(vm, self.0, length_hint))
    }
}

impl From<PyIter<Self>> for PyObjectRef {
    fn from(value: PyIter<Self>) -> Self {
        value.0
    }
}

impl<O> Borrow<PyObject> for PyIter<O>
where
    O: Borrow<PyObject>,
{
    #[inline(always)]
    fn borrow(&self) -> &PyObject {
        self.0.borrow()
    }
}

impl<O> AsRef<PyObject> for PyIter<O>
where
    O: Borrow<PyObject>,
{
    #[inline(always)]
    fn as_ref(&self) -> &PyObject {
        self.0.borrow()
    }
}

impl<O> Deref for PyIter<O>
where
    O: Borrow<PyObject>,
{
    type Target = PyObject;

    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        self.0.borrow()
    }
}

impl ToPyObject for PyIter<PyObjectRef> {
    #[inline(always)]
    fn to_pyobject(self, _vm: &VirtualMachine) -> PyObjectRef {
        self.into()
    }
}

impl TryFromObject for PyIter<PyObjectRef> {
    // This helper function is called at multiple places. First, it is called
    // in the vm when a for loop is entered. Next, it is used when the builtin
    // function 'iter' is called.
    fn try_from_object(vm: &VirtualMachine, iter_target: PyObjectRef) -> PyResult<Self> {
        let get_iter = iter_target.class().slots().iter.load();
        if let Some(get_iter) = get_iter {
            let iter = get_iter(iter_target, vm)?;
            if Self::check(&iter) {
                Ok(Self(iter))
            } else {
                Err(vm.new_type_error(format!(
                    "iter() returned non-iterator of type '{}'",
                    iter.class().slot_name()
                )))
            }
        } else if let Ok(seq_iter) = PySequenceIterator::new(iter_target.clone(), vm) {
            Ok(Self(seq_iter.into_pyobject(vm)))
        } else {
            Err(vm.new_type_error(format!(
                "'{}' object is not iterable",
                iter_target.class().slot_name()
            )))
        }
    }
}

pub enum PyIterReturn<T = PyObjectRef> {
    Return(T),
    StopIteration(Option<PyObjectRef>),
}

impl<T> PyIterReturn<T> {
    #[inline]
    pub fn from_result(result: Result<T, Option<PyObjectRef>>) -> Self {
        match result {
            Ok(value) => Self::Return(value),
            Err(error) => Self::StopIteration(error),
        }
    }

    #[inline]
    pub fn into_result(self) -> Result<T, Option<PyObjectRef>> {
        match self {
            Self::Return(value) => Ok(value),
            Self::StopIteration(error) => Err(error),
        }
    }

    #[inline]
    pub const fn as_result(&self) -> Result<&T, &Option<PyObjectRef>> {
        match self {
            Self::Return(value) => Ok(value),
            Self::StopIteration(error) => Err(error),
        }
    }

    #[inline]
    pub const fn as_result_mut(&mut self) -> Result<&mut T, &mut Option<PyObjectRef>> {
        match self {
            Self::Return(value) => Ok(value),
            Self::StopIteration(error) => Err(error),
        }
    }

    #[inline]
    pub fn map_or<U, F: FnOnce(T) -> U>(self, default: U, function: F) -> U {
        match self {
            Self::Return(value) => function(value),
            Self::StopIteration(_) => default,
        }
    }

    #[inline]
    pub fn unwrap_or(self, default: T) -> T {
        self.into_result().unwrap_or(default)
    }

    #[inline]
    pub fn unwrap_or_else<F: FnOnce(Option<PyObjectRef>) -> T>(self, function: F) -> T {
        self.into_result().unwrap_or_else(function)
    }

    #[inline]
    pub fn expect(self, message: &str) -> T {
        self.into_result().expect(message)
    }

    #[inline]
    pub fn unwrap(self) -> T {
        self.into_result().unwrap()
    }

    #[inline]
    pub fn map<U, F: FnOnce(T) -> U>(self, function: F) -> PyIterReturn<U> {
        match self {
            Self::Return(value) => PyIterReturn::Return(function(value)),
            Self::StopIteration(error) => PyIterReturn::StopIteration(error),
        }
    }

    #[inline]
    pub fn map_or_else<U, M: FnOnce(T) -> U, F: FnOnce(Option<PyObjectRef>) -> U>(
        self,
        fallback: F,
        map: M,
    ) -> U {
        self.map(map).unwrap_or_else(fallback)
    }

    #[inline]
    pub fn and<U>(self, other: PyIterReturn<U>) -> PyIterReturn<U> {
        match self {
            Self::Return(_) => other,
            Self::StopIteration(error) => PyIterReturn::StopIteration(error),
        }
    }

    #[inline]
    pub fn and_then<U, F: FnOnce(T) -> PyIterReturn<U>>(self, function: F) -> PyIterReturn<U> {
        match self {
            Self::Return(value) => function(value),
            Self::StopIteration(error) => PyIterReturn::StopIteration(error),
        }
    }

    #[inline]
    pub fn unwrap_or_default(self) -> T
    where
        T: Default,
    {
        self.into_result().unwrap_or_default()
    }
}

impl<T: core::fmt::Debug> PyIterReturn<T> {
    #[inline]
    pub fn unwrap_err(self) -> Option<PyObjectRef> {
        self.into_result().unwrap_err()
    }
}

impl<T> From<Result<T, Option<PyObjectRef>>> for PyIterReturn<T> {
    fn from(value: Result<T, Option<PyObjectRef>>) -> Self {
        Self::from_result(value)
    }
}

impl<T> From<PyIterReturn<T>> for Result<T, Option<PyObjectRef>> {
    fn from(value: PyIterReturn<T>) -> Self {
        value.into_result()
    }
}

impl<T> IntoIterator for PyIterReturn<T> {
    type Item = T;
    type IntoIter = core::result::IntoIter<T>;

    #[inline]
    fn into_iter(self) -> core::result::IntoIter<T> {
        self.into_result().into_iter()
    }
}

impl<T: Copy> PyIterReturn<&T> {
    pub fn copied(self) -> PyIterReturn<T> {
        self.map(|&value| value)
    }
}

impl<T: Clone> PyIterReturn<&T> {
    pub fn cloned(self) -> PyIterReturn<T> {
        self.map(Clone::clone)
    }
}

impl<T> PyIterReturn<Option<T>> {
    #[inline]
    pub fn transpose(self) -> Option<PyIterReturn<T>> {
        self.into_result().transpose().map(PyIterReturn::from_result)
    }
}

unsafe impl<T: Traverse> Traverse for PyIterReturn<T> {
    fn traverse(&self, tracer_fn: &mut TraverseFn<'_>) {
        match self {
            Self::Return(r) => r.traverse(tracer_fn),
            Self::StopIteration(Some(obj)) => obj.traverse(tracer_fn),
            _ => (),
        }
    }
}

impl PyIterReturn {
    pub fn from_pyresult(result: PyResult, vm: &VirtualMachine) -> PyResult<Self> {
        match result {
            Ok(obj) => Ok(Self::Return(obj)),
            Err(err) if err.fast_isinstance(vm.ctx.exceptions.stop_iteration) => {
                let args = err.get_arg(0);
                Ok(Self::StopIteration(args))
            }
            Err(err) => Err(err),
        }
    }

    pub fn from_getitem_result(result: PyResult, vm: &VirtualMachine) -> PyResult<Self> {
        match result {
            Ok(obj) => Ok(Self::Return(obj)),
            Err(err) if err.fast_isinstance(vm.ctx.exceptions.index_error) => {
                Ok(Self::StopIteration(None))
            }
            Err(err) if err.fast_isinstance(vm.ctx.exceptions.stop_iteration) => {
                let args = err.get_arg(0);
                Ok(Self::StopIteration(args))
            }
            Err(err) => Err(err),
        }
    }

    pub fn into_async_pyresult(self, vm: &VirtualMachine) -> PyResult {
        match self {
            Self::Return(obj) => Ok(obj),
            Self::StopIteration(v) => Err({
                let args = v.map_or_else(Vec::new, |v| vec![v]);
                vm.new_exception(vm.ctx.exceptions.stop_async_iteration.to_owned(), args)
            }),
        }
    }
}

impl ToPyResult for PyIterReturn {
    fn to_pyresult(self, vm: &VirtualMachine) -> PyResult {
        match self {
            Self::Return(obj) => Ok(obj),
            Self::StopIteration(v) => Err(vm.new_stop_iteration(v)),
        }
    }
}

impl ToPyResult for PyResult<PyIterReturn> {
    fn to_pyresult(self, vm: &VirtualMachine) -> PyResult {
        self?.to_pyresult(vm)
    }
}

// Typical rust `Iter` object for `PyIter`
pub struct PyIterIter<'a, T, O = PyObjectRef>
where
    O: Borrow<PyObject>,
{
    vm: &'a VirtualMachine,
    obj: O, // creating PyIter<O> is zero-cost
    length_hint: Option<usize>,
    _phantom: core::marker::PhantomData<T>,
}

unsafe impl<T, O> Traverse for PyIterIter<'_, T, O>
where
    O: Traverse + Borrow<PyObject>,
{
    fn traverse(&self, tracer_fn: &mut TraverseFn<'_>) {
        self.obj.traverse(tracer_fn)
    }
}

impl<'a, T, O> PyIterIter<'a, T, O>
where
    O: Borrow<PyObject>,
{
    pub const fn new(vm: &'a VirtualMachine, obj: O, length_hint: Option<usize>) -> Self {
        Self {
            vm,
            obj,
            length_hint,
            _phantom: core::marker::PhantomData,
        }
    }
}

impl<T, O> Iterator for PyIterIter<'_, T, O>
where
    T: TryFromObject,
    O: Borrow<PyObject>,
{
    type Item = PyResult<T>;

    fn next(&mut self) -> Option<Self::Item> {
        let imp = |next: PyResult<PyIterReturn>| -> PyResult<Option<T>> {
            let Some(obj) = next?.into_result().ok() else {
                return Ok(None);
            };
            Ok(Some(T::try_from_object(self.vm, obj)?))
        };
        let next = PyIter::new(self.obj.borrow()).next(self.vm);
        imp(next).transpose()
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.length_hint.unwrap_or(0), self.length_hint)
    }
}

/// Macro to handle `PyIterReturn` values in iterator implementations.
///
/// Extracts the object from `PyIterReturn::Return(obj)` or performs early return
/// for `PyIterReturn::StopIteration(v)`. This macro should only be used within
/// functions that return `PyResult<PyIterReturn>`.
#[macro_export]
macro_rules! raise_if_stop {
    ($input:expr) => {
        match $input {
            $crate::protocol::PyIterReturn::Return(obj) => obj,
            $crate::protocol::PyIterReturn::StopIteration(v) => {
                return Ok($crate::protocol::PyIterReturn::StopIteration(v))
            }
        }
    };
}

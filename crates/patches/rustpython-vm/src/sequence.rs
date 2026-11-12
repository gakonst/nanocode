use crate::{
    AsObject, PyObject, PyObjectRef, PyResult,
    function::OptionalArg,
    sliceable::SequenceIndexOp,
    types::PyComparisonOp,
    vm::{MAX_MEMORY_SIZE, VirtualMachine},
};
use core::ops::{Deref, Range};

pub trait MutObjectSequenceOp {
    type Inner: ?Sized;

    fn do_get(index: usize, inner: &Self::Inner) -> Option<&PyObject>;
    fn do_lock(&self) -> impl Deref<Target = Self::Inner>;

    fn mut_count(&self, vm: &VirtualMachine, needle: &PyObject) -> PyResult<usize> {
        let mut count = 0;
        self._mut_iter_equal_skeleton::<_, false>(vm, needle, 0..isize::MAX as usize, || {
            count += 1
        })?;
        Ok(count)
    }

    fn mut_index_range(
        &self,
        vm: &VirtualMachine,
        needle: &PyObject,
        range: Range<usize>,
    ) -> PyResult<Option<usize>> {
        self._mut_iter_equal_skeleton::<_, true>(vm, needle, range, || {})
    }

    fn mut_index(&self, vm: &VirtualMachine, needle: &PyObject) -> PyResult<Option<usize>> {
        self.mut_index_range(vm, needle, 0..isize::MAX as usize)
    }

    fn mut_contains(&self, vm: &VirtualMachine, needle: &PyObject) -> PyResult<bool> {
        self.mut_index(vm, needle).map(|x| x.is_some())
    }

    fn _mut_iter_equal_skeleton<F, const SHORT: bool>(
        &self,
        vm: &VirtualMachine,
        needle: &PyObject,
        range: Range<usize>,
        mut f: F,
    ) -> PyResult<Option<usize>>
    where
        F: FnMut(),
    {
        let mut borrower = None;
        let mut i = range.start;

        let index = loop {
            if i >= range.end {
                break None;
            }
            let guard = if let Some(x) = borrower.take() {
                x
            } else {
                self.do_lock()
            };

            let elem = if let Some(x) = Self::do_get(i, &guard) {
                x
            } else {
                break None;
            };

            if elem.is(needle) {
                f();
                if SHORT {
                    break Some(i);
                }
                borrower = Some(guard);
            } else {
                let elem = elem.to_owned();
                drop(guard);

                if elem.rich_compare_bool(needle, PyComparisonOp::Eq, vm)? {
                    f();
                    if SHORT {
                        break Some(i);
                    }
                }
            }
            i += 1;
        };

        Ok(index)
    }
}

pub trait SequenceExt<T: Clone>
where
    Self: AsRef<[T]>,
{
    fn mul(&self, vm: &VirtualMachine, n: isize) -> PyResult<Vec<T>> {
        let n = vm.check_repeat_or_overflow_error(self.as_ref().len(), n)?;
        if self.as_ref().is_empty() {
            return Ok(Vec::new());
        }

        if n > 1 && core::mem::size_of_val(self.as_ref()) >= MAX_MEMORY_SIZE / n {
            return Err(vm.no_memory_error());
        }

        let total = n
            .checked_mul(self.as_ref().len())
            .ok_or_else(|| vm.no_memory_error())?;
        // nanocode: the embedder's memory bound (vm/limit.rs).
        vm.reserve(core::mem::size_of_val(self.as_ref()).saturating_mul(n))?;
        let mut v = Vec::new();
        v.try_reserve_exact(total)
            .map_err(|_| vm.no_memory_error())?;
        for _ in 0..n {
            v.extend_from_slice(self.as_ref());
        }
        Ok(v)
    }
}

impl<T: Clone> SequenceExt<T> for [T] {}

pub trait SequenceMutExt<T: Clone>
where
    Self: AsRef<[T]>,
{
    fn as_vec_mut(&mut self) -> &mut Vec<T>;

    fn imul(&mut self, vm: &VirtualMachine, n: isize) -> PyResult<()> {
        let n = vm.check_repeat_or_overflow_error(self.as_ref().len(), n)?;

        if n > 1 && core::mem::size_of_val(self.as_ref()) >= MAX_MEMORY_SIZE / n {
            // TODO: make a global static NoMemory shared exc object and return its reference.
            return Err(vm.no_memory_error());
        }

        if n == 0 {
            self.as_vec_mut().clear();
        } else if n != 1 && !self.as_ref().is_empty() {
            // nanocode: the embedder's memory bound (vm/limit.rs).
            vm.reserve(core::mem::size_of_val(self.as_ref()).saturating_mul(n - 1))?;
            let len = self.as_ref().len();
            let v = self.as_vec_mut();
            v.try_reserve_exact(len * (n - 1))
                .map_err(|_| vm.no_memory_error())?;
            for _ in 1..n {
                v.extend_from_within(..len);
            }
        }
        Ok(())
    }
}

impl<T: Clone> SequenceMutExt<T> for Vec<T> {
    fn as_vec_mut(&mut self) -> &mut Self {
        self
    }
}

#[derive(FromArgs)]
pub struct OptionalRangeArgs {
    #[pyarg(positional, default = 0)]
    start: PyObjectRef,
    // Platform ssize maximum. Missing is clamped to the sequence length.
    #[pyarg(positional, optional, py_default = "9223372036854775807")]
    stop: OptionalArg<PyObjectRef>,
}

impl OptionalRangeArgs {
    pub fn saturate(self, len: usize, vm: &VirtualMachine) -> PyResult<(usize, usize)> {
        let saturate = |obj: PyObjectRef| -> PyResult<_> {
            Ok(obj.try_index(vm)?.as_bigint().saturated_at(len))
        };
        let start = saturate(self.start)?;
        let stop = self.stop.map_or(Ok(len), saturate)?;
        Ok((start, stop))
    }
}

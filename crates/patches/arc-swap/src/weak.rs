use core::ptr;

use alloc::rc::Weak as RcWeak;
use alloc::sync::Weak;

use crate::RefCnt;

unsafe impl<T> RefCnt for Weak<T> {
    type Base = T;
    fn as_ptr(me: &Self) -> *mut T {
        if Weak::ptr_eq(&Weak::new(), me) {
            ptr::null_mut()
        } else {
            Weak::as_ptr(me) as *mut T
        }
    }
    fn into_ptr(me: Self) -> *mut T {
        if Weak::ptr_eq(&Weak::new(), &me) {
            ptr::null_mut()
        } else {
            Weak::into_raw(me) as *mut T
        }
    }
    unsafe fn from_ptr(ptr: *const T) -> Self {
        if ptr.is_null() {
            Weak::new()
        } else {
            Weak::from_raw(ptr)
        }
    }
}

unsafe impl<T> RefCnt for RcWeak<T> {
    type Base = T;
    fn as_ptr(me: &Self) -> *mut T {
        if RcWeak::ptr_eq(&RcWeak::new(), me) {
            ptr::null_mut()
        } else {
            RcWeak::as_ptr(me) as *mut T
        }
    }
    fn into_ptr(me: Self) -> *mut T {
        if RcWeak::ptr_eq(&RcWeak::new(), &me) {
            ptr::null_mut()
        } else {
            RcWeak::into_raw(me) as *mut T
        }
    }
    unsafe fn from_ptr(ptr: *const T) -> Self {
        if ptr.is_null() {
            RcWeak::new()
        } else {
            RcWeak::from_raw(ptr)
        }
    }
}

use std::{collections::HashMap, ffi::c_int, mem::transmute_copy};

use crate::module::arity_tables;

thread_local! {
    static PARAMS: HashMap<usize, usize> = collect();
}

fn collect() -> HashMap<usize, usize> {
    let mut params = HashMap::new();
    for table in arity_tables() {
        let mut at = table;
        loop {
            let (function, count) = unsafe { (*at, *at.add(1)) };
            if function == 0 {
                break;
            }
            params.insert(function, count);
            at = unsafe { at.add(2) };
        }
    }
    params
}

fn declared(function: usize, expected: usize) -> Option<usize> {
    PARAMS
        .with(|params| params.get(&function).copied())
        .filter(|&count| count != expected && count <= WIDEST)
}

pub trait Word: Copy {
    fn word(self) -> usize;
}

impl<T> Word for *mut T {
    fn word(self) -> usize {
        self as usize
    }
}

impl<T> Word for *const T {
    fn word(self) -> usize {
        self as usize
    }
}

impl Word for isize {
    fn word(self) -> usize {
        self.cast_unsigned()
    }
}

impl Word for c_int {
    fn word(self) -> usize {
        self as isize as usize
    }
}

const WIDEST: usize = 4;

unsafe fn widened<R>(function: usize, count: usize, words: [usize; WIDEST]) -> R {
    let [a, b, c, d] = words;
    unsafe {
        match count {
            0 => transmute_copy::<usize, unsafe extern "C" fn() -> R>(&function)(),
            1 => transmute_copy::<usize, unsafe extern "C" fn(usize) -> R>(&function)(a),
            2 => transmute_copy::<usize, unsafe extern "C" fn(usize, usize) -> R>(&function)(a, b),
            3 => {
                transmute_copy::<usize, unsafe extern "C" fn(usize, usize, usize) -> R>(&function)(
                    a, b, c,
                )
            }
            _ => transmute_copy::<usize, unsafe extern "C" fn(usize, usize, usize, usize) -> R>(
                &function,
            )(a, b, c, d),
        }
    }
}

pub unsafe fn call1<A: Word, R>(function: unsafe extern "C" fn(A) -> R, a: A) -> R {
    match declared(function as usize, 1) {
        Some(count) => unsafe { widened(function as usize, count, [a.word(), 0, 0, 0]) },
        None => unsafe { function(a) },
    }
}

pub unsafe fn call2<A: Word, B: Word, R>(
    function: unsafe extern "C" fn(A, B) -> R,
    a: A,
    b: B,
) -> R {
    match declared(function as usize, 2) {
        Some(count) => unsafe { widened(function as usize, count, [a.word(), b.word(), 0, 0]) },
        None => unsafe { function(a, b) },
    }
}

pub unsafe fn call3<A: Word, B: Word, C: Word, R>(
    function: unsafe extern "C" fn(A, B, C) -> R,
    a: A,
    b: B,
    c: C,
) -> R {
    match declared(function as usize, 3) {
        Some(count) => unsafe {
            widened(function as usize, count, [a.word(), b.word(), c.word(), 0])
        },
        None => unsafe { function(a, b, c) },
    }
}

pub unsafe fn call4<A: Word, B: Word, C: Word, D: Word, R>(
    function: unsafe extern "C" fn(A, B, C, D) -> R,
    a: A,
    b: B,
    c: C,
    d: D,
) -> R {
    match declared(function as usize, 4) {
        Some(count) => unsafe {
            widened(
                function as usize,
                count,
                [a.word(), b.word(), c.word(), d.word()],
            )
        },
        None => unsafe { function(a, b, c, d) },
    }
}

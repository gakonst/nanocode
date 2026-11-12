use std::cell::{Cell, OnceCell};
use std::rc::Rc;

#[derive(Debug)]
pub struct List<T>(Option<Rc<(T, List<T>)>>);

impl<T> Clone for List<T> {
    fn clone(&self) -> Self {
        List(self.0.clone())
    }
}

impl<T> Default for List<T> {
    fn default() -> Self {
        List(None)
    }
}

impl<T> Drop for List<T> {
    fn drop(&mut self) {
        while let Some(node) = self.0.take() {
            match Rc::try_unwrap(node) {
                Ok((_, mut rest)) => self.0 = rest.0.take(),
                Err(_) => break,
            }
        }
    }
}

impl<T> List<T> {
    pub fn cons(self, x: T) -> Self {
        List(Some(Rc::new((x, self))))
    }

    pub fn skip(&self, count: usize) -> &Self {
        let mut at = self;
        for _ in 0..count {
            match &at.0 {
                Some(node) => at = &node.1,
                None => break,
            }
        }
        at
    }

    pub fn get(&self, index: usize) -> Option<&T> {
        self.skip(index).0.as_ref().map(|node| &node.0)
    }

    pub fn iter(&self) -> Iter<'_, T> {
        Iter(self)
    }
}

impl<T: Clone> List<T> {
    pub fn pop(mut self) -> Option<(T, Self)> {
        let node = self.0.take()?;
        Some(Rc::try_unwrap(node).unwrap_or_else(|shared| (shared.0.clone(), shared.1.clone())))
    }
}

pub struct Iter<'l, T>(&'l List<T>);

impl<'l, T> Iterator for Iter<'l, T> {
    type Item = &'l T;

    fn next(&mut self) -> Option<&'l T> {
        let node = self.0.0.as_ref()?;
        self.0 = &node.1;
        Some(&node.0)
    }
}

type Thunk<'a, T> = Box<dyn FnOnce() -> Option<(T, Lazy<'a, T>)> + 'a>;

struct LazyNode<'a, T> {
    value: OnceCell<Option<(T, Lazy<'a, T>)>>,
    pending: Cell<Option<Thunk<'a, T>>>,
}

pub struct Lazy<'a, T>(Rc<LazyNode<'a, T>>);

impl<T> Clone for Lazy<'_, T> {
    fn clone(&self) -> Self {
        Lazy(self.0.clone())
    }
}

impl<'a, T: 'a> Lazy<'a, T> {
    pub fn new(mut iter: impl Iterator<Item = T> + 'a) -> Self {
        let thunk: Thunk<'a, T> = Box::new(move || iter.next().map(|x| (x, Lazy::new(iter))));
        Lazy(Rc::new(LazyNode {
            value: OnceCell::new(),
            pending: Cell::new(Some(thunk)),
        }))
    }

    fn force(&self) -> &Option<(T, Lazy<'a, T>)> {
        self.0.value.get_or_init(|| {
            let thunk = self
                .0
                .pending
                .take()
                .expect("a lazy list node is forced once");
            thunk()
        })
    }
}

impl<'a, T: Clone + 'a> Iterator for Lazy<'a, T> {
    type Item = T;

    fn next(&mut self) -> Option<T> {
        let (x, rest) = self.force().as_ref()?;
        let (x, rest) = (x.clone(), rest.clone());
        *self = rest;
        Some(x)
    }
}

impl<T> Drop for Lazy<'_, T> {
    fn drop(&mut self) {
        while let Some(node) = Rc::get_mut(&mut self.0) {
            let Some(Some((_, rest))) = node.value.take() else {
                break;
            };
            *self = rest;
        }
    }
}

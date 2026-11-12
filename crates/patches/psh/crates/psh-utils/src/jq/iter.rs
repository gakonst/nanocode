pub type BoxIter<'a, T> = Box<dyn Iterator<Item = T> + 'a>;

pub fn box_once<'a, T: 'a>(x: T) -> BoxIter<'a, T> {
    Box::new(core::iter::once(x))
}

pub fn then<'a, T, U: 'a, E: 'a>(
    x: Result<T, E>,
    f: impl FnOnce(T) -> BoxIter<'a, Result<U, E>>,
) -> BoxIter<'a, Result<U, E>> {
    match x {
        Ok(x) => f(x),
        Err(e) => box_once(Err(e)),
    }
}

fn single<T>(iter: &mut impl Iterator<Item = T>) -> Option<T> {
    if iter.size_hint().1 != Some(1) {
        return None;
    }
    iter.next()
}

pub fn map_with<'a, T: Clone + 'a, U: 'a, V: 'a>(
    mut l: impl Iterator<Item = U> + 'a,
    x: T,
    r: impl Fn(U, T) -> V + 'a,
) -> BoxIter<'a, V> {
    match single(&mut l) {
        Some(y) => box_once(r(y, x)),
        None => Box::new(l.map(move |y| r(y, x.clone()))),
    }
}

pub fn flat_map_with<'a, T: Clone + 'a, U: 'a, V: 'a>(
    mut l: impl Iterator<Item = U> + 'a,
    x: T,
    r: impl Fn(U, T) -> BoxIter<'a, V> + 'a,
) -> BoxIter<'a, V> {
    match single(&mut l) {
        Some(y) => r(y, x),
        None => Box::new(l.flat_map(move |y| r(y, x.clone()))),
    }
}

pub fn flat_map_then<'a, T: 'a, U: 'a, E: 'a>(
    mut l: impl Iterator<Item = Result<T, E>> + 'a,
    r: impl Fn(T) -> BoxIter<'a, Result<U, E>> + 'a,
) -> BoxIter<'a, Result<U, E>> {
    match single(&mut l) {
        Some(y) => then(y, r),
        None => Box::new(l.flat_map(move |y| then(y, &r))),
    }
}

pub fn flat_map_then_with<'a, T: Clone + 'a, U: 'a, V: 'a, E: 'a>(
    l: impl Iterator<Item = Result<U, E>> + 'a,
    x: T,
    r: impl Fn(U, T) -> BoxIter<'a, Result<V, E>> + 'a,
) -> BoxIter<'a, Result<V, E>> {
    flat_map_with(l, x, move |y, x| then(y, |y| r(y, x)))
}

pub fn lazy<'a, T: 'a, I: Iterator<Item = T> + 'a>(
    f: impl FnOnce() -> I + 'a,
) -> impl Iterator<Item = T> + 'a {
    core::iter::once_with(f).flatten()
}

pub enum Indices<I, F> {
    Once(core::iter::Once<I>),
    Delayed(F),
}

impl<I: Clone, F: Clone> Clone for Indices<I, F> {
    fn clone(&self) -> Self {
        match self {
            Indices::Once(once) => Indices::Once(once.clone()),
            Indices::Delayed(f) => Indices::Delayed(f.clone()),
        }
    }
}

pub enum IndicesIter<I, J> {
    Once(core::iter::Once<I>),
    Run(J),
}

impl<I, J: Iterator<Item = I>> Iterator for IndicesIter<I, J> {
    type Item = I;

    fn next(&mut self) -> Option<I> {
        match self {
            IndicesIter::Once(once) => once.next(),
            IndicesIter::Run(run) => run.next(),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        match self {
            IndicesIter::Once(once) => once.size_hint(),
            IndicesIter::Run(run) => run.size_hint(),
        }
    }
}

impl<I, J: Iterator<Item = I>, F: FnOnce() -> J> IntoIterator for Indices<I, F> {
    type Item = I;
    type IntoIter = IndicesIter<I, J>;

    fn into_iter(self) -> IndicesIter<I, J> {
        match self {
            Indices::Once(once) => IndicesIter::Once(once),
            Indices::Delayed(f) => IndicesIter::Run(f()),
        }
    }
}

pub fn collect_if_once<I: Iterator, F: FnOnce() -> I + Clone>(f: F) -> Indices<I::Item, F> {
    let mut iter = f.clone()();
    if iter.size_hint().1 == Some(1)
        && let Some(x) = iter.next()
    {
        assert!(
            iter.next().is_none(),
            "an iterator that promised one item gave more"
        );
        return Indices::Once(core::iter::once(x));
    }
    Indices::Delayed(f)
}

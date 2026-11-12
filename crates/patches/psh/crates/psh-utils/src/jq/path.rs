use super::eval::{Exn, ValX, ValXs};
use super::iter::{BoxIter, box_once, flat_map_with, map_with, then};
use super::list::List;
use super::val::{Error, Val, ValR};

#[derive(Clone, Debug)]
pub struct Path<T>(pub Vec<(Part<T>, Opt)>);

#[derive(Clone, Debug)]
pub enum Part<T> {
    Index(T),
    Range(Option<T>, Option<T>),
}

#[derive(Clone, Copy, Debug)]
pub enum Opt {
    Optional,
    Essential,
}

impl<T> Default for Part<T> {
    fn default() -> Self {
        Part::Range(None, None)
    }
}

impl Opt {
    pub fn fail<T, E>(self, x: T, f: impl FnOnce(T) -> E) -> Result<T, E> {
        match self {
            Opt::Optional => Ok(x),
            Opt::Essential => Err(f(x)),
        }
    }
}

pub type PathList = List<Val>;
type ValRs<'a> = BoxIter<'a, ValR>;
type ValPathRs<'a> = BoxIter<'a, Result<(Val, PathList), Error>>;

impl<T> Part<T> {
    pub fn as_ref(&self) -> Part<&T> {
        match self {
            Part::Index(i) => Part::Index(i),
            Part::Range(from, upto) => Part::Range(from.as_ref(), upto.as_ref()),
        }
    }

    pub fn map<U>(self, mut f: impl FnMut(T) -> U) -> Part<U> {
        match self {
            Part::Index(i) => Part::Index(f(i)),
            Part::Range(from, upto) => Part::Range(from.map(&mut f), upto.map(&mut f)),
        }
    }
}

impl<T> Path<T> {
    pub fn map_ref<'p, U>(&'p self, mut f: impl FnMut(&'p T) -> U) -> Path<U> {
        Path(
            self.0
                .iter()
                .map(|(part, opt)| (part.as_ref().map(&mut f), *opt))
                .collect(),
        )
    }
}

impl<E> Part<Result<Val, E>> {
    fn transpose(self) -> Result<Part<Val>, E> {
        Ok(match self {
            Part::Index(i) => Part::Index(i?),
            Part::Range(from, upto) => Part::Range(from.transpose()?, upto.transpose()?),
        })
    }
}

fn part_values<'a, F>(part: Part<F>) -> BoxIter<'a, Part<F::Item>>
where
    F: IntoIterator + Clone + 'a,
    F::Item: Clone + 'a,
{
    match part {
        Part::Index(i) => Box::new(i.into_iter().map(Part::Index)),
        Part::Range(None, None) => box_once(Part::Range(None, None)),
        Part::Range(Some(from), None) => {
            Box::new(from.into_iter().map(|from| Part::Range(Some(from), None)))
        }
        Part::Range(None, Some(upto)) => {
            Box::new(upto.into_iter().map(|upto| Part::Range(None, Some(upto))))
        }
        Part::Range(Some(from), Some(upto)) => {
            flat_map_with(from.into_iter(), upto, |from, upto| {
                map_with(upto.into_iter(), from, |upto, from| {
                    Part::Range(Some(from), Some(upto))
                })
            })
        }
    }
}

fn combinations<'a, F, I>(
    prefix: Vec<(Part<F::Item>, Opt)>,
    mut rest: I,
) -> BoxIter<'a, Vec<(Part<F::Item>, Opt)>>
where
    F: IntoIterator + Clone + 'a,
    F::Item: Clone + 'a,
    I: Iterator<Item = (Part<F>, Opt)> + Clone + 'a,
{
    match rest.next() {
        Some((part, opt)) => flat_map_with(
            part_values(part),
            (prefix, rest),
            move |part, (mut prefix, rest)| {
                prefix.push((part, opt));
                combinations::<F, I>(prefix, rest)
            },
        ),
        None => box_once(prefix),
    }
}

pub fn explode<'a, F, E>(path: Path<F>) -> impl Iterator<Item = Result<Path<Val>, E>> + 'a
where
    F: IntoIterator<Item = Result<Val, E>> + Clone + 'a,
    E: Clone + 'a,
{
    combinations::<F, _>(Vec::new(), path.0.into_iter()).map(|parts| {
        let mut out = Vec::with_capacity(parts.len());
        for (part, opt) in parts {
            out.push((part.transpose()?, opt));
        }
        Ok(Path(out))
    })
}

fn walk<'a, T: 'a>(
    mut parts: impl Iterator<Item = (Part<Val>, Opt)> + Clone + 'a,
    value: T,
    step: fn(Part<Val>, T) -> BoxIter<'a, Result<T, Error>>,
) -> BoxIter<'a, Result<T, Error>> {
    match parts.next() {
        Some((part, opt)) => {
            let essential = matches!(opt, Opt::Essential);
            let ys = step(part, value).filter(move |y| essential || y.is_ok());
            flat_map_with(ys, parts, move |y, parts| then(y, |y| walk(parts, y, step)))
        }
        None => box_once(Ok(value)),
    }
}

fn update_rest<'a, P, F>(mut parts: P, last: (Part<Val>, Opt), value: Val, f: &F) -> ValX
where
    P: Iterator<Item = (Part<Val>, Opt)> + Clone,
    F: Fn(Val) -> ValXs<'a>,
{
    match parts.next() {
        Some((part, opt)) => part_update(&part, value, opt, |v| {
            core::iter::once(update_rest(parts.clone(), last.clone(), v, f))
        }),
        None => part_update(&last.0, value, last.1, f),
    }
}

fn part_run<'a>(part: Part<Val>, value: Val) -> ValRs<'a> {
    match part {
        Part::Index(index) => box_once(value.index(&index)),
        Part::Range(None, None) => value.values(),
        Part::Range(from, upto) => box_once(value.range(from.as_ref(), upto.as_ref())),
    }
}

fn part_paths<'a>(part: Part<Val>, (value, path): (Val, PathList)) -> ValPathRs<'a> {
    match part {
        Part::Index(index) => box_once(value.index(&index).map(|v| (v, path.cons(index)))),
        Part::Range(None, None) => Box::new(
            value
                .key_values()
                .map(move |kv| kv.map(|(k, v)| (v, path.clone().cons(k)))),
        ),
        Part::Range(from, upto) => box_once(
            value
                .range(from.as_ref(), upto.as_ref())
                .map(|v| (v, path.cons(Val::from_range(from, upto)))),
        ),
    }
}

fn part_update<I: Iterator<Item = ValX>>(
    part: &Part<Val>,
    value: Val,
    opt: Opt,
    f: impl Fn(Val) -> I,
) -> ValX {
    match part {
        Part::Index(index) => value.map_index(index, opt, f),
        Part::Range(None, None) => value.map_values(opt, f),
        Part::Range(from, upto) => value.map_range(from.as_ref(), upto.as_ref(), opt, f),
    }
}

impl Path<Val> {
    pub fn run<'a>(self, value: Val) -> ValRs<'a> {
        walk(self.0.into_iter(), value, part_run)
    }

    pub fn paths<'a>(self, value: (Val, PathList)) -> ValPathRs<'a> {
        walk(self.0.into_iter(), value, part_paths)
    }

    pub fn update<'a, F: Fn(Val) -> ValXs<'a>>(mut self, value: Val, f: F) -> ValX {
        match self.0.pop() {
            Some(last) => update_rest(self.0.into_iter(), last, value, &f),
            None => Ok(value),
        }
    }
}

pub fn path_error(value: Val) -> Exn {
    Exn::from(Error::path_expr(value))
}

use std::{
    pin::Pin,
    task::{Context, Poll},
};

pub use crate::sink::{SplitSink, SplitStream};
pub use crate::unordered::{FuturesOrdered, FuturesUnordered};
use crate::{
    future::Future,
    sink::{Sink, split},
};

pub trait Stream {
    type Item;

    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>>;

    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, None)
    }
}

impl<S: Stream + Unpin + ?Sized> Stream for &mut S {
    type Item = S::Item;

    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<S::Item>> {
        S::poll_next(Pin::new(&mut **self.get_mut()), context)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (**self).size_hint()
    }
}

impl<S: Stream + Unpin + ?Sized> Stream for Box<S> {
    type Item = S::Item;

    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<S::Item>> {
        S::poll_next(Pin::new(&mut **self.get_mut()), context)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (**self).size_hint()
    }
}

impl<P> Stream for Pin<P>
where
    P: std::ops::DerefMut + Unpin,
    P::Target: Stream,
{
    type Item = <P::Target as Stream>::Item;

    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.get_mut().as_mut().poll_next(context)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (**self).size_hint()
    }
}

pub trait FusedStream: Stream {
    fn is_terminated(&self) -> bool;
}

impl<S: FusedStream + Unpin + ?Sized> FusedStream for &mut S {
    fn is_terminated(&self) -> bool {
        (**self).is_terminated()
    }
}

impl<P> FusedStream for Pin<P>
where
    P: std::ops::DerefMut + Unpin,
    P::Target: FusedStream,
{
    fn is_terminated(&self) -> bool {
        (**self).is_terminated()
    }
}

pub trait TryStream: Stream {
    type Ok;
    type Error;

    fn try_poll_next(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<Self::Ok, Self::Error>>>;
}

impl<S, T, E> TryStream for S
where
    S: Stream<Item = Result<T, E>> + ?Sized,
{
    type Ok = T;
    type Error = E;

    fn try_poll_next(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<T, E>>> {
        self.poll_next(context)
    }
}

pub type BoxStream<'a, T> = Pin<Box<dyn Stream<Item = T> + Send + 'a>>;
pub type LocalBoxStream<'a, T> = Pin<Box<dyn Stream<Item = T> + 'a>>;

pub trait StreamExt: Stream {
    fn next(&mut self) -> Next<'_, Self>
    where
        Self: Unpin,
    {
        Next { stream: self }
    }

    fn poll_next_unpin(&mut self, context: &mut Context<'_>) -> Poll<Option<Self::Item>>
    where
        Self: Unpin,
    {
        Pin::new(self).poll_next(context)
    }

    fn map<T, F>(self, change: F) -> Map<Self, F>
    where
        F: FnMut(Self::Item) -> T,
        Self: Sized,
    {
        Map {
            stream: Box::pin(self),
            change,
        }
    }

    fn filter<Fut, F>(self, keep: F) -> Filter<Self, Fut, F>
    where
        F: FnMut(&Self::Item) -> Fut,
        Fut: Future<Output = bool>,
        Self: Sized,
    {
        Filter {
            stream: Box::pin(self),
            keep,
            pending: Box::pin(None),
            item: None,
        }
    }

    fn filter_map<Fut, T, F>(self, change: F) -> FilterMap<Self, Fut, F>
    where
        F: FnMut(Self::Item) -> Fut,
        Fut: Future<Output = Option<T>>,
        Self: Sized,
    {
        FilterMap {
            stream: Box::pin(self),
            change,
            pending: Box::pin(None),
        }
    }

    fn then<Fut, F>(self, change: F) -> Then<Self, Fut, F>
    where
        F: FnMut(Self::Item) -> Fut,
        Fut: Future,
        Self: Sized,
    {
        Then {
            stream: Box::pin(self),
            change,
            pending: Box::pin(None),
        }
    }

    fn chain<St>(self, second: St) -> Chain<Self, St>
    where
        St: Stream<Item = Self::Item>,
        Self: Sized,
    {
        Chain {
            first: Some(Box::pin(self)),
            second: Box::pin(second),
        }
    }

    fn take(self, count: usize) -> Take<Self>
    where
        Self: Sized,
    {
        Take {
            stream: Box::pin(self),
            remaining: count,
        }
    }

    fn fuse(self) -> Fuse<Self>
    where
        Self: Sized,
    {
        Fuse {
            stream: Box::pin(self),
            done: false,
        }
    }

    fn collect<C: Default + Extend<Self::Item>>(self) -> Collect<Self, C>
    where
        Self: Sized,
    {
        Collect {
            stream: Box::pin(self),
            collection: C::default(),
        }
    }

    fn buffered(self, window: usize) -> Buffered<Self>
    where
        Self::Item: Future,
        Self: Sized,
    {
        Buffered {
            stream: self.fuse(),
            queue: FuturesOrdered::new(),
            window,
        }
    }

    fn boxed<'a>(self) -> BoxStream<'a, Self::Item>
    where
        Self: Sized + Send + 'a,
    {
        Box::pin(self)
    }

    fn boxed_local<'a>(self) -> LocalBoxStream<'a, Self::Item>
    where
        Self: Sized + 'a,
    {
        Box::pin(self)
    }

    fn split<Item>(self) -> (SplitSink<Self, Item>, SplitStream<Self>)
    where
        Self: Sink<Item> + Sized,
    {
        split(self)
    }
}

impl<T: Stream + ?Sized> StreamExt for T {}

pub trait TryStreamExt: TryStream {
    fn map_ok<T, F>(self, change: F) -> MapOk<Self, F>
    where
        F: FnMut(Self::Ok) -> T,
        Self: Sized,
    {
        MapOk {
            stream: Box::pin(self),
            change,
        }
    }

    fn map_err<E, F>(self, change: F) -> MapErr<Self, F>
    where
        F: FnMut(Self::Error) -> E,
        Self: Sized,
    {
        MapErr {
            stream: Box::pin(self),
            change,
        }
    }

    fn try_next(&mut self) -> TryNext<'_, Self>
    where
        Self: Unpin,
    {
        TryNext { stream: self }
    }

    fn try_poll_next_unpin(
        &mut self,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<Self::Ok, Self::Error>>>
    where
        Self: Unpin,
    {
        Pin::new(self).try_poll_next(context)
    }

    fn try_fold<T, Fut, F>(self, init: T, fold: F) -> TryFold<Self, Fut, T, F>
    where
        F: FnMut(T, Self::Ok) -> Fut,
        Fut: Future<Output = Result<T, Self::Error>>,
        Self: Sized,
    {
        TryFold {
            stream: Box::pin(self),
            fold,
            accumulated: Some(init),
            pending: Box::pin(None),
        }
    }
}

impl<S: TryStream + ?Sized> TryStreamExt for S {}

pub struct Next<'a, S: ?Sized> {
    stream: &'a mut S,
}

impl<S: Stream + Unpin + ?Sized> Future for Next<'_, S> {
    type Output = Option<S::Item>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<S::Item>> {
        Pin::new(&mut *self.get_mut().stream).poll_next(context)
    }
}

pub struct TryNext<'a, S: ?Sized> {
    stream: &'a mut S,
}

impl<S: TryStream + Unpin + ?Sized> Future for TryNext<'_, S> {
    type Output = Result<Option<S::Ok>, S::Error>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut *self.get_mut().stream)
            .try_poll_next(context)
            .map(Option::transpose)
    }
}

pub struct Map<S, F> {
    stream: Pin<Box<S>>,
    change: F,
}

impl<S: Stream, F: FnMut(S::Item) -> T, T> Stream for Map<S, F> {
    type Item = T;

    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<T>> {
        let this = self.get_mut();
        this.stream
            .as_mut()
            .poll_next(context)
            .map(|item| item.map(&mut this.change))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.stream.size_hint()
    }
}

impl<S: FusedStream, F: FnMut(S::Item) -> T, T> FusedStream for Map<S, F> {
    fn is_terminated(&self) -> bool {
        self.stream.is_terminated()
    }
}

pub struct MapOk<S, F> {
    stream: Pin<Box<S>>,
    change: F,
}

impl<S: TryStream, F: FnMut(S::Ok) -> T, T> Stream for MapOk<S, F> {
    type Item = Result<T, S::Error>;

    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        this.stream
            .as_mut()
            .try_poll_next(context)
            .map(|item| item.map(|item| item.map(&mut this.change)))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.stream.size_hint()
    }
}

pub struct MapErr<S, F> {
    stream: Pin<Box<S>>,
    change: F,
}

impl<S: TryStream, F: FnMut(S::Error) -> E, E> Stream for MapErr<S, F> {
    type Item = Result<S::Ok, E>;

    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        this.stream
            .as_mut()
            .try_poll_next(context)
            .map(|item| item.map(|item| item.map_err(&mut this.change)))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.stream.size_hint()
    }
}

pub struct Filter<S: Stream, Fut, F> {
    stream: Pin<Box<S>>,
    keep: F,
    pending: Pin<Box<Option<Fut>>>,
    item: Option<S::Item>,
}

impl<S, Fut, F> Stream for Filter<S, Fut, F>
where
    S: Stream,
    F: FnMut(&S::Item) -> Fut,
    Fut: Future<Output = bool>,
{
    type Item = S::Item;

    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<S::Item>> {
        let this = self.get_mut();
        loop {
            if let Some(pending) = this.pending.as_mut().as_pin_mut() {
                let Poll::Ready(kept) = pending.poll(context) else {
                    return Poll::Pending;
                };
                this.pending.set(None);
                if kept {
                    return Poll::Ready(this.item.take());
                }
                this.item = None;
            } else {
                let Poll::Ready(item) = this.stream.as_mut().poll_next(context) else {
                    return Poll::Pending;
                };
                let Some(item) = item else {
                    return Poll::Ready(None);
                };
                this.pending.set(Some((this.keep)(&item)));
                this.item = Some(item);
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let pending = usize::from(self.item.is_some());
        let (_, upper) = self.stream.size_hint();
        (0, upper.and_then(|upper| upper.checked_add(pending)))
    }
}

pub struct FilterMap<S, Fut, F> {
    stream: Pin<Box<S>>,
    change: F,
    pending: Pin<Box<Option<Fut>>>,
}

impl<S: Unpin, Fut, F> FilterMap<S, Fut, F> {
    pub fn get_mut(&mut self) -> &mut S {
        self.stream.as_mut().get_mut()
    }
}

impl<S: Sink<Item>, Fut, F, Item> Sink<Item> for FilterMap<S, Fut, F> {
    type Error = S::Error;

    fn poll_ready(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Result<(), S::Error>> {
        self.get_mut().stream.as_mut().poll_ready(context)
    }

    fn start_send(self: Pin<&mut Self>, item: Item) -> Result<(), S::Error> {
        self.get_mut().stream.as_mut().start_send(item)
    }

    fn poll_flush(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Result<(), S::Error>> {
        self.get_mut().stream.as_mut().poll_flush(context)
    }

    fn poll_close(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Result<(), S::Error>> {
        self.get_mut().stream.as_mut().poll_close(context)
    }
}

impl<S, Fut, F, T> Stream for FilterMap<S, Fut, F>
where
    S: Stream,
    F: FnMut(S::Item) -> Fut,
    Fut: Future<Output = Option<T>>,
{
    type Item = T;

    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<T>> {
        let this = self.get_mut();
        loop {
            if let Some(pending) = this.pending.as_mut().as_pin_mut() {
                let Poll::Ready(item) = pending.poll(context) else {
                    return Poll::Pending;
                };
                this.pending.set(None);
                if let Some(item) = item {
                    return Poll::Ready(Some(item));
                }
            } else {
                let Poll::Ready(item) = this.stream.as_mut().poll_next(context) else {
                    return Poll::Pending;
                };
                let Some(item) = item else {
                    return Poll::Ready(None);
                };
                this.pending.set(Some((this.change)(item)));
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let pending = usize::from(self.pending.is_some());
        let (_, upper) = self.stream.size_hint();
        (0, upper.and_then(|upper| upper.checked_add(pending)))
    }
}

pub struct Then<S, Fut, F> {
    stream: Pin<Box<S>>,
    change: F,
    pending: Pin<Box<Option<Fut>>>,
}

impl<S, Fut, F> Stream for Then<S, Fut, F>
where
    S: Stream,
    F: FnMut(S::Item) -> Fut,
    Fut: Future,
{
    type Item = Fut::Output;

    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Fut::Output>> {
        let this = self.get_mut();
        loop {
            if let Some(pending) = this.pending.as_mut().as_pin_mut() {
                let Poll::Ready(item) = pending.poll(context) else {
                    return Poll::Pending;
                };
                this.pending.set(None);
                return Poll::Ready(Some(item));
            }
            let Poll::Ready(item) = this.stream.as_mut().poll_next(context) else {
                return Poll::Pending;
            };
            let Some(item) = item else {
                return Poll::Ready(None);
            };
            this.pending.set(Some((this.change)(item)));
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let pending = usize::from(self.pending.is_some());
        let (lower, upper) = self.stream.size_hint();
        (
            lower.saturating_add(pending),
            upper.and_then(|upper| upper.checked_add(pending)),
        )
    }
}

pub struct Chain<S1, S2> {
    first: Option<Pin<Box<S1>>>,
    second: Pin<Box<S2>>,
}

impl<S1: Stream, S2: Stream<Item = S1::Item>> Stream for Chain<S1, S2> {
    type Item = S1::Item;

    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<S1::Item>> {
        let this = self.get_mut();
        if let Some(first) = this.first.as_mut() {
            let Poll::Ready(item) = first.as_mut().poll_next(context) else {
                return Poll::Pending;
            };
            if item.is_some() {
                return Poll::Ready(item);
            }
            this.first = None;
        }
        this.second.as_mut().poll_next(context)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let Some(first) = self.first.as_ref() else {
            return self.second.size_hint();
        };
        let (first_lower, first_upper) = first.size_hint();
        let (second_lower, second_upper) = self.second.size_hint();
        let upper = match (first_upper, second_upper) {
            (Some(first), Some(second)) => first.checked_add(second),
            _ => None,
        };
        (first_lower.saturating_add(second_lower), upper)
    }
}

pub struct Take<S> {
    stream: Pin<Box<S>>,
    remaining: usize,
}

impl<S: Stream> Stream for Take<S> {
    type Item = S::Item;

    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<S::Item>> {
        let this = self.get_mut();
        if this.remaining == 0 {
            return Poll::Ready(None);
        }
        let Poll::Ready(item) = this.stream.as_mut().poll_next(context) else {
            return Poll::Pending;
        };
        if item.is_some() {
            this.remaining -= 1;
        } else {
            this.remaining = 0;
        }
        Poll::Ready(item)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        if self.remaining == 0 {
            return (0, Some(0));
        }
        let (lower, upper) = self.stream.size_hint();
        let lower = lower.min(self.remaining);
        let upper = match upper {
            Some(upper) if upper < self.remaining => Some(upper),
            _ => Some(self.remaining),
        };
        (lower, upper)
    }
}

pub struct Fuse<S> {
    stream: Pin<Box<S>>,
    done: bool,
}

impl<S> Fuse<S> {
    #[must_use]
    pub const fn is_done(&self) -> bool {
        self.done
    }
}

impl<S: Stream> Stream for Fuse<S> {
    type Item = S::Item;

    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<S::Item>> {
        let this = self.get_mut();
        if this.done {
            return Poll::Ready(None);
        }
        let Poll::Ready(item) = this.stream.as_mut().poll_next(context) else {
            return Poll::Pending;
        };
        if item.is_none() {
            this.done = true;
        }
        Poll::Ready(item)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        if self.done {
            (0, Some(0))
        } else {
            self.stream.size_hint()
        }
    }
}

impl<S: Stream> FusedStream for Fuse<S> {
    fn is_terminated(&self) -> bool {
        self.done
    }
}

pub struct Collect<S, C> {
    stream: Pin<Box<S>>,
    collection: C,
}

impl<S: Stream, C: Default + Extend<S::Item>> Future for Collect<S, C> {
    type Output = C;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<C> {
        let this = self.get_mut();
        loop {
            let Poll::Ready(item) = this.stream.as_mut().poll_next(context) else {
                return Poll::Pending;
            };
            match item {
                Some(item) => this.collection.extend(Some(item)),
                None => return Poll::Ready(std::mem::take(&mut this.collection)),
            }
        }
    }
}

pub struct Buffered<S: Stream>
where
    S::Item: Future,
{
    stream: Fuse<S>,
    queue: FuturesOrdered<S::Item>,
    window: usize,
}

impl<S: Stream> Stream for Buffered<S>
where
    S::Item: Future,
{
    type Item = <S::Item as Future>::Output;

    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        while this.queue.len() < this.window {
            match Pin::new(&mut this.stream).poll_next(context) {
                Poll::Ready(Some(future)) => this.queue.push_back(future),
                Poll::Ready(None) | Poll::Pending => break,
            }
        }
        let Poll::Ready(item) = Pin::new(&mut this.queue).poll_next(context) else {
            return Poll::Pending;
        };
        if item.is_some() {
            return Poll::Ready(item);
        }
        if this.stream.is_done() {
            Poll::Ready(None)
        } else {
            Poll::Pending
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let queued = self.queue.len();
        let (lower, upper) = self.stream.size_hint();
        (
            lower.saturating_add(queued),
            upper.and_then(|upper| upper.checked_add(queued)),
        )
    }
}

pub struct TryFold<S, Fut, T, F> {
    stream: Pin<Box<S>>,
    fold: F,
    accumulated: Option<T>,
    pending: Pin<Box<Option<Fut>>>,
}

impl<S, Fut, T, F> Future for TryFold<S, Fut, T, F>
where
    S: TryStream,
    F: FnMut(T, S::Ok) -> Fut,
    Fut: Future<Output = Result<T, S::Error>>,
{
    type Output = Result<T, S::Error>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        loop {
            if let Some(pending) = this.pending.as_mut().as_pin_mut() {
                let Poll::Ready(folded) = pending.poll(context) else {
                    return Poll::Pending;
                };
                this.pending.set(None);
                match folded {
                    Ok(accumulated) => this.accumulated = Some(accumulated),
                    Err(error) => return Poll::Ready(Err(error)),
                }
            } else if this.accumulated.is_some() {
                let Poll::Ready(item) = this.stream.as_mut().try_poll_next(context) else {
                    return Poll::Pending;
                };
                let accumulated = this
                    .accumulated
                    .take()
                    .expect("the accumulator is there between items");
                match item {
                    Some(Ok(item)) => this.pending.set(Some((this.fold)(accumulated, item))),
                    Some(Err(error)) => return Poll::Ready(Err(error)),
                    None => return Poll::Ready(Ok(accumulated)),
                }
            } else {
                panic!("Fold polled after completion");
            }
        }
    }
}

pub fn unfold<T, F, Fut, Item>(init: T, step: F) -> Unfold<T, F, Fut>
where
    F: FnMut(T) -> Fut,
    Fut: Future<Output = Option<(Item, T)>>,
{
    Unfold {
        step,
        state: Some(init),
        pending: Box::pin(None),
    }
}

pub struct Unfold<T, F, Fut> {
    step: F,
    state: Option<T>,
    pending: Pin<Box<Option<Fut>>>,
}

impl<T, F, Fut, Item> Stream for Unfold<T, F, Fut>
where
    F: FnMut(T) -> Fut,
    Fut: Future<Output = Option<(Item, T)>>,
{
    type Item = Item;

    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Item>> {
        let this = self.get_mut();
        if let Some(state) = this.state.take() {
            this.pending.set(Some((this.step)(state)));
        }
        let Some(pending) = this.pending.as_mut().as_pin_mut() else {
            panic!("Unfold must not be polled after it returned `Poll::Ready(None)`");
        };
        let Poll::Ready(step) = pending.poll(context) else {
            return Poll::Pending;
        };
        this.pending.set(None);
        let Some((item, state)) = step else {
            return Poll::Ready(None);
        };
        this.state = Some(state);
        Poll::Ready(Some(item))
    }
}

impl<T, F, Fut, Item> FusedStream for Unfold<T, F, Fut>
where
    F: FnMut(T) -> Fut,
    Fut: Future<Output = Option<(Item, T)>>,
{
    fn is_terminated(&self) -> bool {
        self.state.is_none() && self.pending.is_none()
    }
}

pub fn try_unfold<T, F, Fut, Item, E>(init: T, step: F) -> TryUnfold<T, F, Fut>
where
    F: FnMut(T) -> Fut,
    Fut: Future<Output = Result<Option<(Item, T)>, E>>,
{
    TryUnfold {
        step,
        state: Some(init),
        pending: Box::pin(None),
    }
}

pub struct TryUnfold<T, F, Fut> {
    step: F,
    state: Option<T>,
    pending: Pin<Box<Option<Fut>>>,
}

impl<T, F, Fut, Item, E> Stream for TryUnfold<T, F, Fut>
where
    F: FnMut(T) -> Fut,
    Fut: Future<Output = Result<Option<(Item, T)>, E>>,
{
    type Item = Result<Item, E>;

    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if let Some(state) = this.state.take() {
            this.pending.set(Some((this.step)(state)));
        }
        let Some(pending) = this.pending.as_mut().as_pin_mut() else {
            return Poll::Ready(None);
        };
        let Poll::Ready(step) = pending.poll(context) else {
            return Poll::Pending;
        };
        this.pending.set(None);
        match step {
            Ok(Some((item, state))) => {
                this.state = Some(state);
                Poll::Ready(Some(Ok(item)))
            }
            Ok(None) => Poll::Ready(None),
            Err(error) => Poll::Ready(Some(Err(error))),
        }
    }
}

pub fn once<Fut: Future>(future: Fut) -> Once<Fut> {
    Once {
        pending: Box::pin(Some(future)),
    }
}

pub struct Once<Fut> {
    pending: Pin<Box<Option<Fut>>>,
}

impl<Fut: Future> Stream for Once<Fut> {
    type Item = Fut::Output;

    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Fut::Output>> {
        let this = self.get_mut();
        let Some(pending) = this.pending.as_mut().as_pin_mut() else {
            return Poll::Ready(None);
        };
        let Poll::Ready(output) = pending.poll(context) else {
            return Poll::Pending;
        };
        this.pending.set(None);
        Poll::Ready(Some(output))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        if self.pending.is_some() {
            (1, Some(1))
        } else {
            (0, Some(0))
        }
    }
}

impl<Fut: Future> FusedStream for Once<Fut> {
    fn is_terminated(&self) -> bool {
        self.pending.is_none()
    }
}

pub fn iter<I: IntoIterator>(items: I) -> Iter<I::IntoIter> {
    Iter {
        items: items.into_iter(),
    }
}

pub struct Iter<I> {
    items: I,
}

impl<I> Unpin for Iter<I> {}

impl<I: Iterator> Stream for Iter<I> {
    type Item = I::Item;

    fn poll_next(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<Option<I::Item>> {
        Poll::Ready(self.get_mut().items.next())
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.items.size_hint()
    }
}

#[must_use]
pub const fn empty<T>() -> Empty<T> {
    Empty {
        item: std::marker::PhantomData,
    }
}

pub struct Empty<T> {
    item: std::marker::PhantomData<T>,
}

impl<T> Unpin for Empty<T> {}

impl<T> Stream for Empty<T> {
    type Item = T;

    fn poll_next(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<Option<T>> {
        Poll::Ready(None)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, Some(0))
    }
}

impl<T> FusedStream for Empty<T> {
    fn is_terminated(&self) -> bool {
        true
    }
}

impl<S, F> Unpin for Map<S, F> {}
impl<S, F> Unpin for MapOk<S, F> {}
impl<S, F> Unpin for MapErr<S, F> {}
impl<S: Stream, Fut, F> Unpin for Filter<S, Fut, F> {}
impl<S, Fut, F> Unpin for FilterMap<S, Fut, F> {}
impl<S, Fut, F> Unpin for Then<S, Fut, F> {}
impl<S, C> Unpin for Collect<S, C> {}
impl<S: Stream> Unpin for Buffered<S> where S::Item: Future {}
impl<S, Fut, T, F> Unpin for TryFold<S, Fut, T, F> {}
impl<T, F, Fut> Unpin for Unfold<T, F, Fut> {}
impl<T, F, Fut> Unpin for TryUnfold<T, F, Fut> {}

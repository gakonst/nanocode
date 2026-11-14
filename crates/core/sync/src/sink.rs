use std::{
    marker::PhantomData,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use nanocode_sys::{AtomicBox, AtomicWaker};

use crate::{future::Future, stream::Stream};

pub trait Sink<Item> {
    type Error;

    fn poll_ready(self: Pin<&mut Self>, context: &mut Context<'_>)
    -> Poll<Result<(), Self::Error>>;

    fn start_send(self: Pin<&mut Self>, item: Item) -> Result<(), Self::Error>;

    fn poll_flush(self: Pin<&mut Self>, context: &mut Context<'_>)
    -> Poll<Result<(), Self::Error>>;

    fn poll_close(self: Pin<&mut Self>, context: &mut Context<'_>)
    -> Poll<Result<(), Self::Error>>;
}

impl<S: Sink<Item> + Unpin + ?Sized, Item> Sink<Item> for &mut S {
    type Error = S::Error;

    fn poll_ready(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Result<(), S::Error>> {
        Pin::new(&mut **self.get_mut()).poll_ready(context)
    }

    fn start_send(self: Pin<&mut Self>, item: Item) -> Result<(), S::Error> {
        Pin::new(&mut **self.get_mut()).start_send(item)
    }

    fn poll_flush(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Result<(), S::Error>> {
        Pin::new(&mut **self.get_mut()).poll_flush(context)
    }

    fn poll_close(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Result<(), S::Error>> {
        Pin::new(&mut **self.get_mut()).poll_close(context)
    }
}

impl<S: Sink<Item> + Unpin + ?Sized, Item> Sink<Item> for Box<S> {
    type Error = S::Error;

    fn poll_ready(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Result<(), S::Error>> {
        Pin::new(&mut **self.get_mut()).poll_ready(context)
    }

    fn start_send(self: Pin<&mut Self>, item: Item) -> Result<(), S::Error> {
        Pin::new(&mut **self.get_mut()).start_send(item)
    }

    fn poll_flush(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Result<(), S::Error>> {
        Pin::new(&mut **self.get_mut()).poll_flush(context)
    }

    fn poll_close(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Result<(), S::Error>> {
        Pin::new(&mut **self.get_mut()).poll_close(context)
    }
}

impl<P, Item> Sink<Item> for Pin<P>
where
    P: std::ops::DerefMut + Unpin,
    P::Target: Sink<Item>,
{
    type Error = <P::Target as Sink<Item>>::Error;

    fn poll_ready(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        self.get_mut().as_mut().poll_ready(context)
    }

    fn start_send(self: Pin<&mut Self>, item: Item) -> Result<(), Self::Error> {
        self.get_mut().as_mut().start_send(item)
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        self.get_mut().as_mut().poll_flush(context)
    }

    fn poll_close(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        self.get_mut().as_mut().poll_close(context)
    }
}

pub trait SinkExt<Item>: Sink<Item> {
    fn send(&mut self, item: Item) -> Send<'_, Self, Item>
    where
        Self: Unpin,
    {
        Send {
            sink: self,
            item: Some(item),
        }
    }

    fn feed(&mut self, item: Item) -> Feed<'_, Self, Item>
    where
        Self: Unpin,
    {
        Feed {
            sink: self,
            item: Some(item),
        }
    }

    fn flush(&mut self) -> Flush<'_, Self, Item>
    where
        Self: Unpin,
    {
        Flush {
            sink: self,
            item: PhantomData,
        }
    }

    fn close(&mut self) -> Close<'_, Self, Item>
    where
        Self: Unpin,
    {
        Close {
            sink: self,
            item: PhantomData,
        }
    }

    fn poll_ready_unpin(&mut self, context: &mut Context<'_>) -> Poll<Result<(), Self::Error>>
    where
        Self: Unpin,
    {
        Pin::new(self).poll_ready(context)
    }

    fn start_send_unpin(&mut self, item: Item) -> Result<(), Self::Error>
    where
        Self: Unpin,
    {
        Pin::new(self).start_send(item)
    }

    fn poll_flush_unpin(&mut self, context: &mut Context<'_>) -> Poll<Result<(), Self::Error>>
    where
        Self: Unpin,
    {
        Pin::new(self).poll_flush(context)
    }

    fn poll_close_unpin(&mut self, context: &mut Context<'_>) -> Poll<Result<(), Self::Error>>
    where
        Self: Unpin,
    {
        Pin::new(self).poll_close(context)
    }

    fn with<U, Fut, F, E>(self, change: F) -> With<Self, Item, U, Fut, F>
    where
        F: FnMut(U) -> Fut,
        Fut: Future<Output = Result<Item, E>>,
        E: From<Self::Error>,
        Self: Sized,
    {
        With {
            sink: Box::pin(self),
            change,
            pending: Box::pin(None),
            items: PhantomData,
        }
    }
}

impl<T: Sink<Item> + ?Sized, Item> SinkExt<Item> for T {}

pub struct Feed<'a, S: ?Sized, Item> {
    sink: &'a mut S,
    item: Option<Item>,
}

impl<S: ?Sized, Item> Unpin for Feed<'_, S, Item> {}

impl<S: Sink<Item> + Unpin + ?Sized, Item> Future for Feed<'_, S, Item> {
    type Output = Result<(), S::Error>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let mut sink = Pin::new(&mut *this.sink);
        match sink.as_mut().poll_ready(context) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
            Poll::Ready(Ok(())) => {}
        }
        let item = this.item.take().expect("polled Feed after completion");
        Poll::Ready(sink.start_send(item))
    }
}

pub struct Send<'a, S: ?Sized, Item> {
    sink: &'a mut S,
    item: Option<Item>,
}

impl<S: ?Sized, Item> Unpin for Send<'_, S, Item> {}

impl<S: Sink<Item> + Unpin + ?Sized, Item> Future for Send<'_, S, Item> {
    type Output = Result<(), S::Error>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let mut sink = Pin::new(&mut *this.sink);
        if let Some(item) = this.item.take() {
            match sink.as_mut().poll_ready(context) {
                Poll::Pending => {
                    this.item = Some(item);
                    return Poll::Pending;
                }
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Ready(Ok(())) => sink.as_mut().start_send(item)?,
            }
        }
        sink.poll_flush(context)
    }
}

pub struct Flush<'a, S: ?Sized, Item> {
    sink: &'a mut S,
    item: PhantomData<fn(Item)>,
}

impl<S: ?Sized, Item> Unpin for Flush<'_, S, Item> {}

impl<S: Sink<Item> + Unpin + ?Sized, Item> Future for Flush<'_, S, Item> {
    type Output = Result<(), S::Error>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut *self.get_mut().sink).poll_flush(context)
    }
}

pub struct Close<'a, S: ?Sized, Item> {
    sink: &'a mut S,
    item: PhantomData<fn(Item)>,
}

impl<S: ?Sized, Item> Unpin for Close<'_, S, Item> {}

impl<S: Sink<Item> + Unpin + ?Sized, Item> Future for Close<'_, S, Item> {
    type Output = Result<(), S::Error>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut *self.get_mut().sink).poll_close(context)
    }
}

pub struct With<S, Item, U, Fut, F> {
    sink: Pin<Box<S>>,
    change: F,
    pending: Pin<Box<Option<Fut>>>,
    items: PhantomData<fn(U) -> Item>,
}

impl<S: Unpin, Item, U, Fut, F> With<S, Item, U, Fut, F> {
    pub fn get_mut(&mut self) -> &mut S {
        self.sink.as_mut().get_mut()
    }
}

impl<S, Item, U, Fut, F, E> With<S, Item, U, Fut, F>
where
    S: Sink<Item>,
    F: FnMut(U) -> Fut,
    Fut: Future<Output = Result<Item, E>>,
    E: From<S::Error>,
{
    fn poll_pending(&mut self, context: &mut Context<'_>) -> Poll<Result<(), E>> {
        let Some(pending) = self.pending.as_mut().as_pin_mut() else {
            return Poll::Ready(Ok(()));
        };
        let Poll::Ready(item) = pending.poll(context) else {
            return Poll::Pending;
        };
        let item = item?;
        self.pending.set(None);
        self.sink.as_mut().start_send(item)?;
        Poll::Ready(Ok(()))
    }

    fn poll_then(
        &mut self,
        context: &mut Context<'_>,
        then: impl FnOnce(Pin<&mut S>, &mut Context<'_>) -> Poll<Result<(), S::Error>>,
    ) -> Poll<Result<(), E>> {
        match self.poll_pending(context) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
            Poll::Ready(Ok(())) => {}
        }
        then(self.sink.as_mut(), context).map(|done| done.map_err(E::from))
    }
}

impl<S, Item, U, Fut, F, E> Sink<U> for With<S, Item, U, Fut, F>
where
    S: Sink<Item>,
    F: FnMut(U) -> Fut,
    Fut: Future<Output = Result<Item, E>>,
    E: From<S::Error>,
{
    type Error = E;

    fn poll_ready(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Result<(), E>> {
        self.get_mut().poll_then(context, Sink::poll_ready)
    }

    fn start_send(self: Pin<&mut Self>, item: U) -> Result<(), E> {
        let this = self.get_mut();
        assert!(this.pending.is_none(), "With takes one item at a time");
        this.pending.set(Some((this.change)(item)));
        Ok(())
    }

    fn poll_flush(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Result<(), E>> {
        self.get_mut().poll_then(context, Sink::poll_flush)
    }

    fn poll_close(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Result<(), E>> {
        self.get_mut().poll_then(context, Sink::poll_close)
    }
}

impl<S: Stream + Sink<Item>, Item, U, Fut, F> Stream for With<S, Item, U, Fut, F> {
    type Item = S::Item;

    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<S::Item>> {
        self.get_mut().sink.as_mut().poll_next(context)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.sink.size_hint()
    }
}

struct Halves<T> {
    value: AtomicBox<Pin<Box<T>>>,
    stream_waker: AtomicWaker,
    sink_waker: AtomicWaker,
}

#[derive(Clone, Copy)]
enum Half {
    Stream,
    Sink,
}

struct Held<'a, T> {
    halves: &'a Halves<T>,
    half: Half,
    value: Option<Box<Pin<Box<T>>>>,
}

impl<T> Held<'_, T> {
    fn as_pin_mut(&mut self) -> Pin<&mut T> {
        Pin::as_mut(
            self.value
                .as_mut()
                .expect("a held half holds the value until it drops"),
        )
    }
}

impl<T> Drop for Held<'_, T> {
    fn drop(&mut self) {
        self.halves.value.store(self.value.take());
        match self.half {
            Half::Stream => self.halves.sink_waker.wake(),
            Half::Sink => self.halves.stream_waker.wake(),
        }
    }
}

impl<T> Halves<T> {
    fn poll_hold(&self, half: Half, context: &mut Context<'_>) -> Poll<Held<'_, T>> {
        let own = match half {
            Half::Stream => &self.stream_waker,
            Half::Sink => &self.sink_waker,
        };
        let value = match self.value.take() {
            Some(value) => value,
            None => {
                own.register(context.waker());
                let Some(value) = self.value.take() else {
                    return Poll::Pending;
                };
                drop(own.take());
                value
            }
        };
        Poll::Ready(Held {
            halves: self,
            half,
            value: Some(value),
        })
    }
}

pub struct SplitStream<S> {
    halves: Arc<Halves<S>>,
}

pub struct SplitSink<S, Item> {
    halves: Arc<Halves<S>>,
    slot: Option<Item>,
}

impl<S, Item> Unpin for SplitSink<S, Item> {}

pub fn split<S: Stream + Sink<Item>, Item>(both: S) -> (SplitSink<S, Item>, SplitStream<S>) {
    let halves = Arc::new(Halves {
        value: AtomicBox::new(Some(Box::new(Box::pin(both)))),
        stream_waker: AtomicWaker::new(),
        sink_waker: AtomicWaker::new(),
    });
    (
        SplitSink {
            halves: Arc::clone(&halves),
            slot: None,
        },
        SplitStream { halves },
    )
}

impl<S: Stream> Stream for SplitStream<S> {
    type Item = S::Item;

    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<S::Item>> {
        let Poll::Ready(mut held) = self.halves.poll_hold(Half::Stream, context) else {
            return Poll::Pending;
        };
        held.as_pin_mut().poll_next(context)
    }
}

impl<S: Sink<Item>, Item> SplitSink<S, Item> {
    fn poll_flush_slot(
        mut held: Pin<&mut S>,
        slot: &mut Option<Item>,
        context: &mut Context<'_>,
    ) -> Poll<Result<(), S::Error>> {
        if slot.is_none() {
            return Poll::Ready(Ok(()));
        }
        match held.as_mut().poll_ready(context) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
            Poll::Ready(Ok(())) => {
                let item = slot.take().expect("the slot holds an item");
                Poll::Ready(held.start_send(item))
            }
        }
    }

    fn poll_then(
        &mut self,
        context: &mut Context<'_>,
        then: impl FnOnce(Pin<&mut S>, &mut Context<'_>) -> Poll<Result<(), S::Error>>,
    ) -> Poll<Result<(), S::Error>> {
        let Poll::Ready(mut held) = self.halves.poll_hold(Half::Sink, context) else {
            return Poll::Pending;
        };
        match Self::poll_flush_slot(held.as_pin_mut(), &mut self.slot, context) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
            Poll::Ready(Ok(())) => {}
        }
        then(held.as_pin_mut(), context)
    }
}

impl<S: Sink<Item>, Item> Sink<Item> for SplitSink<S, Item> {
    type Error = S::Error;

    fn poll_ready(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Result<(), S::Error>> {
        let this = self.get_mut();
        loop {
            if this.slot.is_none() {
                return Poll::Ready(Ok(()));
            }
            let Poll::Ready(mut held) = this.halves.poll_hold(Half::Sink, context) else {
                return Poll::Pending;
            };
            match Self::poll_flush_slot(held.as_pin_mut(), &mut this.slot, context) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Ready(Ok(())) => {}
            }
        }
    }

    fn start_send(self: Pin<&mut Self>, item: Item) -> Result<(), S::Error> {
        self.get_mut().slot = Some(item);
        Ok(())
    }

    fn poll_flush(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Result<(), S::Error>> {
        self.get_mut().poll_then(context, Sink::poll_flush)
    }

    fn poll_close(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Result<(), S::Error>> {
        self.get_mut().poll_then(context, Sink::poll_close)
    }
}

impl<S, Item, U, Fut, F> Unpin for With<S, Item, U, Fut, F> {}

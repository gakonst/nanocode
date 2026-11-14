use std::{
    future::Future,
    panic::{AssertUnwindSafe, catch_unwind},
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll},
};

use nanocode_sys::{AtomicBox, AtomicWaker};

use crate::task::{Abortable, Current, Id, JoinError, JoinHandle, Joinable};

pub type Boxed = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

struct Remote<T> {
    id: Id,
    output: AtomicBox<Result<T, JoinError>>,
    finished: AtomicBool,
    aborted: AtomicBool,
    joiner: AtomicWaker,
    runner: AtomicWaker,
}

impl<T> Remote<T> {
    fn complete(&self, output: Result<T, JoinError>) {
        if self.finished.load(Ordering::Acquire) {
            return;
        }
        self.output.store(Some(Box::new(output)));
        self.finished.store(true, Ordering::Release);
        self.joiner.wake();
    }
}

impl<T: Send + 'static> Joinable<T> for Remote<T> {
    fn poll_join(&self, context: &mut Context<'_>) -> Poll<Result<T, JoinError>> {
        if !self.finished.load(Ordering::Acquire) {
            self.joiner.register(context.waker());
            if !self.finished.load(Ordering::Acquire) {
                return Poll::Pending;
            }
        }
        match self.output.take() {
            Some(output) => Poll::Ready(*output),
            None => panic!("a JoinHandle's output is taken once, after the task finished"),
        }
    }

    fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }

    fn id(&self) -> Id {
        self.id
    }
}

impl<T: Send + 'static> Abortable for Remote<T> {
    fn abort_task(self: Arc<Self>) {
        self.aborted.store(true, Ordering::Release);
        self.runner.wake();
    }

    fn finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }

    fn task_id(&self) -> Id {
        self.id
    }
}

struct Driven<F: Future> {
    cell: Arc<Remote<F::Output>>,
    future: Option<Pin<Box<F>>>,
}

impl<F: Future> Future for Driven<F> {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<()> {
        let id = self.cell.id;
        self.cell.runner.register(context.waker());
        if self.cell.aborted.load(Ordering::Acquire) {
            if let Some(future) = self.future.take() {
                let _ = catch_unwind(AssertUnwindSafe(|| drop(future)));
            }
            self.cell.complete(Err(JoinError::cancelled(id)));
            return Poll::Ready(());
        }
        let Some(future) = self.future.as_mut() else {
            return Poll::Ready(());
        };
        let polled = {
            let _current = Current::enter(Some(id));
            catch_unwind(AssertUnwindSafe(|| future.as_mut().poll(context)))
        };
        match polled {
            Ok(Poll::Pending) => Poll::Pending,
            Ok(Poll::Ready(output)) => {
                self.future = None;
                self.cell.complete(Ok(output));
                Poll::Ready(())
            }
            Err(payload) => {
                if let Some(future) = self.future.take() {
                    let _ = catch_unwind(AssertUnwindSafe(|| drop(future)));
                }
                self.cell.complete(Err(JoinError::panic(id, payload)));
                Poll::Ready(())
            }
        }
    }
}

impl<F: Future> Drop for Driven<F> {
    fn drop(&mut self) {
        let id = self.cell.id;
        self.future = None;
        self.cell.complete(Err(JoinError::cancelled(id)));
    }
}

pub fn spawn_with<F>(future: F, spawn: impl FnOnce(Boxed)) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    let cell = Arc::new(Remote {
        id: Id::next(),
        output: AtomicBox::empty(),
        finished: AtomicBool::new(false),
        aborted: AtomicBool::new(false),
        joiner: AtomicWaker::new(),
        runner: AtomicWaker::new(),
    });
    spawn(Box::pin(Driven {
        cell: cell.clone(),
        future: Some(Box::pin(future)),
    }));
    JoinHandle::new(cell.clone(), cell)
}

use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};

use futures_util::{
    FutureExt as _,
    future::{AbortHandle, Abortable},
};
use nanocode_agent_core::backend::BackendFuture;
use nanocode_platform::{Instant, MaybeSend};
use tokio::sync::oneshot;

#[derive(Debug)]
pub(crate) struct TaskError;

impl std::fmt::Display for TaskError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("task was aborted")
    }
}

pub(crate) struct Task<T> {
    result: oneshot::Receiver<T>,
    abort: AbortHandle,
}

pub(crate) struct TaskAbortGuard(AbortHandle);

impl Drop for TaskAbortGuard {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl<T> Task<T> {
    pub(crate) fn abort_on_drop(&self) -> TaskAbortGuard {
        TaskAbortGuard(self.abort.clone())
    }

    pub(crate) fn abort(&self) {
        self.abort.abort();
    }
}

impl<T> Future for Task<T> {
    type Output = Result<T, TaskError>;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.result)
            .poll(context)
            .map(|result| result.map_err(|_| TaskError))
    }
}

pub(crate) fn spawn<T, F>(future: F) -> Task<T>
where
    T: MaybeSend + 'static,
    F: Future<Output = T> + MaybeSend + 'static,
{
    let (send, result) = oneshot::channel();
    let (abort, registration) = AbortHandle::new_pair();
    spawn_detached(Box::pin(
        Abortable::new(
            future.map(move |output| {
                let _ = send.send(output);
            }),
            registration,
        )
        .map(drop),
    ));
    Task { result, abort }
}

#[cfg(not(target_family = "wasm"))]
fn workers() -> usize {
    std::thread::available_parallelism().map_or(1, std::num::NonZero::get)
}

#[cfg(target_family = "wasm")]
const fn workers() -> usize {
    1
}

pub(crate) async fn spread<T, R, F>(items: Vec<T>, work: F) -> std::io::Result<Vec<R>>
where
    T: MaybeSend + 'static,
    R: MaybeSend + 'static,
    F: Fn(T) -> std::io::Result<R> + Clone + MaybeSend + 'static,
{
    let parts = workers().min(items.len());
    if parts < 2 {
        return items.into_iter().map(work).collect();
    }
    let size = items.len().div_ceil(parts);
    let mut items = items.into_iter();
    let mut running = Vec::with_capacity(parts);
    loop {
        let part = items.by_ref().take(size).collect::<Vec<_>>();
        if part.is_empty() {
            break;
        }
        let work = work.clone();
        running.push(spawn(async move {
            part.into_iter()
                .map(work)
                .collect::<std::io::Result<Vec<_>>>()
        }));
    }
    let mut all = Vec::new();
    for part in running {
        all.extend(
            part.await
                .map_err(|error| std::io::Error::other(error.to_string()))??,
        );
    }
    Ok(all)
}

pub(crate) async fn timeout_at<T, F>(deadline: Instant, future: F) -> Result<T, ()>
where
    F: Future<Output = T>,
{
    let delay = deadline.saturating_duration_since(Instant::now());
    futures_util::pin_mut!(future);
    let timeout = sleep(delay);
    futures_util::pin_mut!(timeout);
    match futures_util::future::select(future, timeout).await {
        futures_util::future::Either::Left((output, _)) => Ok(output),
        futures_util::future::Either::Right(((), _)) => Err(()),
    }
}

fn spawn_detached(future: BackendFuture<()>) {
    #[cfg(not(target_family = "wasm"))]
    drop(tokio::spawn(future));
    #[cfg(target_family = "wasm")]
    nanocode_platform::spawn_local(future);
}

#[cfg(not(target_family = "wasm"))]
pub(crate) async fn sleep(duration: Duration) {
    tokio::time::sleep(duration).await;
}

#[cfg(target_family = "wasm")]
pub(crate) async fn sleep(duration: Duration) {
    nanocode_platform::sleep(duration).await;
}

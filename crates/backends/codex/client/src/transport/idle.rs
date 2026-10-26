use std::{future::Future, time::Duration};

#[cfg(not(target_family = "wasm"))]
#[derive(Default)]
pub(crate) struct IdleTimer(Option<std::pin::Pin<Box<tokio::time::Sleep>>>);

#[cfg(target_family = "wasm")]
#[derive(Default)]
pub(crate) struct IdleTimer;

impl IdleTimer {
    #[cfg(not(target_family = "wasm"))]
    pub(crate) async fn within<T>(
        &mut self,
        timeout: Duration,
        read: impl Future<Output = T>,
    ) -> Option<T> {
        let deadline = tokio::time::Instant::now() + timeout;
        let sleep = self
            .0
            .get_or_insert_with(|| Box::pin(tokio::time::sleep_until(deadline)));
        let mut read = std::pin::pin!(read);
        loop {
            tokio::select! {
                biased;
                value = &mut read => return Some(value),
                () = sleep.as_mut() => {
                    if tokio::time::Instant::now() >= deadline {
                        return None;
                    }
                    sleep.as_mut().reset(deadline);
                }
            }
        }
    }

    #[cfg(target_family = "wasm")]
    pub(crate) async fn within<T>(
        &mut self,
        _timeout: Duration,
        read: impl Future<Output = T>,
    ) -> Option<T> {
        Some(read.await)
    }
}

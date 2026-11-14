use std::{future::Future, time::Duration};

#[cfg(not(target_family = "wasm"))]
#[derive(Default)]
pub(crate) struct IdleTimer(Option<nanocode_platform::Sleep>);

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
        let deadline = nanocode_platform::now() + timeout;
        let sleep = self
            .0
            .get_or_insert_with(|| nanocode_platform::Sleep::until(deadline));
        let mut read = std::pin::pin!(read);
        loop {
            nanocode_sync::select! {
                value = &mut read => return Some(value),
                () = &mut *sleep => {
                    if nanocode_platform::now() >= deadline {
                        return None;
                    }
                    sleep.reset(deadline);
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

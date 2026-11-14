use std::{future::Future, time::Duration};

#[cfg(not(target_family = "wasm"))]
#[derive(Default)]
pub(crate) struct IdleTimer(Option<std::pin::Pin<Box<dyn Future<Output = ()> + Send>>>);

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
        let mut read = std::pin::pin!(read);
        loop {
            let sleep = self
                .0
                .get_or_insert_with(|| Box::pin(nanocode_platform::sleep(timeout)));
            tokio::select! {
                biased;
                value = &mut read => return Some(value),
                () = sleep.as_mut() => {}
            }
            let now = nanocode_platform::now();
            if now >= deadline {
                self.0 = None;
                return None;
            }
            self.0 = Some(Box::pin(nanocode_platform::sleep(deadline - now)));
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

use std::{future::Future, time::Duration};

pub(crate) const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Default)]
pub(crate) struct IdleTimer(Option<nanocode_platform::Sleep>);

impl IdleTimer {
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
                        self.0 = None;
                        return None;
                    }
                    sleep.reset(deadline);
                }
            }
        }
    }
}

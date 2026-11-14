#[cfg(unix)]
pub(crate) use nanocode_sys::unix::poll::{Event, Events, Poller};

#[cfg(unix)]
pub(crate) type Raw<'a> = std::os::fd::BorrowedFd<'a>;

#[cfg(windows)]
pub(crate) use windows::{Event, Events, Poller};

#[cfg(windows)]
pub(crate) type Raw<'a> = std::os::windows::io::BorrowedSocket<'a>;

pub trait AsRaw {
    fn as_raw_source(&self) -> Raw<'_>;
}

#[cfg(unix)]
impl<T: std::os::fd::AsFd> AsRaw for T {
    fn as_raw_source(&self) -> Raw<'_> {
        self.as_fd()
    }
}

#[cfg(windows)]
impl<T: std::os::windows::io::AsSocket> AsRaw for T {
    fn as_raw_source(&self) -> Raw<'_> {
        self.as_socket()
    }
}

#[cfg(windows)]
mod windows {
    use std::{
        io,
        sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError, sync_channel},
        time::Duration,
    };

    use nanocode_sys::AtomicBox;

    use super::Raw;

    pub const WAKE_TOKEN: u64 = u64::MAX;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Event {
        pub token: u64,
        pub readable: bool,
        pub writable: bool,
        pub read_closed: bool,
        pub write_closed: bool,
        pub error: bool,
    }

    pub struct Events {
        woken: Option<Event>,
    }

    impl Events {
        #[must_use]
        pub const fn with_capacity(_capacity: usize) -> Self {
            Self { woken: None }
        }

        pub fn iter(&self) -> impl Iterator<Item = Event> + '_ {
            self.woken.iter().copied()
        }
    }

    pub struct Poller {
        sender: SyncSender<()>,
        receiver: AtomicBox<Receiver<()>>,
    }

    impl Poller {
        pub fn new() -> io::Result<Self> {
            let (sender, receiver) = sync_channel(1);
            Ok(Self {
                sender,
                receiver: AtomicBox::new(Some(Box::new(receiver))),
            })
        }

        pub const fn register(&self, _source: Raw<'_>, _token: u64) -> io::Result<()> {
            Ok(())
        }

        pub const fn deregister(&self, _source: Raw<'_>) -> io::Result<()> {
            Ok(())
        }

        pub fn wait(&self, events: &mut Events, timeout: Option<Duration>) -> io::Result<()> {
            events.woken = None;
            let Some(receiver) = self.receiver.take() else {
                return Err(io::Error::other("the poller is waited on by one driver"));
            };
            let woken = match timeout {
                Some(timeout) => match receiver.recv_timeout(timeout) {
                    Ok(()) => true,
                    Err(RecvTimeoutError::Timeout) => false,
                    Err(RecvTimeoutError::Disconnected) => {
                        return Err(io::Error::other("the poller's wake channel closed"));
                    }
                },
                None => receiver.recv().is_ok(),
            };
            while receiver.try_recv().is_ok() {}
            self.receiver.store(Some(receiver));
            if woken {
                events.woken = Some(Event {
                    token: WAKE_TOKEN,
                    readable: true,
                    writable: false,
                    read_closed: false,
                    write_closed: false,
                    error: false,
                });
            }
            Ok(())
        }

        pub fn wake(&self) -> io::Result<()> {
            match self.sender.try_send(()) {
                Ok(()) | Err(TrySendError::Full(())) => Ok(()),
                Err(TrySendError::Disconnected(())) => {
                    Err(io::Error::other("the poller's wake channel closed"))
                }
            }
        }
    }
}

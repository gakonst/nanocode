use std::io;

#[cfg(unix)]
pub mod unix {
    use std::{
        future::poll_fn,
        io,
        os::fd::BorrowedFd,
        sync::{
            Arc, OnceLock, Weak,
            atomic::{AtomicU64, Ordering},
        },
        task::{Context, Poll, Wake, Waker},
    };

    use nanocode_sys::{AtomicWaker, unix::signal as sys};

    use crate::{net::Source, queue::Shared, reactor::Direction};

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct SignalKind(i32);

    impl SignalKind {
        #[must_use]
        pub const fn from_raw(signal: i32) -> Self {
            Self(signal)
        }

        #[must_use]
        pub const fn as_raw_value(&self) -> i32 {
            self.0
        }

        #[must_use]
        pub const fn alarm() -> Self {
            Self(sys::SIGALRM)
        }

        #[must_use]
        pub const fn child() -> Self {
            Self(sys::SIGCHLD)
        }

        #[must_use]
        pub const fn hangup() -> Self {
            Self(sys::SIGHUP)
        }

        #[must_use]
        pub const fn interrupt() -> Self {
            Self(sys::SIGINT)
        }

        #[must_use]
        pub const fn pipe() -> Self {
            Self(sys::SIGPIPE)
        }

        #[must_use]
        pub const fn quit() -> Self {
            Self(sys::SIGQUIT)
        }

        #[must_use]
        pub const fn terminate() -> Self {
            Self(sys::SIGTERM)
        }

        #[must_use]
        pub const fn user_defined1() -> Self {
            Self(sys::SIGUSR1)
        }

        #[must_use]
        pub const fn user_defined2() -> Self {
            Self(sys::SIGUSR2)
        }

        #[must_use]
        pub const fn window_change() -> Self {
            Self(sys::SIGWINCH)
        }
    }

    impl From<i32> for SignalKind {
        fn from(signal: i32) -> Self {
            Self(signal)
        }
    }

    struct Listener {
        seen: AtomicU64,
        waker: AtomicWaker,
    }

    struct Registry {
        source: Source<BorrowedFd<'static>>,
        generations: Box<[AtomicU64]>,
        listeners: Box<[Shared<Weak<Listener>>]>,
        pump: Arc<Pump>,
    }

    struct Pump;

    impl Wake for Pump {
        fn wake(self: Arc<Self>) {
            if let Ok(registry) = registry() {
                registry.pump();
            }
        }

        fn wake_by_ref(self: &Arc<Self>) {
            if let Ok(registry) = registry() {
                registry.pump();
            }
        }
    }

    static REGISTRY: OnceLock<Result<Registry, (io::ErrorKind, String)>> = OnceLock::new();

    fn registry() -> io::Result<&'static Registry> {
        let registry = REGISTRY
            .get_or_init(|| {
                let start = || -> io::Result<Registry> {
                    let count = sys::signal_count();
                    Ok(Registry {
                        source: Source::new(sys::wakeups()?)?,
                        generations: (0..count).map(|_| AtomicU64::new(0)).collect(),
                        listeners: (0..count).map(|_| Shared::new()).collect(),
                        pump: Arc::new(Pump),
                    })
                };
                start().map_err(|error| (error.kind(), error.to_string()))
            })
            .as_ref()
            .map_err(|(kind, message)| io::Error::new(*kind, message.clone()))?;
        Ok(registry)
    }

    impl Registry {
        fn pump(&self) {
            let waker = Waker::from(self.pump.clone());
            let mut context = Context::from_waker(&waker);
            while let Poll::Ready(ready) = self.source.poll_ready(&mut context, Direction::Read) {
                let mut drained = [0_u8; 64];
                while let Ok(read) = nanocode_sys::unix::read(self.source.get_ref(), &mut drained) {
                    if read == 0 {
                        break;
                    }
                }
                self.source.clear(ready);
                self.deliver();
            }
            self.deliver();
        }

        fn deliver(&self) {
            for (index, generation) in self.generations.iter().enumerate() {
                let Ok(signal) = i32::try_from(index) else {
                    continue;
                };
                if !sys::take_pending(signal) {
                    continue;
                }
                generation.fetch_add(1, Ordering::SeqCst);
                let listeners = &self.listeners[index];
                for listener in listeners.drain() {
                    if let Some(live) = listener.upgrade() {
                        live.waker.wake_by_ref();
                        listeners.push(listener);
                    }
                }
            }
        }
    }

    pub struct Signal {
        index: usize,
        listener: Arc<Listener>,
        registry: &'static Registry,
    }

    impl std::fmt::Debug for Signal {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter
                .debug_struct("Signal")
                .field("signal", &self.index)
                .finish()
        }
    }

    pub fn signal(kind: SignalKind) -> io::Result<Signal> {
        let index = usize::try_from(kind.0)
            .ok()
            .filter(|index| *index > 0 && *index < sys::signal_count())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid signal number"))?;
        let registry = registry()?;
        sys::listen(kind.0)?;
        let listener = Arc::new(Listener {
            seen: AtomicU64::new(registry.generations[index].load(Ordering::SeqCst)),
            waker: AtomicWaker::new(),
        });
        registry.listeners[index].push(Arc::downgrade(&listener));
        registry.pump();
        Ok(Signal {
            index,
            listener,
            registry,
        })
    }

    impl Signal {
        pub fn poll_recv(&mut self, context: &mut Context<'_>) -> Poll<Option<()>> {
            let generation = &self.registry.generations[self.index];
            let observed = |listener: &Listener| {
                let current = generation.load(Ordering::SeqCst);
                let seen = listener.seen.load(Ordering::SeqCst);
                (current > seen).then(|| listener.seen.store(current, Ordering::SeqCst))
            };
            if observed(&self.listener).is_some() {
                return Poll::Ready(Some(()));
            }
            self.listener.waker.register(context.waker());
            self.registry.pump();
            if observed(&self.listener).is_some() {
                return Poll::Ready(Some(()));
            }
            Poll::Pending
        }

        pub async fn recv(&mut self) -> Option<()> {
            poll_fn(|context| self.poll_recv(context)).await
        }
    }
}

#[cfg(unix)]
pub async fn ctrl_c() -> io::Result<()> {
    let mut interrupt = unix::signal(unix::SignalKind::interrupt())?;
    interrupt.recv().await;
    Ok(())
}

#[cfg(not(unix))]
pub async fn ctrl_c() -> io::Result<()> {
    std::future::pending::<()>().await;
    Ok(())
}

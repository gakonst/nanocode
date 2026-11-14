use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    task::{Context, Poll},
    thread::Thread,
    time::{Duration, Instant},
};

use nanocode_sys::{AtomicBox, AtomicWaker, SwapOption};

use crate::{
    queue::Shared,
    runtime::Handle,
    task::{Abortable, Current, Id, JoinError, JoinHandle, Joinable},
};

const BLOCKING_THREAD: &str = "nanocode-blocking";
pub(crate) const MAX_BLOCKING_THREADS: usize = 512;
pub(crate) const KEEP_ALIVE: Duration = Duration::from_secs(10);

type Job = Box<dyn FnOnce() + Send + 'static>;

struct Idle {
    thread: Thread,
    claimed: AtomicBool,
}

pub(crate) struct Pool {
    jobs: Shared<Job>,
    idle: Shared<Arc<Idle>>,
    threads: AtomicUsize,
    max: usize,
    keep_alive: Duration,
    shutdown: AtomicBool,
    waiter: SwapOption<Thread>,
    name: String,
}

impl Pool {
    pub(crate) fn new(max: usize, keep_alive: Duration, name: Option<String>) -> Self {
        Self {
            jobs: Shared::new(),
            idle: Shared::new(),
            threads: AtomicUsize::new(0),
            max,
            keep_alive,
            shutdown: AtomicBool::new(false),
            waiter: SwapOption::new(None),
            name: name.unwrap_or_else(|| BLOCKING_THREAD.to_owned()),
        }
    }

    pub(crate) fn spawn(self: &Arc<Self>, handle: Option<&Handle>, job: Job) {
        if self.shutdown.load(Ordering::SeqCst) {
            drop(job);
            return;
        }
        self.jobs.push(job);
        while let Some(idle) = self.idle.pop() {
            if idle
                .claimed
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                idle.thread.unpark();
                return;
            }
        }
        if self.threads.fetch_add(1, Ordering::SeqCst) >= self.max {
            self.threads.fetch_sub(1, Ordering::SeqCst);
            return;
        }
        let pool = self.clone();
        let handle = handle.cloned();
        let spawned = std::thread::Builder::new()
            .name(self.name.clone())
            .spawn(move || pool.work(handle.as_ref()));
        if spawned.is_err() {
            self.threads.fetch_sub(1, Ordering::SeqCst);
        }
    }

    fn work(&self, handle: Option<&Handle>) {
        let _context = handle.map(Handle::enter);
        loop {
            while let Some(job) = self.jobs.pop() {
                job();
            }
            if self.shutdown.load(Ordering::SeqCst) {
                break;
            }
            let idle = Arc::new(Idle {
                thread: std::thread::current(),
                claimed: AtomicBool::new(false),
            });
            self.idle.push(idle.clone());
            if (self.jobs.len() > 0 || self.shutdown.load(Ordering::SeqCst))
                && idle
                    .claimed
                    .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
            {
                continue;
            }
            let deadline = Instant::now() + self.keep_alive;
            loop {
                if idle.claimed.load(Ordering::Acquire) || self.shutdown.load(Ordering::SeqCst) {
                    break;
                }
                let now = Instant::now();
                if now >= deadline {
                    break;
                }
                std::thread::park_timeout(deadline - now);
            }
            if idle
                .claimed
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
                && self.jobs.len() == 0
            {
                break;
            }
        }
        if self.threads.fetch_sub(1, Ordering::SeqCst) == 1
            && let Some(waiter) = self.waiter.load_full()
        {
            waiter.unpark();
        }
    }

    pub(crate) fn shutdown(&self, deadline: Option<Instant>) {
        self.shutdown.store(true, Ordering::SeqCst);
        self.waiter.store(Some(Arc::new(std::thread::current())));
        for idle in self.idle.drain() {
            idle.claimed.store(true, Ordering::Release);
            idle.thread.unpark();
        }
        loop {
            if self.threads.load(Ordering::SeqCst) == 0 {
                break;
            }
            match deadline {
                None => std::thread::park(),
                Some(deadline) => {
                    let now = Instant::now();
                    if now >= deadline {
                        break;
                    }
                    std::thread::park_timeout(deadline - now);
                }
            }
        }
        drop(self.jobs.drain());
    }
}

pub(crate) struct Blocking<T> {
    id: Id,
    output: AtomicBox<Result<T, JoinError>>,
    finished: AtomicBool,
    aborted: AtomicBool,
    joiner: AtomicWaker,
}

impl<T: Send + 'static> Blocking<T> {
    fn complete(&self, output: Result<T, JoinError>) {
        self.output.store(Some(Box::new(output)));
        self.finished.store(true, Ordering::Release);
        self.joiner.wake();
    }
}

impl<T: Send + 'static> Joinable<T> for Blocking<T> {
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

impl<T: Send + 'static> Abortable for Blocking<T> {
    fn abort_task(self: Arc<Self>) {
        self.aborted.store(true, Ordering::Release);
    }

    fn finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }

    fn task_id(&self) -> Id {
        self.id
    }
}

struct Run<T: Send + 'static, F> {
    task: Arc<Blocking<T>>,
    work: Option<F>,
}

impl<T: Send + 'static, F> Drop for Run<T, F> {
    fn drop(&mut self) {
        if !self.task.finished.load(Ordering::Acquire) {
            self.task.complete(Err(JoinError::cancelled(self.task.id)));
        }
    }
}

static GLOBAL: OnceLock<Arc<Pool>> = OnceLock::new();

pub(crate) fn spawn_anywhere<F, T>(work: F) -> JoinHandle<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    match Handle::try_current() {
        Ok(handle) => handle.spawn_blocking(work),
        Err(_) => spawn(
            None,
            GLOBAL.get_or_init(|| Arc::new(Pool::new(MAX_BLOCKING_THREADS, KEEP_ALIVE, None))),
            work,
        ),
    }
}

pub(crate) fn spawn<F, T>(handle: Option<&Handle>, pool: &Arc<Pool>, work: F) -> JoinHandle<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let task = Arc::new(Blocking {
        id: Id::next(),
        output: AtomicBox::empty(),
        finished: AtomicBool::new(false),
        aborted: AtomicBool::new(false),
        joiner: AtomicWaker::new(),
    });
    let mut run = Run {
        task: task.clone(),
        work: Some(work),
    };
    pool.spawn(
        handle,
        Box::new(move || {
            let Some(work) = run.work.take() else {
                return;
            };
            if run.task.aborted.load(Ordering::Acquire) {
                return;
            }
            let _current = Current::enter(Some(run.task.id));
            let output = catch_unwind(AssertUnwindSafe(work))
                .map_err(|payload| JoinError::panic(run.task.id, payload));
            run.task.complete(output);
        }),
    );
    JoinHandle::new(task.clone(), task)
}

use futures_util::future::BoxFuture;
use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
    time::Duration,
};
use tokio::{sync::mpsc, task::JoinHandle};

const IDLE: Duration = Duration::from_secs(5);
const KEPT: usize = 1024;

type Commit<T> = Arc<dyn Fn(String, Vec<T>) -> BoxFuture<'static, ()> + Send + Sync>;

#[derive(Clone, Copy, Debug)]
pub struct NotCommitted;

impl std::fmt::Display for NotCommitted {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("not committed")
    }
}

impl std::error::Error for NotCommitted {}

pub struct Lanes<T> {
    queue: OnceLock<Sender<(String, T)>>,
    batch: usize,
    bound: Option<(usize, fn())>,
}

impl<T: Send + 'static> Lanes<T> {
    #[must_use]
    pub const fn new(batch: usize) -> Self {
        Self {
            queue: OnceLock::new(),
            batch,
            bound: None,
        }
    }

    #[must_use]
    pub const fn bounded(batch: usize, queued: usize, refused: fn()) -> Self {
        Self {
            queue: OnceLock::new(),
            batch,
            bound: Some((queued, refused)),
        }
    }

    pub fn send(
        &self,
        key: String,
        item: T,
        commit: impl Fn(String, Vec<T>) -> BoxFuture<'static, ()> + Send + Sync + 'static,
    ) -> Result<(), T> {
        let queue = self.queue.get_or_init(|| {
            let (queue, items) = channel(self.bound.map(|(queued, _)| queued));
            let refused = self.bound.map(|(_, refused)| refused);
            tokio::spawn(route(items, Arc::new(commit), self.batch, refused));
            queue
        });
        queue.send((key, item)).map_err(|refused| {
            if let Some((_, count)) = self.bound {
                count();
            }
            refused.into_inner().1
        })
    }
}

pub async fn together<'a, T: Sync, R, E, F>(
    batch: &'a [T],
    commit: impl Fn(&'a [T]) -> F + Send,
) -> Vec<Result<R, E>>
where
    F: Future<Output = Result<Vec<R>, E>> + Send,
    E: From<NotCommitted>,
{
    match commit(batch).await {
        Ok(results) => results.into_iter().map(Ok).collect(),
        Err(error) if batch.len() == 1 => vec![Err(error)],
        Err(_) => {
            let mut results = Vec::with_capacity(batch.len());
            for item in batch {
                let alone = commit(std::slice::from_ref(item)).await;
                results
                    .push(alone.and_then(|mut one| one.pop().ok_or_else(|| NotCommitted.into())));
            }
            results
        }
    }
}

enum Sender<T> {
    Bounded(mpsc::Sender<T>),
    Unbounded(mpsc::UnboundedSender<T>),
}

enum Receiver<T> {
    Bounded(mpsc::Receiver<T>),
    Unbounded(mpsc::UnboundedReceiver<T>),
}

enum Refused<T> {
    Full(T),
    Closed(T),
}

impl<T> Refused<T> {
    fn into_inner(self) -> T {
        match self {
            Self::Full(item) | Self::Closed(item) => item,
        }
    }
}

fn channel<T>(bound: Option<usize>) -> (Sender<T>, Receiver<T>) {
    match bound {
        Some(bound) => {
            let (sender, receiver) = mpsc::channel(bound);
            (Sender::Bounded(sender), Receiver::Bounded(receiver))
        }
        None => {
            let (sender, receiver) = mpsc::unbounded_channel();
            (Sender::Unbounded(sender), Receiver::Unbounded(receiver))
        }
    }
}

impl<T> Sender<T> {
    fn send(&self, item: T) -> Result<(), Refused<T>> {
        match self {
            Self::Bounded(sender) => sender.try_send(item).map_err(|error| match error {
                mpsc::error::TrySendError::Full(item) => Refused::Full(item),
                mpsc::error::TrySendError::Closed(item) => Refused::Closed(item),
            }),
            Self::Unbounded(sender) => sender.send(item).map_err(|error| Refused::Closed(error.0)),
        }
    }
}

impl<T> Receiver<T> {
    async fn recv(&mut self) -> Option<T> {
        match self {
            Self::Bounded(receiver) => receiver.recv().await,
            Self::Unbounded(receiver) => receiver.recv().await,
        }
    }

    async fn recv_many(&mut self, taken: &mut Vec<T>, limit: usize) -> usize {
        match self {
            Self::Bounded(receiver) => receiver.recv_many(taken, limit).await,
            Self::Unbounded(receiver) => receiver.recv_many(taken, limit).await,
        }
    }

    fn close(&mut self) {
        match self {
            Self::Bounded(receiver) => receiver.close(),
            Self::Unbounded(receiver) => receiver.close(),
        }
    }
}

struct Lane<T> {
    queue: Sender<T>,
    worker: JoinHandle<()>,
}

async fn route<T: Send + 'static>(
    mut items: Receiver<(String, T)>,
    commit: Commit<T>,
    batch: usize,
    refused: Option<fn()>,
) {
    let mut lanes: HashMap<String, Lane<T>> = HashMap::new();
    while let Some((key, item)) = items.recv().await {
        let item = match lanes.get(&key) {
            Some(lane) => match lane.queue.send(item) {
                Ok(()) => continue,
                Err(Refused::Full(item)) => {
                    if let Some(count) = refused {
                        count();
                    }
                    drop(item);
                    continue;
                }
                Err(Refused::Closed(item)) => item,
            },
            None => item,
        };
        if lanes.len() > KEPT {
            lanes.retain(|_, lane| !lane.worker.is_finished());
        }
        let previous = lanes.remove(&key).map(|lane| lane.worker);
        let (queue, received) = channel(refused.map(|_| batch));
        drop(queue.send(item));
        let worker = tokio::spawn(work(
            key.clone(),
            received,
            Arc::clone(&commit),
            batch,
            previous,
        ));
        lanes.insert(key, Lane { queue, worker });
    }
}

async fn work<T: Send + 'static>(
    key: String,
    mut items: Receiver<T>,
    commit: Commit<T>,
    batch: usize,
    previous: Option<JoinHandle<()>>,
) {
    if let Some(previous) = previous {
        drop(previous.await);
    }
    let mut taken = Vec::with_capacity(batch);
    loop {
        match tokio::time::timeout(IDLE, items.recv_many(&mut taken, batch)).await {
            Ok(0) => return,
            Ok(_) => commit(key.clone(), std::mem::take(&mut taken)).await,
            Err(_) => {
                items.close();
                while items.recv_many(&mut taken, batch).await > 0 {
                    commit(key.clone(), std::mem::take(&mut taken)).await;
                }
                return;
            }
        }
    }
}

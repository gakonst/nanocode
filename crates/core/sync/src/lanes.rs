use futures_util::future::BoxFuture;
use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
    time::Duration,
};
use tokio::{
    sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel},
    task::JoinHandle,
};

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
    queue: OnceLock<UnboundedSender<(String, T)>>,
    batch: usize,
}

impl<T: Send + 'static> Lanes<T> {
    #[must_use]
    pub const fn new(batch: usize) -> Self {
        Self {
            queue: OnceLock::new(),
            batch,
        }
    }

    pub fn send(
        &self,
        key: String,
        item: T,
        commit: impl Fn(String, Vec<T>) -> BoxFuture<'static, ()> + Send + Sync + 'static,
    ) -> Result<(), T> {
        let queue = self.queue.get_or_init(|| {
            let (queue, items) = unbounded_channel();
            tokio::spawn(route(items, Arc::new(commit), self.batch));
            queue
        });
        queue.send((key, item)).map_err(|refused| refused.0.1)
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

struct Lane<T> {
    queue: UnboundedSender<T>,
    worker: JoinHandle<()>,
}

async fn route<T: Send + 'static>(
    mut items: UnboundedReceiver<(String, T)>,
    commit: Commit<T>,
    batch: usize,
) {
    let mut lanes: HashMap<String, Lane<T>> = HashMap::new();
    while let Some((key, item)) = items.recv().await {
        let item = match lanes.get(&key) {
            Some(lane) => match lane.queue.send(item) {
                Ok(()) => continue,
                Err(closed) => closed.0,
            },
            None => item,
        };
        if lanes.len() > KEPT {
            lanes.retain(|_, lane| !lane.worker.is_finished());
        }
        let previous = lanes.remove(&key).map(|lane| lane.worker);
        let (queue, received) = unbounded_channel();
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
    mut items: UnboundedReceiver<T>,
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

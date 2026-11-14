use crate::future::BoxFuture;
use nanocode_platform::timeout;
use std::{
    collections::HashMap,
    future::Future,
    mem, slice,
    sync::{Arc, OnceLock},
    time::Duration,
};
use tokio::{
    sync::mpsc::{self, error::TrySendError},
    task::JoinHandle,
};

#[derive(Clone, Copy, Debug)]
pub struct Bounds {
    pub queued: usize,
    pub batch: usize,
    pub idle: Duration,
    pub lanes: usize,
}

type Commit<T> = Arc<dyn Fn(String, Vec<T>) -> BoxFuture<'static, ()> + Send + Sync>;

pub struct Lanes<T> {
    queue: OnceLock<mpsc::Sender<(String, T)>>,
    bounds: Bounds,
    refused: fn(T),
}

struct Lane<T> {
    queue: mpsc::Sender<T>,
    worker: JoinHandle<()>,
}

impl<T: Send + 'static> Lanes<T> {
    #[must_use]
    pub const fn bounded(bounds: Bounds, refused: fn(T)) -> Self {
        Self {
            queue: OnceLock::new(),
            bounds,
            refused,
        }
    }

    pub fn send(
        &self,
        key: String,
        item: T,
        commit: impl Fn(String, Vec<T>) -> BoxFuture<'static, ()> + Send + Sync + 'static,
    ) -> Result<(), T> {
        let queue = self.queue.get_or_init(|| {
            let (queue, items) = mpsc::channel(self.bounds.queued);
            nanocode_platform::spawn_task(route(
                items,
                Arc::new(commit),
                self.bounds,
                self.refused,
            ));
            queue
        });
        queue
            .try_send((key, item))
            .map_err(|error| error.into_inner().1)
    }
}

pub async fn together<'a, T: Sync, R, E, F>(
    batch: &'a [T],
    commit: impl Fn(&'a [T]) -> F + Send,
) -> Vec<Result<Option<R>, E>>
where
    F: Future<Output = Result<Vec<R>, E>> + Send,
{
    match commit(batch).await {
        Ok(results) => results.into_iter().map(|result| Ok(Some(result))).collect(),
        Err(error) if batch.len() == 1 => vec![Err(error)],
        Err(_) => {
            let mut results = Vec::with_capacity(batch.len());
            for item in batch {
                let alone = commit(slice::from_ref(item)).await;
                results.push(alone.map(|mut one| one.pop()));
            }
            results
        }
    }
}

async fn route<T: Send + 'static>(
    mut items: mpsc::Receiver<(String, T)>,
    commit: Commit<T>,
    bounds: Bounds,
    refused: fn(T),
) {
    let mut lanes: HashMap<String, Lane<T>> = HashMap::new();
    while let Some((key, item)) = items.recv().await {
        let item = match lanes.get(&key) {
            Some(lane) => match lane.queue.try_send(item) {
                Ok(()) => continue,
                Err(TrySendError::Full(item)) => {
                    refused(item);
                    continue;
                }
                Err(TrySendError::Closed(item)) => item,
            },
            None => item,
        };
        if lanes.len() > bounds.lanes {
            lanes.retain(|_, lane| !lane.worker.is_finished());
        }
        let previous = lanes.remove(&key).map(|lane| lane.worker);
        let (queue, received) = mpsc::channel(bounds.batch);
        drop(queue.try_send(item));
        let worker = nanocode_platform::spawn_task(work(
            key.clone(),
            received,
            Arc::clone(&commit),
            bounds,
            previous,
        ));
        lanes.insert(key, Lane { queue, worker });
    }
}

async fn work<T: Send + 'static>(
    key: String,
    mut items: mpsc::Receiver<T>,
    commit: Commit<T>,
    bounds: Bounds,
    previous: Option<JoinHandle<()>>,
) {
    if let Some(previous) = previous {
        drop(previous.await);
    }
    let mut taken = Vec::with_capacity(bounds.batch);
    loop {
        match timeout(bounds.idle, items.recv_many(&mut taken, bounds.batch)).await {
            Ok(0) => return,
            Ok(_) => commit(key.clone(), mem::take(&mut taken)).await,
            Err(_) => {
                items.close();
                while items.recv_many(&mut taken, bounds.batch).await > 0 {
                    commit(key.clone(), mem::take(&mut taken)).await;
                }
                return;
            }
        }
    }
}

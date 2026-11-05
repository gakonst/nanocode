use futures_util::future::BoxFuture;
use nanocode_database::{Hub, Observe, Outbox, Wake};
use sqlx::{
    Connection as _, PgConnection,
    postgres::{PgConnectOptions, PgListener, PgPoolOptions},
    query_scalar,
};
use std::{
    mem,
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
    time::Duration,
};
use tokio::{spawn, time::sleep};

#[derive(Clone)]
pub struct Forwarding {
    pub wake_all: &'static str,
    pub forwarder: Arc<AtomicU32>,
    pub batch: usize,
    pub retry: Duration,
    pub observe: Arc<dyn Observe>,
}

pub async fn listen(
    options: &PgConnectOptions,
    hub: Hub,
    forwarding: &Forwarding,
) -> Result<(), sqlx::Error> {
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .idle_timeout(None)
        .max_lifetime(None)
        .connect_with(options.clone())
        .await?;
    let mut listener = PgListener::connect_with(&pool).await?;
    listener.listen_all(hub.names()).await?;
    let Forwarding {
        wake_all,
        forwarder,
        retry,
        observe,
        ..
    } = forwarding.clone();
    spawn(async move {
        loop {
            let (channel, id): (Option<String>, Arc<str>) = match listener.try_recv().await {
                Ok(Some(note)) if note.process_id() == forwarder.load(Ordering::Relaxed) => {
                    continue;
                }
                Ok(Some(note)) if note.channel() != wake_all => {
                    (Some(note.channel().to_owned()), note.payload().into())
                }
                Ok(_) => (None, "".into()),
                Err(error) => {
                    observe.listen_failed(&error);
                    sleep(retry).await;
                    (None, "".into())
                }
            };
            hub.wake(channel.as_deref(), &id);
        }
    });
    Ok(())
}

pub async fn forward<S>(
    options: PgConnectOptions,
    mut outbox: Outbox,
    forwarding: Forwarding,
    send: S,
) where
    S: for<'c> Fn(&'c mut PgConnection, &'c [Wake]) -> BoxFuture<'c, Result<(), sqlx::Error>>,
{
    let Forwarding {
        wake_all,
        forwarder,
        batch: limit,
        retry,
        observe,
    } = forwarding;
    let mut conn: Option<PgConnection> = None;
    let mut batch = Vec::new();
    let mut connected_before = false;
    loop {
        let received = if batch.is_empty() {
            outbox.recv_many(&mut batch, limit).await
        } else {
            batch.len()
        };
        if received == 0 {
            return;
        }
        if outbox.overflowed() {
            batch.push(Wake::new(wake_all, "", false));
        }
        if conn.is_none() {
            match PgConnection::connect_with(&options).await {
                Ok(mut connected) => {
                    if let Ok(pid) = query_scalar::<_, i32>("SELECT pg_backend_pid()")
                        .fetch_one(&mut connected)
                        .await
                        .inspect_err(|error| observe.forward_unavailable(error))
                    {
                        forwarder.store(pid.cast_unsigned(), Ordering::Relaxed);
                        conn = Some(connected);
                        if mem::replace(&mut connected_before, true) {
                            batch.push(Wake::new(wake_all, "", false));
                        }
                    }
                }
                Err(error) => observe.forward_unavailable(&error),
            }
        }
        let Some(connected) = conn.as_mut() else {
            sleep(retry).await;
            continue;
        };
        batch.sort_unstable();
        batch.dedup_by(|later, first| {
            later.channel == first.channel && later.payload == first.payload
        });
        match send(connected, &batch).await {
            Ok(()) => batch.clear(),
            Err(error) => {
                observe.forward_failed(&error);
                conn = None;
                sleep(retry).await;
            }
        }
    }
}

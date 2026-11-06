use sqlx::{AssertSqlSafe, Error, PgConnection, Row as _, SqlStr, raw_sql};
use std::{
    collections::HashMap,
    fmt::Display,
    future::Future,
    ops::DerefMut,
    time::{Duration, Instant},
};
use tokio::{spawn, sync::mpsc};

pub struct Slow {
    pub name: &'static str,
    pub sql: SqlStr,
    pub took: Duration,
}

pub struct Ran {
    pub took: Duration,
    pub ok: bool,
    pub over: bool,
}

pub async fn timed<T>(
    budget: Duration,
    work: impl Future<Output = Result<T, Error>>,
) -> (Result<T, Error>, Ran) {
    let started = Instant::now();
    let result = work.await;
    let took = started.elapsed();
    let ran = Ran {
        took,
        ok: result.is_ok(),
        over: took > budget,
    };
    (result, ran)
}

pub async fn generic_plan(conn: &mut PgConnection, sql: &str) -> Result<String, Error> {
    let rows = raw_sql(AssertSqlSafe(format!("EXPLAIN (GENERIC_PLAN) {sql}")))
        .fetch_all(conn)
        .await?;
    let lines: Vec<String> = rows.iter().filter_map(|row| row.try_get(0).ok()).collect();
    Ok(lines.join("\n"))
}

pub struct SlowPlans(mpsc::Sender<Slow>);

impl SlowPlans {
    pub fn start<C, E, F, Connect>(
        pending: usize,
        every: Duration,
        connect: Connect,
        explained: impl Fn(&Slow, Result<String, E>) + Send + 'static,
    ) -> Self
    where
        C: DerefMut<Target = PgConnection> + Send,
        E: From<Error> + Display + Send,
        F: Future<Output = Result<C, E>> + Send,
        Connect: Fn() -> F + Send + Sync + 'static,
    {
        let (slow, mut received) = mpsc::channel(pending);
        spawn(async move {
            let mut explained_at: HashMap<&'static str, Instant> = HashMap::new();
            while let Some(slow) = received.recv().await {
                let slow: Slow = slow;
                explained_at.retain(|_, at| at.elapsed() < every);
                if explained_at.contains_key(slow.name) {
                    continue;
                }
                explained_at.insert(slow.name, Instant::now());
                let plan = async {
                    let mut conn = connect().await?;
                    Ok(generic_plan(&mut conn, slow.sql.as_str()).await?)
                }
                .await;
                explained(&slow, plan);
            }
        });
        Self(slow)
    }

    pub fn note(&self, slow: Slow) -> bool {
        self.0.try_send(slow).is_ok()
    }
}

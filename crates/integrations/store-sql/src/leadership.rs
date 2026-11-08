use sqlx::{Connection as _, Error, PgConnection, postgres::PgConnectOptions, query_scalar};
use std::time::Duration;
use tokio::time::sleep;

pub struct Leadership(PgConnection);

impl Leadership {
    pub async fn acquire(
        options: &PgConnectOptions,
        key: i64,
        every: Duration,
        failed: impl Fn(&Error),
    ) -> Self {
        loop {
            let attempt = async {
                let mut conn = PgConnection::connect_with(options).await?;
                let locked: bool = query_scalar(
                    "SELECT CASE WHEN pg_is_in_recovery() THEN false ELSE pg_try_advisory_lock($1) END",
                )
                .bind(key)
                .fetch_one(&mut conn)
                .await?;
                Ok::<_, Error>(locked.then_some(conn))
            };
            match attempt.await {
                Ok(Some(conn)) => return Self(conn),
                Ok(None) => {}
                Err(error) => failed(&error),
            }
            sleep(every).await;
        }
    }

    pub async fn lost(&mut self, every: Duration) {
        while self.0.ping().await.is_ok() {
            sleep(every).await;
        }
    }
}

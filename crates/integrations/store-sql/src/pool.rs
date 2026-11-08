use sqlx::{Error, PgPool};
use std::time::Duration;
use tokio::time::timeout;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Warmed {
    Full,
    Failed,
    TimedOut,
}

pub async fn warm(pool: &PgPool, within: Duration, failed: impl FnOnce(&Error)) -> Warmed {
    let held = timeout(
        within,
        futures_util::future::try_join_all(
            (0..pool.options().get_max_connections()).map(|_| pool.acquire()),
        ),
    )
    .await;
    match held {
        Ok(Ok(_)) => Warmed::Full,
        Ok(Err(error)) => {
            failed(&error);
            Warmed::Failed
        }
        Err(_) => Warmed::TimedOut,
    }
}

#[must_use]
pub fn unavailable(error: &Error) -> bool {
    match error {
        Error::Io(_) | Error::Tls(_) | Error::Protocol(_) => true,
        Error::Database(error) => error.code().is_some_and(|code| {
            code.starts_with("08") || matches!(&*code, "57P01" | "57P02" | "57P03" | "25006")
        }),
        _ => false,
    }
}

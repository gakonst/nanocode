use sqlx::{
    AssertSqlSafe, Connection as _, Error, PgConnection, postgres::PgConnectOptions, query,
    query_as, query_scalar, raw_sql,
};
use std::{collections::BTreeSet, time::Duration};
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

pub async fn locked<T, E: From<Error>>(
    conn: &mut PgConnection,
    key: i64,
    work: impl AsyncFnOnce(&mut PgConnection) -> Result<T, E>,
) -> Result<T, E> {
    query("SELECT pg_advisory_lock($1)")
        .bind(key)
        .execute(&mut *conn)
        .await?;
    let done = work(&mut *conn).await;
    let unlocked = query("SELECT pg_advisory_unlock($1)")
        .bind(key)
        .execute(&mut *conn)
        .await;
    let done = done?;
    unlocked?;
    Ok(done)
}

pub async fn try_lock(conn: &mut PgConnection, key: i64) -> Result<bool, Error> {
    query_scalar("SELECT pg_try_advisory_lock($1)")
        .bind(key)
        .fetch_one(conn)
        .await
}

pub async fn index(
    conn: &mut PgConnection,
    name: &str,
    schemas: &[&str],
) -> Result<Option<(String, bool)>, Error> {
    if schemas.is_empty() {
        return query_as(
            "SELECT indexrelid::regclass::text, indisvalid FROM pg_index WHERE indexrelid=to_regclass($1)",
        )
        .bind(name)
        .fetch_optional(conn)
        .await;
    }
    query_as(
        "SELECT n.nspname::text || '.' || c.relname::text, i.indisvalid FROM pg_index i
            JOIN pg_class c ON c.oid=i.indexrelid JOIN pg_namespace n ON n.oid=c.relnamespace
            WHERE c.relname=$1 AND n.nspname=ANY($2)",
    )
    .bind(name)
    .bind(schemas)
    .fetch_optional(conn)
    .await
}

pub async fn ensure_index(
    conn: &mut PgConnection,
    name: &str,
    schemas: &[&str],
    create: &str,
) -> Result<bool, Error> {
    match index(&mut *conn, name, schemas).await? {
        Some((_, true)) => return Ok(false),
        Some((qualified, false)) => {
            raw_sql(AssertSqlSafe(format!(
                "DROP INDEX CONCURRENTLY IF EXISTS {qualified}"
            )))
            .execute(&mut *conn)
            .await?;
        }
        None => {}
    }
    raw_sql(AssertSqlSafe(create.to_owned()))
        .execute(conn)
        .await?;
    Ok(true)
}

pub async fn drop_replaced(conn: &mut PgConnection, old: &str, new: &str) -> Result<bool, Error> {
    if !matches!(index(&mut *conn, new, &[]).await?, Some((_, true)))
        || index(&mut *conn, old, &[]).await?.is_none()
    {
        return Ok(false);
    }
    raw_sql(AssertSqlSafe(format!(
        "DROP INDEX CONCURRENTLY IF EXISTS {old}"
    )))
    .execute(conn)
    .await?;
    Ok(true)
}

#[must_use]
pub fn after(sql: &str) -> Option<i64> {
    sql.lines()
        .next()
        .and_then(|line| line.strip_prefix("-- after:"))
        .and_then(|after| after.trim().split('_').next()?.parse().ok())
}

pub async fn applied(conn: &mut PgConnection, table: &str) -> Result<BTreeSet<i64>, Error> {
    let exists: bool = query_scalar("SELECT to_regclass($1) IS NOT NULL")
        .bind(table)
        .fetch_one(&mut *conn)
        .await?;
    if !exists {
        return Ok(BTreeSet::new());
    }
    Ok(
        query_scalar(AssertSqlSafe(format!("SELECT version FROM {table}")))
            .fetch_all(conn)
            .await?
            .into_iter()
            .collect(),
    )
}

pub fn allowed<'a>(
    contractions: impl IntoIterator<Item = (i64, &'a str)>,
    applied: &BTreeSet<i64>,
    oldest: i64,
) -> Result<Vec<i64>, i64> {
    let mut allowed = Vec::new();
    for (version, sql) in contractions {
        if applied.contains(&version) {
            continue;
        }
        if after(sql).ok_or(version)? > oldest {
            break;
        }
        allowed.push(version);
    }
    Ok(allowed)
}

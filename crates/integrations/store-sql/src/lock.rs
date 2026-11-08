use nanocode_durability_core::LockKey;
use sqlx::{PgConnection, query_scalar};

#[must_use]
pub fn begin(setup: &[&str], write_lock: i64, keys: &[LockKey]) -> String {
    let locks = keys.iter().map(|&LockKey { class, id, shared }| {
        let kind = if shared { "_shared" } else { "" };
        format!("pg_advisory_xact_lock{kind}({class},{id})")
    });
    let targets: Vec<String> = setup
        .iter()
        .map(|target| (*target).to_owned())
        .chain([format!("pg_advisory_xact_lock_shared({write_lock})")])
        .chain(locks)
        .collect();
    format!("BEGIN;SELECT {};", targets.join(","))
}

pub async fn held(
    connection: &mut PgConnection,
    key: LockKey,
    write_lock: i64,
) -> Result<bool, sqlx::Error> {
    let LockKey { class, id, shared } = key;
    query_scalar(
        "SELECT EXISTS(SELECT 1 FROM pg_locks WHERE pid=pg_backend_pid()
                AND locktype='advisory' AND granted
                AND (mode='ExclusiveLock' OR (mode='ShareLock' AND $3))
                AND classid::bigint=$1 AND objid::bigint=$2 AND objsubid=2)
             OR EXISTS(SELECT 1 FROM pg_locks WHERE pid=pg_backend_pid()
                AND locktype='advisory' AND granted AND mode='ExclusiveLock'
                AND classid::bigint=$4 AND objid::bigint=$5 AND objsubid=1)",
    )
    .bind(i64::from(u32::from_ne_bytes(class.to_ne_bytes())))
    .bind(i64::from(u32::from_ne_bytes(id.to_ne_bytes())))
    .bind(shared)
    .bind(write_lock >> 32)
    .bind(write_lock & 0xffff_ffff)
    .fetch_one(connection)
    .await
}

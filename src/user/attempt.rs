//! In-flight OAuth sign-ins: the hashed `state` parameter and the path the sign-in returns
//! to, held until the provider redirects back with it.

use jiff::Timestamp;
use sqlx::Row;

use crate::database::{Database, DatabaseError};

pub async fn create(
    database: &Database,
    state_hash: &str,
    return_path: &str,
    expires_at: Timestamp,
) -> Result<(), DatabaseError> {
    sqlx::query(
        "INSERT INTO auth_attempts (state_hash, return_path, expires_at_millis) VALUES (?, ?, ?)",
    )
    .bind(state_hash)
    .bind(return_path)
    .bind(expires_at.as_millisecond())
    .execute(&database.pool)
    .await?;
    Ok(())
}

/// The return path of an unexpired attempt. Consuming deletes the attempt either way, so a
/// state is good for one callback.
pub async fn consume(
    database: &Database,
    state_hash: &str,
    now: Timestamp,
) -> Result<Option<String>, DatabaseError> {
    let mut transaction = database.write().await?;
    let attempt: Option<(String, i64)> = sqlx::query(
        "SELECT return_path, expires_at_millis FROM auth_attempts WHERE state_hash = ?",
    )
    .bind(state_hash)
    .fetch_optional(&mut *transaction)
    .await?
    .map(|row| {
        Ok::<_, sqlx::Error>((
            row.try_get("return_path")?,
            row.try_get("expires_at_millis")?,
        ))
    })
    .transpose()?;
    sqlx::query("DELETE FROM auth_attempts WHERE state_hash = ?")
        .bind(state_hash)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(attempt
        .filter(|(_, expires_at_millis)| *expires_at_millis > now.as_millisecond())
        .map(|(return_path, _)| return_path))
}

/// Expired attempts are rejected by every authentication predicate, so deleting them is
/// housekeeping. It runs on the schedule instead of the request path, where it made each
/// read wait for SQLite's single writer.
pub async fn delete_expired(database: &Database, now: Timestamp) -> Result<u64, DatabaseError> {
    let result = sqlx::query("DELETE FROM auth_attempts WHERE expires_at_millis <= ?")
        .bind(now.as_millisecond())
        .execute(&database.pool)
        .await?;
    Ok(result.rows_affected())
}

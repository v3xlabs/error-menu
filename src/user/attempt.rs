//! In-flight OAuth sign-ins: the hashed `state` parameter, held until the provider
//! redirects back with it.

use jiff::Timestamp;
use sqlx::Row;

use crate::database::{Database, DatabaseError};

pub async fn create(
    database: &Database,
    state_hash: &str,
    expires_at: Timestamp,
) -> Result<(), DatabaseError> {
    sqlx::query("INSERT INTO auth_attempts (state_hash, expires_at_millis) VALUES (?, ?)")
        .bind(state_hash)
        .bind(expires_at.as_millisecond())
        .execute(&database.pool)
        .await?;
    Ok(())
}

pub async fn consume(
    database: &Database,
    state_hash: &str,
    now: Timestamp,
) -> Result<bool, DatabaseError> {
    let mut transaction = database.pool.begin().await?;
    let expires_at_millis: Option<i64> =
        sqlx::query("SELECT expires_at_millis FROM auth_attempts WHERE state_hash = ?")
            .bind(state_hash)
            .fetch_optional(&mut *transaction)
            .await?
            .map(|row| row.try_get("expires_at_millis"))
            .transpose()?;
    sqlx::query("DELETE FROM auth_attempts WHERE state_hash = ?")
        .bind(state_hash)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(expires_at_millis.is_some_and(|expires_at_millis| expires_at_millis > now.as_millisecond()))
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

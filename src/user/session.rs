//! Browser sessions: a hashed cookie value that names a user until it expires.

use jiff::Timestamp;

use crate::database::codec::DecodeRow;
use crate::database::{Database, DatabaseError};
use crate::id::Id;
use crate::user::User;

pub async fn create(
    database: &Database,
    user_id: Id<User>,
    token_hash: &str,
    expires_at: Timestamp,
) -> Result<(), DatabaseError> {
    sqlx::query("INSERT INTO sessions (token_hash, user_id, expires_at_millis) VALUES (?, ?, ?)")
        .bind(token_hash)
        .bind(user_id.raw())
        .bind(expires_at.as_millisecond())
        .execute(&database.pool)
        .await?;
    Ok(())
}

pub async fn user_for(
    database: &Database,
    token_hash: &str,
    now: Timestamp,
) -> Result<Option<User>, DatabaseError> {
    let row = sqlx::query(
        "SELECT u.id, u.oidc_issuer, u.oidc_subject, u.display_name, u.access_role AS role, u.created_at, u.last_signed_in_at \
         FROM sessions s JOIN users u ON u.id = s.user_id \
         WHERE s.token_hash = ? AND s.expires_at_millis > ?",
    )
    .bind(token_hash)
    .bind(now.as_millisecond())
    .fetch_optional(&database.pool)
    .await?;
    row.as_ref().map(User::decode_row).transpose()
}

pub async fn delete(database: &Database, token_hash: &str) -> Result<bool, DatabaseError> {
    let result = sqlx::query("DELETE FROM sessions WHERE token_hash = ?")
        .bind(token_hash)
        .execute(&database.pool)
        .await?;
    Ok(result.rows_affected() != 0)
}

/// Expired sessions are rejected by every authentication predicate, so deleting them is
/// housekeeping. It runs on the schedule instead of the request path, where it made each
/// read wait for SQLite's single writer.
pub async fn delete_expired(database: &Database, now: Timestamp) -> Result<u64, DatabaseError> {
    let result = sqlx::query("DELETE FROM sessions WHERE expires_at_millis <= ?")
        .bind(now.as_millisecond())
        .execute(&database.pool)
        .await?;
    Ok(result.rows_affected())
}

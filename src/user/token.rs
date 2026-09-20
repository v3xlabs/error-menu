//! API tokens: a named, optionally expiring credential a user creates for scripts.

use jiff::Timestamp;
use sqlx::Row;
use sqlx::sqlite::SqliteRow;

use crate::database::codec::DecodeRow;
use crate::database::{Database, DatabaseError};
use crate::id::Id;
use crate::user::User;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiToken {
    pub name: String,
    pub created_at: Timestamp,
    pub expires_at: Option<Timestamp>,
}

impl ApiToken {
    pub async fn create(
        database: &Database,
        user_id: Id<User>,
        token_hash: &str,
        name: &str,
        expires_at: Option<Timestamp>,
    ) -> Result<ApiToken, DatabaseError> {
        let created_at = Timestamp::now();
        sqlx::query(
            "INSERT INTO api_tokens (token_hash, user_id, name, created_at_millis, expires_at_millis) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(token_hash)
        .bind(user_id.raw())
        .bind(name)
        .bind(created_at.as_millisecond())
        .bind(expires_at.map(|expires_at| expires_at.as_millisecond()))
        .execute(&database.pool)
        .await?;
        Ok(ApiToken {
            name: name.to_owned(),
            created_at,
            expires_at,
        })
    }

    pub async fn list(
        database: &Database,
        user_id: Id<User>,
        now: Timestamp,
    ) -> Result<Vec<ApiToken>, DatabaseError> {
        let rows = sqlx::query(
            "SELECT name, created_at_millis, expires_at_millis FROM api_tokens \
             WHERE user_id = ? AND (expires_at_millis IS NULL OR expires_at_millis > ?) \
             ORDER BY created_at_millis DESC",
        )
        .bind(user_id.raw())
        .bind(now.as_millisecond())
        .fetch_all(&database.pool)
        .await?;
        rows.iter().map(decode_api_token).collect()
    }

    pub async fn user_for(
        database: &Database,
        token_hash: &str,
        now: Timestamp,
    ) -> Result<Option<User>, DatabaseError> {
        let row = sqlx::query(
            "SELECT u.id, u.oidc_issuer, u.oidc_subject, u.display_name, u.access_role AS role, u.created_at, u.last_signed_in_at \
             FROM api_tokens t JOIN users u ON u.id = t.user_id \
             WHERE t.token_hash = ? AND (t.expires_at_millis IS NULL OR t.expires_at_millis > ?)",
        )
        .bind(token_hash)
        .bind(now.as_millisecond())
        .fetch_optional(&database.pool)
        .await?;
        row.as_ref().map(User::decode_row).transpose()
    }

    pub async fn revoke(
        database: &Database,
        user_id: Id<User>,
        name: &str,
    ) -> Result<bool, DatabaseError> {
        let result = sqlx::query("DELETE FROM api_tokens WHERE user_id = ? AND name = ?")
            .bind(user_id.raw())
            .bind(name)
            .execute(&database.pool)
            .await?;
        Ok(result.rows_affected() != 0)
    }

    /// Expired tokens are rejected by every authentication predicate, so deleting them is
    /// housekeeping. It runs on the schedule instead of the request path, where it made
    /// each read wait for SQLite's single writer.
    pub async fn delete_expired(database: &Database, now: Timestamp) -> Result<u64, DatabaseError> {
        let result = sqlx::query(
            "DELETE FROM api_tokens WHERE expires_at_millis IS NOT NULL AND expires_at_millis <= ?",
        )
        .bind(now.as_millisecond())
        .execute(&database.pool)
        .await?;
        Ok(result.rows_affected())
    }
}

fn decode_api_token(row: &SqliteRow) -> Result<ApiToken, DatabaseError> {
    let created_at_millis: i64 = row.try_get("created_at_millis")?;
    let expires_at_millis: Option<i64> = row.try_get("expires_at_millis")?;
    Ok(ApiToken {
        name: row.try_get("name")?,
        created_at: Timestamp::from_millisecond(created_at_millis).map_err(|_| {
            DatabaseError::Unreadable {
                field: "api_token.created_at_millis",
                value: created_at_millis.to_string(),
            }
        })?,
        expires_at: expires_at_millis
            .map(Timestamp::from_millisecond)
            .transpose()
            .map_err(|_| DatabaseError::Unreadable {
                field: "api_token.expires_at_millis",
                value: expires_at_millis.unwrap_or_default().to_string(),
            })?,
    })
}

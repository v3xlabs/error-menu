use std::str::FromStr;

use jiff::Timestamp;
use sqlx::Row;

use crate::id::Id;
use crate::user::{User, UserRole};

use super::{Store, StoreError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserRoleChange {
    Updated,
    Missing,
    FinalAdmin,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiToken {
    pub name: String,
    pub created_at: Timestamp,
    pub expires_at: Option<Timestamp>,
}

impl Store {
    pub async fn register_user(
        &self,
        oidc_issuer: &str,
        oidc_subject: &str,
        display_name: &str,
        allow_registration: bool,
    ) -> Result<Option<User>, StoreError> {
        let now = Timestamp::now();
        let mut transaction = self.pool.begin().await?;
        if let Some(row) = sqlx::query(
            "SELECT id, oidc_issuer, oidc_subject, display_name, access_role AS role, created_at, last_signed_in_at \
             FROM users WHERE oidc_issuer = ? AND oidc_subject = ?",
        )
        .bind(oidc_issuer)
        .bind(oidc_subject)
        .fetch_optional(&mut *transaction)
        .await?
        {
            let mut user = decode_user(row)?;
            sqlx::query("UPDATE users SET display_name = ?, last_signed_in_at = ? WHERE id = ?")
                .bind(display_name)
                .bind(now.to_string())
                .bind(user.id.raw())
                .execute(&mut *transaction)
                .await?;
            user.display_name = display_name.to_owned();
            user.last_signed_in_at = now;
            transaction.commit().await?;
            return Ok(Some(user));
        }

        if !allow_registration {
            transaction.rollback().await?;
            return Ok(None);
        }

        let user_count: i64 = sqlx::query("SELECT COUNT(*) AS count FROM users")
            .fetch_one(&mut *transaction)
            .await?
            .try_get("count")?;
        let role = if user_count == 0 {
            UserRole::Admin
        } else {
            UserRole::Guest
        };
        let id: Id<User> = self.ids.next();
        sqlx::query(
            "INSERT INTO users (id, oidc_issuer, oidc_subject, display_name, role, access_role, created_at, last_signed_in_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id.raw())
        .bind(oidc_issuer)
        .bind(oidc_subject)
        .bind(display_name)
        .bind(role.legacy_storage_role())
        .bind(role.as_str())
        .bind(now.to_string())
        .bind(now.to_string())
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;

        Ok(Some(User {
            id,
            oidc_issuer: oidc_issuer.to_owned(),
            oidc_subject: oidc_subject.to_owned(),
            display_name: display_name.to_owned(),
            role,
            created_at: now,
            last_signed_in_at: now,
        }))
    }

    pub async fn user(&self, user_id: Id<User>) -> Result<Option<User>, StoreError> {
        let row = sqlx::query(
            "SELECT id, oidc_issuer, oidc_subject, display_name, access_role AS role, created_at, last_signed_in_at \
             FROM users WHERE id = ?",
        )
        .bind(user_id.raw())
        .fetch_optional(&self.pool)
        .await?;
        row.map(decode_user).transpose()
    }

    pub async fn list_users(&self) -> Result<Vec<User>, StoreError> {
        let rows = sqlx::query(
            "SELECT id, oidc_issuer, oidc_subject, display_name, access_role AS role, created_at, last_signed_in_at \
             FROM users ORDER BY id ASC",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(decode_user).collect()
    }

    pub async fn set_user_role(
        &self,
        user_id: Id<User>,
        role: UserRole,
    ) -> Result<UserRoleChange, StoreError> {
        let mut transaction = self.pool.begin().await?;
        let current_role: Option<String> =
            sqlx::query("SELECT access_role FROM users WHERE id = ?")
                .bind(user_id.raw())
                .fetch_optional(&mut *transaction)
                .await?
                .map(|row| row.try_get("access_role"))
                .transpose()?;
        let Some(current_role) = current_role else {
            transaction.rollback().await?;
            return Ok(UserRoleChange::Missing);
        };
        let current_role = UserRole::from_str(&current_role).ok_or(StoreError::Unreadable {
            field: "user.access_role",
            value: current_role,
        })?;
        if current_role == UserRole::Admin && role != UserRole::Admin {
            let admins: i64 =
                sqlx::query("SELECT COUNT(*) AS count FROM users WHERE access_role = 'admin'")
                    .fetch_one(&mut *transaction)
                    .await?
                    .try_get("count")?;
            if admins == 1 {
                transaction.rollback().await?;
                return Ok(UserRoleChange::FinalAdmin);
            }
        }
        sqlx::query("UPDATE users SET role = ?, access_role = ? WHERE id = ?")
            .bind(role.legacy_storage_role())
            .bind(role.as_str())
            .bind(user_id.raw())
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
        Ok(UserRoleChange::Updated)
    }

    async fn cleanup_expired_auth_records(&self, now_millis: i64) -> Result<(), StoreError> {
        sqlx::query("DELETE FROM auth_attempts WHERE expires_at_millis <= ?")
            .bind(now_millis)
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM sessions WHERE expires_at_millis <= ?")
            .bind(now_millis)
            .execute(&self.pool)
            .await?;
        sqlx::query(
            "DELETE FROM api_tokens WHERE expires_at_millis IS NOT NULL AND expires_at_millis <= ?",
        )
        .bind(now_millis)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn create_auth_attempt(
        &self,
        state_hash: &str,
        expires_at: Timestamp,
    ) -> Result<(), StoreError> {
        self.cleanup_expired_auth_records(Timestamp::now().as_millisecond())
            .await?;
        sqlx::query("INSERT INTO auth_attempts (state_hash, expires_at_millis) VALUES (?, ?)")
            .bind(state_hash)
            .bind(expires_at.as_millisecond())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn consume_auth_attempt(
        &self,
        state_hash: &str,
        now: Timestamp,
    ) -> Result<bool, StoreError> {
        let mut transaction = self.pool.begin().await?;
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
        Ok(expires_at_millis
            .is_some_and(|expires_at_millis| expires_at_millis > now.as_millisecond()))
    }

    pub async fn create_session(
        &self,
        user_id: Id<User>,
        token_hash: &str,
        expires_at: Timestamp,
    ) -> Result<(), StoreError> {
        self.cleanup_expired_auth_records(Timestamp::now().as_millisecond())
            .await?;
        sqlx::query(
            "INSERT INTO sessions (token_hash, user_id, expires_at_millis) VALUES (?, ?, ?)",
        )
        .bind(token_hash)
        .bind(user_id.raw())
        .bind(expires_at.as_millisecond())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn user_for_session(
        &self,
        token_hash: &str,
        now: Timestamp,
    ) -> Result<Option<User>, StoreError> {
        self.cleanup_expired_auth_records(now.as_millisecond())
            .await?;
        let row = sqlx::query(
            "SELECT u.id, u.oidc_issuer, u.oidc_subject, u.display_name, u.access_role AS role, u.created_at, u.last_signed_in_at \
             FROM sessions s JOIN users u ON u.id = s.user_id \
             WHERE s.token_hash = ? AND s.expires_at_millis > ?",
        )
        .bind(token_hash)
        .bind(now.as_millisecond())
        .fetch_optional(&self.pool)
        .await?;
        row.map(decode_user).transpose()
    }

    pub async fn delete_session(&self, token_hash: &str) -> Result<bool, StoreError> {
        let result = sqlx::query("DELETE FROM sessions WHERE token_hash = ?")
            .bind(token_hash)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() != 0)
    }

    pub async fn create_api_token(
        &self,
        user_id: Id<User>,
        token_hash: &str,
        name: &str,
        expires_at: Option<Timestamp>,
    ) -> Result<ApiToken, StoreError> {
        let created_at = Timestamp::now();
        self.cleanup_expired_auth_records(created_at.as_millisecond())
            .await?;
        sqlx::query(
            "INSERT INTO api_tokens (token_hash, user_id, name, created_at_millis, expires_at_millis) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(token_hash)
        .bind(user_id.raw())
        .bind(name)
        .bind(created_at.as_millisecond())
        .bind(expires_at.map(|expires_at| expires_at.as_millisecond()))
        .execute(&self.pool)
        .await?;
        Ok(ApiToken {
            name: name.to_owned(),
            created_at,
            expires_at,
        })
    }

    pub async fn list_api_tokens(
        &self,
        user_id: Id<User>,
        now: Timestamp,
    ) -> Result<Vec<ApiToken>, StoreError> {
        self.cleanup_expired_auth_records(now.as_millisecond())
            .await?;
        let rows = sqlx::query(
            "SELECT name, created_at_millis, expires_at_millis FROM api_tokens \
             WHERE user_id = ? ORDER BY created_at_millis DESC",
        )
        .bind(user_id.raw())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(decode_api_token).collect()
    }

    pub async fn user_for_api_token(
        &self,
        token_hash: &str,
        now: Timestamp,
    ) -> Result<Option<User>, StoreError> {
        self.cleanup_expired_auth_records(now.as_millisecond())
            .await?;
        let row = sqlx::query(
            "SELECT u.id, u.oidc_issuer, u.oidc_subject, u.display_name, u.access_role AS role, u.created_at, u.last_signed_in_at \
             FROM api_tokens t JOIN users u ON u.id = t.user_id \
             WHERE t.token_hash = ? AND (t.expires_at_millis IS NULL OR t.expires_at_millis > ?)",
        )
        .bind(token_hash)
        .bind(now.as_millisecond())
        .fetch_optional(&self.pool)
        .await?;
        row.map(decode_user).transpose()
    }

    pub async fn revoke_api_token(
        &self,
        user_id: Id<User>,
        name: &str,
    ) -> Result<bool, StoreError> {
        let result = sqlx::query("DELETE FROM api_tokens WHERE user_id = ? AND name = ?")
            .bind(user_id.raw())
            .bind(name)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() != 0)
    }
}

fn decode_user(row: sqlx::sqlite::SqliteRow) -> Result<User, StoreError> {
    let role: String = row.try_get("role")?;
    let role = UserRole::from_str(&role).ok_or(StoreError::Unreadable {
        field: "user.role",
        value: role,
    })?;

    Ok(User {
        id: Id::from_raw(row.try_get("id")?),
        oidc_issuer: row.try_get("oidc_issuer")?,
        oidc_subject: row.try_get("oidc_subject")?,
        display_name: row.try_get("display_name")?,
        role,
        created_at: decode_timestamp(row.try_get("created_at")?)?,
        last_signed_in_at: decode_timestamp(row.try_get("last_signed_in_at")?)?,
    })
}

fn decode_api_token(row: sqlx::sqlite::SqliteRow) -> Result<ApiToken, StoreError> {
    let created_at_millis: i64 = row.try_get("created_at_millis")?;
    let expires_at_millis: Option<i64> = row.try_get("expires_at_millis")?;
    Ok(ApiToken {
        name: row.try_get("name")?,
        created_at: Timestamp::from_millisecond(created_at_millis).map_err(|_| {
            StoreError::Unreadable {
                field: "api_token.created_at_millis",
                value: created_at_millis.to_string(),
            }
        })?,
        expires_at: expires_at_millis
            .map(Timestamp::from_millisecond)
            .transpose()
            .map_err(|_| StoreError::Unreadable {
                field: "api_token.expires_at_millis",
                value: expires_at_millis.unwrap_or_default().to_string(),
            })?,
    })
}

fn decode_timestamp(timestamp: String) -> Result<Timestamp, StoreError> {
    timestamp.parse().map_err(|_| StoreError::Unreadable {
        field: "timestamp",
        value: timestamp,
    })
}

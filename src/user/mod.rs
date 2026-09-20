pub mod attempt;
pub mod session;
pub mod token;

use jiff::Timestamp;
use sqlx::Row;
use sqlx::sqlite::SqliteRow;

use crate::database::codec::{DecodeRow, FromStored, StoredAs};
use crate::database::{Database, DatabaseError};
use crate::id::Id;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserRole {
    Guest,
    Admin,
    Member,
}

impl UserRole {
    /// The instance role column was added beside an older one that only knows members and
    /// administrators, and both are still written so a rollback keeps working.
    pub const fn legacy_storage_role(self) -> &'static str {
        match self {
            Self::Guest => "member",
            Self::Admin => "admin",
            Self::Member => "member",
        }
    }
}

impl StoredAs for UserRole {
    fn stored(&self) -> &'static str {
        match self {
            Self::Guest => "guest",
            Self::Admin => "admin",
            Self::Member => "member",
        }
    }
}

impl FromStored for UserRole {
    const FIELD: &'static str = "user.role";

    fn parse_stored(value: &str) -> Option<Self> {
        match value {
            "guest" => Some(Self::Guest),
            "admin" => Some(Self::Admin),
            "member" => Some(Self::Member),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserRoleChange {
    Updated,
    Missing,
    FinalAdmin,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    pub id: Id<User>,
    pub oidc_issuer: String,
    pub oidc_subject: String,
    pub display_name: String,
    pub role: UserRole,
    pub created_at: Timestamp,
    pub last_signed_in_at: Timestamp,
}

impl User {
    pub async fn register(
        database: &Database,
        oidc_issuer: &str,
        oidc_subject: &str,
        display_name: &str,
        allow_registration: bool,
    ) -> Result<Option<User>, DatabaseError> {
        let now = Timestamp::now();
        let mut transaction = database.write().await?;
        if let Some(row) = sqlx::query(
            "SELECT id, oidc_issuer, oidc_subject, display_name, access_role AS role, created_at, last_signed_in_at \
             FROM users WHERE oidc_issuer = ? AND oidc_subject = ?",
        )
        .bind(oidc_issuer)
        .bind(oidc_subject)
        .fetch_optional(&mut *transaction)
        .await?
        {
            let mut user = User::decode_row(&row)?;
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
        let id: Id<User> = database.ids.next();
        sqlx::query(
            "INSERT INTO users (id, oidc_issuer, oidc_subject, display_name, role, access_role, created_at, last_signed_in_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id.raw())
        .bind(oidc_issuer)
        .bind(oidc_subject)
        .bind(display_name)
        .bind(role.legacy_storage_role())
        .bind(role.stored())
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

    pub async fn load(
        database: &Database,
        user_id: Id<User>,
    ) -> Result<Option<User>, DatabaseError> {
        let row = sqlx::query(
            "SELECT id, oidc_issuer, oidc_subject, display_name, access_role AS role, created_at, last_signed_in_at \
             FROM users WHERE id = ?",
        )
        .bind(user_id.raw())
        .fetch_optional(&database.pool)
        .await?;
        row.as_ref().map(User::decode_row).transpose()
    }

    pub async fn list(database: &Database) -> Result<Vec<User>, DatabaseError> {
        let rows = sqlx::query(
            "SELECT id, oidc_issuer, oidc_subject, display_name, access_role AS role, created_at, last_signed_in_at \
             FROM users ORDER BY id ASC",
        )
        .fetch_all(&database.pool)
        .await?;
        rows.iter().map(User::decode_row).collect()
    }

    pub async fn set_role(
        database: &Database,
        user_id: Id<User>,
        role: UserRole,
    ) -> Result<UserRoleChange, DatabaseError> {
        let mut transaction = database.write().await?;
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
        let current_role = UserRole::from_stored(&current_role)?;
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
            .bind(role.stored())
            .bind(user_id.raw())
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
        Ok(UserRoleChange::Updated)
    }
}

impl DecodeRow for User {
    fn decode_row(row: &SqliteRow) -> Result<Self, DatabaseError> {
        let role = UserRole::read(row, "role")?;

        Ok(User {
            id: Id::from_raw(row.try_get("id")?),
            oidc_issuer: row.try_get("oidc_issuer")?,
            oidc_subject: row.try_get("oidc_subject")?,
            display_name: row.try_get("display_name")?,
            role,
            created_at: Timestamp::read(row, "created_at")?,
            last_signed_in_at: Timestamp::read(row, "last_signed_in_at")?,
        })
    }
}

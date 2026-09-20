//! How a value is written into a column and read back out. Every enum the database keeps
//! as text names its own spelling next to the type itself, so the two directions sit
//! together and a column that stops being readable says which field it was.

use jiff::Timestamp;
use sqlx::Row;
use sqlx::sqlite::SqliteRow;

use super::DatabaseError;

/// The text a value is stored as. A stored spelling is not the API spelling and not the
/// display name: changing one of those must not silently rewrite the database.
pub trait StoredAs {
    fn stored(&self) -> &'static str;
}

pub trait FromStored: Sized {
    /// Names the column in the error when the text does not parse.
    const FIELD: &'static str;

    fn parse_stored(value: &str) -> Option<Self>;

    fn from_stored(value: &str) -> Result<Self, DatabaseError> {
        Self::parse_stored(value).ok_or_else(|| DatabaseError::Unreadable {
            field: Self::FIELD,
            value: value.to_owned(),
        })
    }

    fn read(row: &SqliteRow, column: &str) -> Result<Self, DatabaseError> {
        Self::from_stored(&row.try_get::<String, _>(column)?)
    }
}

/// A whole row read as one value.
pub trait DecodeRow: Sized {
    fn decode_row(row: &SqliteRow) -> Result<Self, DatabaseError>;
}

impl FromStored for Timestamp {
    const FIELD: &'static str = "timestamp";

    fn parse_stored(value: &str) -> Option<Self> {
        value.parse().ok()
    }
}

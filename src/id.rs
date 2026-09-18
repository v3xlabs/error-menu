use std::cmp::Ordering as CmpOrdering;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// 2025-01-01T00:00:00Z. Ids stay inside 41 bits of milliseconds until 2094.
const EPOCH_MS: u64 = 1_735_689_600_000;

const SEQUENCE_BITS: u32 = 12;
const NODE_BITS: u32 = 10;
const SEQUENCE_MASK: u64 = (1 << SEQUENCE_BITS) - 1;
const NODE_MASK: u64 = (1 << NODE_BITS) - 1;

const ALPHABET: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
/// 62^11 exceeds i64::MAX, and a fixed width keeps string order equal to numeric order.
const ENCODED_LEN: usize = 11;

/// A snowflake, typed by the entity it identifies. The database column is the integer;
/// the encoded form exists only at the API and URL boundary.
pub struct Id<T> {
    value: i64,
    marker: PhantomData<fn() -> T>,
}

impl<T> Id<T> {
    pub fn from_raw(value: i64) -> Self {
        Self {
            value,
            marker: PhantomData,
        }
    }

    pub fn raw(self) -> i64 {
        self.value
    }

    pub fn encode(self) -> String {
        let mut value = self.value as u64;
        let mut buffer = [b'0'; ENCODED_LEN];
        let mut index = ENCODED_LEN;
        while index > 0 {
            index -= 1;
            buffer[index] = ALPHABET[(value % 62) as usize];
            value /= 62;
        }
        buffer.iter().map(|byte| *byte as char).collect()
    }

    pub fn milliseconds(self) -> u64 {
        (self.value as u64 >> (SEQUENCE_BITS + NODE_BITS)) + EPOCH_MS
    }
}

impl<T> Clone for Id<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Id<T> {}

impl<T> PartialEq for Id<T> {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}

impl<T> Eq for Id<T> {}

impl<T> PartialOrd for Id<T> {
    fn partial_cmp(&self, other: &Self) -> Option<CmpOrdering> {
        Some(self.cmp(other))
    }
}

impl<T> Ord for Id<T> {
    fn cmp(&self, other: &Self) -> CmpOrdering {
        self.value.cmp(&other.value)
    }
}

impl<T> Hash for Id<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.value.hash(state);
    }
}

impl<T> fmt::Display for Id<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.encode())
    }
}

impl<T> fmt::Debug for Id<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Id({})", self.encode())
    }
}

impl<T> serde::Serialize for Id<T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.encode())
    }
}

impl<'de, T> serde::Deserialize<'de> for Id<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let encoded = <String as serde::Deserialize>::deserialize(deserializer)?;
        encoded.parse().map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DecodeError {
    #[error("id must be {ENCODED_LEN} characters, got {0}")]
    Length(usize),
    #[error("character {0:?} is not in the base62 alphabet")]
    Character(char),
    #[error("value does not fit in a snowflake")]
    Overflow,
}

impl<T> FromStr for Id<T> {
    type Err = DecodeError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if text.len() != ENCODED_LEN {
            return Err(DecodeError::Length(text.len()));
        }

        let mut value: u64 = 0;
        for character in text.chars() {
            let digit = ALPHABET
                .iter()
                .position(|candidate| *candidate as char == character)
                .ok_or(DecodeError::Character(character))?;
            value = value
                .checked_mul(62)
                .and_then(|shifted| shifted.checked_add(digit as u64))
                .ok_or(DecodeError::Overflow)?;
        }

        if value > i64::MAX as u64 {
            return Err(DecodeError::Overflow);
        }

        Ok(Self::from_raw(value as i64))
    }
}

/// Mints ids for one node. The app mints every id; workers post results without them.
pub struct IdGenerator {
    node: u64,
    state: AtomicU64,
}

impl IdGenerator {
    pub fn new(node: u16) -> Self {
        Self {
            node: u64::from(node) & NODE_MASK,
            state: AtomicU64::new(0),
        }
    }

    pub fn next<T>(&self) -> Id<T> {
        loop {
            let now = milliseconds_since_epoch();
            let previous = self.state.load(Ordering::Relaxed);
            let previous_milliseconds = previous >> SEQUENCE_BITS;
            let previous_sequence = previous & SEQUENCE_MASK;

            // A clock that moves backwards keeps the previous millisecond, so ids stay ordered.
            let (milliseconds, sequence) = if now > previous_milliseconds {
                (now, 0)
            } else {
                (previous_milliseconds, previous_sequence + 1)
            };

            if sequence > SEQUENCE_MASK {
                std::hint::spin_loop();
                continue;
            }

            let next = (milliseconds << SEQUENCE_BITS) | sequence;
            if self
                .state
                .compare_exchange_weak(previous, next, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                let raw = (milliseconds << (SEQUENCE_BITS + NODE_BITS))
                    | (self.node << SEQUENCE_BITS)
                    | sequence;
                return Id::from_raw(raw as i64);
            }
        }
    }
}

fn milliseconds_since_epoch() -> u64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(EPOCH_MS);
    now.saturating_sub(EPOCH_MS)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Thing;

    #[test]
    fn encodes_to_fixed_width() {
        assert_eq!(Id::<Thing>::from_raw(0).encode(), "00000000000");
        assert_eq!(Id::<Thing>::from_raw(61).encode(), "0000000000z");
        assert_eq!(Id::<Thing>::from_raw(62).encode(), "00000000010");
    }

    #[test]
    fn round_trips() {
        for raw in [0, 1, 61, 62, 12_345_678, i64::MAX] {
            let id = Id::<Thing>::from_raw(raw);
            let parsed: Id<Thing> = id.encode().parse().expect("decodes");
            assert_eq!(parsed, id);
        }
    }

    #[test]
    fn string_order_matches_numeric_order() {
        let generator = IdGenerator::new(7);
        let mut previous = generator.next::<Thing>();
        for _ in 0..1000 {
            let current = generator.next::<Thing>();
            assert!(current > previous);
            assert!(current.encode() > previous.encode());
            previous = current;
        }
    }

    #[test]
    fn rejects_malformed_input() {
        assert_eq!("short".parse::<Id<Thing>>(), Err(DecodeError::Length(5)));
        assert_eq!(
            "0000000000!".parse::<Id<Thing>>(),
            Err(DecodeError::Character('!'))
        );
        assert_eq!(
            "zzzzzzzzzzz".parse::<Id<Thing>>(),
            Err(DecodeError::Overflow)
        );
    }

    #[test]
    fn carries_its_timestamp() {
        let generator = IdGenerator::new(0);
        let id = generator.next::<Thing>();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("after unix epoch")
            .as_millis() as u64;
        assert!(id.milliseconds().abs_diff(now) < 1000);
    }
}

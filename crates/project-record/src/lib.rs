//! Shared path-safe project-record identifiers.
//!
//! Record IDs are fixed-width Crockford base32 encodings of Unix epoch
//! milliseconds. The fixed width keeps lexicographic order aligned with
//! chronological order.

use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

pub const RECORD_ID_WIDTH: usize = 13;
pub const RECORD_ID_ALPHABET: &str = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
pub const MAX_COLLISION_PROBES: u64 = 1000;

const ALPHABET_BYTES: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordIdError {
    InvalidLength { value: String },
    InvalidCharacter { value: String, ch: char },
    TimestampOverflow,
    TimeBeforeUnixEpoch,
    ExcessiveCollisions { base_millis: u64, attempts: u64 },
}

impl fmt::Display for RecordIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLength { value } => write!(
                f,
                "invalid record id length for {value:?}: expected {RECORD_ID_WIDTH}"
            ),
            Self::InvalidCharacter { value, ch } => {
                write!(f, "invalid record id character {ch:?} in {value:?}")
            }
            Self::TimestampOverflow => f.write_str("record id timestamp overflow"),
            Self::TimeBeforeUnixEpoch => f.write_str("system time is before Unix epoch"),
            Self::ExcessiveCollisions {
                base_millis,
                attempts,
            } => write!(
                f,
                "too many record id collisions for timestamp {base_millis} after {attempts} attempts"
            ),
        }
    }
}

impl std::error::Error for RecordIdError {}

pub fn unix_epoch_millis_now() -> Result<u64, RecordIdError> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| RecordIdError::TimeBeforeUnixEpoch)?;
    u64::try_from(duration.as_millis()).map_err(|_| RecordIdError::TimestampOverflow)
}

pub fn encode_unix_epoch_millis(millis: u64) -> String {
    let mut value = millis;
    let mut out = [b'0'; RECORD_ID_WIDTH];
    for slot in out.iter_mut().rev() {
        *slot = ALPHABET_BYTES[(value & 0b11111) as usize];
        value >>= 5;
    }
    String::from_utf8(out.to_vec()).expect("record id alphabet is ASCII")
}

pub fn decode_unix_epoch_millis(id: &str) -> Result<u64, RecordIdError> {
    if id.len() != RECORD_ID_WIDTH {
        return Err(RecordIdError::InvalidLength {
            value: id.to_string(),
        });
    }
    let mut value = 0_u64;
    for ch in id.chars() {
        let digit = decode_digit(ch, id)? as u64;
        value = value
            .checked_mul(32)
            .and_then(|value| value.checked_add(digit))
            .ok_or(RecordIdError::TimestampOverflow)?;
    }
    Ok(value)
}

pub fn validate_record_id(id: &str) -> Result<(), RecordIdError> {
    decode_unix_epoch_millis(id).map(|_| ())
}

pub fn allocate_record_id<F>(base_millis: u64, mut exists: F) -> Result<String, RecordIdError>
where
    F: FnMut(&str) -> bool,
{
    for offset in 0..MAX_COLLISION_PROBES {
        let millis = base_millis
            .checked_add(offset)
            .ok_or(RecordIdError::TimestampOverflow)?;
        let id = encode_unix_epoch_millis(millis);
        if !exists(&id) {
            return Ok(id);
        }
    }
    Err(RecordIdError::ExcessiveCollisions {
        base_millis,
        attempts: MAX_COLLISION_PROBES,
    })
}

fn decode_digit(ch: char, value: &str) -> Result<u8, RecordIdError> {
    let digit = match ch {
        '0'..='9' => ch as u8 - b'0',
        'A'..='H' => ch as u8 - b'A' + 10,
        'J'..='K' => ch as u8 - b'J' + 18,
        'M'..='N' => ch as u8 - b'M' + 20,
        'P'..='T' => ch as u8 - b'P' + 22,
        'V'..='Z' => ch as u8 - b'V' + 27,
        _ => {
            return Err(RecordIdError::InvalidCharacter {
                value: value.to_string(),
                ch,
            });
        }
    };
    Ok(digit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_fixed_width_crockford_base32() {
        assert_eq!(encode_unix_epoch_millis(0), "0000000000000");
        assert_eq!(encode_unix_epoch_millis(31), "000000000000Z");
        assert_eq!(encode_unix_epoch_millis(32), "0000000000010");
        assert_eq!(decode_unix_epoch_millis("0000000000010").unwrap(), 32);
    }

    #[test]
    fn lexicographic_order_matches_numeric_order() {
        let values = [0, 1, 31, 32, 33, 1024, 1_782_554_447_000, u64::MAX];
        let encoded = values
            .iter()
            .map(|value| encode_unix_epoch_millis(*value))
            .collect::<Vec<_>>();
        let mut sorted = encoded.clone();
        sorted.sort();
        assert_eq!(encoded, sorted);
    }

    #[test]
    fn rejects_ambiguous_or_path_unsafe_characters() {
        for id in [
            "000000000000I",
            "000000000000L",
            "000000000000O",
            "00000000000/0",
            "ZZZZZZZZZZZZZ",
        ] {
            assert!(validate_record_id(id).is_err(), "{id} should be invalid");
        }
    }

    #[test]
    fn collision_allocation_increments_milliseconds_without_suffixes() {
        let base = 1_782_554_447_000;
        let first = encode_unix_epoch_millis(base);
        let second = encode_unix_epoch_millis(base + 1);
        let allocated = allocate_record_id(base, |id| id == first).unwrap();
        assert_eq!(allocated, second);
    }

    #[test]
    fn collision_allocation_is_bounded() {
        let err = allocate_record_id(42, |_| true).unwrap_err();
        assert_eq!(
            err,
            RecordIdError::ExcessiveCollisions {
                base_millis: 42,
                attempts: MAX_COLLISION_PROBES,
            }
        );
    }
}

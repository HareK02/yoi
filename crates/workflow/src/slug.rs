//! Slug type and validation.
//!
//! Syntax (agent-skills compatible):
//!   ^[a-z0-9](?:[a-z0-9-]{0,62}[a-z0-9])?$
//!   - 1–64 chars
//!   - lowercase ASCII alphanumerics and `-`
//!   - cannot start or end with `-`
//!   - no consecutive `--`

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize};

use crate::WorkflowLintError;

const MIN_LEN: usize = 1;
const MAX_LEN: usize = 64;

/// Validated slug. Constructible only via [`Slug::parse`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct Slug(String);

impl Slug {
    /// Parse and validate. Returns [`WorkflowLintError::InvalidSlug`] on rejection.
    pub fn parse(s: impl Into<String>) -> Result<Self, WorkflowLintError> {
        let s = s.into();
        if is_valid_slug(&s) {
            Ok(Self(s))
        } else {
            Err(WorkflowLintError::InvalidSlug(s))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl fmt::Display for Slug {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for Slug {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl FromStr for Slug {
    type Err = WorkflowLintError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl<'de> Deserialize<'de> for Slug {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Self::parse(raw).map_err(serde::de::Error::custom)
    }
}

/// Pure-fn predicate matching the agent-skills slug regex without
/// pulling in the `regex` crate.
pub fn is_valid_slug(s: &str) -> bool {
    let bytes = s.as_bytes();
    let len = bytes.len();
    if len < MIN_LEN || len > MAX_LEN {
        return false;
    }
    if !is_alnum_lower(bytes[0]) || !is_alnum_lower(bytes[len - 1]) {
        return false;
    }
    let mut prev_dash = false;
    for &b in bytes {
        if b == b'-' {
            if prev_dash {
                return false;
            }
            prev_dash = true;
        } else if is_alnum_lower(b) {
            prev_dash = false;
        } else {
            return false;
        }
    }
    true
}

fn is_alnum_lower(b: u8) -> bool {
    b.is_ascii_digit() || b.is_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_basic_slugs() {
        for s in ["a", "ab", "abc-def", "x9", "a-b-c", "123", "a-1"] {
            assert!(is_valid_slug(s), "expected `{s}` valid");
            assert!(Slug::parse(s).is_ok());
        }
    }

    #[test]
    fn rejects_bad_slugs() {
        for s in [
            "", "-", "-foo", "foo-", "Foo", "foo_bar", "foo bar", "foo--bar", "foo.bar", "ä",
        ] {
            assert!(!is_valid_slug(s), "expected `{s}` invalid");
            assert!(Slug::parse(s).is_err());
        }
    }

    #[test]
    fn enforces_length_bounds() {
        let too_long = "a".repeat(MAX_LEN + 1);
        assert!(!is_valid_slug(&too_long));
        let max = "a".repeat(MAX_LEN);
        assert!(is_valid_slug(&max));
    }

    #[test]
    fn deserializes_via_serde() {
        let json = "\"valid-slug\"";
        let slug: Slug = serde_json::from_str(json).unwrap();
        assert_eq!(slug.as_str(), "valid-slug");

        let bad = "\"BAD\"";
        let err: Result<Slug, _> = serde_json::from_str(bad);
        assert!(err.is_err());
    }
}

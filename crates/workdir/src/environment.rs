//! Model-facing session coordinates, separate from provider paths and authority.

use crate::{WorkdirPath, WorkdirSessionCapabilities, WorkdirToolScopeRule};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A snapshot of the environment actually used by a session's operations.
///
/// Cwd is a logical Workdir-root coordinate. Scope targets use that root or
/// explicit absolute provider paths. Scope is an additional allow ceiling, not
/// a replacement for provider checks. Providers
/// may retain further restrictions (including deny rules); this projection
/// never authorizes an operation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkdirSessionEnvironment {
    #[serde(deserialize_with = "deserialize_cwd")]
    pub cwd: WorkdirPath,
    pub scope: Option<Vec<WorkdirToolScopeRule>>,
    pub capabilities: WorkdirSessionCapabilities,
    pub restrictions: Vec<String>,
}

fn deserialize_cwd<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<WorkdirPath, D::Error> {
    let path = String::deserialize(deserializer)?;
    WorkdirPath::new(path).map_err(serde::de::Error::custom)
}

impl WorkdirSessionEnvironment {
    pub fn root(capabilities: WorkdirSessionCapabilities) -> Self {
        Self {
            cwd: WorkdirPath::root(),
            scope: None,
            capabilities,
            restrictions: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_cwd_is_normalized_and_cannot_claim_an_absolute_or_parent_base() {
        let mut wire = serde_json::to_value(WorkdirSessionEnvironment::root(
            WorkdirSessionCapabilities::READ_ONLY,
        ))
        .unwrap();
        for invalid in [
            "/absolute",
            "../parent",
            "nested/../parent",
            "nested\\other",
        ] {
            wire["cwd"] = invalid.into();
            assert!(
                serde_json::from_value::<WorkdirSessionEnvironment>(wire.clone()).is_err(),
                "{invalid}"
            );
        }
        wire["cwd"] = "./nested//child".into();
        let decoded: WorkdirSessionEnvironment = serde_json::from_value(wire).unwrap();
        assert_eq!(decoded.cwd.as_str(), "nested/child");
    }
}

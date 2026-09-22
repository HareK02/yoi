//! Filesystem-independent Ticket role domain types.
//!
//! Ticket policy and role launch configuration are projected by the Workspace
//! control plane. This module deliberately contains no repository-local config
//! loader, path fallback, or storage backend selection.

use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TicketRole {
    Intake,
    Orchestrator,
    Coder,
    Reviewer,
}

impl TicketRole {
    pub const ALL: [TicketRole; 4] = [
        TicketRole::Intake,
        TicketRole::Orchestrator,
        TicketRole::Coder,
        TicketRole::Reviewer,
    ];

    pub fn supported_names() -> Vec<&'static str> {
        Self::ALL.iter().map(|role| role.as_str()).collect()
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Intake => "intake",
            Self::Orchestrator => "orchestrator",
            Self::Coder => "coder",
            Self::Reviewer => "reviewer",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "intake" => Some(Self::Intake),
            "orchestrator" => Some(Self::Orchestrator),
            "coder" => Some(Self::Coder),
            "reviewer" => Some(Self::Reviewer),
            _ => None,
        }
    }

    pub fn default_profile(self) -> &'static str {
        match self {
            Self::Intake => "builtin:intake",
            Self::Orchestrator => "builtin:orchestrator",
            Self::Coder => "builtin:coder",
            Self::Reviewer => "builtin:reviewer",
        }
    }
}

impl fmt::Display for TicketRole {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_config_has_no_repository_local_authority() {
        let production = include_str!("config.rs")
            .split_once("#[cfg(test)]\nmod tests")
            .map(|(production, _)| production)
            .expect("config test module marker");
        for forbidden in [
            "load_workspace",
            ".yoi/",
            "TicketBackendConfig",
            "std::fs",
            "PathBuf",
        ] {
            assert!(
                !production.contains(forbidden),
                "repository-local Ticket config authority returned through {forbidden}"
            );
        }
    }

    #[test]
    fn fixed_roles_round_trip_without_filesystem_configuration() {
        for role in TicketRole::ALL {
            assert_eq!(TicketRole::parse(role.as_str()), Some(role));
            assert!(role.default_profile().starts_with("builtin:"));
        }
        assert_eq!(TicketRole::parse("operator"), None);
    }
}

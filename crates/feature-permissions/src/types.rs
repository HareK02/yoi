//! Wire values describe grants, never authority-bearing handles. Unknown fields
//! are rejected so old payload-bearing inputs cannot silently broaden a grant.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use ts_rs::TS;

macro_rules! identifier {
    ($name:ident, $validate:expr) => {
        #[derive(
            Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, TS, schemars::JsonSchema,
        )]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn parse(value: impl Into<String>) -> Result<Self, String> {
                let value = value.into();
                if ($validate)(&value) {
                    Ok(Self(value))
                } else {
                    Err(format!("invalid {}: {value:?}", stringify!($name)))
                }
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                Self::parse(String::deserialize(d)?).map_err(serde::de::Error::custom)
            }
        }
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

fn segment(value: &str) -> bool {
    !value.is_empty()
        && value.as_bytes()[0].is_ascii_lowercase()
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}
fn identity(value: &str) -> bool {
    !value.is_empty() && !value.chars().any(|c| c.is_control() || c.is_whitespace())
}
identifier!(WorkspaceId, identity);
identifier!(FeatureIdentity, identity);
identifier!(PermissionSetId, segment);
identifier!(NamespaceId, segment);
identifier!(PermissionKey, |value: &str| value.split('.').count() >= 2
    && value.split('.').all(segment));

impl PermissionKey {
    pub fn namespace(&self) -> NamespaceId {
        NamespaceId(self.0.split('.').next().expect("validated key").to_owned())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NamespacePermissions {
    pub namespace: NamespaceId,
    pub keys: Vec<PermissionKey>,
}

/// The discriminant, not an identifier's spelling, selects a reference kind.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum PermissionReference {
    Set(PermissionSetId),
    Namespace(NamespacePermissions),
}

/// `None` at the consuming policy boundary means unset. An empty assignment is
/// explicit deny-all; this crate never chooses or reinstates a default.
#[derive(
    Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema,
)]
#[serde(deny_unknown_fields)]
pub struct PermissionAssignment {
    pub grants: Vec<PermissionReference>,
    pub exclusions: Vec<PermissionReference>,
    pub revoked: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PermissionDefinition {
    pub key: PermissionKey,
    pub description: String,
    pub children: Vec<PermissionKey>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetContribution {
    pub set: PermissionSetId,
    pub children: Vec<PermissionReference>,
}

/// This input deliberately has no owner field. The Host supplies the registered
/// identity/namespace handle independently of guest-controlled declaration data.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NamespaceDeclaration {
    pub permissions: Vec<PermissionDefinition>,
    pub sets: Vec<SetContribution>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NamespaceCatalog {
    pub namespace: NamespaceId,
    pub feature: FeatureIdentity,
    pub enabled: bool,
    pub declaration: NamespaceDeclaration,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PermissionCatalog {
    pub workspace: WorkspaceId,
    pub namespaces: Vec<NamespaceCatalog>,
}

#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS, schemars::JsonSchema,
)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum OriginStep {
    Set(PermissionSetId),
    Feature(FeatureIdentity),
    Key(PermissionKey),
}

#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS, schemars::JsonSchema,
)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum PermissionDiagnostic {
    Unset,
    Revoked,
    UnknownSet(PermissionSetId),
    UnknownNamespace(NamespaceId),
    UnknownKey(PermissionKey),
    DisabledFeature(FeatureIdentity),
    Excluded(PermissionKey),
}

/// A management projection, not an executable credential. Only the Host's opaque
/// resolution can be queried through a namespace handle.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PermissionResolution {
    pub workspace: WorkspaceId,
    pub allowed: BTreeMap<PermissionKey, Vec<Vec<OriginStep>>>,
    pub diagnostics: Vec<PermissionDiagnostic>,
}

/// The management boundary uses exactly the same assignment as text/storage.
/// Workspace comes from authenticated routing, not a repeated permission field.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ValidatePermissionsRequest {
    pub assignment: PermissionAssignment,
}

pub fn typescript() -> String {
    let config = ts_rs::Config::default();
    let declarations = [
        WorkspaceId::decl(&config),
        FeatureIdentity::decl(&config),
        PermissionSetId::decl(&config),
        NamespaceId::decl(&config),
        PermissionKey::decl(&config),
        NamespacePermissions::decl(&config),
        PermissionReference::decl(&config),
        PermissionAssignment::decl(&config),
        PermissionDefinition::decl(&config),
        SetContribution::decl(&config),
        NamespaceDeclaration::decl(&config),
        NamespaceCatalog::decl(&config),
        PermissionCatalog::decl(&config),
        OriginStep::decl(&config),
        PermissionDiagnostic::decl(&config),
        PermissionResolution::decl(&config),
        ValidatePermissionsRequest::decl(&config),
    ];
    format!(
        "// Generated by feature-permissions; do not edit.\n{}\n",
        declarations
            .iter()
            .map(|d| format!("export {d}"))
            .collect::<Vec<_>>()
            .join("\n")
    )
}

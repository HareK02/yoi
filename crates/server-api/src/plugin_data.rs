//! Ticket extension-data wire boundary. Workspace, installation and execution
//! identity come exclusively from the authenticated, owner-issued invocation.
//! Guest selectors are never authorization. Receipts share the HTTPS invocation
//! scope and expiry, and are checked after live authorization but before cell CAS.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const PLUGIN_TICKET_BODY_LIMIT: usize = 64 * 1024;
pub const PLUGIN_DATA_VALUE_LIMIT: usize = 64 * 1024;
pub const PLUGIN_DATA_LIST_LIMIT: u32 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum PluginTicketAccess {
    Read,
    ExtensionWrite,
}

/// Canonical Ticket identity is returned only after resolve authorizes it. It is
/// still a selector: every subsequent operation must reauthorize with the owner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct PluginTicketSnapshot {
    pub title: String,
    pub case_state: PluginTicketCaseState,
    pub content_digest: String,
    pub body: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum PluginTicketCaseState {
    Open,
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct PluginDataCollection {
    pub collection: String,
    pub schema_id: String,
    pub schema_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct PluginDataAddress {
    pub collection: PluginDataCollection,
    pub key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(
    tag = "kind",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum PluginDataExpectation {
    NeverWritten,
    LastWrite(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(
    tag = "kind",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum PluginDataCell {
    NeverWritten,
    Present {
        value_json: String,
        last_write: String,
    },
    Deleted {
        last_delete: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PluginDataMutation {
    Put {
        expected: PluginDataExpectation,
        value_json: String,
    },
    Delete {
        expected_last_write: String,
    },
}

/// A saved mutation result, not an observation of the current cell. The digest
/// covers the canonical resulting cell (including its final operation identity).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct PluginDataReceipt {
    pub mutation_id: String,
    pub result_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(
    tag = "kind",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum PluginDataMutationLookup {
    NotRecorded,
    Committed(PluginDataReceipt),
    Expired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct PluginDataEntry {
    pub key: String,
    pub cell: PluginDataCell,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct PluginDataPage {
    pub entries: Vec<PluginDataEntry>,
    pub next_cursor: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn cell_states_and_receipt_absence_are_not_conflated() {
        let never = serde_json::to_value(PluginDataCell::NeverWritten).unwrap();
        let deleted = serde_json::to_value(PluginDataCell::Deleted {
            last_delete: "delete-operation".into(),
        })
        .unwrap();
        assert_ne!(never, deleted);
        assert_eq!(
            serde_json::from_value::<PluginDataCell>(never).unwrap(),
            PluginDataCell::NeverWritten
        );
        assert_ne!(
            serde_json::to_value(PluginDataMutationLookup::NotRecorded).unwrap(),
            serde_json::to_value(PluginDataMutationLookup::Expired).unwrap()
        );
    }
    #[test]
    fn address_rejects_guest_workspace_installation_or_schema_aliases() {
        let address = json!({"collection":{"collection":"notes","schema_id":"note","schema_version":"1"},"key":"main"});
        assert!(serde_json::from_value::<PluginDataAddress>(address.clone()).is_ok());
        for field in ["workspace_id", "installation_id", "ticket_id"] {
            let mut forged = address.clone();
            forged[field] = json!("foreign");
            assert!(serde_json::from_value::<PluginDataAddress>(forged).is_err());
        }
        let mut numeric_version = address;
        numeric_version["collection"]["schema_version"] = json!(1);
        assert!(serde_json::from_value::<PluginDataAddress>(numeric_version).is_err());
    }
}

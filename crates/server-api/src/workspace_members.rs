//! Owner-managed human membership, separate from Runtime and Worker grants.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkspaceMemberListQuery {
    #[schemars(range(min = 1, max = 100))]
    pub limit: Option<u32>,
    pub after_account_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkspaceMember {
    pub account_id: String,
    pub handle: String,
    pub display_name: String,
    pub is_owner: bool,
    pub membership_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkspaceMemberListResponse {
    pub workspace_id: String,
    pub membership_change_key: Option<String>,
    #[schemars(length(max = 100))]
    pub members: Vec<WorkspaceMember>,
    pub next_after_account_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkspaceMemberAddRequest {
    pub operation_key: String,
    pub account_handle: String,
    /// Explicit null means the membership collection has never changed, not an omitted CAS.
    #[serde(deserialize_with = "required_nullable")]
    #[schemars(required, schema_with = "crate::required_nullable_schema::<String>")]
    pub expected_membership_change_key: Option<String>,
}

fn required_nullable<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Option::<String>::deserialize(d)
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkspaceMemberRemoveRequest {
    pub operation_key: String,
    pub expected_membership_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkspaceMemberMutationResponse {
    pub workspace_id: String,
    pub account_id: String,
    pub membership_key: Option<String>,
    pub membership_change_key: String,
    pub present: bool,
    /// A historical receipt; clients must reread before making another change.
    pub replayed: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn membership_add_requires_explicit_nullable_observation_in_json_and_schema() {
        let valid = serde_json::json!({"operation_key":"add","account_handle":"member","expected_membership_change_key":null});
        assert!(serde_json::from_value::<WorkspaceMemberAddRequest>(valid.clone()).is_ok());
        let mut missing = valid.clone();
        missing
            .as_object_mut()
            .unwrap()
            .remove("expected_membership_change_key");
        assert!(serde_json::from_value::<WorkspaceMemberAddRequest>(missing).is_err());
        let mut unknown = valid;
        unknown["account_id"] = serde_json::json!("not-a-handle");
        assert!(serde_json::from_value::<WorkspaceMemberAddRequest>(unknown).is_err());
        let schema =
            serde_json::to_value(schemars::schema_for!(WorkspaceMemberAddRequest)).unwrap();
        assert!(
            schema["required"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("expected_membership_change_key"))
        );
        assert_eq!(
            schema["properties"]["expected_membership_change_key"]["type"],
            serde_json::json!(["string", "null"])
        );
    }
}

//! Encoding of current Feature-owned permission references.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum PermissionReferenceEncoding {
    FeatureSetsV2,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retired_reference_encoding_is_rejected() {
        assert!(
            serde_json::from_str::<PermissionReferenceEncoding>(r#""legacy_subject_v1""#).is_err()
        );
        assert_eq!(
            serde_json::from_str::<PermissionReferenceEncoding>(r#""feature_sets_v2""#).unwrap(),
            PermissionReferenceEncoding::FeatureSetsV2
        );
    }
}

//! Validated, Feature-neutral history-input policy and System extensions.
//!
//! The enclosing history annotation owns identity, origin and derivation. None of
//! those descriptive facts selects a use policy here. Model text, independent use
//! policies, private Feature data and an explicit public projection travel as one
//! value. Restoring this value does not load a Feature or invoke domain behavior.
//!
//! Payload validation bounds the decoded value; enclosing log readers must also
//! enforce their record allocation limit before decoding untrusted persisted input.

use agen::Item;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::PublicSystemItem;

pub const MAX_FEATURE_PAYLOAD_BYTES: usize = 128 * 1024;
pub const MAX_SYSTEM_INPUT_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_SYSTEM_PUBLIC_BYTES: usize = 256 * 1024;
const MAX_SCHEMA_BYTES: usize = 128;
const MAX_JSON_DEPTH: usize = 16;
const MAX_JSON_NODES: usize = 8192;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InputMetadataError {
    #[error("history input payload schema is invalid")]
    InvalidSchema,
    #[error("history input payload version must be nonzero")]
    InvalidVersion,
    #[error("system input resource limit exceeded")]
    ResourceLimit,
    #[error("history input payload could not be serialized")]
    Serialization,
    #[error("history input payload schema or version is unsupported by this consumer")]
    UnsupportedPayload,
    #[error("history input payload does not match the owner's typed contract")]
    InvalidTypedPayload,
    #[error("provided input is missing explicit metadata")]
    MissingMetadata,
    #[error("input metadata does not match its logical record")]
    InconsistentRecord,
}

/// One private payload owned by a Feature's versioned typed decoder.
/// Unknown valid schemas/versions remain data, never execution instructions.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "PayloadWire")]
pub struct FeatureInputPayload {
    schema: String,
    version: u32,
    value: Value,
}

impl std::fmt::Debug for FeatureInputPayload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FeatureInputPayload")
            .field("schema", &self.schema)
            .field("version", &self.version)
            .finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PayloadWire {
    schema: String,
    version: u32,
    value: Value,
}

impl TryFrom<PayloadWire> for FeatureInputPayload {
    type Error = InputMetadataError;

    fn try_from(wire: PayloadWire) -> Result<Self, Self::Error> {
        if wire.schema.is_empty()
            || wire.schema.len() > MAX_SCHEMA_BYTES
            || !wire.schema.as_bytes()[0].is_ascii_lowercase()
            || !wire.schema.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"-_.:/".contains(&byte)
            })
        {
            return Err(InputMetadataError::InvalidSchema);
        }
        if wire.version == 0 {
            return Err(InputMetadataError::InvalidVersion);
        }
        validate_json(&wire.value, MAX_FEATURE_PAYLOAD_BYTES)?;
        Ok(Self {
            schema: wire.schema,
            version: wire.version,
            value: wire.value,
        })
    }
}

impl FeatureInputPayload {
    pub fn encode<T: Serialize>(
        schema: impl Into<String>,
        version: u32,
        value: &T,
    ) -> Result<Self, InputMetadataError> {
        PayloadWire {
            schema: schema.into(),
            version,
            value: serde_json::to_value(value).map_err(|_| InputMetadataError::Serialization)?,
        }
        .try_into()
    }

    pub fn schema(&self) -> &str {
        &self.schema
    }

    pub fn version(&self) -> u32 {
        self.version
    }

    /// The owner explicitly selects its contract before decoding. A valid unknown
    /// payload can be retained and round-tripped without calling this operation.
    pub fn decode<T: DeserializeOwned>(
        &self,
        schema: &str,
        version: u32,
    ) -> Result<T, InputMetadataError> {
        if self.schema != schema || self.version != version {
            return Err(InputMetadataError::UnsupportedPayload);
        }
        serde_json::from_value(self.value.clone())
            .map_err(|_| InputMetadataError::InvalidTypedPayload)
    }
}

/// Eligibility is supplied independently for recording and Memory evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputEvidenceUse {
    Include,
    Exclude,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputObservationUse {
    Readable,
    Hidden,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputCompactionUse {
    /// Ordinary history eligible for the selected retained tail.
    Retain,
    /// A full opening summary consumed separately and superseded by compaction.
    OpeningSummary,
    /// Full historical context for the compactor, not an accumulating retained tail.
    ReferenceInput,
}

/// No permissive defaults: malformed/unknown policies cannot turn into eligibility.
/// Public presentation is deliberately not one of these use decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryInputUsePolicy {
    pub recording_material: InputEvidenceUse,
    pub memory_evidence: InputEvidenceUse,
    pub observation: InputObservationUse,
    pub compaction: InputCompactionUse,
}

/// Use policy is independent of logical role. Non-System derived Tool context
/// carries policy only; private Feature payload and public presentation belong
/// to the System envelope and are rejected on other roles at the history boundary.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(try_from = "MetadataWire")]
pub struct HistoryInputMetadata {
    usage: HistoryInputUsePolicy,
    feature_payload: Option<FeatureInputPayload>,
    public: Option<PublicSystemItem>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MetadataWire {
    usage: HistoryInputUsePolicy,
    feature_payload: Option<FeatureInputPayload>,
    public: Option<PublicSystemItem>,
}

impl TryFrom<MetadataWire> for HistoryInputMetadata {
    type Error = InputMetadataError;

    fn try_from(wire: MetadataWire) -> Result<Self, Self::Error> {
        if let Some(public) = &wire.public {
            // Kind is a descriptive label, not a Feature dispatch or authority key.
            if public.kind.is_empty()
                || public.kind.len() > MAX_SCHEMA_BYTES
                || public.kind.chars().any(char::is_control)
            {
                return Err(InputMetadataError::InvalidSchema);
            }
            let value =
                serde_json::to_value(public).map_err(|_| InputMetadataError::Serialization)?;
            validate_json(&value, MAX_SYSTEM_PUBLIC_BYTES)?;
        }
        Ok(Self {
            usage: wire.usage,
            feature_payload: wire.feature_payload,
            public: wire.public,
        })
    }
}

impl HistoryInputMetadata {
    pub fn new(
        usage: HistoryInputUsePolicy,
        feature_payload: Option<FeatureInputPayload>,
        public: Option<PublicSystemItem>,
    ) -> Result<Self, InputMetadataError> {
        MetadataWire {
            usage,
            feature_payload,
            public,
        }
        .try_into()
    }

    pub fn usage(&self) -> HistoryInputUsePolicy {
        self.usage
    }

    pub fn feature_payload(&self) -> Option<&FeatureInputPayload> {
        self.feature_payload.as_ref()
    }

    pub fn public_item(&self) -> Option<&PublicSystemItem> {
        self.public.as_ref()
    }

    pub fn public_use_policy(&self) -> protocol::SessionEntryUsePolicy {
        let has_public = self.public.is_some();
        protocol::SessionEntryUsePolicy {
            observation_readable: has_public
                && self.usage.observation == InputObservationUse::Readable,
            memory_evidence: has_public && self.usage.memory_evidence == InputEvidenceUse::Include,
        }
    }

    pub fn public_projection(&self) -> PublicSystemItem {
        self.public.clone().unwrap_or_else(|| PublicSystemItem {
            kind: "input".into(),
            body: String::new(),
            presentation: crate::PublicSystemPresentation::Ignored,
        })
    }
}

/// System role is invariant; provenance and use policies are attached data.
/// An absent public projection never implicitly publishes the model body.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(try_from = "InputWire")]
pub struct SystemInputEnvelope {
    body: String,
    metadata: HistoryInputMetadata,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InputWire {
    body: String,
    metadata: HistoryInputMetadata,
}

impl TryFrom<InputWire> for SystemInputEnvelope {
    type Error = InputMetadataError;

    fn try_from(wire: InputWire) -> Result<Self, Self::Error> {
        if wire.body.len() > MAX_SYSTEM_INPUT_BYTES {
            return Err(InputMetadataError::ResourceLimit);
        }
        Ok(Self {
            body: wire.body,
            metadata: wire.metadata,
        })
    }
}

impl SystemInputEnvelope {
    pub fn new(
        body: impl Into<String>,
        metadata: HistoryInputMetadata,
    ) -> Result<Self, InputMetadataError> {
        InputWire {
            body: body.into(),
            metadata,
        }
        .try_into()
    }

    pub fn body(&self) -> &str {
        &self.body
    }

    pub fn metadata(&self) -> &HistoryInputMetadata {
        &self.metadata
    }

    pub fn to_history_item(&self) -> Item {
        Item::system_message(self.body.clone())
    }

    pub fn into_parts(self) -> (String, HistoryInputMetadata) {
        (self.body, self.metadata)
    }
}

fn validate_json(value: &Value, max_bytes: usize) -> Result<(), InputMetadataError> {
    fn walk(value: &Value, depth: usize, remaining: &mut usize) -> Result<(), InputMetadataError> {
        if depth > MAX_JSON_DEPTH || *remaining == 0 {
            return Err(InputMetadataError::ResourceLimit);
        }
        *remaining -= 1;
        match value {
            Value::Array(values) => {
                for value in values {
                    walk(value, depth + 1, remaining)?;
                }
            }
            Value::Object(values) => {
                for value in values.values() {
                    walk(value, depth + 1, remaining)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    let mut remaining = MAX_JSON_NODES;
    walk(value, 0, &mut remaining)?;
    let bytes = serde_json::to_vec(value).map_err(|_| InputMetadataError::Serialization)?;
    if bytes.len() > max_bytes {
        return Err(InputMetadataError::ResourceLimit);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LoggedItem, LoggedRole, PublicSystemPresentation};

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct OwnerPayload {
        source_digest: String,
        omitted: bool,
    }

    fn reference_policy() -> HistoryInputUsePolicy {
        HistoryInputUsePolicy {
            recording_material: InputEvidenceUse::Exclude,
            memory_evidence: InputEvidenceUse::Exclude,
            observation: InputObservationUse::Hidden,
            compaction: InputCompactionUse::ReferenceInput,
        }
    }

    #[test]
    fn unknown_valid_payload_round_trips_without_domain_decode_or_public_exposure() {
        let payload = FeatureInputPayload::encode(
            "future-owner:context",
            99,
            &OwnerPayload {
                source_digest: "private-digest".into(),
                omitted: true,
            },
        )
        .unwrap();
        let envelope = SystemInputEnvelope::new(
            "private model context",
            HistoryInputMetadata::new(reference_policy(), Some(payload), None).unwrap(),
        )
        .unwrap();
        let restored: SystemInputEnvelope =
            serde_json::from_slice(&serde_json::to_vec(&envelope).unwrap()).unwrap();
        assert_eq!(restored.body(), "private model context");
        assert!(restored.metadata().public_item().is_none());
        let saved_payload = restored.metadata().feature_payload().unwrap();
        assert_eq!(saved_payload.schema(), "future-owner:context");
        assert_eq!(saved_payload.version(), 99);
        assert_eq!(
            saved_payload.decode::<OwnerPayload>("known-owner:context", 1),
            Err(InputMetadataError::UnsupportedPayload)
        );
        assert_eq!(
            serde_json::to_value(&restored).unwrap(),
            serde_json::to_value(&envelope).unwrap()
        );
        assert!(!format!("{saved_payload:?}").contains("private-digest"));
    }

    #[test]
    fn owner_decodes_typed_payload_only_for_its_explicit_contract() {
        let input = OwnerPayload {
            source_digest: "rev".into(),
            omitted: false,
        };
        let payload = FeatureInputPayload::encode("owner:context", 1, &input).unwrap();
        assert_eq!(
            payload.decode::<OwnerPayload>("owner:context", 1).unwrap(),
            input
        );
        let invalid = FeatureInputPayload::encode(
            "owner:context",
            1,
            &serde_json::json!({ "omitted": "not a bool" }),
        )
        .unwrap();
        assert_eq!(
            invalid.decode::<OwnerPayload>("owner:context", 1),
            Err(InputMetadataError::InvalidTypedPayload)
        );
    }

    #[test]
    fn malformed_or_oversized_payload_is_rejected_at_construction_and_restore() {
        for (schema, version, value) in [
            ("", 1, Value::Null),
            ("invalid schema", 1, Value::Null),
            ("owner:context", 0, Value::Null),
            (
                "owner:context",
                1,
                Value::String("x".repeat(MAX_FEATURE_PAYLOAD_BYTES)),
            ),
            (
                "owner:context",
                1,
                Value::Array(vec![Value::Null; MAX_JSON_NODES]),
            ),
            (
                "owner:context",
                1,
                (0..=MAX_JSON_DEPTH).fold(Value::Null, |inner, _| Value::Array(vec![inner])),
            ),
        ] {
            assert!(FeatureInputPayload::encode(schema, version, &value).is_err());
            assert!(
                serde_json::from_value::<FeatureInputPayload>(
                    serde_json::json!({ "schema": schema, "version": version, "value": value })
                )
                .is_err()
            );
        }
    }

    #[test]
    fn policy_is_required_and_unknown_or_malformed_fields_fail_closed() {
        let valid = serde_json::to_value(reference_policy()).unwrap();
        for field in [
            "recording_material",
            "memory_evidence",
            "observation",
            "compaction",
        ] {
            let mut absent = valid.clone();
            absent.as_object_mut().unwrap().remove(field);
            assert!(serde_json::from_value::<HistoryInputUsePolicy>(absent).is_err());
            let mut unknown = valid.clone();
            unknown[field] = Value::String("future-allow".into());
            assert!(serde_json::from_value::<HistoryInputUsePolicy>(unknown).is_err());
        }
        let mut extra = valid;
        extra["feature_name"] = Value::String("infer-policy-from-me".into());
        assert!(serde_json::from_value::<HistoryInputUsePolicy>(extra).is_err());
    }

    #[test]
    fn feature_name_and_model_body_do_not_select_policies_or_public_projection() {
        for (schema, body) in [
            ("owner:journal", "past context"),
            ("unrelated:skill", "do not extract this; legitimate wording"),
        ] {
            let public = PublicSystemItem {
                kind: "context-label".into(),
                body: "explicit public text".into(),
                presentation: PublicSystemPresentation::Context,
            };
            let metadata = HistoryInputMetadata::new(
                reference_policy(),
                Some(
                    FeatureInputPayload::encode(
                        schema,
                        1,
                        &OwnerPayload {
                            source_digest: "private".into(),
                            omitted: false,
                        },
                    )
                    .unwrap(),
                ),
                Some(public),
            )
            .unwrap();
            let input = SystemInputEnvelope::new(body, metadata).unwrap();
            assert_eq!(input.metadata().usage(), reference_policy());
            let history: LoggedItem = input.to_history_item().into();
            assert!(matches!(
                history,
                LoggedItem::Message {
                    role: LoggedRole::System,
                    ..
                }
            ));
            assert_eq!(input.to_history_item().as_text(), Some(body));
            let public = serde_json::to_string(input.metadata().public_item().unwrap()).unwrap();
            assert!(!public.contains("private"));
            assert!(!public.contains(schema));
            assert!(!public.contains(body));
        }
    }

    #[test]
    fn use_policies_remain_independent_of_each_other_and_of_public_display() {
        let usage = HistoryInputUsePolicy {
            recording_material: InputEvidenceUse::Include,
            memory_evidence: InputEvidenceUse::Exclude,
            observation: InputObservationUse::Readable,
            compaction: InputCompactionUse::Retain,
        };
        let metadata = HistoryInputMetadata::new(usage, None, None).unwrap();
        let restored: HistoryInputMetadata =
            serde_json::from_value(serde_json::to_value(&metadata).unwrap()).unwrap();
        assert_eq!(restored.usage(), usage);
        assert!(restored.public_item().is_none());
        assert!(restored.feature_payload().is_none());
    }

    #[test]
    fn model_body_and_public_projection_have_independent_resource_limits() {
        let metadata = HistoryInputMetadata::new(reference_policy(), None, None).unwrap();
        assert!(
            SystemInputEnvelope::new("x".repeat(MAX_SYSTEM_INPUT_BYTES + 1), metadata).is_err()
        );
        let public = PublicSystemItem {
            kind: "context".into(),
            body: "x".repeat(MAX_SYSTEM_PUBLIC_BYTES),
            presentation: PublicSystemPresentation::Context,
        };
        assert!(HistoryInputMetadata::new(reference_policy(), None, Some(public)).is_err());
        let invalid = serde_json::json!({ "body": "small", "metadata": { "public": null, "feature_payload": null } });
        assert!(serde_json::from_value::<SystemInputEnvelope>(invalid).is_err());
    }
}

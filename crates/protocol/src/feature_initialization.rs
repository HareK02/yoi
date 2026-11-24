//! Durable Feature-owned initialization data, not authority or live capabilities.
//!
//! Public requests and Host-resolved connections use the same bounded envelope on
//! separate authenticated boundaries. A receiver must select the registered owner,
//! check its exact compatibility epoch and decode its own type before use. Merely
//! decoding this envelope neither selects a provider nor authorizes an operation.
//! Credentials, live handles and local storage paths do not belong in this data.

use serde::{Deserialize, Deserializer, Serialize, de::DeserializeOwned};
use serde_json::Value;

pub const MAX_INITIALIZATIONS: usize = 32;
pub const MAX_FEATURE_ID_BYTES: usize = 128;
pub const MAX_PAYLOAD_BYTES: usize = 16 * 1024;
pub const MAX_INITIALIZATION_BYTES: usize = 128 * 1024;

/// One owner's versioned payload. Fields are private so in-process construction
/// and wire decoding enforce the same transport bounds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct FeatureInitialization {
    #[cfg_attr(feature = "json-schema", schemars(length(max = MAX_FEATURE_ID_BYTES)))]
    feature_id: String,
    #[cfg_attr(feature = "json-schema", schemars(range(min = 1)))]
    version: u32,
    #[cfg_attr(feature = "typescript", ts(type = "unknown"))]
    payload: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireInitialization {
    feature_id: String,
    version: u32,
    payload: Value,
}

impl FeatureInitialization {
    pub fn encode<T: Serialize>(
        feature_id: &str,
        version: u32,
        payload: &T,
    ) -> Result<Self, String> {
        let payload = serde_json::to_value(payload)
            .map_err(|_| "Feature initialization payload could not be encoded".to_owned())?;
        Self::from_wire(WireInitialization {
            feature_id: feature_id.to_owned(),
            version,
            payload,
        })
    }

    fn from_wire(wire: WireInitialization) -> Result<Self, String> {
        let valid_id = wire.feature_id.len() <= MAX_FEATURE_ID_BYTES
            && wire
                .feature_id
                .split_once(':')
                .is_some_and(|(source, name)| !source.is_empty() && !name.is_empty())
            && wire.feature_id.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'-' | b'_' | b'.' | b'/')
            });
        if !valid_id {
            return Err(
                "Feature initialization requires a bounded source-qualified identifier".into(),
            );
        }
        if wire.version == 0 {
            return Err("Feature initialization compatibility epoch must be nonzero".into());
        }
        if !wire.payload.is_object() {
            return Err("Feature initialization payload must be an object".into());
        }
        let bytes = serde_json::to_vec(&wire.payload)
            .map_err(|_| "Feature initialization payload could not be encoded".to_owned())?;
        if bytes.len() > MAX_PAYLOAD_BYTES {
            return Err("Feature initialization payload exceeds its byte limit".into());
        }
        Ok(Self {
            feature_id: wire.feature_id,
            version: wire.version,
            payload: wire.payload,
        })
    }

    pub fn feature_id(&self) -> &str {
        &self.feature_id
    }

    pub fn version(&self) -> u32 {
        self.version
    }

    /// Owners must use a concrete DTO (including its unknown-field policy), not
    /// `Value`, and perform domain validation and current authorization separately.
    /// Payload values are deliberately absent from transport diagnostics.
    pub fn decode<T: DeserializeOwned>(
        &self,
        expected_feature: &str,
        expected_version: u32,
    ) -> Result<T, String> {
        if self.feature_id != expected_feature {
            return Err("Feature initialization was routed to a different owner".into());
        }
        if self.version != expected_version {
            return Err(format!(
                "unsupported Feature initialization epoch {} for {} (expected {})",
                self.version, self.feature_id, expected_version
            ));
        }
        serde_json::from_value(self.payload.clone())
            .map_err(|_| format!("invalid initialization payload for {}", self.feature_id))
    }
}

impl<'de> Deserialize<'de> for FeatureInitialization {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::from_wire(WireInitialization::deserialize(deserializer)?)
            .map_err(serde::de::Error::custom)
    }
}

/// A canonical set: ordering cannot change create intent, and duplicate owners
/// are rejected rather than replaced. The selected Feature plan still decides
/// whether an owner/provider exists and is compatible with the saved composition.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(transparent)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct FeatureInitializations(
    #[cfg_attr(feature = "json-schema", schemars(length(max = MAX_INITIALIZATIONS)))]
    Vec<FeatureInitialization>,
);

impl FeatureInitializations {
    pub fn new(mut records: Vec<FeatureInitialization>) -> Result<Self, String> {
        if records.len() > MAX_INITIALIZATIONS {
            return Err("too many Feature initializations".into());
        }
        records.sort_by(|left, right| left.feature_id.cmp(&right.feature_id));
        if records
            .windows(2)
            .any(|pair| pair[0].feature_id == pair[1].feature_id)
        {
            return Err("duplicate Feature initialization owner".into());
        }
        let bytes = serde_json::to_vec(&records)
            .map_err(|_| "Feature initialization set could not be encoded".to_owned())?;
        if bytes.len() > MAX_INITIALIZATION_BYTES {
            return Err("Feature initialization set exceeds its byte limit".into());
        }
        Ok(Self(records))
    }

    pub fn iter(&self) -> impl Iterator<Item = &FeatureInitialization> {
        self.0.iter()
    }

    pub fn get(&self, feature_id: &str) -> Option<&FeatureInitialization> {
        self.0
            .binary_search_by(|record| record.feature_id.as_str().cmp(feature_id))
            .ok()
            .map(|index| &self.0[index])
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }
}

impl<'de> Deserialize<'de> for FeatureInitializations {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct BoundedSet;
        impl<'de> serde::de::Visitor<'de> for BoundedSet {
            type Value = FeatureInitializations;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a bounded array of Feature initializations")
            }

            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Self::Value, A::Error> {
                let mut records = Vec::new();
                let mut bytes = 2;
                while let Some(record) = seq.next_element::<FeatureInitialization>()? {
                    if records.len() == MAX_INITIALIZATIONS {
                        return Err(serde::de::Error::custom("too many Feature initializations"));
                    }
                    bytes += serde_json::to_vec(&record)
                        .map_err(serde::de::Error::custom)?
                        .len()
                        + usize::from(!records.is_empty());
                    if bytes > MAX_INITIALIZATION_BYTES {
                        return Err(serde::de::Error::custom(
                            "Feature initialization set exceeds its byte limit",
                        ));
                    }
                    records.push(record);
                }
                FeatureInitializations::new(records).map_err(serde::de::Error::custom)
            }
        }
        deserializer.deserialize_seq(BoundedSet)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct TestConnection {
        stable_ref: String,
    }

    #[test]
    fn owner_codec_checks_epoch_and_payload_before_domain_use() {
        let expected = TestConnection {
            stable_ref: "connection-1".into(),
        };
        let record = FeatureInitialization::encode("test:connection", 2, &expected).unwrap();
        let encoded =
            serde_json::to_vec(&FeatureInitializations::new(vec![record]).unwrap()).unwrap();
        let reopened: FeatureInitializations = serde_json::from_slice(&encoded).unwrap();
        let record = reopened.get("test:connection").unwrap();
        assert_eq!(
            record
                .decode::<TestConnection>("test:connection", 2)
                .unwrap(),
            expected
        );
        assert!(record.decode::<TestConnection>("test:other", 2).is_err());
        assert!(
            record
                .decode::<TestConnection>("test:connection", 1)
                .is_err()
        );
        let malformed =
            FeatureInitialization::encode("test:connection", 2, &json!({"stable_ref": 12}))
                .unwrap();
        assert!(
            malformed
                .decode::<TestConnection>("test:connection", 2)
                .is_err()
        );
        let extra = FeatureInitialization::encode(
            "test:connection",
            2,
            &json!({"stable_ref":"ref", "credential":"private-value"}),
        )
        .unwrap();
        let error = extra
            .decode::<TestConnection>("test:connection", 2)
            .unwrap_err();
        assert!(!error.contains("private-value"));
    }

    #[test]
    fn wire_rejects_invalid_envelopes_and_oversized_payloads() {
        for input in [
            json!({"feature_id":"unqualified", "version":1, "payload":{}}),
            json!({"feature_id":"test:connection", "version":0, "payload":{}}),
            json!({"feature_id":"test:connection", "version":1, "payload":[]}),
            json!({"feature_id":"test:connection", "version":1, "payload":{}, "resolved":true}),
            json!({"feature_id":"test:connection", "version":1, "payload":{"text":"x".repeat(MAX_PAYLOAD_BYTES)}}),
        ] {
            assert!(serde_json::from_value::<FeatureInitialization>(input).is_err());
        }
        assert!(
            FeatureInitialization::encode(
                "test:connection",
                1,
                &json!({"text":"x".repeat(MAX_PAYLOAD_BYTES)})
            )
            .is_err()
        );
    }

    #[test]
    fn wire_set_rejects_duplicate_owners_count_and_aggregate_bytes() {
        let record = FeatureInitialization::encode("test:connection", 1, &json!({})).unwrap();
        let duplicate = json!([record.clone(), record]);
        assert!(serde_json::from_value::<FeatureInitializations>(duplicate).is_err());
        let records = (0..=MAX_INITIALIZATIONS)
            .map(|index| {
                FeatureInitialization::encode(&format!("test:{index}"), 1, &json!({})).unwrap()
            })
            .collect::<Vec<_>>();
        assert!(FeatureInitializations::new(records.clone()).is_err());
        assert!(serde_json::from_value::<FeatureInitializations>(json!(records)).is_err());
        let large = (0..9)
            .map(|index| {
                FeatureInitialization::encode(
                    &format!("test:{index}"),
                    1,
                    &json!({"text":"x".repeat(MAX_PAYLOAD_BYTES - 32)}),
                )
                .unwrap()
            })
            .collect();
        assert!(FeatureInitializations::new(large).is_err());
    }

    #[test]
    fn connection_order_is_canonical_but_changed_payload_changes_intent() {
        let a = FeatureInitialization::encode("test:a", 1, &json!({"ref":"one"})).unwrap();
        let b = FeatureInitialization::encode("test:b", 1, &json!({"ref":"two"})).unwrap();
        let first = FeatureInitializations::new(vec![a.clone(), b.clone()]).unwrap();
        let reordered = FeatureInitializations::new(vec![b.clone(), a]).unwrap();
        assert_eq!(
            serde_json::to_vec(&first).unwrap(),
            serde_json::to_vec(&reordered).unwrap()
        );
        let changed =
            FeatureInitialization::encode("test:a", 1, &json!({"ref":"changed"})).unwrap();
        assert_ne!(
            first,
            FeatureInitializations::new(vec![changed, b]).unwrap()
        );
    }
}

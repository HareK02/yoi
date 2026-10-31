//! Bounded wire handling. A validated receiver value is still not a disclosure capability.
use serde::{Deserialize, Serialize};
use std::{io::Write, marker::PhantomData};

use crate::*;

/// Both the transport limit and the remaining enclosing-envelope budget are mandatory.
#[derive(Debug, Clone, Copy)]
pub struct Budget {
    bytes: usize,
    depth: usize,
}
impl Budget {
    pub fn new(transport_limit: usize, remaining_envelope_bytes: usize) -> Self {
        Self {
            bytes: transport_limit
                .min(remaining_envelope_bytes)
                .min(MAX_FAILURE_BYTES),
            depth: MAX_JSON_DEPTH,
        }
    }
    /// Use the smaller transport depth and the remaining depth after enclosing wrappers.
    /// A JSON-string embedding must additionally budget its escaped bytes at that boundary.
    pub fn with_depth(mut self, transport_depth: usize, remaining_envelope_depth: usize) -> Self {
        self.depth = self
            .depth
            .min(transport_depth)
            .min(remaining_envelope_depth);
        self
    }
    pub fn bytes(self) -> usize {
        self.bytes
    }
    pub fn depth(self) -> usize {
        self.depth
    }
}

/// DTO/schema source only. Deserialization does not produce a ProjectedFailure.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct FailureEnvelope<D> {
    #[cfg_attr(feature = "json-schema", schemars(range(min = 1)))]
    pub version: u16,
    #[cfg_attr(feature = "json-schema", schemars(length(max = 8)))]
    pub required_extensions: Vec<String>,
    #[cfg_attr(feature = "json-schema", schemars(length(min = 1, max = 128)))]
    pub domain: String,
    #[cfg_attr(feature = "json-schema", schemars(length(min = 1, max = 128)))]
    pub reason: String,
    pub category: Category,
    #[cfg_attr(feature = "json-schema", schemars(range(min = 1)))]
    pub details_version: u16,
    pub details: D,
    #[cfg_attr(feature = "json-schema", schemars(length(min = 1, max = 2048)))]
    pub explanation: String,
    pub context: FailureContext,
}

// A tolerant reader header keeps future category spellings opaque. Public schema/TS authority
// stays on FailureEnvelope; only supported identities are deserialized into its typed shape.
#[derive(Deserialize)]
struct ReaderEnvelope {
    version: u16,
    required_extensions: Vec<String>,
    domain: String,
    reason: String,
    category: String,
    details_version: u16,
    #[serde(rename = "details")]
    _details: serde_json::Value,
    explanation: String,
    context: FailureContext,
}

pub struct ReceivedFailure<D> {
    facts: FailureFacts<D>,
}
impl<D: FailureCause> ReceivedFailure<D> {
    /// Reprojection with current authorization is required before any encoder can use this.
    pub fn facts(&self) -> &FailureFacts<D> {
        &self.facts
    }
}

/// Unknown bytes are retained opaquely, not serializable or available for blind forwarding.
/// Readers may use safe local stage/correlation separately; no reason inference is provided.
pub struct UnsupportedFailure {
    payload: Vec<u8>,
}
impl UnsupportedFailure {
    pub fn retained_bytes(&self) -> usize {
        self.payload.len()
    }
}
pub enum DecodeFailure<D> {
    Known(ReceivedFailure<D>),
    Unsupported(UnsupportedFailure),
    Corrupt(ContractError),
}

pub fn decode_failure<D: WireDetails>(input: &[u8], budget: Budget) -> DecodeFailure<D> {
    match decode_checked(input, budget) {
        Ok(value) => value,
        Err(error) => DecodeFailure::Corrupt(error),
    }
}

pub(crate) fn decode_checked<D: WireDetails>(
    input: &[u8],
    budget: Budget,
) -> Result<DecodeFailure<D>, ContractError> {
    check_json_bounds(input, budget)?;
    let raw: ReaderEnvelope = serde_json::from_slice(input).map_err(|_| ContractError::Invalid)?;
    validate_envelope_common(&raw)?;
    if raw.version != SCHEMA_VERSION
        || !raw.required_extensions.is_empty()
        || !D::supports(&raw.domain, &raw.reason, raw.details_version)
    {
        return Ok(DecodeFailure::Unsupported(UnsupportedFailure {
            payload: input.to_vec(),
        }));
    }
    // Deserialize from the original bytes again, not a Value: typed duplicate fields must fail.
    let typed: FailureEnvelope<D> =
        serde_json::from_slice(input).map_err(|_| ContractError::InvalidDetails)?;
    accept_typed_envelope(typed).map(DecodeFailure::Known)
}

pub(crate) fn accept_typed_envelope<D: WireDetails>(
    typed: FailureEnvelope<D>,
) -> Result<ReceivedFailure<D>, ContractError> {
    let identity = typed.details.identity();
    if identity.domain != typed.domain
        || identity.reason != typed.reason
        || identity.category != typed.category
        || identity.details_version != typed.details_version
    {
        return Err(ContractError::InvalidDetails);
    }
    typed.details.validate()?;
    let facts = FailureFacts::from_received(typed.details, typed.context);
    validate_failure(&facts)?;
    Ok(ReceivedFailure { facts })
}

fn validate_envelope_common(raw: &ReaderEnvelope) -> Result<(), ContractError> {
    if raw.version == 0 || raw.details_version == 0 {
        return Err(ContractError::Invalid);
    }
    validate_identifier(&raw.domain)?;
    if raw.required_extensions.len() > MAX_DIAGNOSTICS {
        return Err(ContractError::Invalid);
    }
    for extension in &raw.required_extensions {
        validate_identifier(extension)?;
    }
    validate_identifier(&raw.reason)?;
    validate_identifier(&raw.category)?;
    validate_explanation(&raw.explanation)?;
    raw.context.validate()
}

/// Linear lexical depth guard runs before serde allocates any JSON tree. Serde validates grammar.
pub(crate) fn check_json_bounds(input: &[u8], budget: Budget) -> Result<(), ContractError> {
    if input.len() > budget.bytes() {
        return Err(ContractError::TooLarge);
    }
    let (mut depth, mut in_string, mut escaped) = (0usize, false, false);
    for &byte in input {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
        } else {
            match byte {
                b'"' => in_string = true,
                b'{' | b'[' => {
                    depth += 1;
                    if depth > budget.depth() {
                        return Err(ContractError::TooDeep);
                    }
                }
                b'}' | b']' => {
                    depth = depth.checked_sub(1).ok_or(ContractError::Invalid)?;
                }
                _ => {}
            }
        }
    }
    Ok(())
}

struct BoundedWriter {
    bytes: Vec<u8>,
    budget: usize,
    exceeded: bool,
}
impl Write for BoundedWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.budget.saturating_sub(self.bytes.len()) {
            self.exceeded = true;
            return Err(std::io::Error::other("failure byte budget"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
pub(crate) fn encode_bounded<T: Serialize>(
    value: &T,
    budget: Budget,
) -> Result<Vec<u8>, ContractError> {
    let mut writer = BoundedWriter {
        bytes: Vec::new(),
        budget: budget.bytes(),
        exceeded: false,
    };
    if serde_json::to_writer(&mut writer, value).is_err() {
        return Err(if writer.exceeded {
            ContractError::TooLarge
        } else {
            ContractError::Encoding
        });
    }
    check_json_bounds(&writer.bytes, budget)?;
    Ok(writer.bytes)
}

/// The audience remains present until the transport deliberately takes the bytes.
pub struct EncodedFailure<A> {
    bytes: Vec<u8>,
    audience: PhantomData<A>,
}
impl<A: Audience> EncodedFailure<A> {
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
}

pub fn encode_projected_failure<A: Audience, D: WireDetails>(
    failure: &ProjectedFailure<A, D>,
    budget: Budget,
) -> Result<EncodedFailure<A>, ContractError> {
    let bytes = encode_bounded(&failure.envelope, budget)?;
    Ok(EncodedFailure {
        bytes,
        audience: PhantomData,
    })
}

/// Deliberately not a FailureEnvelope: it reports inability to disclose, not a new domain cause.
pub const STATIC_FALLBACK: &[u8] =
    br#"{"failure_unavailable":true,"detail":"Failure details unavailable."}"#;

pub struct BoundaryEncoding<A> {
    pub encoded: EncodedFailure<A>,
    pub conversion_error: Option<ContractError>,
    /// Kept separate from static payload; the boundary must allowlist it for disclosure.
    pub correlation: Option<CorrelationRef>,
}

/// No formatting of errors, recursive projection or operation invocation occurs on this path.
/// If even the static payload cannot fit, return a boundary error; never truncate required facts.
pub fn encode_or_fallback<A: Audience, D: WireDetails>(
    projection: Result<&ProjectedFailure<A, D>, ContractError>,
    budget: Budget,
    correlation: Option<CorrelationRef>,
) -> Result<BoundaryEncoding<A>, ContractError> {
    let result = projection.and_then(|failure| encode_projected_failure(failure, budget));
    let (encoded, conversion_error) = match result {
        Ok(encoded) => (encoded, None),
        Err(error) => {
            check_json_bounds(STATIC_FALLBACK, budget)?;
            (
                EncodedFailure {
                    bytes: STATIC_FALLBACK.to_vec(),
                    audience: PhantomData,
                },
                Some(error),
            )
        }
    };
    Ok(BoundaryEncoding {
        encoded,
        conversion_error,
        correlation,
    })
}

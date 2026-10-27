//! Portable public capture values. DTOs are transport data, not authorization handles.
//! Checked construction validates an explicitly public projection; adapters must never put
//! a provider System body or private Feature payload in `PublicSystem`.

use super::*;
use serde::de::Error as _;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::io::Write;

pub const CAPTURE_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureUsePolicy {
    pub recording_material: bool,
    pub memory_evidence: bool,
    pub observation_readable: bool,
}

impl CaptureUsePolicy {
    pub const PUBLIC: Self = Self {
        recording_material: true,
        memory_evidence: true,
        observation_readable: true,
    };
    pub fn any(self) -> bool {
        self.recording_material || self.memory_evidence || self.observation_readable
    }
    pub fn allows(self, purpose: CapturePurpose) -> bool {
        match purpose {
            CapturePurpose::Recording => self.recording_material,
            CapturePurpose::MemoryEvidence => self.memory_evidence,
            CapturePurpose::Observation => self.observation_readable,
        }
    }
}

impl From<crate::HistoryInputUsePolicy> for CaptureUsePolicy {
    fn from(policy: crate::HistoryInputUsePolicy) -> Self {
        Self {
            recording_material: policy.recording_material == crate::InputEvidenceUse::Include,
            memory_evidence: policy.memory_evidence == crate::InputEvidenceUse::Include,
            observation_readable: policy.observation == crate::InputObservationUse::Readable,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapturePurpose {
    Recording,
    MemoryEvidence,
    Observation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureRole {
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CaptureContentPart {
    Text { text: String },
    Refusal { refusal: String },
}

impl CaptureContentPart {
    fn text(&self) -> &str {
        match self {
            Self::Text { text } => text,
            Self::Refusal { refusal } => refusal,
        }
    }
}

/// The type has no reasoning, raw System prompt, Feature payload, or image bytes variant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CaptureBody {
    /// Canonical recording bytes retain the historical Journal descriptor contract.
    /// A nonzero offset or short text is an explicit fragment, never a full body.
    /// History entries retain their original raw segment-history row, including
    /// hidden rows in the count. A typed Host boundary has no history row: its
    /// `source.entry_index` is the next-history gap at the original boundary.
    CanonicalPublic {
        descriptor: crate::public_material::PublicMaterialDescriptor,
        byte_offset: u64,
        text: String,
        boundary: Option<crate::public_material::PublicRunBoundary>,
    },
    Message {
        role: CaptureRole,
        content: Vec<CaptureContentPart>,
    },
    PublicSystem {
        text: String,
    },
    ToolCall {
        call_id: String,
        name: String,
        arguments: String,
    },
    ToolResult {
        call_id: String,
        summary: String,
        content: Option<String>,
        is_error: bool,
        attachment_count: usize,
    },
}

impl CaptureBody {
    pub(super) fn project(
        item: &Item,
        origin: &SessionEntryProvenance,
        explicit: bool,
    ) -> Option<Self> {
        match item {
            Item::Message { role, content, .. } => {
                message_reference_kind(
                    origin,
                    role,
                    explicit.then_some(protocol::SessionEntryUsePolicy {
                        observation_readable: true,
                        memory_evidence: true,
                    }),
                )?;
                if *role == Role::System {
                    if !explicit {
                        return None;
                    }
                    return Some(Self::PublicSystem {
                        text: content.iter().map(|p| p.as_text()).collect::<String>(),
                    });
                }
                Some(Self::Message {
                    role: if *role == Role::User {
                        CaptureRole::User
                    } else {
                        CaptureRole::Assistant
                    },
                    content: content
                        .iter()
                        .map(|p| match p {
                            agen::ContentPart::Text { text } => {
                                CaptureContentPart::Text { text: text.clone() }
                            }
                            agen::ContentPart::Refusal { refusal } => CaptureContentPart::Refusal {
                                refusal: refusal.clone(),
                            },
                        })
                        .collect(),
                })
            }
            Item::ToolCall {
                call_id,
                name,
                arguments,
                ..
            } => Some(Self::ToolCall {
                call_id: call_id.clone(),
                name: name.clone(),
                arguments: arguments.clone(),
            }),
            Item::ToolResult {
                call_id,
                summary,
                content,
                is_error,
                attachments,
                ..
            } => Some(Self::ToolResult {
                call_id: call_id.clone(),
                summary: summary.clone(),
                content: content.clone(),
                is_error: *is_error,
                attachment_count: attachments.len(),
            }),
            Item::Reasoning { .. } | Item::AssistantResponseBoundary { .. } => None,
        }
    }

    pub(super) fn to_item(&self) -> Item {
        match self {
            Self::CanonicalPublic { text, .. } => Item::system_message(text.clone()),
            Self::Message { role, content } => Item::Message {
                id: None,
                status: None,
                role: match role {
                    CaptureRole::User => Role::User,
                    CaptureRole::Assistant => Role::Assistant,
                },
                content: content
                    .iter()
                    .map(|p| match p {
                        CaptureContentPart::Text { text } => {
                            agen::ContentPart::Text { text: text.clone() }
                        }
                        CaptureContentPart::Refusal { refusal } => agen::ContentPart::Refusal {
                            refusal: refusal.clone(),
                        },
                    })
                    .collect(),
            },
            Self::PublicSystem { text } => Item::system_message(text.clone()),
            Self::ToolCall {
                call_id,
                name,
                arguments,
            } => Item::tool_call(call_id.clone(), name.clone(), arguments.clone()),
            Self::ToolResult {
                call_id,
                summary,
                content,
                is_error,
                ..
            } => {
                Item::tool_result_item(call_id.clone(), summary.clone(), content.clone(), *is_error)
            }
        }
    }

    fn fields(&self) -> Vec<(RawField, &str)> {
        match self {
            Self::CanonicalPublic { text, .. } => vec![(RawField::CanonicalPublic, text)],
            Self::Message { content, .. } => content
                .iter()
                .enumerate()
                .map(|(i, p)| (RawField::MessagePart(i), p.text()))
                .collect(),
            Self::PublicSystem { text } => vec![(RawField::PublicSystem, text)],
            Self::ToolCall { arguments, .. } => vec![(RawField::ToolArguments, arguments)],
            Self::ToolResult {
                summary, content, ..
            } => {
                let mut fields = vec![(RawField::ToolSummary, summary.as_str())];
                if let Some(content) = content {
                    fields.push((RawField::ToolContent, content));
                }
                fields
            }
        }
    }

    fn metadata(&self, field: RawField) -> CaptureBodyMetadata {
        match self {
            Self::CanonicalPublic {
                descriptor,
                byte_offset,
                boundary,
                ..
            } => CaptureBodyMetadata::CanonicalPublic {
                descriptor: descriptor.clone(),
                byte_offset: *byte_offset,
                boundary: *boundary,
            },
            Self::Message { role, content } => CaptureBodyMetadata::Message {
                role: *role,
                refusal: matches!(field, RawField::MessagePart(i) if matches!(content.get(i), Some(CaptureContentPart::Refusal { .. }))),
            },
            Self::PublicSystem { .. } => CaptureBodyMetadata::PublicSystem,
            Self::ToolCall { call_id, name, .. } => CaptureBodyMetadata::ToolCall {
                call_id: call_id.clone(),
                name: name.clone(),
            },
            Self::ToolResult {
                call_id,
                content,
                is_error,
                attachment_count,
                ..
            } => CaptureBodyMetadata::ToolResult {
                call_id: call_id.clone(),
                has_content: content.is_some(),
                is_error: *is_error,
                attachment_count: *attachment_count,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureSource {
    pub segment_id: String,
    /// Original raw segment-history position, not captured/public view order.
    /// For `CanonicalPublic` Host boundaries only, this is the next-history gap;
    /// several typed Host events may share that gap with the following history row.
    pub entry_index: u64,
    pub provenance: SessionEntryProvenance,
    /// Full original origin when supplied by the committed-history adapter.
    #[serde(deserialize_with = "strict_origin")]
    pub history_origin: Option<crate::LoggedSessionHistoryOrigin>,
    pub derived_from: Vec<String>,
}

// Existing history origin types predate strict DTOs. Comparing their canonical value
// rejects unknown nested keys instead of silently accepting/losing them at this boundary.
fn strict_origin<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<crate::LoggedSessionHistoryOrigin>, D::Error> {
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    value
        .map(|value| {
            let origin: crate::LoggedSessionHistoryOrigin =
                serde_json::from_value(value.clone()).map_err(D::Error::custom)?;
            if serde_json::to_value(&origin).map_err(D::Error::custom)? != value {
                return Err(D::Error::custom("noncanonical or unknown origin fields"));
            }
            Ok(origin)
        })
        .transpose()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureEntryDto {
    pub entry_id: String,
    /// Position in this captured view, independent of the original segment position.
    pub order: u64,
    pub source: CaptureSource,
    pub policy: CaptureUsePolicy,
    pub body: CaptureBody,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureDto {
    pub version: u32,
    pub session_id: String,
    pub segment_id: String,
    /// Extent of view order, including positions omitted by public projection.
    pub view_entry_count: u64,
    pub entries: Vec<CaptureEntryDto>,
}

#[derive(Debug, Clone, Copy)]
pub struct CaptureLimits {
    pub max_bytes: usize,
    pub max_entries: usize,
    pub max_text_bytes: usize,
    pub max_identity_bytes: usize,
    pub max_derivation_sources: usize,
}

impl Default for CaptureLimits {
    fn default() -> Self {
        Self {
            max_bytes: 16 * 1024 * 1024,
            max_entries: 100_000,
            max_text_bytes: 8 * 1024 * 1024,
            max_identity_bytes: 4096,
            max_derivation_sources: 1024,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("unsupported capture representation version")]
    UnsupportedVersion,
    #[error("capture exceeds caller resource limits")]
    ResourceLimit,
    #[error("invalid capture: {0}")]
    Invalid(&'static str),
    #[error("reference or cursor belongs to another capture/session/purpose")]
    ForeignReference,
    #[error("unknown entry reference")]
    UnknownReference,
    #[error("entry is not eligible for this purpose")]
    PolicyDenied,
    #[error("invalid entry range or UTF-8 byte offset")]
    InvalidRange,
    #[error("budget cannot fit the next item and continuation metadata")]
    BudgetTooSmall,
    #[error("invalid capture JSON: {0}")]
    Json(#[from] serde_json::Error),
}

struct ByteCounter {
    bytes: usize,
    limit: usize,
}
impl Write for ByteCounter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.bytes = self
            .bytes
            .checked_add(buf.len())
            .ok_or_else(|| std::io::Error::other("capture byte limit"))?;
        if self.bytes > self.limit {
            return Err(std::io::Error::other("capture byte limit"));
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub(super) fn json_size<T: Serialize>(value: &T, limit: usize) -> Result<usize, CaptureError> {
    let mut counter = ByteCounter { bytes: 0, limit };
    serde_json::to_writer(&mut counter, value).map_err(|_| CaptureError::ResourceLimit)?;
    Ok(counter.bytes)
}

fn identity(value: &str, limits: CaptureLimits) -> Result<(), CaptureError> {
    if value.is_empty()
        || value.len() > limits.max_identity_bytes
        || value.chars().any(char::is_control)
    {
        return Err(CaptureError::Invalid("identity"));
    }
    Ok(())
}

impl CaptureDto {
    pub fn validate(&self, limits: CaptureLimits) -> Result<(), CaptureError> {
        if self.version != CAPTURE_VERSION {
            return Err(CaptureError::UnsupportedVersion);
        }
        if self.entries.len() > limits.max_entries {
            return Err(CaptureError::ResourceLimit);
        }
        identity(&self.session_id, limits)?;
        identity(&self.segment_id, limits)?;
        let mut ids = HashSet::new();
        let mut sources = HashSet::new();
        let mut recording_source_floor = HashMap::<&str, u64>::new();
        let mut previous = None;
        for entry in &self.entries {
            identity(&entry.entry_id, limits)?;
            if SessionEntryRef::parse(&format!("E{}", entry.entry_id)).is_none()
                || !ids.insert(&entry.entry_id)
            {
                return Err(CaptureError::Invalid("entry identity"));
            }
            if entry.order >= self.view_entry_count {
                return Err(CaptureError::Invalid("entry outside view extent"));
            }
            if previous.is_some_and(|order| order >= entry.order) {
                return Err(CaptureError::Invalid("entry order"));
            }
            previous = Some(entry.order);
            identity(&entry.source.segment_id, limits)?;
            let host_boundary = matches!(
                &entry.body,
                CaptureBody::CanonicalPublic {
                    boundary: Some(_),
                    ..
                }
            );
            if !host_boundary
                && !sources.insert((&entry.source.segment_id, entry.source.entry_index))
            {
                return Err(CaptureError::Invalid("duplicate source position"));
            }
            if !entry.policy.any() {
                return Err(CaptureError::Invalid("unusable retained entry"));
            }
            if entry.source.derived_from.len() > limits.max_derivation_sources {
                return Err(CaptureError::ResourceLimit);
            }
            for source in &entry.source.derived_from {
                identity(source, limits)?;
            }
            if let Some(origin) = &entry.source.history_origin {
                if public_provenance(origin) != entry.source.provenance {
                    return Err(CaptureError::Invalid("origin mismatch"));
                }
                json_size(origin, limits.max_text_bytes)?;
            }
            for (_, text) in entry.body.fields() {
                if text.len() > limits.max_text_bytes {
                    return Err(CaptureError::ResourceLimit);
                }
            }
            match &entry.body {
                CaptureBody::CanonicalPublic {
                    descriptor,
                    byte_offset,
                    text,
                    boundary,
                } => {
                    if descriptor.identity.entry_ref != format!("E{}", entry.entry_id)
                        || descriptor.identity.segment_id.to_string() != entry.source.segment_id
                        || descriptor.canonical_order != entry.order
                        || text.is_empty()
                        || descriptor.total_bytes == 0
                        || byte_offset
                            .checked_add(text.len() as u64)
                            .is_none_or(|end| end > descriptor.total_bytes)
                        || descriptor.sha256.len() != 64
                        || !descriptor
                            .sha256
                            .bytes()
                            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                        || !entry.policy.recording_material
                        || entry.policy.memory_evidence
                        || entry.policy.observation_readable
                    {
                        return Err(CaptureError::Invalid("canonical descriptor"));
                    }
                    match (&descriptor.origin, &entry.source.history_origin, boundary) {
                        (
                            crate::public_material::PublicMaterialOrigin::History { origin },
                            Some(stored),
                            None,
                        ) if origin == stored => {}
                        (
                            crate::public_material::PublicMaterialOrigin::HostRunBoundary,
                            None,
                            Some(_),
                        ) => {}
                        _ => return Err(CaptureError::Invalid("canonical origin")),
                    }
                    // A partial DTO cannot prove the hidden history extent, but can
                    // reject aliases and reversals of original per-Segment positions.
                    // Host events anchor a gap, not a fabricated history row.
                    let floor = recording_source_floor
                        .entry(&entry.source.segment_id)
                        .or_default();
                    if entry.source.entry_index < *floor {
                        return Err(CaptureError::Invalid("recording source position order"));
                    }
                    *floor = if boundary.is_some() {
                        entry.source.entry_index
                    } else {
                        entry
                            .source
                            .entry_index
                            .checked_add(1)
                            .ok_or(CaptureError::Invalid("source position overflow"))?
                    };
                    if *byte_offset == 0 && text.len() as u64 == descriptor.total_bytes {
                        let digest: String = Sha256::digest(text.as_bytes())
                            .iter()
                            .map(|b| format!("{b:02x}"))
                            .collect();
                        if digest != descriptor.sha256 {
                            return Err(CaptureError::Invalid("canonical digest"));
                        }
                        if let Some(boundary) = boundary {
                            let expected =
                                serde_json::json!({"kind":"run_boundary", "boundary":boundary});
                            if serde_json::from_str::<serde_json::Value>(text)? != expected {
                                return Err(CaptureError::Invalid("run boundary body"));
                            }
                        }
                    }
                }
                CaptureBody::Message { content, .. } if content.is_empty() => {
                    return Err(CaptureError::Invalid("empty message parts"));
                }
                CaptureBody::ToolCall { call_id, name, .. } => {
                    identity(call_id, limits)?;
                    identity(name, limits)?;
                }
                CaptureBody::ToolResult { call_id, .. } => identity(call_id, limits)?,
                _ => {}
            }
        }
        json_size(self, limits.max_bytes)?;
        Ok(())
    }

    pub fn encode(&self, limits: CaptureLimits) -> Result<Vec<u8>, CaptureError> {
        self.validate(limits)?;
        Ok(serde_json::to_vec(self)?)
    }

    pub fn decode(bytes: &[u8], limits: CaptureLimits) -> Result<Self, CaptureError> {
        if bytes.len() > limits.max_bytes {
            return Err(CaptureError::ResourceLimit);
        }
        let dto: Self = serde_json::from_slice(bytes)?;
        dto.validate(limits)?;
        Ok(dto)
    }

    pub(super) fn fingerprint(&self) -> String {
        // These DTOs have only finite, infallibly serializable JSON primitives.
        let bytes = serde_json::to_vec(self).expect("capture DTO serialization");
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScopedSessionEntryRef {
    pub session_id: String,
    pub capture_id: String,
    pub entry_ref: SessionEntryRef,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RawField {
    CanonicalPublic,
    MessagePart(usize),
    PublicSystem,
    ToolArguments,
    ToolSummary,
    ToolContent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawRead {
    pub reference: ScopedSessionEntryRef,
    pub purpose: CapturePurpose,
    pub field: RawField,
    pub offset: usize,
    pub text: String,
    pub next_offset: Option<usize>,
    pub total_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CaptureBodyMetadata {
    CanonicalPublic {
        descriptor: crate::public_material::PublicMaterialDescriptor,
        byte_offset: u64,
        boundary: Option<crate::public_material::PublicRunBoundary>,
    },
    Message {
        role: CaptureRole,
        refusal: bool,
    },
    PublicSystem,
    ToolCall {
        call_id: String,
        name: String,
    },
    ToolResult {
        call_id: String,
        has_content: bool,
        is_error: bool,
        attachment_count: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureCursor {
    pub session_id: String,
    pub capture_id: String,
    pub purpose: CapturePurpose,
    pub entry_position: usize,
    pub field_index: usize,
    pub byte_offset: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureSliceItem {
    pub reference: ScopedSessionEntryRef,
    pub order: u64,
    pub source: CaptureSource,
    pub policy: CaptureUsePolicy,
    pub metadata: CaptureBodyMetadata,
    pub field: RawField,
    pub offset: usize,
    pub total_bytes: usize,
    /// Raw source bytes, not a rendered preview or ellipsis-bearing truncation.
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureSlice {
    pub version: u32,
    pub session_id: String,
    pub capture_id: String,
    pub purpose: CapturePurpose,
    pub items: Vec<CaptureSliceItem>,
    pub next_cursor: Option<CaptureCursor>,
}

/// Bytes and value remain paired and immutable so the size proof cannot go stale.
#[derive(Debug, Clone)]
pub struct BudgetedCaptureSlice {
    slice: CaptureSlice,
    json: Vec<u8>,
}
impl BudgetedCaptureSlice {
    pub fn slice(&self) -> &CaptureSlice {
        &self.slice
    }
    pub fn as_json(&self) -> &[u8] {
        &self.json
    }
    pub fn encoded_len(&self) -> usize {
        self.json.len()
    }
}

impl SessionCapture {
    pub fn session_id(&self) -> &str {
        &self.session_id
    }
    pub fn segment_id(&self) -> &str {
        &self.segment_id
    }
    pub fn capture_id(&self) -> &str {
        &self.capture_id
    }

    pub fn to_dto(&self) -> CaptureDto {
        CaptureDto {
            version: CAPTURE_VERSION,
            session_id: self.session_id.clone(),
            segment_id: self.segment_id.clone(),
            view_entry_count: self.view_entry_count,
            entries: self.entries.iter().map(|e| e.dto.clone()).collect(),
        }
    }

    pub fn try_from_dto(dto: CaptureDto, limits: CaptureLimits) -> Result<Self, CaptureError> {
        dto.validate(limits)?;
        Ok(Self::from_projected(
            dto.session_id,
            dto.segment_id,
            dto.view_entry_count,
            dto.entries,
        ))
    }

    pub fn encode(&self, limits: CaptureLimits) -> Result<Vec<u8>, CaptureError> {
        self.to_dto().encode(limits)
    }
    pub fn decode(bytes: &[u8], limits: CaptureLimits) -> Result<Self, CaptureError> {
        Self::try_from_dto(CaptureDto::decode(bytes, limits)?, limits)
    }

    /// Select fixed view positions. UI order is rebased, while every original
    /// source identity/position, body, provenance and policy stays unchanged.
    pub fn select_order_range(&self, range: std::ops::Range<u64>) -> Result<Self, CaptureError> {
        if range.start > range.end || range.end > self.view_entry_count {
            return Err(CaptureError::InvalidRange);
        }
        let entries = self
            .entries()
            .filter(|entry| range.contains(&entry.order))
            .map(|entry| {
                let mut entry = entry.clone();
                entry.order -= range.start;
                entry
            })
            .collect();
        Ok(Self::from_projected(
            self.session_id.clone(),
            self.segment_id.clone(),
            range.end - range.start,
            entries,
        ))
    }

    /// Host-owned target selection may exclude entries without renumbering either
    /// the fixed view or its original sources. No source I/O or business policy is applied.
    pub fn retain_entries(&self, mut include: impl FnMut(&CaptureEntryDto) -> bool) -> Self {
        Self::from_projected(
            self.session_id.clone(),
            self.segment_id.clone(),
            self.view_entry_count,
            self.entries()
                .filter(|entry| include(entry))
                .cloned()
                .collect(),
        )
    }

    pub fn scoped_ref(
        &self,
        reference: &SessionEntryRef,
    ) -> Result<ScopedSessionEntryRef, CaptureError> {
        if self.view_order_for_ref(reference).is_none() {
            return Err(CaptureError::UnknownReference);
        }
        Ok(ScopedSessionEntryRef {
            session_id: self.session_id.clone(),
            capture_id: self.capture_id.clone(),
            entry_ref: reference.clone(),
        })
    }

    pub fn resolve_ref(
        &self,
        reference: &ScopedSessionEntryRef,
        purpose: CapturePurpose,
    ) -> Result<&CaptureEntryDto, CaptureError> {
        if reference.session_id != self.session_id || reference.capture_id != self.capture_id {
            return Err(CaptureError::ForeignReference);
        }
        let entry = self
            .entries
            .iter()
            .find(|entry| {
                SessionEntryRef::from_history_entry_id(&entry.entry_id) == reference.entry_ref
            })
            .ok_or(CaptureError::UnknownReference)?;
        if !entry.dto.policy.allows(purpose) {
            return Err(CaptureError::PolicyDenied);
        }
        Ok(&entry.dto)
    }

    pub fn checked_range(
        &self,
        from: &ScopedSessionEntryRef,
        through: &ScopedSessionEntryRef,
        purpose: CapturePurpose,
    ) -> Result<Vec<&CaptureEntryDto>, CaptureError> {
        let start = self.resolve_ref(from, purpose)?.order;
        let end = self.resolve_ref(through, purpose)?.order;
        if start > end {
            return Err(CaptureError::InvalidRange);
        }
        Ok(self
            .entries
            .iter()
            .map(|entry| &entry.dto)
            .filter(|entry| {
                entry.order >= start && entry.order <= end && entry.policy.allows(purpose)
            })
            .collect())
    }

    pub(super) fn validate_search(&self, options: &SearchOptions) -> Result<(), CaptureError> {
        let from = options
            .from
            .as_ref()
            .map(|r| {
                self.view_order_for_ref(r)
                    .ok_or(CaptureError::UnknownReference)
            })
            .transpose()?;
        let through = options
            .through
            .as_ref()
            .map(|r| {
                self.view_order_for_ref(r)
                    .ok_or(CaptureError::UnknownReference)
            })
            .transpose()?;
        if from.unwrap_or(options.min_entry_index.unwrap_or(0)) > through.unwrap_or(u64::MAX) {
            return Err(CaptureError::InvalidRange);
        }
        Ok(())
    }

    pub fn search_checked(&self, options: &SearchOptions) -> Result<Vec<SearchHit>, CaptureError> {
        self.validate_search(options)?;
        Ok(self.search(options))
    }

    pub fn read_checked(
        &self,
        selector: ReadSelector<'_>,
        options: ReadOptions,
    ) -> Result<ReadResult, CaptureError> {
        match selector {
            ReadSelector::Id(id) => {
                let reference = SessionEntryRef::parse(id).ok_or(CaptureError::UnknownReference)?;
                self.view_order_for_ref(&reference)
                    .ok_or(CaptureError::UnknownReference)?;
            }
            ReadSelector::EntryRange([start, end]) => {
                if start > end
                    || !self.index.iter().any(|entry| entry.entry_range[0] == start)
                    || !self.index.iter().any(|entry| entry.entry_range[0] == end)
                {
                    return Err(CaptureError::InvalidRange);
                }
            }
        }
        Ok(self.read(selector, options))
    }

    pub fn read_raw(
        &self,
        reference: &ScopedSessionEntryRef,
        purpose: CapturePurpose,
        field: RawField,
        offset: usize,
        max_bytes: usize,
    ) -> Result<RawRead, CaptureError> {
        let entry = self.resolve_ref(reference, purpose)?;
        let text = entry
            .body
            .fields()
            .into_iter()
            .find(|(f, _)| *f == field)
            .map(|(_, text)| text)
            .ok_or(CaptureError::InvalidRange)?;
        if offset > text.len() || !text.is_char_boundary(offset) {
            return Err(CaptureError::InvalidRange);
        }
        let mut end = offset.saturating_add(max_bytes).min(text.len());
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        if end == offset && offset < text.len() {
            return Err(CaptureError::BudgetTooSmall);
        }
        Ok(RawRead {
            reference: reference.clone(),
            purpose,
            field,
            offset,
            text: text[offset..end].to_owned(),
            next_offset: (end < text.len()).then_some(end),
            total_bytes: text.len(),
        })
    }

    fn cursor(
        &self,
        purpose: CapturePurpose,
        entry_position: usize,
        field_index: usize,
        byte_offset: usize,
    ) -> CaptureCursor {
        CaptureCursor {
            session_id: self.session_id.clone(),
            capture_id: self.capture_id.clone(),
            purpose,
            entry_position,
            field_index,
            byte_offset,
        }
    }

    fn normalize_cursor(&self, mut cursor: CaptureCursor) -> Option<CaptureCursor> {
        while let Some(entry) = self.entries.get(cursor.entry_position) {
            if entry.dto.policy.allows(cursor.purpose)
                && cursor.field_index < entry.dto.body.fields().len()
            {
                return Some(cursor);
            }
            cursor.entry_position += 1;
            cursor.field_index = 0;
            cursor.byte_offset = 0;
        }
        None
    }

    /// The returned JSON includes every item's source/policy, escaped body, and continuation.
    /// Both the item count and the exact encoded length are bounded, with no Job-specific limit.
    pub fn slice(
        &self,
        purpose: CapturePurpose,
        cursor: Option<&CaptureCursor>,
        max_items: usize,
        max_bytes: usize,
    ) -> Result<BudgetedCaptureSlice, CaptureError> {
        if max_items == 0 {
            return Err(CaptureError::BudgetTooSmall);
        }
        let start = if let Some(cursor) = cursor {
            if cursor.session_id != self.session_id
                || cursor.capture_id != self.capture_id
                || cursor.purpose != purpose
            {
                return Err(CaptureError::ForeignReference);
            }
            let entry = self
                .entries
                .get(cursor.entry_position)
                .ok_or(CaptureError::InvalidRange)?;
            if !entry.dto.policy.allows(purpose) {
                return Err(CaptureError::PolicyDenied);
            }
            let fields = entry.dto.body.fields();
            let text = fields
                .get(cursor.field_index)
                .ok_or(CaptureError::InvalidRange)?
                .1;
            if cursor.byte_offset > text.len() || !text.is_char_boundary(cursor.byte_offset) {
                return Err(CaptureError::InvalidRange);
            }
            Some(cursor.clone())
        } else {
            self.normalize_cursor(self.cursor(purpose, 0, 0, 0))
        };
        let mut slice = CaptureSlice {
            version: CAPTURE_VERSION,
            session_id: self.session_id.clone(),
            capture_id: self.capture_id.clone(),
            purpose,
            items: Vec::new(),
            next_cursor: start,
        };
        if json_size(&slice, max_bytes).is_err() {
            return Err(CaptureError::BudgetTooSmall);
        }
        while slice.items.len() < max_items {
            let Some(current) = slice.next_cursor.clone() else {
                break;
            };
            let dto = &self.entries[current.entry_position].dto;
            let fields = dto.body.fields();
            let (field, text) = fields[current.field_index];
            let reference = ScopedSessionEntryRef {
                session_id: self.session_id.clone(),
                capture_id: self.capture_id.clone(),
                entry_ref: SessionEntryRef(format!("E{}", dto.entry_id)),
            };
            let item = CaptureSliceItem {
                reference,
                order: dto.order,
                source: dto.source.clone(),
                policy: dto.policy,
                metadata: dto.body.metadata(field),
                field,
                offset: current.byte_offset,
                total_bytes: text.len(),
                text: String::new(),
            };
            slice.items.push(item);
            let item_index = slice.items.len() - 1;
            // Search only the partial interval: its cursor stays in the same field,
            // making encoded size monotonic even when completing a field drops metadata.
            let remaining = &text[current.byte_offset..];
            let mut candidate_bytes = remaining.len().min(max_bytes);
            while !remaining.is_char_boundary(candidate_bytes) {
                candidate_bytes -= 1;
            }
            let mut boundaries: Vec<usize> = remaining[..candidate_bytes]
                .char_indices()
                .map(|(i, _)| i)
                .collect();
            boundaries.push(candidate_bytes);
            let mut accepted = None;
            let mut low = 0;
            let mut high = boundaries.len();
            while low < high {
                let mid = low + (high - low) / 2;
                let end = current.byte_offset + boundaries[mid];
                slice.items[item_index].text = text[current.byte_offset..end].to_owned();
                slice.next_cursor =
                    Some(self.cursor(purpose, current.entry_position, current.field_index, end));
                if json_size(&slice, max_bytes).is_ok() {
                    accepted = Some(end);
                    low = mid + 1;
                } else {
                    high = mid;
                }
            }
            // A completed field can fit even when its partial continuation does not.
            let completed = self.normalize_cursor(self.cursor(
                purpose,
                current.entry_position,
                current.field_index + 1,
                0,
            ));
            if remaining.len() <= max_bytes {
                slice.items[item_index].text = remaining.to_owned();
                slice.next_cursor = completed.clone();
                if json_size(&slice, max_bytes).is_ok() {
                    accepted = Some(text.len());
                }
            }
            let Some(end) = accepted.filter(|end| *end > current.byte_offset || text.is_empty())
            else {
                slice.items.pop();
                slice.next_cursor = Some(current);
                if slice.items.is_empty() {
                    return Err(CaptureError::BudgetTooSmall);
                }
                break;
            };
            slice.items[item_index].text = text[current.byte_offset..end].to_owned();
            slice.next_cursor = if end == text.len() {
                completed
            } else {
                Some(self.cursor(purpose, current.entry_position, current.field_index, end))
            };
        }
        let json = serde_json::to_vec(&slice)?;
        if json.len() > max_bytes {
            return Err(CaptureError::BudgetTooSmall);
        }
        Ok(BudgetedCaptureSlice { slice, json })
    }
}

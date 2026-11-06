//! Host-side construction of bounded Journal generation input.
//!
//! Admission and byte identity belong to the Session Host. Selection neither starts Jobs nor
//! treats previews, failed fetches, or missing bytes as reflected. Production readers verify
//! original canonical identity while streaming and return only the requested bounded fragment.

use super::*;
use session_store::capture::{CaptureBody, CaptureDto, CaptureLimits, RecordingCaptureSlice};
use sha2::{Digest, Sha256};

/// A Job owns only this bounded original-body prefix. The initial content condition binds
/// the separate generation snapshot (whose potentially large Journal text remains paged).
/// Neither possessing these bytes nor delivering them is a processing checkpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JournalInput {
    pub version: u32,
    pub subject_id: String,
    pub journal_id: String,
    pub initial_state_token: String,
    pub initial_open_id: Option<String>,
    pub previous_fixed_end: Option<JournalSourcePointer>,
    pub selected_ranges: Vec<JournalSourceRange>,
    pub capture: CaptureDto,
}

impl JournalInput {
    /// Candidate slice ceiling for admission before a Journal exists. This is the Job input
    /// contract, not a text-byte allowance or a guarantee that the eventual input fits.
    /// Capture metadata/continuation are charged by the reader; [`Self::build`] additionally
    /// charges the actual Journal header and duplicated ranges. The transport owner must
    /// separately bound its outer availability/response envelope.
    pub const fn capture_budget() -> usize {
        job::MAX_JOB_INPUT_BYTES
    }

    /// Capture callback runs once for this immutable Job, never once per page. Its budget is
    /// for the whole `RecordingCaptureSlice`, not just `CaptureDto` or canonical text. Fitting
    /// uses only that captured value and measures the complete serialized input, including the
    /// initial Journal condition and ranges duplicated beside the capture's descriptors.
    /// A shortened input must be followed from its saved checkpoint, not the reader's `next`.
    pub fn build(
        store: &SubjektivStore,
        host: &JournalHostContext,
        journal_id: &str,
        capture_once: impl FnOnce(u64, u64, usize) -> JournalResult<CaptureDto>,
    ) -> JournalResult<Self> {
        let (journal, open, previous_fixed_end) = store.database.try_with_connection(|tx| {
            super::store::check_host(store, tx, host, true)?;
            Ok::<_, JournalError>((
                super::store::journal(tx, host, journal_id, true)?,
                super::store::open(tx, journal_id)?,
                super::store::previous_fixed(tx, journal_id)?.map(|p| p.0),
            ))
        })?;
        let mut input = Self {
            version: 1,
            subject_id: journal.subject_id.clone(),
            journal_id: journal_id.into(),
            initial_state_token: journal.state_token,
            initial_open_id: open.map(|e| e.id),
            previous_fixed_end,
            selected_ranges: Vec::new(),
            capture: CaptureDto {
                version: 1,
                session_id: journal.source_session_id,
                segment_id: "pending".into(),
                view_entry_count: 0,
                entries: Vec::new(),
            },
        };
        // The fixed Job envelope excludes the capture placeholder. A slice wraps the capture
        // with `next`; even `null` occupies bytes. Use that minimum wrapper here, leaving the
        // reader to charge the actual continuation. Duplicated ranges depend on the returned
        // descriptors and are fitted below, not guessed from a text/metadata ratio.
        let capture_bytes = serde_json::to_vec(&input.capture)?.len();
        let envelope = serde_json::to_vec(&input)?.len() - capture_bytes;
        let slice_envelope = serde_json::to_vec(&RecordingCaptureSlice {
            capture: input.capture.clone(),
            next: None,
        })?
        .len()
            - capture_bytes;
        let budget = job::MAX_JOB_INPUT_BYTES
            .checked_sub(envelope)
            .and_then(|remaining| remaining.checked_add(slice_envelope))
            .ok_or_else(|| invalid("Journal envelope exceeds Job input limit"))?
            .min(Self::capture_budget());
        input.capture = capture_once(journal.process_order, journal.process_byte, budget)?;
        // Capture runs outside SQLite; a delayed callback cannot deliver deleted content.
        store
            .database
            .try_with_connection(|tx| super::store::check_host(store, tx, host, true))?;
        input.selected_ranges = capture_ranges(&input.capture)?;
        if input.selected_ranges.is_empty() {
            if serde_json::to_vec(&input)?.len() > job::MAX_JOB_INPUT_BYTES {
                return Err(invalid("empty Journal input exceeds Job input limit"));
            }
            return Ok(input);
        }
        input.fit()?;
        let value = serde_json::to_value(&input)?;
        Self::decode(&value)
    }

    fn fit(&mut self) -> JournalResult<()> {
        if self.selected_ranges.len() <= MAX_JOURNAL_RANGES
            && serde_json::to_vec(self)?.len() <= job::MAX_JOB_INPUT_BYTES
        {
            return Ok(());
        }
        let original = self.capture.clone();
        let ranges = self.selected_ranges.clone();
        let mut high = original
            .entries
            .iter()
            .take(MAX_JOURNAL_RANGES)
            .try_fold(0usize, |total, entry| match &entry.body {
                CaptureBody::CanonicalPublic { text, .. } => total.checked_add(text.len()),
                _ => None,
            })
            .ok_or_else(|| invalid("recording input length overflow"))?;
        let mut low = 0;
        // Search canonical prefix length, not a CaptureDto budget: a later entry's metadata
        // may be larger than all earlier bodies. Subtracting that excess from a slice budget
        // can falsely reject an earlier prefix that fits. Serialized prefix size is monotone
        // even across scalar/entry boundaries, including decimal range-end growth.
        while low < high {
            let mid = low + (high - low).div_ceil(2);
            self.select_prefix(&original, &ranges, mid);
            if serde_json::to_vec(self)?.len() <= job::MAX_JOB_INPUT_BYTES {
                low = mid;
            } else {
                high = mid - 1;
            }
        }
        self.select_prefix(&original, &ranges, low);
        if self.selected_ranges.is_empty() {
            return Err(invalid("Journal envelope cannot fit one source scalar"));
        }
        Ok(())
    }

    fn select_prefix(
        &mut self,
        original: &CaptureDto,
        ranges: &[JournalSourceRange],
        mut bytes: usize,
    ) {
        self.capture.entries.clear();
        self.selected_ranges.clear();
        for (entry, range) in original.entries.iter().zip(ranges).take(MAX_JOURNAL_RANGES) {
            let CaptureBody::CanonicalPublic { text, .. } = &entry.body else {
                unreachable!("capture_ranges checked the body representation");
            };
            let mut end = bytes.min(text.len());
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            if end == 0 {
                break;
            }
            let mut selected = entry.clone();
            if let CaptureBody::CanonicalPublic { text, .. } = &mut selected.body {
                text.truncate(end);
            }
            let mut range = range.clone();
            range.bytes.end = range.bytes.start + end as u64;
            self.capture.entries.push(selected);
            self.selected_ranges.push(range);
            if end < text.len() {
                break;
            }
            bytes -= end;
        }
    }

    pub fn decode(value: &serde_json::Value) -> JournalResult<Self> {
        if serde_json::to_vec(value)?.len() > job::MAX_JOB_INPUT_BYTES {
            return Err(invalid(
                "Journal input exceeds whole serialized Job input limit",
            ));
        }
        let input: Self = serde_json::from_value(value.clone())?;
        input
            .capture
            .validate(CaptureLimits {
                max_bytes: job::MAX_JOB_INPUT_BYTES,
                max_entries: MAX_JOURNAL_RANGES,
                max_text_bytes: job::MAX_JOB_INPUT_BYTES,
                ..CaptureLimits::default()
            })
            .map_err(|e| invalid(&e.to_string()))?;
        if input.version != 1
            || input.subject_id.is_empty()
            || input.journal_id.is_empty()
            || input.initial_state_token.is_empty()
            || input.selected_ranges.is_empty()
            || input.selected_ranges.len() > MAX_JOURNAL_RANGES
            || input
                .selected_ranges
                .iter()
                .any(|r| r.source.session_id != input.capture.session_id)
        {
            return Err(invalid("invalid Journal input identity or selection"));
        }
        if capture_ranges(&input.capture)? != input.selected_ranges {
            return Err(conflict(
                "Journal body snapshot does not match selected canonical ranges",
            ));
        }
        Ok(input)
    }

    pub fn validate_generation(&self, generation: &JournalGeneration) -> JournalResult<()> {
        if generation.journal_id != self.journal_id
            || generation.initial_journal.subject_id != self.subject_id
            || generation.initial_journal.source_session_id != self.capture.session_id
            || generation.initial_journal.state_token != self.initial_state_token
            || generation.initial_open.as_ref().map(|e| &e.id) != self.initial_open_id.as_ref()
            || generation.previous_fixed_end != self.previous_fixed_end
            || generation.selected_ranges != self.selected_ranges
        {
            return Err(conflict(
                "saved Journal input differs from its initial generation snapshot",
            ));
        }
        Ok(())
    }
}

/// Converts the shared recording projection into Journal's coverage language without
/// interpreting model text or constructing a second capture representation.
pub fn capture_ranges(capture: &CaptureDto) -> JournalResult<Vec<JournalSourceRange>> {
    use session_store::capture::CaptureBody;
    let mut ranges = Vec::new();
    for entry in &capture.entries {
        let CaptureBody::CanonicalPublic {
            descriptor,
            byte_offset,
            text,
            ..
        } = &entry.body
        else {
            return Err(invalid(
                "Journal requires canonical original recording capture",
            ));
        };
        if !entry.policy.recording_material
            || text.is_empty()
            || descriptor.identity.segment_id.to_string() != entry.source.segment_id
            || descriptor.identity.entry_ref != format!("E{}", entry.entry_id)
            || descriptor.canonical_order != entry.order
            || byte_offset
                .checked_add(text.len() as u64)
                .is_none_or(|end| end > descriptor.total_bytes)
        {
            return Err(conflict(
                "recording body identity, policy or range mismatch",
            ));
        }
        let source = JournalSource {
            session_id: capture.session_id.clone(),
            pointer: JournalSourcePointer {
                segment_id: descriptor.identity.segment_id,
                entry_ref: descriptor.identity.entry_ref.clone(),
            },
            order: descriptor.canonical_order,
            content_sha256: descriptor.sha256.clone(),
            total_bytes: descriptor.total_bytes,
        };
        if *byte_offset == 0 && text.len() as u64 == descriptor.total_bytes {
            verify_canonical_text(&source, text)?;
        }
        ranges.push(JournalSourceRange {
            source,
            bytes: JournalByteRange {
                start: *byte_offset,
                end: *byte_offset + text.len() as u64,
            },
        });
    }
    for pair in ranges.windows(2) {
        if pair[0].source.order.checked_add(1) != Some(pair[1].source.order)
            || pair[0].bytes.end != pair[0].source.total_bytes
            || pair[1].bytes.start != 0
        {
            return Err(conflict(
                "recording capture is not an oldest contiguous prefix",
            ));
        }
    }
    Ok(ranges)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JournalMaterial {
    pub range: JournalSourceRange,
    pub text: String,
}

/// Adapter for Host captures already bounded by their storage read contract. Filesystem/Runtime
/// callers should use [`select_materials_bounded`] rather than allocating a whole large entry.
pub fn select_materials(
    store: &SubjektivStore,
    host: &JournalHostContext,
    journal_id: &str,
    max_bytes: usize,
    mut read: impl FnMut(&JournalSource) -> JournalResult<String>,
) -> JournalResult<Vec<JournalMaterial>> {
    select_materials_bounded(
        store,
        host,
        journal_id,
        max_bytes,
        |source, start, limit| {
            let text = read(source)?;
            verify_canonical_text(source, &text)?;
            let start = usize::try_from(start).map_err(|_| invalid("source offset overflow"))?;
            if !text.is_char_boundary(start) {
                return Err(conflict("source offset is not UTF-8 aligned"));
            }
            let mut end = start.saturating_add(limit).min(text.len());
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            Ok(JournalMaterial {
                range: JournalSourceRange {
                    source: source.clone(),
                    bytes: JournalByteRange {
                        start: start as u64,
                        end: end as u64,
                    },
                },
                text: text[start..end].into(),
            })
        },
    )
}

fn invalid(message: &str) -> JournalError {
    JournalError::InvalidInput(message.into())
}
fn conflict(message: &str) -> JournalError {
    JournalError::Conflict(message.into())
}

/// Selects the oldest contiguous input under a whole serialized materials-array budget.
/// The Host callback verifies original public provenance/digest/length while streaming, and
/// returns at most `limit` canonical UTF-8 bytes at exactly `start`. Its descriptor must equal
/// the admitted source. This in-process attestation must never be deserialized from model input.
pub fn select_materials_bounded(
    store: &SubjektivStore,
    host: &JournalHostContext,
    journal_id: &str,
    max_bytes: usize,
    mut read: impl FnMut(&JournalSource, u64, usize) -> JournalResult<JournalMaterial>,
) -> JournalResult<Vec<JournalMaterial>> {
    let journal = store.read_journal(host, journal_id)?;
    let mut order = journal.process_order;
    let mut offset = journal.process_byte;
    let mut materials = Vec::new();
    let mut remaining = max_bytes.saturating_sub(2);
    while materials.len() < MAX_JOURNAL_RANGES {
        let status = match store.read_journal_source(host, journal_id, order) {
            Ok(status) => status,
            Err(JournalError::NotFound(_)) => break,
            Err(error) => return Err(error),
        };
        if offset == status.source.total_bytes {
            order = order
                .checked_add(1)
                .ok_or_else(|| invalid("source order exhausted"))?;
            offset = 0;
            continue;
        }
        let Some(accepted) = status
            .accepted
            .iter()
            .find(|r| r.start <= offset && offset < r.end)
        else {
            break;
        };
        let mut end = accepted.end;
        for unavailable in &status.unavailable {
            if unavailable.start <= offset && offset < unavailable.end {
                end = offset;
                break;
            }
            if offset < unavailable.start {
                end = end.min(unavailable.start);
            }
        }
        if end == offset {
            break;
        }
        let empty = JournalMaterial {
            range: JournalSourceRange {
                source: status.source.clone(),
                bytes: JournalByteRange {
                    start: offset,
                    end: offset,
                },
            },
            text: String::new(),
        };
        let envelope = serde_json::to_vec(&empty)?.len() + usize::from(!materials.is_empty());
        // Reserve decimal endpoint growth and enough room for one UTF-8 scalar.
        let allowance = remaining.saturating_sub(envelope.saturating_add(20));
        if allowance < 4 {
            if materials.is_empty() {
                return Err(invalid(
                    "input budget cannot fit source metadata and a UTF-8 scalar",
                ));
            }
            break;
        }
        let limit = allowance.min((end - offset).min(usize::MAX as u64) as usize);
        let mut material = read(&status.source, offset, limit)?;
        if material.range.source != status.source
            || material.range.bytes.start != offset
            || material.range.bytes.end != offset.saturating_add(material.text.len() as u64)
            || material.range.bytes.end > end
            || material.text.is_empty()
            || material.text.len() > limit
        {
            return Err(conflict(
                "Host source chunk does not match the requested canonical byte range",
            ));
        }
        let mut allowance = remaining - envelope;
        let mut selected_bytes = 0;
        for ch in material.text.chars() {
            let cost = match ch {
                '"' | '\\' | '\u{8}' | '\u{c}' | '\n' | '\r' | '\t' => 2,
                ch if ch < '\u{20}' => 6,
                ch => ch.len_utf8(),
            };
            if cost.saturating_add(20) > allowance {
                break;
            }
            allowance -= cost;
            selected_bytes += ch.len_utf8();
        }
        if selected_bytes == 0 {
            if materials.is_empty() {
                return Err(invalid("input budget cannot fit one escaped UTF-8 scalar"));
            }
            break;
        }
        material.text.truncate(selected_bytes);
        material.range.bytes.end = offset + selected_bytes as u64;
        let cost = serde_json::to_vec(&material)?.len() + usize::from(!materials.is_empty());
        if cost > remaining {
            return Err(invalid("generation input budget accounting failed"));
        }
        remaining -= cost;
        offset = material.range.bytes.end;
        materials.push(material);
        if offset < end {
            break;
        }
    }
    Ok(materials)
}

/// Verifies the full canonical public role/tool framing of an already bounded Host capture.
pub fn verify_canonical_text(source: &JournalSource, text: &str) -> JournalResult<()> {
    if text.len() as u64 != source.total_bytes
        || Sha256::digest(text.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
            != source.content_sha256
    {
        return Err(conflict("canonical public source bytes changed"));
    }
    Ok(())
}

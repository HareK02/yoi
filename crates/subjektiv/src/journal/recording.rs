//! Host-side construction of bounded Journal generation input.
//!
//! Admission and byte identity belong to the Session Host. Selection neither starts Jobs nor
//! treats previews, failed fetches, or missing bytes as reflected. Production readers verify
//! original canonical identity while streaming and return only the requested bounded fragment.

use super::*;
use sha2::{Digest, Sha256};

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

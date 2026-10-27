use super::*;
use crate::{issued_id, now};
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourcePageCursor {
    scope_id: String,
    subject_id: String,
    journal_id: String,
    source_fingerprint: String,
    after_order: u64,
}

// Hash rows incrementally rather than materializing an unbounded coverage ledger. The caller
// holds one storage transaction for both this content condition and the bounded page projection.
fn source_fingerprint(tx: &Connection, journal_id: &str) -> JournalResult<String> {
    let mut hash = Sha256::new();
    let mut stmt=tx.prepare("SELECT source_order, source_json, accepted_json, unavailable_json, reflected_bytes FROM journal_sources WHERE journal_id=?1 ORDER BY source_order")?;
    let mut rows = stmt.query([journal_id])?;
    while let Some(row) = rows.next()? {
        let value = (
            row.get::<_, u64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, u64>(4)?,
        );
        let bytes = serde_json::to_vec(&value)?;
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
    }
    Ok(hash.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

fn invalid(message: &str) -> JournalError {
    JournalError::InvalidInput(message.into())
}
fn conflict(message: &str) -> JournalError {
    JournalError::Conflict(message.into())
}
fn label(value: &str) -> JournalResult<()> {
    if value.is_empty()
        || value.len() > 128
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err(invalid(
            "identifier must be 1..128 bytes without whitespace or controls",
        ));
    }
    Ok(())
}
fn text(value: &str, maximum: usize) -> JournalResult<()> {
    if value.len() > maximum {
        return Err(invalid("text exceeds storage byte limit"));
    }
    Ok(())
}
fn hash<T: Serialize>(value: &T) -> JournalResult<String> {
    Ok(Sha256::digest(serde_json::to_vec(value)?)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}
fn page_limit(limit: usize) -> JournalResult<usize> {
    if !(1..=MAX_JOURNAL_PAGE).contains(&limit) {
        return Err(invalid("page limit must be 1..32"));
    }
    Ok(limit)
}
pub(super) fn fit_page<T: Serialize>(
    rows: Vec<(String, T)>,
    mut more: bool,
) -> JournalResult<JournalPage<T>> {
    let mut page = JournalPage {
        items: Vec::new(),
        next: None,
    };
    let mut last = None;
    for (key, item) in rows {
        page.items.push(item);
        page.next = Some(key.clone());
        if serde_json::to_string_pretty(&page)?.len() > MAX_JOURNAL_RESPONSE_BYTES {
            page.items.pop();
            if page.items.is_empty() {
                return Err(invalid("single metadata item exceeds page budget"));
            }
            more = true;
            break;
        }
        last = Some(key);
    }
    page.next = if more { last } else { None };
    Ok(page)
}
pub(super) fn check_session_host(
    store: &SubjektivStore,
    tx: &Connection,
    host: &JournalHostContext,
) -> JournalResult<()> {
    if host.scope_id != store.scope_id() {
        return Err(JournalError::PermissionDenied(
            "different storage scope".into(),
        ));
    }
    let attr = host
        .attribution
        .as_ref()
        .ok_or_else(|| JournalError::PermissionDenied("Session authority required".into()))?;
    let raw: Option<String> = tx.query_row("SELECT record_json FROM subject_session_attributions WHERE session_id=?1 AND subject_id=?2", params![attr.session_id, host.subject_id], |row| row.get(0)).optional()?;
    if raw
        .map(|s| serde_json::from_str::<SubjectSessionAttribution>(&s))
        .transpose()?
        .as_ref()
        != Some(attr)
    {
        return Err(JournalError::PermissionDenied(
            "Session attribution mismatch".into(),
        ));
    }
    Ok(())
}

pub(super) fn check_host(
    store: &SubjektivStore,
    tx: &Connection,
    host: &JournalHostContext,
    write: bool,
) -> JournalResult<()> {
    if host.scope_id != store.scope_id() {
        return Err(JournalError::PermissionDenied(
            "different storage scope".into(),
        ));
    }
    if write {
        let state: String = tx.query_row(
            "SELECT state FROM subjects WHERE subject_id=?1",
            [&host.subject_id],
            |r| r.get(0),
        )?;
        if state != "active" {
            return Err(JournalError::PermissionDenied("Subject is retired".into()));
        }
        check_session_host(store, tx, host)?;
        super::binding::check_writer(tx, host)?;
    }
    Ok(())
}
pub(super) fn journal(
    tx: &Connection,
    host: &JournalHostContext,
    id: &str,
    write: bool,
) -> JournalResult<Journal> {
    let j = tx.query_row("SELECT id, subject_id, source_session_id, title, current_position, process_order, process_byte, state_token, created_at, updated_at FROM journals WHERE id=?1 AND subject_id=?2", params![id, host.subject_id], |r| Ok(Journal { id:r.get(0)?, subject_id:r.get(1)?, source_session_id:r.get(2)?, title:r.get(3)?, current_position:r.get(4)?, process_order:r.get(5)?, process_byte:r.get(6)?, state_token:r.get(7)?, created_at:r.get(8)?, updated_at:r.get(9)? })).optional()?.ok_or_else(|| JournalError::NotFound(id.into()))?;
    if write
        && host.attribution.as_ref().map(|a| a.session_id.as_str())
            != Some(j.source_session_id.as_str())
    {
        return Err(JournalError::PermissionDenied(
            "different source Session".into(),
        ));
    }
    Ok(j)
}
pub(super) fn entry(tx: &Connection, journal_id: &str, id: &str) -> JournalResult<JournalEntry> {
    let row = tx.query_row("SELECT id, journal_id, position, state, title, body, ongoing, source_end_json, created_at, updated_at, fixed_at FROM journal_entries WHERE journal_id=?1 AND id=?2", params![journal_id, id], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,u64>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,String>(5)?,r.get::<_,String>(6)?,r.get::<_,Option<String>>(7)?,r.get::<_,String>(8)?,r.get::<_,String>(9)?,r.get::<_,Option<String>>(10)?))).optional()?.ok_or_else(|| JournalError::NotFound(id.into()))?;
    let mut stmt = tx.prepare(
        "SELECT target_id FROM journal_corrections WHERE entry_id=?1 ORDER BY target_id",
    )?;
    let correction_refs = stmt
        .query_map([id], |r| r.get(0))?
        .collect::<Result<Vec<String>, _>>()?;
    Ok(JournalEntry {
        id: row.0,
        journal_id: row.1,
        position: row.2,
        state: if row.3 == "open" {
            JournalEntryState::Open
        } else {
            JournalEntryState::Fixed
        },
        title: row.4,
        body: row.5,
        ongoing: row.6,
        source_end: row.7.map(|s| serde_json::from_str(&s)).transpose()?,
        created_at: row.8,
        updated_at: row.9,
        fixed_at: row.10,
        correction_refs,
    })
}
pub(super) fn open(tx: &Connection, journal_id: &str) -> JournalResult<Option<JournalEntry>> {
    let id: Option<String> = tx
        .query_row(
            "SELECT id FROM journal_entries WHERE journal_id=?1 AND state='open'",
            [journal_id],
            |r| r.get(0),
        )
        .optional()?;
    id.map(|id| entry(tx, journal_id, &id)).transpose()
}
pub(super) fn previous_fixed(
    tx: &Connection,
    journal_id: &str,
) -> JournalResult<Option<(JournalSourcePointer, u64)>> {
    let raw: Option<(String,u64)> = tx.query_row("SELECT source_end_json, source_order FROM journal_entries WHERE journal_id=?1 AND state='fixed' ORDER BY position DESC LIMIT 1", [journal_id], |r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    raw.map(|(s, o)| Ok((serde_json::from_str(&s)?, o)))
        .transpose()
}
fn refresh_state(tx: &Connection, j: &mut Journal) -> JournalResult<()> {
    let tail: Option<String> = tx
        .query_row(
            "SELECT id FROM journal_entries WHERE journal_id=?1 ORDER BY position DESC LIMIT 1",
            [&j.id],
            |r| r.get(0),
        )
        .optional()?;
    let tail = tail.map(|id| entry(tx, &j.id, &id)).transpose()?;
    j.state_token = hash(&(
        &j.state_token,
        &j.id,
        &j.title,
        &j.current_position,
        j.process_order,
        j.process_byte,
        tail,
    ))?;
    j.updated_at = now();
    tx.execute("UPDATE journals SET current_position=?2, process_order=?3, process_byte=?4, state_token=?5, updated_at=?6 WHERE id=?1", params![j.id,j.current_position,j.process_order,j.process_byte,j.state_token,j.updated_at])?;
    Ok(())
}
fn check_source(range: &JournalSourceRange, session: &str) -> JournalResult<()> {
    let s = &range.source;
    label(&s.session_id)?;
    let suffix = s
        .pointer
        .entry_ref
        .strip_prefix('E')
        .ok_or_else(|| invalid("source entry reference must be a public E reference"))?;
    // Session storage's stable legacy Run references include Session, Segment and record
    // offset; they are longer than a newly allocated UUID but remain bounded public IDs.
    if suffix.is_empty()
        || suffix.len() > 128
        || !suffix
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return Err(invalid("invalid public entry reference"));
    }
    if s.session_id != session {
        return Err(JournalError::PermissionDenied(
            "source belongs to another Session".into(),
        ));
    }
    if s.order >= i64::MAX as u64
        || s.total_bytes == 0
        || s.total_bytes > i64::MAX as u64
        || range.bytes.start >= range.bytes.end
        || range.bytes.end > s.total_bytes
        || s.content_sha256.len() != 64
        || !s.content_sha256.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(invalid("invalid source identity or half-open byte range"));
    }
    Ok(())
}
pub(super) fn merge(mut ranges: Vec<JournalByteRange>) -> JournalResult<Vec<JournalByteRange>> {
    ranges.sort_by_key(|r| r.start);
    let mut out: Vec<JournalByteRange> = Vec::new();
    for r in ranges {
        if let Some(last) = out.last_mut().filter(|last| r.start <= last.end) {
            last.end = last.end.max(r.end);
        } else {
            out.push(r);
        }
    }
    if out.len() > 256 {
        return Err(invalid(
            "too many disjoint ranges; admit bounded contiguous fragments",
        ));
    }
    Ok(out)
}
fn contains(ranges: &[JournalByteRange], r: JournalByteRange) -> bool {
    ranges.iter().any(|a| a.start <= r.start && a.end >= r.end)
}
fn overlaps(ranges: &[JournalByteRange], r: JournalByteRange) -> bool {
    ranges.iter().any(|a| a.start < r.end && r.start < a.end)
}
fn source(tx: &Connection, journal_id: &str, order: u64) -> JournalResult<JournalSourceStatus> {
    let raw:Option<(String,String,String,u64)>=tx.query_row("SELECT source_json,accepted_json,unavailable_json,reflected_bytes FROM journal_sources WHERE journal_id=?1 AND source_order=?2",params![journal_id,order],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
    let (s, a, u, r) =
        raw.ok_or_else(|| JournalError::NotFound(format!("source order {order}")))?;
    Ok(JournalSourceStatus {
        source: serde_json::from_str(&s)?,
        accepted: serde_json::from_str(&a)?,
        unavailable: serde_json::from_str(&u)?,
        reflected_bytes: r,
    })
}
fn check_ranges(
    tx: &Connection,
    j: &Journal,
    ranges: &[JournalSourceRange],
    available: bool,
) -> JournalResult<()> {
    if ranges.is_empty() || ranges.len() > MAX_JOURNAL_RANGES {
        return Err(invalid("source range count must be 1..64"));
    }
    for r in ranges {
        check_source(r, &j.source_session_id)?;
        let stored = source(tx, &j.id, r.source.order)?;
        if stored.source != r.source {
            return Err(conflict("source identity differs from accepted history"));
        }
        if !contains(&stored.accepted, r.bytes) {
            return Err(invalid("source range was not accepted"));
        }
        if available && overlaps(&stored.unavailable, r.bytes) {
            return Err(conflict("selected history is unavailable"));
        }
    }
    Ok(())
}
fn prefix(j: &Journal, ranges: &[JournalSourceRange]) -> JournalResult<(u64, u64)> {
    let (mut order, mut byte) = (j.process_order, j.process_byte);
    for r in ranges {
        if r.source.order != order || r.bytes.start != byte {
            return Err(invalid(
                "cannot skip a missing, unread or unreflected source prefix",
            ));
        }
        byte = r.bytes.end;
        if byte == r.source.total_bytes {
            order += 1;
            byte = 0;
        }
    }
    Ok((order, byte))
}
fn in_snapshot(snapshot: &JournalGeneration, ranges: &[JournalSourceRange]) -> JournalResult<()> {
    if ranges.len() > MAX_JOURNAL_RANGES {
        return Err(invalid("too many source ranges"));
    }
    for r in ranges {
        let selected = merge(
            snapshot
                .selected_ranges
                .iter()
                .filter(|a| a.source == r.source)
                .map(|a| a.bytes)
                .collect(),
        )?;
        if !contains(&selected, r.bytes) {
            return Err(invalid("operation exceeds immutable generation input"));
        }
    }
    Ok(())
}
fn generation(
    tx: &Connection,
    host: &JournalHostContext,
    id: &str,
) -> JournalResult<(JournalGeneration, bool)> {
    let raw:Option<(String,bool)>=tx.query_row("SELECT g.snapshot_json,g.active FROM journal_generations g JOIN journals j ON j.id=g.journal_id WHERE g.id=?1 AND j.subject_id=?2",params![id,host.subject_id],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    let (raw, active) = raw.ok_or_else(|| JournalError::NotFound(id.into()))?;
    let snapshot: JournalGeneration = serde_json::from_str(&raw)?;
    journal(tx, host, &snapshot.journal_id, true)?;
    Ok((snapshot, active))
}
pub(super) fn job(
    store: &SubjektivStore,
    tx: &Connection,
    ctx: &JournalJobContext,
    active: bool,
) -> JournalResult<JournalGeneration> {
    // Recovery can inspect the original generation and exact saved receipts after release;
    // only a new mutation requires the still-current writer lifetime.
    check_host(store, tx, &ctx.host, active)?;
    check_session_host(store, tx, &ctx.host)?;
    let (g, is_active) = generation(tx, &ctx.host, &ctx.generation_id)?;
    if g.job != ctx.binding {
        return Err(JournalError::PermissionDenied(
            "different Job attempt".into(),
        ));
    }
    if active && !is_active {
        return Err(conflict("generation is closed or cancelled"));
    }
    Ok(g)
}
fn saved(
    tx: &Connection,
    journal_id: &str,
    id: &str,
) -> JournalResult<Option<JournalSavedOperation>> {
    let raw:Option<(String,String)>=tx.query_row("SELECT request_json,receipt_json FROM journal_operations WHERE journal_id=?1 AND request_id=?2",params![journal_id,id],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    raw.map(|(a, b)| {
        Ok(JournalSavedOperation {
            operation: serde_json::from_str(&a)?,
            receipt: serde_json::from_str(&b)?,
        })
    })
    .transpose()
}
fn endpoint(
    tx: &Connection,
    j: &Journal,
    g: &JournalGeneration,
    p: &JournalSourcePointer,
) -> JournalResult<u64> {
    let r = g
        .selected_ranges
        .iter()
        .find(|r| &r.source.pointer == p && r.bytes.end == r.source.total_bytes)
        .ok_or_else(|| invalid("endpoint must identify a fully selected entry end"))?;
    let s = source(tx, &j.id, r.source.order)?;
    if s.source != r.source
        || !contains(
            &s.accepted,
            JournalByteRange {
                start: 0,
                end: s.source.total_bytes,
            },
        )
        || !s.unavailable.is_empty()
    {
        return Err(conflict("endpoint identity/availability differs"));
    }
    // The earlier bytes may have been reflected by an earlier generation, but cannot be unread.
    let selected = merge(
        g.selected_ranges
            .iter()
            .filter(|a| a.source == r.source)
            .map(|a| a.bytes)
            .collect(),
    )?;
    if !contains(
        &selected,
        JournalByteRange {
            start: s.reflected_bytes,
            end: s.source.total_bytes,
        },
    ) && s.reflected_bytes < s.source.total_bytes
    {
        return Err(invalid("endpoint entry has an unread byte prefix"));
    }
    if previous_fixed(tx, &j.id)?.is_some_and(|(_, order)| order >= r.source.order) {
        return Err(invalid(
            "fixed endpoint must advance canonical Session order",
        ));
    }
    Ok(r.source.order)
}

impl SubjektivStore {
    /// Admits Host-verified canonical fragments without generating text or starting a Job.
    /// Duplicate/overlapping fragments merge. Identity replacement aborts the entire admission.
    pub fn accept_journal_sources(
        &self,
        host: &JournalHostContext,
        title: &str,
        ranges: &[JournalSourceRange],
    ) -> JournalResult<JournalId> {
        text(title, 1024)?;
        self.database.try_transaction(|tx| {
            check_host(self,tx,host,true)?;
            super::lifecycle::reject_erased(tx, host)?;
            let session=&host.attribution.as_ref().expect("checked").session_id;
            if ranges.is_empty() || ranges.len()>MAX_JOURNAL_RANGES {return Err(invalid("admission count must be 1..64"));}
            let existing:Option<String>=tx.query_row("SELECT id FROM journals WHERE source_session_id=?1",[session],|r|r.get(0)).optional()?;
            let id=if let Some(id)=existing {journal(tx,host,&id,true)?;id} else {
                let id=issued_id("journal");let time=now();
                tx.execute("INSERT INTO journals(id,subject_id,source_session_id,title,current_position,state_token,created_at,updated_at) VALUES(?1,?2,?3,?4,'','',?5,?5)",params![id,host.subject_id,session,title,time])?;
                let mut j=journal(tx,host,&id,true)?;refresh_state(tx,&mut j)?;id
            };
            for r in ranges {
                check_source(r,session)?;
                let raw:Option<String>=tx.query_row("SELECT source_json FROM journal_sources WHERE journal_id=?1 AND source_order=?2",params![id,r.source.order],|r|r.get(0)).optional()?;
                if let Some(raw)=raw {
                    if serde_json::from_str::<JournalSource>(&raw)?!=r.source {return Err(conflict("same source order has a different identity (rewind/replacement)"));}
                    let s=source(tx,&id,r.source.order)?;
                    let mut accepted=s.accepted;accepted.push(r.bytes);
                    tx.execute("UPDATE journal_sources SET accepted_json=?3 WHERE journal_id=?1 AND source_order=?2",params![id,r.source.order,serde_json::to_string(&merge(accepted)?)?])?;
                } else {
                    let pointer=serde_json::to_string(&r.source.pointer)?;
                    if tx.query_row("SELECT EXISTS(SELECT 1 FROM journal_sources WHERE journal_id=?1 AND (pointer_json=?2 OR entry_ref=?3))",params![id,pointer,r.source.pointer.entry_ref],|r|r.get::<_,bool>(0))? {return Err(conflict("canonical entry already admitted at a different order"));}
                    tx.execute("INSERT INTO journal_sources(journal_id,source_order,pointer_json,entry_ref,source_json,accepted_json,unavailable_json) VALUES(?1,?2,?3,?4,?5,?6,'[]')",params![id,r.source.order,pointer,r.source.pointer.entry_ref,serde_json::to_string(&r.source)?,serde_json::to_string(&vec![r.bytes])?])?;
                }
            }
            Ok(id)
        })
    }
    /// Availability is Host-attested and separate from reflection; disappearance never marks input processed.
    pub fn mark_journal_sources_unavailable(
        &self,
        host: &JournalHostContext,
        id: &str,
        ranges: &[JournalSourceRange],
    ) -> JournalResult<()> {
        self.database.try_transaction(|tx| {
            check_host(self,tx,host,true)?;let j=journal(tx,host,id,true)?;check_ranges(tx,&j,ranges,false)?;
            for r in ranges {let s=source(tx,id,r.source.order)?;let mut u=s.unavailable;u.push(r.bytes);tx.execute("UPDATE journal_sources SET unavailable_json=?3 WHERE journal_id=?1 AND source_order=?2",params![id,r.source.order,serde_json::to_string(&merge(u)?)?])?;}
            Ok(())
        })
    }
    /// Recovery after a Host has reacquired the same canonical bytes. This changes availability,
    /// never their identity, accepted coverage, or reflected cursor.
    pub fn mark_journal_sources_available(
        &self,
        host: &JournalHostContext,
        id: &str,
        ranges: &[JournalSourceRange],
    ) -> JournalResult<()> {
        self.database.try_transaction(|tx| {
            check_host(self,tx,host,true)?;let j=journal(tx,host,id,true)?;check_ranges(tx,&j,ranges,false)?;
            for r in ranges {
                let s=source(tx,id,r.source.order)?;let mut remaining=Vec::new();
                for u in s.unavailable {
                    if u.start<r.bytes.start {remaining.push(JournalByteRange {start:u.start,end:u.end.min(r.bytes.start)});}
                    if u.end>r.bytes.end {remaining.push(JournalByteRange {start:u.start.max(r.bytes.end),end:u.end});}
                }
                tx.execute("UPDATE journal_sources SET unavailable_json=?3 WHERE journal_id=?1 AND source_order=?2",params![id,r.source.order,serde_json::to_string(&merge(remaining)?)?])?;
            }
            Ok(())
        })
    }
    pub fn active_journal_generation(
        &self,
        host: &JournalHostContext,
        id: &str,
    ) -> JournalResult<Option<JournalGeneration>> {
        self.database.try_with_connection(|tx| {
            check_host(self, tx, host, true)?;
            journal(tx, host, id, true)?;
            let generation_id: Option<String> = tx
                .query_row(
                    "SELECT id FROM journal_generations WHERE journal_id=?1 AND active=1",
                    [id],
                    |r| r.get(0),
                )
                .optional()?;
            generation_id
                .map(|id| Ok(generation(tx, host, &id)?.0))
                .transpose()
        })
    }
    pub fn prepare_journal_generation(
        &self,
        host: &JournalHostContext,
        id: &str,
        binding: JobAttemptBinding,
        selected_ranges: Vec<JournalSourceRange>,
    ) -> JournalResult<JournalGeneration> {
        label(&binding.job_id)?;
        label(&binding.attempt_id)?;
        self.database.try_transaction(|tx| {
            check_host(self,tx,host,true)?;let j=journal(tx,host,id,true)?;
            let request=serde_json::to_string(&selected_ranges)?;
            let existing:Option<(String,String)>=tx.query_row("SELECT request_json,snapshot_json FROM journal_generations WHERE journal_id=?1 AND job_id=?2 AND attempt_id=?3",params![id,binding.job_id,binding.attempt_id],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
            if let Some((old,snapshot))=existing {if old!=request {return Err(conflict("Job attempt already bound to different source selection"));}return Ok(serde_json::from_str(&snapshot)?);}
            if tx.query_row("SELECT EXISTS(SELECT 1 FROM journal_generations WHERE journal_id=?1 AND active=1)",[id],|r|r.get::<_,bool>(0))? {return Err(conflict("another active generation owns this Journal"));}
            check_ranges(tx,&j,&selected_ranges,true)?;prefix(&j,&selected_ranges)?;
            let snapshot=JournalGeneration {id:issued_id("journal-generation"),journal_id:id.into(),job:binding,selected_ranges,initial_journal:j,initial_open:open(tx,id)?,previous_fixed_end:previous_fixed(tx,id)?.map(|p|p.0)};
            tx.execute("INSERT INTO journal_generations(id,journal_id,job_id,attempt_id,request_json,snapshot_json,active) VALUES(?1,?2,?3,?4,?5,?6,1)",params![snapshot.id,id,snapshot.job.job_id,snapshot.job.attempt_id,request,serde_json::to_string(&snapshot)?])?;
            Ok(snapshot)
        })
    }
    /// Rebind a saved body input to a new attempt without resampling its initial state.
    /// The first binding freezes the full initial Journal snapshot; retries reuse it and
    /// retain operation receipts. Different Jobs cannot borrow that snapshot as authority.
    pub fn prepare_journal_input(
        &self,
        host: &JournalHostContext,
        input: &super::recording::JournalInput,
        binding: JobAttemptBinding,
    ) -> JournalResult<JournalGeneration> {
        label(&binding.job_id)?;
        label(&binding.attempt_id)?;
        self.database.try_transaction(|tx| {
            check_host(self, tx, host, false)?;
            check_session_host(self, tx, host)?;
            let current = journal(tx, host, &input.journal_id, true)?;
            let existing: Option<String> = tx.query_row(
                "SELECT snapshot_json FROM journal_generations WHERE journal_id=?1 AND job_id=?2 AND attempt_id=?3",
                params![input.journal_id, binding.job_id, binding.attempt_id], |r| r.get(0)).optional()?;
            if let Some(raw) = existing {
                let g = serde_json::from_str(&raw)?;
                input.validate_generation(&g)?;
                return Ok(g);
            }
            // Retained receipt lookup does not reacquire writer authority. Only a new
            // generation requires the current active Subject/Session writer lifetime.
            check_host(self, tx, host, true)?;
            if tx.query_row("SELECT EXISTS(SELECT 1 FROM journal_generations WHERE journal_id=?1 AND active=1)", [&input.journal_id], |r| r.get::<_, bool>(0))? {
                return Err(conflict("another active generation owns this Journal"));
            }
            let prior: Option<String> = tx.query_row(
                "SELECT snapshot_json FROM journal_generations WHERE journal_id=?1 AND job_id=?2 ORDER BY rowid LIMIT 1",
                params![input.journal_id, binding.job_id], |r| r.get(0)).optional()?;
            let mut g = if let Some(raw) = prior {
                serde_json::from_str::<JournalGeneration>(&raw)?
            } else {
                JournalGeneration { id: String::new(), journal_id: input.journal_id.clone(), job: binding.clone(),
                    selected_ranges: input.selected_ranges.clone(), initial_journal: current.clone(),
                    initial_open: open(tx, &input.journal_id)?, previous_fixed_end: previous_fixed(tx, &input.journal_id)?.map(|p| p.0) }
            };
            input.validate_generation(&g)?;
            check_ranges(tx, &current, &input.selected_ranges, true)?;
            let end = prefix(&g.initial_journal, &input.selected_ranges)?;
            let cursor = (current.process_order, current.process_byte);
            if cursor < (g.initial_journal.process_order, g.initial_journal.process_byte) || cursor > end {
                return Err(conflict("saved input no longer owns the current processing cursor"));
            }
            g.id = issued_id("journal-generation");
            g.job = binding;
            tx.execute("INSERT INTO journal_generations(id,journal_id,job_id,attempt_id,request_json,snapshot_json,active) VALUES(?1,?2,?3,?4,?5,?6,1)",
                params![g.id,g.journal_id,g.job.job_id,g.job.attempt_id,serde_json::to_string(&g.selected_ranges)?,serde_json::to_string(&g)?])?;
            Ok(g)
        })
    }

    pub(super) fn validate_journal_job(&self, ctx: &JournalJobContext) -> JournalResult<()> {
        self.database.try_with_connection(|tx| {
            job(self, tx, ctx, true)?;
            Ok(())
        })
    }
    /// Finish/cancel are Host lifecycle decisions. Both fence new operations, preserve successful receipts and do not fix open entries.
    pub fn close_journal_generation(
        &self,
        host: &JournalHostContext,
        generation_id: &str,
        binding: &JobAttemptBinding,
    ) -> JournalResult<()> {
        self.database.try_transaction(|tx| {
            check_host(self, tx, host, true)?;
            let (g, _) = generation(tx, host, generation_id)?;
            if &g.job != binding {
                return Err(JournalError::PermissionDenied(
                    "different Job attempt".into(),
                ));
            }
            tx.execute(
                "UPDATE journal_generations SET active=0 WHERE id=?1",
                [generation_id],
            )?;
            Ok(())
        })
    }
    pub fn read_journal_generation(
        &self,
        host: &JournalHostContext,
        generation_id: &str,
    ) -> JournalResult<JournalGeneration> {
        self.database.try_with_connection(|tx| {
            check_host(self, tx, host, true)?;
            Ok(generation(tx, host, generation_id)?.0)
        })
    }
    /// The immutable snapshot is for Host input construction, capped by the text/range storage limits.
    /// User/model readers use the metadata and UTF-8 text pages instead of this full Host snapshot.
    pub fn journal_resume_state(
        &self,
        host: &JournalHostContext,
        id: &str,
    ) -> JournalResult<(Journal, Option<JournalEntry>)> {
        self.database.try_with_connection(|tx| {
            check_host(self, tx, host, true)?;
            Ok((journal(tx, host, id, true)?, open(tx, id)?))
        })
    }
    pub fn execute_journal(
        &self,
        ctx: &JournalJobContext,
        operation: JournalOperation,
    ) -> JournalResult<JournalReceipt> {
        label(&operation.request_id)?;
        self.database.try_transaction(|tx| {
            let g=job(self,tx,ctx,false)?;
            if let Some(old)=saved(tx,&g.journal_id,&operation.request_id)? {
                if old.receipt.generation_id!=g.id || old.operation!=operation {return Err(conflict("operation identifier already used with different input or generation"));}
                return Ok(old.receipt);
            }
            job(self,tx,ctx,true)?;
            let mut j=journal(tx,&ctx.host,&g.journal_id,true)?;
            if operation.expected_state!=j.state_token {return Err(conflict("stored Journal content changed; reread before applying"));}
            in_snapshot(&g,&operation.covered_ranges)?;
            if !operation.covered_ranges.is_empty() {check_ranges(tx,&j,&operation.covered_ranges,true)?;}
            // A fully checkpointed selection has no pending source to recover another
            // activity write if the Job subsequently fails. Receipts and position repair
            // remain available, but new activity belongs to the next selected prefix.
            if matches!(operation.action, JournalAction::Create {..}|JournalAction::Rewrite {..}|JournalAction::Fix {..}|JournalAction::ConfirmUnchanged)
                && (j.process_order,j.process_byte)==prefix(&g.initial_journal,&g.selected_ranges)? {
                return Err(conflict("generation activity is already fully checkpointed; finish this Job before recording new input"));
            }
            let mut entry_id=None;
            match &operation.action {
                JournalAction::Create {title,body,ongoing,state,source_end,correction_refs}=> {
                    text(title,1024)?;text(body,MAX_JOURNAL_TEXT_BYTES)?;text(ongoing,MAX_JOURNAL_TEXT_BYTES)?;
                    if correction_refs.len()>16 {return Err(invalid("at most 16 correction targets"));}
                    if open(tx,&j.id)?.is_some() {return Err(conflict("fix existing open before appending"));}
                    let source_order=match (state,source_end) {
                        (JournalEntryState::Open,None)=>None,
                        (JournalEntryState::Fixed,Some(p)) if ongoing.is_empty()=>Some(endpoint(tx,&j,&g,p)?),
                        _=>return Err(invalid("open must have no endpoint; fixed requires endpoint and empty ongoing")),
                    };
                    let id=issued_id("journal-entry");let time=now();
                    let position:u64=tx.query_row("SELECT COALESCE(MAX(position)+1,0) FROM journal_entries WHERE journal_id=?1",[&j.id],|r|r.get(0))?;
                    tx.execute("INSERT INTO journal_entries(id,subject_id,journal_id,position,state,title,body,ongoing,source_end_json,source_order,created_at,updated_at,fixed_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?11,?12)",params![id,j.subject_id,j.id,position,if *state==JournalEntryState::Open {"open"} else {"fixed"},title,body,ongoing,source_end.as_ref().map(serde_json::to_string).transpose()?,source_order,time,source_order.map(|_|time.clone())])?;
                    for target in correction_refs {
                        if !tx.query_row("SELECT EXISTS(SELECT 1 FROM journal_entries WHERE subject_id=?1 AND id=?2)",params![j.subject_id,target],|r|r.get::<_,bool>(0))? {return Err(JournalError::PermissionDenied("correction target is outside Subject or missing".into()));}
                        tx.execute("INSERT INTO journal_corrections(subject_id,entry_id,target_id) VALUES(?1,?2,?3)",params![j.subject_id,id,target])?;
                    }
                    entry_id=Some(id);
                }
                JournalAction::Rewrite {entry_id:id,title,body,ongoing}=> {
                    text(title,1024)?;text(body,MAX_JOURNAL_TEXT_BYTES)?;text(ongoing,MAX_JOURNAL_TEXT_BYTES)?;
                    let e=entry(tx,&j.id,id)?;if e.state!=JournalEntryState::Open {return Err(conflict("fixed entry cannot be rewritten"));}
                    tx.execute("UPDATE journal_entries SET title=?2,body=?3,ongoing=?4,updated_at=?5 WHERE id=?1",params![id,title,body,ongoing,now()])?;entry_id=Some(id.clone());
                }
                JournalAction::Fix {entry_id:id,source_end}=> {
                    let e=entry(tx,&j.id,id)?;if e.state!=JournalEntryState::Open {return Err(conflict("only open entry can be fixed"));}
                    let order=endpoint(tx,&j,&g,source_end)?;let time=now();
                    tx.execute("UPDATE journal_entries SET state='fixed',ongoing='',source_end_json=?2,source_order=?3,updated_at=?4,fixed_at=?4 WHERE id=?1",params![id,serde_json::to_string(source_end)?,order,time])?;entry_id=Some(id.clone());
                }
                JournalAction::UpdatePosition {current_position}=> {
                    if !operation.covered_ranges.is_empty() {return Err(invalid("position update cannot attest reflected history"));}
                    text(current_position,MAX_JOURNAL_TEXT_BYTES)?;j.current_position=current_position.clone();
                }
                JournalAction::ConfirmUnchanged=> {if operation.covered_ranges.is_empty() {return Err(invalid("no-change confirmation must name actually checked input"));}}
                JournalAction::Checkpoint {record_operation_ids}=> {
                    check_ranges(tx,&j,&operation.covered_ranges,true)?;
                    let (order,byte)=prefix(&j,&operation.covered_ranges)?;
                    if record_operation_ids.is_empty() || record_operation_ids.len()>MAX_JOURNAL_RANGES {return Err(invalid("checkpoint requires 1..64 saved record receipts"));}
                    let mut evidence:Vec<JournalSourceRange>=Vec::new();
                    for id in record_operation_ids {
                        label(id)?;
                        let saved=saved(tx,&j.id,id)?.ok_or_else(||JournalError::NotFound(id.clone()))?;
                        if matches!(saved.operation.action,JournalAction::UpdatePosition {..}|JournalAction::Checkpoint {..}) {return Err(invalid("receipt is not a saved record or no-change confirmation"));}
                        evidence.extend(saved.operation.covered_ranges);
                    }
                    for r in &operation.covered_ranges {
                        let merged=merge(evidence.iter().filter(|a|a.source==r.source).map(|a|a.bytes).collect())?;
                        if !contains(&merged,r.bytes) {return Err(invalid("checkpoint has input with no durable record correspondence"));}
                        tx.execute("UPDATE journal_sources SET reflected_bytes=?3 WHERE journal_id=?1 AND source_order=?2",params![j.id,r.source.order,r.bytes.end])?;
                    }
                    // Advancing reflection without position would strand a terminal failed
                    // Job: source selection would see no pending bytes to repair that memo.
                    super::binding::require_saved_position(tx,&g.id)?;
                    j.process_order=order;j.process_byte=byte;
                }
            }
            refresh_state(tx,&mut j)?;
            let receipt=JournalReceipt {request_id:operation.request_id.clone(),generation_id:g.id,entry_id,state_token:j.state_token,process_order:j.process_order,process_byte:j.process_byte};
            tx.execute("INSERT INTO journal_operations(journal_id,request_id,generation_id,request_json,receipt_json) VALUES(?1,?2,?3,?4,?5)",params![j.id,operation.request_id,receipt.generation_id,serde_json::to_string(&operation)?,serde_json::to_string(&receipt)?])?;
            Ok(receipt)
        })
    }
    /// The checkpoint has the same retry identity rules as every individual record operation.
    pub fn checkpoint_journal(
        &self,
        ctx: &JournalJobContext,
        operation: JournalOperation,
    ) -> JournalResult<JournalReceipt> {
        if !matches!(operation.action, JournalAction::Checkpoint { .. }) {
            return Err(invalid("checkpoint API requires a checkpoint action"));
        }
        self.execute_journal(ctx, operation)
    }
    /// Bounded metadata about the saved effect. Coverage has its own immutable pages, so
    /// restart recovery need not retrieve or retain the original model-generated body.
    pub fn read_journal_operation_status(
        &self,
        host: &JournalHostContext,
        journal_id: &str,
        id: &str,
    ) -> JournalResult<JournalOperationStatus> {
        self.database.try_with_connection(|tx| {
            check_host(self, tx, host, false)?;
            journal(tx, host, journal_id, false)?;
            let saved =
                saved(tx, journal_id, id)?.ok_or_else(|| JournalError::NotFound(id.into()))?;
            let kind = match saved.operation.action {
                JournalAction::Create { .. } => JournalOperationKind::Create,
                JournalAction::Rewrite { .. } => JournalOperationKind::Rewrite,
                JournalAction::Fix { .. } => JournalOperationKind::Fix,
                JournalAction::UpdatePosition { .. } => JournalOperationKind::UpdatePosition,
                JournalAction::ConfirmUnchanged => JournalOperationKind::ConfirmUnchanged,
                JournalAction::Checkpoint { .. } => JournalOperationKind::Checkpoint,
            };
            Ok(JournalOperationStatus {
                receipt: saved.receipt,
                kind,
                covered_range_count: saved.operation.covered_ranges.len(),
            })
        })
    }
    pub fn read_journal_operation_coverage(
        &self,
        host: &JournalHostContext,
        journal_id: &str,
        id: &str,
        offset: usize,
        limit: usize,
    ) -> JournalResult<JournalPage<JournalSourceRange>> {
        let limit = page_limit(limit)?;
        self.database.try_with_connection(|tx| {
            check_host(self, tx, host, false)?;
            journal(tx, host, journal_id, false)?;
            let saved =
                saved(tx, journal_id, id)?.ok_or_else(|| JournalError::NotFound(id.into()))?;
            let len = saved.operation.covered_ranges.len();
            if offset > len {
                return Err(invalid("coverage offset exceeds saved operation"));
            }
            let rows = saved
                .operation
                .covered_ranges
                .into_iter()
                .enumerate()
                .skip(offset)
                .take(limit)
                .map(|(i, r)| ((i + 1).to_string(), r))
                .collect();
            fit_page(rows, len - offset > limit)
        })
    }
    /// Receipt lookup remains possible after cancellation and Session deletion. No new mutation is authorized by a receipt.
    pub fn query_journal_operation(
        &self,
        host: &JournalHostContext,
        journal_id: &str,
        id: &str,
    ) -> JournalResult<Option<JournalReceipt>> {
        self.database.try_with_connection(|tx| {
            check_host(self, tx, host, false)?;
            journal(tx, host, journal_id, false)?;
            Ok(saved(tx, journal_id, id)?.map(|s| s.receipt))
        })
    }
    pub fn list_journal_operations(
        &self,
        host: &JournalHostContext,
        journal_id: &str,
        after: Option<&str>,
        limit: usize,
    ) -> JournalResult<JournalPage<JournalReceipt>> {
        let limit = page_limit(limit)?;
        self.database.try_with_connection(|tx| {
            check_host(self,tx,host,false)?;journal(tx,host,journal_id,false)?;
            // Immutable request IDs are not chronological; the durable SQLite rowid provides keyset order.
            let after=after.map(str::parse::<i64>).transpose().map_err(|_|invalid("invalid operation cursor"))?.unwrap_or(0);
            let mut stmt=tx.prepare("SELECT rowid,receipt_json FROM journal_operations WHERE journal_id=?1 AND rowid>?2 ORDER BY rowid LIMIT ?3")?;
            let rows=stmt.query_map(params![journal_id,after,limit+1],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?)))?.collect::<Result<Vec<_>,_>>()?;
            let more=rows.len()>limit;let mut items=Vec::new();
            for (id,raw) in rows.into_iter().take(limit) {items.push((id.to_string(),serde_json::from_str::<JournalReceipt>(&raw)?));}
            fit_page(items,more)
        })
    }
    /// Coverage changes can fill an earlier gap or extend an already listed entry. Continuation
    /// pins the complete ledger content condition, rejecting changes instead of losing those bytes.
    pub fn list_journal_sources(
        &self,
        host: &JournalHostContext,
        journal_id: &str,
        cursor: Option<&str>,
        limit: usize,
    ) -> JournalResult<JournalPage<JournalSourceStatus>> {
        let limit = page_limit(limit)?;
        self.database.try_transaction(|tx| {
            check_host(self,tx,host,false)?;journal(tx,host,journal_id,false)?;
            let fingerprint=source_fingerprint(tx,journal_id)?;
            let after=if let Some(cursor)=cursor {
                if cursor.len()>2048 {return Err(invalid("source cursor exceeds byte limit"));}
                let cursor:SourcePageCursor=serde_json::from_str(cursor).map_err(|_|invalid("invalid source cursor"))?;
                if cursor.scope_id!=host.scope_id || cursor.subject_id!=host.subject_id || cursor.journal_id!=journal_id || cursor.after_order>i64::MAX as u64 {return Err(invalid("source cursor belongs to a different scope/Subject/Journal or invalid order"));}
                if cursor.source_fingerprint!=fingerprint {return Err(conflict("source coverage changed; restart pagination from current state"));}
                Some(cursor.after_order)
            } else {None};
            let mut stmt=tx.prepare("SELECT source_order FROM journal_sources WHERE journal_id=?1 AND (?2 IS NULL OR source_order>?2) ORDER BY source_order LIMIT ?3")?;
            let orders=stmt.query_map(params![journal_id,after,limit+1],|r|r.get::<_,u64>(0))?.collect::<Result<Vec<_>,_>>()?;
            let more=orders.len()>limit;let mut items=Vec::new();
            for order in orders.into_iter().take(limit) {
                let next=serde_json::to_string(&SourcePageCursor {scope_id:host.scope_id.clone(),subject_id:host.subject_id.clone(),journal_id:journal_id.into(),source_fingerprint:fingerprint.clone(),after_order:order})?;
                items.push((next,source(tx,journal_id,order)?));
            }
            fit_page(items,more)
        })
    }
    /// Bounded point lookup for Host resume from `process_order`, without scanning reflected pages.
    pub fn read_journal_source(
        &self,
        host: &JournalHostContext,
        journal_id: &str,
        order: u64,
    ) -> JournalResult<JournalSourceStatus> {
        self.database.try_with_connection(|tx| {
            check_host(self, tx, host, false)?;
            journal(tx, host, journal_id, false)?;
            source(tx, journal_id, order)
        })
    }
    /// Returns metadata only. IDs and entry positions are append-stable keysets, so appends/rewrites cannot shift unread pages.
    pub fn list_journals(
        &self,
        host: &JournalHostContext,
        after: Option<&str>,
        limit: usize,
    ) -> JournalResult<JournalPage<JournalMetadata>> {
        let limit = page_limit(limit)?;
        self.database.try_with_connection(|tx| {
            check_host(self, tx, host, false)?;
            let mut stmt = tx.prepare(
                "SELECT id FROM journals WHERE subject_id=?1 AND id>?2 ORDER BY id LIMIT ?3",
            )?;
            let ids = stmt
                .query_map(
                    params![host.subject_id, after.unwrap_or(""), limit + 1],
                    |r| r.get::<_, String>(0),
                )?
                .collect::<Result<Vec<_>, _>>()?;
            let more = ids.len() > limit;
            let mut items = Vec::new();
            for id in ids.into_iter().take(limit) {
                items.push((id.clone(), metadata(tx, host, &id)?));
            }
            fit_page(items, more)
        })
    }
    pub fn read_journal(
        &self,
        host: &JournalHostContext,
        id: &str,
    ) -> JournalResult<JournalMetadata> {
        self.database.try_with_connection(|tx| {
            check_host(self, tx, host, false)?;
            metadata(tx, host, id)
        })
    }
    pub fn list_journal_entries(
        &self,
        host: &JournalHostContext,
        journal_id: &str,
        after: Option<u64>,
        limit: usize,
    ) -> JournalResult<JournalPage<JournalEntrySummary>> {
        let limit = page_limit(limit)?;
        self.database.try_with_connection(|tx| {
            check_host(self,tx,host,false)?;journal(tx,host,journal_id,false)?;
            let mut stmt=tx.prepare("SELECT id FROM journal_entries WHERE journal_id=?1 AND (?2 IS NULL OR position>?2) ORDER BY position LIMIT ?3")?;
            let ids=stmt.query_map(params![journal_id,after,limit+1],|r|r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
            let more=ids.len()>limit;let mut items=Vec::new();for id in ids.into_iter().take(limit) {let e=entry(tx,journal_id,&id)?;items.push((e.position.to_string(),summary(tx,e)?));}
            fit_page(items,more)
        })
    }
    /// Continuation must present the observed content token. A concurrent open/position rewrite
    /// rejects stale offsets rather than returning a torn document or skipping unread bytes.
    pub fn read_journal_text(
        &self,
        host: &JournalHostContext,
        journal_id: &str,
        entry_id: Option<&str>,
        field: JournalTextField,
        expected_token: Option<&str>,
        byte_offset: usize,
        max_bytes: usize,
    ) -> JournalResult<JournalTextPage> {
        if !(4..=MAX_JOURNAL_READ_BYTES).contains(&max_bytes) {
            return Err(invalid("text page must be 4..16384 bytes"));
        }
        self.database.try_with_connection(|tx| {
            check_host(self, tx, host, false)?;
            let j = journal(tx, host, journal_id, false)?;
            let (body, token) = match (field, entry_id) {
                (JournalTextField::CurrentPosition, None) => (j.current_position, j.state_token),
                (JournalTextField::Body, Some(id)) => {
                    let e = entry(tx, journal_id, id)?;
                    let token = hash(&e)?;
                    (e.body, token)
                }
                (JournalTextField::Ongoing, Some(id)) => {
                    let e = entry(tx, journal_id, id)?;
                    let token = hash(&e)?;
                    (e.ongoing, token)
                }
                _ => return Err(invalid("text field/entry selector mismatch")),
            };
            if (byte_offset > 0 && expected_token.is_none())
                || expected_token.is_some_and(|t| t != token)
            {
                return Err(conflict(
                    "text changed or continuation has no observed content token",
                ));
            }
            if byte_offset > body.len() || !body.is_char_boundary(byte_offset) {
                return Err(invalid("offset is not a UTF-8 byte boundary"));
            }
            let mut end = byte_offset.saturating_add(max_bytes).min(body.len());
            while !body.is_char_boundary(end) {
                end -= 1;
            }
            loop {
                let page = JournalTextPage {
                    text: body[byte_offset..end].into(),
                    next_byte: (end < body.len()).then_some(end),
                    content_token: token.clone(),
                };
                if serde_json::to_string_pretty(&page)?.len() <= MAX_JOURNAL_RESPONSE_BYTES {
                    return Ok(page);
                }
                end = byte_offset + (end - byte_offset) / 2;
                while !body.is_char_boundary(end) {
                    end -= 1;
                }
                if end == byte_offset {
                    return Err(invalid("text envelope exceeds response budget"));
                }
            }
        })
    }
}
fn metadata(
    tx: &Connection,
    host: &JournalHostContext,
    id: &str,
) -> JournalResult<JournalMetadata> {
    let j = journal(tx, host, id, false)?;
    Ok(JournalMetadata {
        id: j.id,
        subject_id: j.subject_id,
        source_session_id: j.source_session_id,
        title: j.title,
        current_position_bytes: j.current_position.len(),
        process_order: j.process_order,
        process_byte: j.process_byte,
        state_token: j.state_token,
        created_at: j.created_at,
        updated_at: j.updated_at,
    })
}
pub(super) fn summary(tx: &Connection, e: JournalEntry) -> JournalResult<JournalEntrySummary> {
    let previous:Option<String>=tx.query_row("SELECT source_end_json FROM journal_entries WHERE journal_id=?1 AND position<?2 AND state='fixed' ORDER BY position DESC LIMIT 1",params![e.journal_id,e.position],|r|r.get(0)).optional()?;
    let source_available = if e.source_end.is_some() {
        let end: u64 = tx.query_row(
            "SELECT source_order FROM journal_entries WHERE id=?1",
            [&e.id],
            |r| r.get(0),
        )?;
        let previous_order:Option<u64>=tx.query_row("SELECT source_order FROM journal_entries WHERE journal_id=?1 AND position<?2 AND state='fixed' ORDER BY position DESC LIMIT 1",params![e.journal_id,e.position],|r|r.get(0)).optional()?;
        let start = previous_order.map_or(0, |o| o + 1);
        let (count,missing):(u64,u64)=tx.query_row("SELECT COUNT(*),COALESCE(SUM(unavailable_json<>'[]'),0) FROM journal_sources WHERE journal_id=?1 AND source_order BETWEEN ?2 AND ?3",params![e.journal_id,start,end],|r|Ok((r.get(0)?,r.get(1)?)))?;
        count == end - start + 1 && missing == 0
    } else {
        false
    };
    let token = hash(&e)?;
    Ok(JournalEntrySummary {
        id: e.id,
        journal_id: e.journal_id,
        position: e.position,
        state: e.state,
        title: e.title,
        source_end: e.source_end,
        previous_fixed_end: previous.map(|s| serde_json::from_str(&s)).transpose()?,
        source_available,
        correction_refs: e.correction_refs,
        content_token: token,
        created_at: e.created_at,
        updated_at: e.updated_at,
        fixed_at: e.fixed_at,
    })
}

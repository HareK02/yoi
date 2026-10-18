//! User-authenticated, read-only Web Journal API. Saved Journal and immutable Memory edges
//! never depend on a live source Worker, original history reads, or model execution.
use super::*;
use subjektiv::journal::memory_links::*;
use subjektiv::journal::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum BrowserRequest {
    Sessions {
        after: Option<String>,
        limit: usize,
    },
    Entries {
        journal_id: Option<String>,
        query: Option<String>,
        after: Option<String>,
        limit: usize,
    },
    Entry {
        journal_id: String,
        entry_id: String,
    },
    Text {
        journal_id: String,
        entry_id: Option<String>,
        field: JournalTextField,
        expected_token: Option<String>,
        byte_offset: usize,
        max_bytes: usize,
    },
    Corrections {
        journal_id: String,
        entry_id: String,
        after: Option<String>,
        limit: usize,
    },
    MemorySources {
        selector: MemoryJournalSelector,
        source_offset: usize,
        limit: usize,
    },
    RelatedMemory {
        journal_id: String,
        entry_id: String,
        after: Option<RelatedMemoryCursor>,
        limit: usize,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub(super) enum BrowserResponse {
    Sessions(JournalPage<BrowserSession>),
    Entries(JournalPage<JournalEntrySummary>),
    Entry(BrowserEntry),
    Text(JournalTextPage),
    Corrections(JournalPage<JournalEntrySummary>),
    MemorySources(BrowserMemorySources),
    RelatedMemory(BrowserRelatedMemories),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum SourceState {
    Missing,
    Unavailable,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct SourceStatus {
    /// `missing` requires confirmed metadata absence; `unavailable` includes unknown readability.
    pub state: SourceState,
    /// Only supplied for a retained, exact Session mapping, under this user's Workspace authority.
    /// A navigation option is not an attestation that all original bytes still exist.
    pub history_href: Option<String>,
}
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct BrowserSession {
    pub journal: JournalMetadata,
    pub unreflected_sources: bool,
    pub original_source: SourceStatus,
}
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct BrowserEntry {
    pub journal: JournalMetadata,
    pub entry: JournalEntrySummary,
    pub correction_targets: Vec<JournalEntrySummary>,
    pub original_source: SourceStatus,
}
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct BrowserMemorySource {
    pub link: MemoryJournalSourceLink,
    pub entry: Option<JournalEntrySummary>,
    pub original_source: SourceStatus,
}
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct BrowserMemorySources {
    pub selector: MemoryJournalSelector,
    pub sources: Vec<BrowserMemorySource>,
    pub total_sources: usize,
    pub next_source_index: Option<usize>,
}
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct BrowserRelatedMemory {
    pub related: RelatedMemory,
    pub claim: String,
}
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct BrowserRelatedMemories {
    pub items: Vec<BrowserRelatedMemory>,
    pub next: Option<RelatedMemoryCursor>,
}

fn domain(error: JournalError) -> Error {
    match error {
        JournalError::PermissionDenied(message) => Error::WorkspacePermissionDenied(message),
        JournalError::Conflict(message) => Error::RepositoryConflict(message),
        JournalError::InvalidInput(message) => Error::InvalidInput(message),
        JournalError::NotFound(id) => {
            Error::InvalidInput(format!("Journal record not found: {id}"))
        }
        JournalError::Subject(error) => subjektiv_store_error(error),
        other => Error::Store(other.to_string()),
    }
}

fn source_status(
    api: &WorkspaceApi,
    store: &crate::subjektiv::SubjektivStore,
    subject_id: &str,
    session: Option<&str>,
) -> SourceStatus {
    let unavailable = || SourceStatus {
        state: SourceState::Unavailable,
        history_href: None,
    };
    let Some(session) = session else {
        return unavailable();
    };
    let Ok(Some(attr)) = store.session_attribution(session) else {
        return unavailable();
    };
    if attr.subject_id != subject_id {
        return unavailable();
    }
    let worker = RuntimeWorkerRef::new(&attr.runtime_id, &attr.worker_id);
    let Ok(archive) =
        api.config_store
            .worker_session_archive(&api.config.workspace_id, &worker, session)
    else {
        return unavailable();
    };
    let Ok(registry) = api
        .store
        .get_worker_registry(&api.config.workspace_id, &worker)
    else {
        return unavailable();
    };
    if archive.is_none() && registry.is_none() {
        return SourceStatus {
            state: SourceState::Missing,
            history_href: None,
        };
    }
    // The console addresses the current Session only. Never point an old attributed
    // Session at a different current conversation, or restore a Worker to manufacture a link.
    let exact = registry
        .as_ref()
        .is_some_and(|r| r.session_ref.as_deref() == Some(session));
    let history_href = if exact {
        api.store
            .worker_resource_key(&api.config.workspace_id, &worker)
            .ok()
            .flatten()
            .map(|key| browser_worker_console_href(&api.config.workspace_id, &key))
    } else {
        None
    };
    SourceStatus {
        state: SourceState::Unavailable,
        history_href,
    }
}

/// Exact route recognition for the normal user read-authority check. Do not include
/// any action that writes or reconciles state in this endpoint's tagged request type.
pub(super) fn is_read_path(path: &str, workspace: &str) -> bool {
    path.strip_prefix(&format!("/api/w/{workspace}/subjektiv/subjects/"))
        .and_then(|tail| tail.strip_suffix("/journal-browser"))
        .is_some_and(|subject| !subject.is_empty() && !subject.contains('/'))
}

pub(super) async fn route(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedSubjektivSubjectPath>,
    actor: Option<Extension<RequestActor>>,
    Json(request): Json<BrowserRequest>,
) -> ApiResult<Json<BrowserResponse>> {
    // Runtime signatures, singleton leases and Job grants are not user browser authority.
    if actor.is_none() {
        return Err(Error::WorkspacePermissionDenied(
            "Journal browser requires user authentication".into(),
        )
        .into());
    }
    api.store
        .db_executor()
        .run(move || {
            validate_workspace_scope(&api, &path.workspace_id)?;
            let store = open_subjektiv_store(&api)?;
            require_subjektiv_subject(&store, &path.subject_id)?;
            let host =
                JournalHostContext::validated_reader(&store, &path.subject_id).map_err(domain)?;
            let response =
                execute(&api, &store, &host, &path.subject_id, request).map_err(domain)?;
            if serde_json::to_vec(&response)
                .map_err(|e| Error::Store(e.to_string()))?
                .len()
                > MAX_JOURNAL_RESPONSE_BYTES
            {
                return Err(Error::InvalidInput(
                    "Journal browser response exceeds byte budget; request a smaller page".into(),
                )
                .into());
            }
            Ok(Json(response))
        })
        .await
}

fn execute(
    api: &WorkspaceApi,
    store: &crate::subjektiv::SubjektivStore,
    host: &JournalHostContext,
    subject_id: &str,
    request: BrowserRequest,
) -> JournalResult<BrowserResponse> {
    Ok(match request {
        BrowserRequest::Sessions { after, limit } => {
            if after.as_ref().is_some_and(|s| s.len() > 256) {
                return Err(JournalError::InvalidInput(
                    "Journal cursor exceeds byte limit".into(),
                ));
            }
            let page = store.list_journals(host, after.as_deref(), limit)?;
            let mut items = Vec::new();
            for journal in page.items {
                let unreflected_sources =
                    store.journal_has_unreflected_sources(host, &journal.id)?;
                let original_source =
                    source_status(api, store, subject_id, Some(&journal.source_session_id));
                items.push(BrowserSession {
                    journal,
                    unreflected_sources,
                    original_source,
                });
            }
            BrowserResponse::Sessions(JournalPage {
                items,
                next: page.next,
            })
        }
        BrowserRequest::Entries {
            journal_id,
            query,
            after,
            limit,
        } => BrowserResponse::Entries(store.browse_journal_entries(
            host,
            journal_id.as_deref(),
            query.as_deref(),
            None,
            after.as_deref(),
            limit,
        )?),
        BrowserRequest::Entry {
            journal_id,
            entry_id,
        } => {
            let journal = store.read_journal(host, &journal_id)?;
            let entry = store.journal_entry_summary(host, &journal_id, &entry_id)?;
            let original_source =
                source_status(api, store, subject_id, Some(&journal.source_session_id));
            let correction_targets =
                store.journal_correction_targets(host, &journal_id, &entry_id)?;
            BrowserResponse::Entry(BrowserEntry {
                journal,
                entry,
                correction_targets,
                original_source,
            })
        }
        BrowserRequest::Text {
            journal_id,
            entry_id,
            field,
            expected_token,
            byte_offset,
            max_bytes,
        } => BrowserResponse::Text(store.read_journal_text(
            host,
            &journal_id,
            entry_id.as_deref(),
            field,
            expected_token.as_deref(),
            byte_offset,
            max_bytes,
        )?),
        BrowserRequest::Corrections {
            journal_id,
            entry_id,
            after,
            limit,
        } => BrowserResponse::Corrections(store.browse_journal_entries(
            host,
            Some(&journal_id),
            None,
            Some(&entry_id),
            after.as_deref(),
            limit,
        )?),
        BrowserRequest::MemorySources {
            selector,
            source_offset,
            limit,
        } => {
            if !(1..=MAX_JOURNAL_PAGE).contains(&limit) {
                return Err(JournalError::InvalidInput(
                    "page limit must be 1..32".into(),
                ));
            }
            let response = store.execute_memory_journal(
                host,
                MemoryJournalRequest::LinksPage {
                    selector: selector.clone(),
                    source_offset,
                    limit: limit.min(crate::subjektiv::MAX_STAGING_ANCHORS),
                },
            )?;
            let MemoryJournalResponse::Links(page) = response else {
                unreachable!()
            };
            let candidate = store.staging_candidate(subject_id, &selector.candidate_id)?;
            let mut sources = Vec::new();
            for mut link in page.sources {
                let source_session = candidate
                    .as_ref()
                    .and_then(|c| c.source_refs.get(link.source_index))
                    .and_then(|s| s.session_id.as_deref());
                let mut original_source = source_status(api, store, subject_id, source_session);
                let entry = if let SourceConnection::Linked {
                    journal_id,
                    entry_id,
                    target: JournalTargetAvailability::Available,
                    ..
                } = &link.connection
                {
                    let journal = store.read_journal(host, journal_id)?;
                    original_source =
                        source_status(api, store, subject_id, Some(&journal.source_session_id));
                    Some(store.journal_entry_summary(host, journal_id, entry_id)?)
                } else {
                    None
                };
                if link.original_source == OriginalSourceAvailability::Missing {
                    // A saved missing-byte attestation does not prove deletion of the whole
                    // original conversation. Keep only independently confirmed metadata absence
                    // as `missing`; otherwise expose unavailable material without a history link.
                    if original_source.state != SourceState::Missing {
                        original_source.state = SourceState::Unavailable;
                    }
                    original_source.history_href = None;
                }
                if original_source.state == SourceState::Missing {
                    link.original_source = OriginalSourceAvailability::Missing;
                }
                sources.push(BrowserMemorySource {
                    link,
                    entry,
                    original_source,
                });
            }
            BrowserResponse::MemorySources(BrowserMemorySources {
                selector,
                sources,
                total_sources: page.total_sources,
                next_source_index: page.next_source_index,
            })
        }
        BrowserRequest::RelatedMemory {
            journal_id,
            entry_id,
            after,
            limit,
        } => {
            let response = store.execute_memory_journal(
                host,
                MemoryJournalRequest::Related {
                    journal_id,
                    entry_id,
                    after,
                    limit,
                },
            )?;
            let MemoryJournalResponse::Related(page) = response else {
                unreachable!()
            };
            let mut items = Vec::new();
            for related in page.items {
                let memory = store
                    .memory_change(
                        subject_id,
                        &related.selector.memory_id,
                        related.selector.change_id.clone(),
                    )?
                    .ok_or_else(|| JournalError::NotFound("related Memory change".into()))?;
                items.push(BrowserRelatedMemory {
                    related,
                    claim: bounded_memory_excerpt(&memory.claim, 256),
                });
            }
            BrowserResponse::RelatedMemory(BrowserRelatedMemories {
                items,
                next: page.next,
            })
        }
    })
}

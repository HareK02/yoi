//! Public consumer boundary: real capture adapters, checked DTO codec and pure readers.
//! No Worker, persistence, live Session provider, or substituted capture implementation.

use agen::{HistoryEntry, Item};
use protocol::{
    SessionEntryProvenance, SessionSnapshot, SessionSnapshotEntry, SessionSnapshotEntryData,
};
use session_store::capture::*;
use session_store::{
    FeatureInputPayload, HistoryInputMetadata, HistoryInputUsePolicy, InputCompactionUse,
    InputEvidenceUse, InputObservationUse, LoggedHistoryDerivation, LoggedSessionHistoryEntryId,
    LoggedSessionHistoryMetadata, LoggedSessionHistoryOrigin, PublicSystemItem,
    PublicSystemPresentation,
};

fn history(
    id: &str,
    item: Item,
    input_metadata: Option<HistoryInputMetadata>,
) -> HistoryEntry<LoggedSessionHistoryMetadata> {
    HistoryEntry::new(
        item,
        LoggedSessionHistoryMetadata {
            entry_id: LoggedSessionHistoryEntryId(id.into()),
            origin: LoggedSessionHistoryOrigin::LocalHumanInput,
            derivation: Some(LoggedHistoryDerivation {
                sources: vec![LoggedSessionHistoryEntryId("earlier-entry".into())],
            }),
            input_metadata,
        },
    )
}

fn usage(recording: bool, memory: bool, observation: bool) -> HistoryInputUsePolicy {
    HistoryInputUsePolicy {
        recording_material: if recording {
            InputEvidenceUse::Include
        } else {
            InputEvidenceUse::Exclude
        },
        memory_evidence: if memory {
            InputEvidenceUse::Include
        } else {
            InputEvidenceUse::Exclude
        },
        observation: if observation {
            InputObservationUse::Readable
        } else {
            InputObservationUse::Hidden
        },
        compaction: InputCompactionUse::Retain,
    }
}

fn reference(capture: &SessionCapture, id: &str) -> ScopedSessionEntryRef {
    capture
        .scoped_ref(&SessionEntryRef::parse(id).unwrap())
        .unwrap()
}

#[test]
fn public_consumer_roundtrip_retains_body_original_source_order_origin_and_independent_policy() {
    let entries = vec![
        history(
            "recording",
            Item::user_message("record-only"),
            Some(HistoryInputMetadata::new(usage(true, false, false), None, None).unwrap()),
        ),
        history(
            "memory",
            Item::user_message("memory-only"),
            Some(HistoryInputMetadata::new(usage(false, true, false), None, None).unwrap()),
        ),
        history(
            "observation",
            Item::user_message("observation-only"),
            Some(HistoryInputMetadata::new(usage(false, false, true), None, None).unwrap()),
        ),
    ];
    let capture = SessionCapture::from_complete_history("synthetic-session", "selected", entries);
    let mut dto = capture.to_dto();
    // The checked boundary also accepts entries retained from different original segments.
    dto.entries[0].source.segment_id = "original-a".into();
    dto.entries[0].source.entry_index = 80;
    dto.entries[1].source.segment_id = "original-b".into();
    dto.entries[1].source.entry_index = 3;
    let capture = SessionCapture::try_from_dto(dto.clone(), CaptureLimits::default()).unwrap();
    let encoded = capture.encode(CaptureLimits::default()).unwrap();
    let restored = SessionCapture::decode(&encoded, CaptureLimits::default()).unwrap();
    assert_eq!(restored.to_dto(), dto);
    assert_eq!(restored.capture_id(), capture.capture_id());
    for (id, allowed) in [
        ("Erecording", CapturePurpose::Recording),
        ("Ememory", CapturePurpose::MemoryEvidence),
        ("Eobservation", CapturePurpose::Observation),
    ] {
        let reference = reference(&capture, id);
        assert_eq!(
            restored.resolve_ref(&reference, allowed).unwrap(),
            capture.resolve_ref(&reference, allowed).unwrap()
        );
        for purpose in [
            CapturePurpose::Recording,
            CapturePurpose::MemoryEvidence,
            CapturePurpose::Observation,
        ] {
            assert_eq!(
                restored.resolve_ref(&reference, purpose).is_ok(),
                purpose == allowed
            );
        }
    }
    assert_eq!(restored.overview().len(), 1);
    assert_eq!(restored.search(&SearchOptions::default()).len(), 1);
    assert!(restored.evidence_for("Erecording").is_none());
    let evidence = restored.evidence_for("Ememory").unwrap();
    assert_eq!(evidence.segment_id, "original-b");
    assert_eq!(evidence.entry_range, [3, 3]);
    assert!(evidence.excerpt.contains("memory-only"));
    let recorded = restored
        .slice(CapturePurpose::Recording, None, 10, 16_000)
        .unwrap();
    assert_eq!(recorded.slice().items.len(), 1);
    assert_eq!(recorded.slice().items[0].text, "record-only");
    for (purpose, expected) in [
        (CapturePurpose::MemoryEvidence, "memory-only"),
        (CapturePurpose::Observation, "observation-only"),
    ] {
        let slice = restored.slice(purpose, None, 10, 16_000).unwrap();
        assert_eq!(slice.slice().items.len(), 1);
        assert_eq!(slice.slice().items[0].text, expected);
        assert!(
            restored
                .read_raw(
                    &reference(&restored, "Erecording"),
                    purpose,
                    RawField::MessagePart(0),
                    0,
                    100
                )
                .is_err()
        );
    }
}

#[test]
fn common_data_drops_reasoning_raw_system_and_private_payload_even_when_unindexed() {
    let payload = FeatureInputPayload::encode(
        "capture-test",
        1,
        &serde_json::json!({"secret": "PRIVATE-PAYLOAD"}),
    )
    .unwrap();
    let public = PublicSystemItem {
        kind: "notice".into(),
        body: "public notice".into(),
        presentation: PublicSystemPresentation::Ignored,
    };
    let system =
        HistoryInputMetadata::new(usage(true, true, true), Some(payload.clone()), Some(public))
            .unwrap();
    let private = HistoryInputMetadata::new(usage(true, true, true), Some(payload), None).unwrap();
    let capture = SessionCapture::from_complete_history(
        "s",
        "segment",
        vec![
            history("raw", Item::system_message("RAW-SYSTEM-PROMPT"), None),
            history("reasoning", Item::reasoning("HIDDEN-REASONING"), None),
            history(
                "private",
                Item::system_message("PRIVATE-SYSTEM-BODY"),
                Some(private),
            ),
            history(
                "public",
                Item::system_message("MODEL-SYSTEM-BODY"),
                Some(system),
            ),
            history("ordinary", Item::user_message("ordinary"), None),
        ],
    );
    let encoded = String::from_utf8(capture.encode(CaptureLimits::default()).unwrap()).unwrap();
    let retained_debug = format!("{capture:?}");
    for secret in [
        "RAW-SYSTEM-PROMPT",
        "HIDDEN-REASONING",
        "PRIVATE-SYSTEM-BODY",
        "MODEL-SYSTEM-BODY",
        "PRIVATE-PAYLOAD",
    ] {
        assert!(!encoded.contains(secret));
        assert!(!retained_debug.contains(secret));
    }
    assert_eq!(capture.to_dto().entries.len(), 2);
    assert!(encoded.contains("public notice"));
    // Filtered entries do not renumber the public source positions.
    assert_eq!(capture.to_dto().entries[0].order, 3);
    assert_eq!(
        capture.view_order_for_ref(&SessionEntryRef::parse("Eordinary").unwrap()),
        Some(4)
    );
}

#[test]
fn protocol_projection_does_not_invent_recording_permission() {
    let capture = SessionCapture::from_session_snapshot(
        "s",
        "seg",
        SessionSnapshot {
            pending_submissions: Default::default(),
            entries: vec![SessionSnapshotEntry {
                entry_id: "projected".into(),
                timestamp: 0,
                provenance: SessionEntryProvenance::HumanInput,
                derived_from: vec!["source".into()],
                use_policy: Some(protocol::SessionEntryUsePolicy {
                    observation_readable: true,
                    memory_evidence: false,
                }),
                data: SessionSnapshotEntryData::Message {
                    role: protocol::SessionMessageRole::User,
                    content: vec![protocol::SessionContentPart::Text {
                        text: "projected".into(),
                    }],
                },
            }],
        },
    );
    let reference = reference(&capture, "Eprojected");
    assert!(matches!(
        capture.resolve_ref(&reference, CapturePurpose::Recording),
        Err(CaptureError::PolicyDenied)
    ));
    assert!(
        capture
            .resolve_ref(&reference, CapturePurpose::Observation)
            .is_ok()
    );
    assert!(capture.evidence_for("Eprojected").is_none());
}

#[test]
fn malformed_dtos_and_oversized_representations_are_rejected() {
    let capture = SessionCapture::new("s", "seg", vec![Item::user_message("body")]);
    let original = serde_json::to_value(capture.to_dto()).unwrap();
    let mut invalids = Vec::new();
    let mut value = original.clone();
    value["version"] = 999.into();
    invalids.push(value);
    let mut value = original.clone();
    value["unexpected"] = true.into();
    invalids.push(value);
    let mut value = original.clone();
    value["entries"][0]["policy"]["recording_material"] = "include".into();
    invalids.push(value);
    let mut value = original.clone();
    value["entries"][0]["policy"]
        .as_object_mut()
        .unwrap()
        .remove("memory_evidence");
    invalids.push(value);
    let mut value = original.clone();
    value["entries"][0]["body"] = serde_json::json!({"kind":"reasoning","text":"secret"});
    invalids.push(value);
    let mut value = original.clone();
    value["entries"][0]["body"]["feature_payload"] = serde_json::json!({"private":true});
    invalids.push(value);
    let mut value = original.clone();
    value["entries"][0]["body"]["role"] = "system".into();
    invalids.push(value);
    let mut value = original.clone();
    value["entries"][0]["source"]["unknown"] = true.into();
    invalids.push(value);
    let mut value = original.clone();
    value["entries"][0]["source"]["history_origin"] =
        serde_json::json!({"kind":"legacy_unknown","unknown":true});
    invalids.push(value);
    let mut value = original.clone();
    value["entries"][0]["entry_id"] = "bad/id".into();
    invalids.push(value);
    let mut value = original.clone();
    value["entries"]
        .as_array_mut()
        .unwrap()
        .push(original["entries"][0].clone());
    invalids.push(value);
    for invalid in invalids {
        let bytes = serde_json::to_vec(&invalid).unwrap();
        assert!(
            SessionCapture::decode(&bytes, CaptureLimits::default()).is_err(),
            "accepted {invalid}"
        );
    }
    let bytes = capture.encode(CaptureLimits::default()).unwrap();
    assert!(
        SessionCapture::decode(
            &bytes,
            CaptureLimits {
                max_bytes: bytes.len() - 1,
                ..CaptureLimits::default()
            }
        )
        .is_err()
    );
    assert!(
        capture
            .encode(CaptureLimits {
                max_entries: 0,
                ..CaptureLimits::default()
            })
            .is_err()
    );
    assert!(
        capture
            .encode(CaptureLimits {
                max_text_bytes: 3,
                ..CaptureLimits::default()
            })
            .is_err()
    );
    let mut reversed = capture.to_dto();
    reversed.view_entry_count = 2;
    let mut second = reversed.entries[0].clone();
    second.entry_id = "second".into();
    second.source.entry_index = 4;
    second.order = 0;
    reversed.entries[0].order = 1;
    reversed.entries.push(second);
    assert!(SessionCapture::try_from_dto(reversed, CaptureLimits::default()).is_err());
    let mut duplicate_source = capture.to_dto();
    duplicate_source.view_entry_count = 2;
    let mut second = duplicate_source.entries[0].clone();
    second.entry_id = "distinct-id".into();
    second.order = 1;
    duplicate_source.entries.push(second);
    assert!(matches!(
        SessionCapture::try_from_dto(duplicate_source, CaptureLimits::default()),
        Err(CaptureError::Invalid("duplicate source position"))
    ));
}

#[test]
fn raw_utf8_tool_input_and_result_are_lossless_without_preview_markers() {
    let arguments = "\"quoted\"\\path\n日本語🙂".repeat(300);
    let result = "result\n\t\\\"λ🙂".repeat(400);
    let capture = SessionCapture::new(
        "s",
        "seg",
        vec![
            Item::tool_call("call", "tool", arguments.clone()),
            Item::tool_result_item("call", "summary", Some(result.clone()), false),
        ],
    );
    for (id, field, expected) in [
        ("E00000000", RawField::ToolArguments, arguments),
        ("E00000001", RawField::ToolContent, result),
    ] {
        let reference = reference(&capture, id);
        let mut offset = 0;
        let mut actual = String::new();
        loop {
            let part = capture
                .read_raw(&reference, CapturePurpose::Observation, field, offset, 17)
                .unwrap();
            assert!(part.text.len() <= 17);
            assert_eq!(part.offset, offset);
            actual.push_str(&part.text);
            match part.next_offset {
                Some(next) => {
                    assert!(next > offset);
                    offset = next;
                }
                None => break,
            }
        }
        assert_eq!(actual, expected);
    }
    let unicode = SessionCapture::new("s", "seg", vec![Item::tool_call("call", "tool", "🙂")]);
    let reference = reference(&unicode, "E00000000");
    assert!(matches!(
        unicode.read_raw(
            &reference,
            CapturePurpose::Observation,
            RawField::ToolArguments,
            1,
            8
        ),
        Err(CaptureError::InvalidRange)
    ));
    assert!(matches!(
        unicode.read_raw(
            &reference,
            CapturePurpose::Observation,
            RawField::ToolArguments,
            0,
            3
        ),
        Err(CaptureError::BudgetTooSmall)
    ));
    assert!(matches!(
        unicode.read_raw(
            &reference,
            CapturePurpose::Observation,
            RawField::ToolArguments,
            5,
            8
        ),
        Err(CaptureError::InvalidRange)
    ));
}

#[test]
fn escaped_json_slice_budget_includes_metadata_and_bound_continuation() {
    let body = "\"\\\n\t🙂日本語".repeat(500);
    let capture = SessionCapture::new(
        "synthetic-session",
        "segment",
        vec![Item::tool_call("call", "tool", body.clone())],
    );
    let other_session = SessionCapture::new(
        "other-session",
        "segment",
        vec![Item::tool_call("call", "tool", body.clone())],
    );
    let changed = SessionCapture::new(
        "synthetic-session",
        "segment",
        vec![Item::tool_call("call", "tool", "changed")],
    );
    let mut cursor = None;
    let mut actual = String::new();
    let budget = 1400;
    loop {
        let part = capture
            .slice(CapturePurpose::Recording, cursor.as_ref(), 1, budget)
            .unwrap();
        assert!(part.encoded_len() <= budget);
        assert_eq!(part.encoded_len(), part.as_json().len());
        assert_eq!(serde_json::to_vec(part.slice()).unwrap(), part.as_json());
        assert_eq!(part.slice().items.len(), 1);
        let item = &part.slice().items[0];
        assert_eq!(item.offset, actual.len());
        actual.push_str(&item.text);
        cursor = part.slice().next_cursor.clone();
        if let Some(cursor) = &cursor {
            assert!(matches!(
                other_session.slice(CapturePurpose::Recording, Some(cursor), 1, budget),
                Err(CaptureError::ForeignReference)
            ));
            assert!(matches!(
                changed.slice(CapturePurpose::Recording, Some(cursor), 1, budget),
                Err(CaptureError::ForeignReference)
            ));
            assert!(matches!(
                capture.slice(CapturePurpose::Observation, Some(cursor), 1, budget),
                Err(CaptureError::ForeignReference)
            ));
        } else {
            break;
        }
    }
    assert_eq!(actual, body);
    assert!(matches!(
        capture.slice(CapturePurpose::Recording, None, 1, 8),
        Err(CaptureError::BudgetTooSmall)
    ));
    assert!(matches!(
        capture.slice(CapturePurpose::Recording, None, 0, budget),
        Err(CaptureError::BudgetTooSmall)
    ));
}

#[test]
fn snapshot_immutability_and_scoped_operations_reject_foreign_or_unknown_refs_and_bad_ranges() {
    let mut source = vec![
        Item::user_message("before"),
        Item::assistant_message("answer"),
    ];
    let capture = SessionCapture::new("s", "seg", source.clone());
    let fixed_bytes = capture.encode(CaptureLimits::default()).unwrap();
    let first = reference(&capture, "E00000000");
    let second = reference(&capture, "E00000001");
    source.push(Item::user_message("appended"));
    source[0] = Item::user_message("compacted");
    source.clear();
    assert_eq!(
        capture.encode(CaptureLimits::default()).unwrap(),
        fixed_bytes
    );
    assert_eq!(
        capture.clone().encode(CaptureLimits::default()).unwrap(),
        fixed_bytes
    );
    let foreign = SessionCapture::new(
        "different",
        "seg",
        vec![
            Item::user_message("before"),
            Item::assistant_message("answer"),
        ],
    );
    assert!(matches!(
        foreign.resolve_ref(&first, CapturePurpose::Observation),
        Err(CaptureError::ForeignReference)
    ));
    assert!(matches!(
        capture.checked_range(&second, &first, CapturePurpose::Observation),
        Err(CaptureError::InvalidRange)
    ));
    assert_eq!(
        capture
            .checked_range(&first, &second, CapturePurpose::Observation)
            .unwrap()
            .len(),
        2
    );
    let unknown = SessionEntryRef::parse("E99999999").unwrap();
    assert_eq!(capture.view_order_for_ref(&unknown), None);
    assert!(matches!(
        capture.scoped_ref(&unknown),
        Err(CaptureError::UnknownReference)
    ));
    assert!(
        capture
            .search_checked(&SearchOptions {
                from: Some(unknown.clone()),
                ..Default::default()
            })
            .is_err()
    );
    assert!(
        capture
            .search(&SearchOptions {
                through: Some(unknown),
                ..Default::default()
            })
            .is_empty()
    );
    assert!(
        capture
            .read_checked(ReadSelector::EntryRange([1, 0]), ReadOptions::default())
            .is_err()
    );
    assert!(
        capture
            .read_checked(ReadSelector::EntryRange([0, 99]), ReadOptions::default())
            .is_err()
    );
}

#[test]
fn slices_preserve_empty_and_long_result_fields_under_item_and_metadata_budgets() {
    let summary = "summary\n\"🙂".repeat(100);
    let content = "content\t\\日本語".repeat(200);
    let capture = SessionCapture::new(
        "s",
        "seg",
        vec![
            Item::tool_result_item("first", summary.clone(), Some(content.clone()), true),
            Item::tool_result_item("empty", "", Some(String::new()), false),
        ],
    );
    let mut cursor = None;
    let mut fields: Vec<(String, RawField, String)> = Vec::new();
    loop {
        let part = capture
            .slice(CapturePurpose::Recording, cursor.as_ref(), 2, 1500)
            .unwrap();
        assert!(part.slice().items.len() <= 2);
        assert!(serde_json::to_vec(part.slice()).unwrap().len() <= 1500);
        for item in &part.slice().items {
            let id = item.reference.entry_ref.to_string();
            if let Some((last_id, last_field, text)) = fields.last_mut() {
                if *last_id == id && *last_field == item.field {
                    assert_eq!(text.len(), item.offset);
                    text.push_str(&item.text);
                    continue;
                }
            }
            assert_eq!(item.offset, 0);
            fields.push((id, item.field, item.text.clone()));
        }
        cursor = part.slice().next_cursor.clone();
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(
        fields,
        vec![
            ("E00000000".into(), RawField::ToolSummary, summary),
            ("E00000000".into(), RawField::ToolContent, content),
            ("E00000001".into(), RawField::ToolSummary, String::new()),
            ("E00000001".into(), RawField::ToolContent, String::new()),
        ]
    );
    let metadata_heavy = SessionCapture::new(
        "s",
        "seg",
        vec![Item::tool_call("call", "name".repeat(200), "x")],
    );
    assert!(matches!(
        metadata_heavy.slice(CapturePurpose::Recording, None, 1, 700),
        Err(CaptureError::BudgetTooSmall)
    ));
}

#[test]
fn evidence_uses_original_segment_positions_after_roundtrip_and_view_selection() {
    let full = SessionCapture::new(
        "s",
        "selected",
        vec![
            Item::user_message("first"),
            Item::assistant_message("second"),
        ],
    );
    let mut dto = full.to_dto();
    dto.entries[0].source.segment_id = "original-a".into();
    dto.entries[0].source.entry_index = 80;
    dto.entries[1].source.segment_id = "original-b".into();
    dto.entries[1].source.entry_index = 3;
    let encoded = dto.encode(CaptureLimits::default()).unwrap();
    let restored = SessionCapture::decode(&encoded, CaptureLimits::default()).unwrap();
    for (id, segment, index, order) in [
        ("E00000000", "original-a", 80, 0),
        ("E00000001", "original-b", 3, 1),
    ] {
        let evidence = restored.evidence_for(id).unwrap();
        assert_eq!(evidence.segment_id, segment);
        assert_eq!(evidence.entry_range, [index; 2]);
        let read = restored.read(ReadSelector::Id(id), ReadOptions::default());
        assert_eq!(read.entries[0].entry_range, [order; 2]);
    }
    let selected = restored.select_order_range(1..2).unwrap();
    assert_eq!(selected.overview()[0].entry_range, [0, 0]);
    let evidence = selected.evidence_for("E00000001").unwrap();
    assert_eq!(evidence.segment_id, "original-b");
    assert_eq!(evidence.entry_range, [3, 3]);
    let roundtrip = SessionCapture::decode(
        &selected.encode(CaptureLimits::default()).unwrap(),
        CaptureLimits::default(),
    )
    .unwrap();
    assert_eq!(roundtrip.to_dto(), selected.to_dto());
    assert_eq!(
        roundtrip.evidence_for("E00000001").unwrap().entry_range,
        [3, 3]
    );
}

#[test]
fn fixed_history_selection_preserves_original_coordinates_and_checks_private_position_bounds() {
    let history = vec![
        history("old", Item::user_message("old"), None),
        history("hidden", Item::reasoning("PRIVATE"), None),
        history("selected", Item::user_message("selected"), None),
        history("private-tail", Item::system_message("PRIVATE"), None),
    ];
    let full = SessionCapture::from_complete_history("s", "seg", history);
    let suffix = full.select_order_range(1..4).unwrap();
    let original = full
        .entries()
        .find(|entry| entry.entry_id == "selected")
        .unwrap();
    let selected = suffix.entries().next().unwrap();
    assert_eq!(selected.entry_id, original.entry_id);
    assert_eq!(selected.source, original.source);
    assert_eq!(selected.body, original.body);
    assert_eq!(selected.policy, original.policy);
    assert!(suffix.evidence_for("Eold").is_none());
    assert_eq!(suffix.search(&SearchOptions::default()).len(), 1);
    assert_eq!(
        suffix.view_order_for_ref(&SessionEntryRef::parse("Eselected").unwrap()),
        Some(1)
    );
    assert_eq!(selected.source.entry_index, 2);
    assert_eq!(selected.order, 1);
    assert_eq!(suffix.to_dto().view_entry_count, 3);
    assert_eq!(
        suffix.evidence_for("Eselected").unwrap().entry_range,
        [2, 2]
    );
    let nested = suffix.select_order_range(1..2).unwrap();
    assert_eq!(nested.entries().next().unwrap().order, 0);
    assert_eq!(nested.entries().next().unwrap().source, original.source);
    let restored = SessionCapture::decode(
        &suffix.encode(CaptureLimits::default()).unwrap(),
        CaptureLimits::default(),
    )
    .unwrap();
    assert_eq!(restored.to_dto(), suffix.to_dto());
    assert!(full.select_order_range(4..5).is_err());
    let reversed = std::ops::Range { start: 3, end: 2 };
    assert!(full.select_order_range(reversed).is_err());
    let hidden_only = full.select_order_range(3..4).unwrap();
    assert_eq!(hidden_only.entries().len(), 0);
    assert_eq!(hidden_only.to_dto().view_entry_count, 1);
    let mut invalid = full.to_dto();
    invalid.view_entry_count = 2; // the selected entry's view order is outside this extent
    assert!(SessionCapture::try_from_dto(invalid, CaptureLimits::default()).is_err());
    assert_eq!(
        full.retain_entries(|entry| entry.entry_id == "selected")
            .entries()
            .next()
            .unwrap()
            .source,
        original.source
    );
    assert_eq!(full.to_dto().view_entry_count, 4);
}

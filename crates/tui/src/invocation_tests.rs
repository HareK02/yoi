//! T-696 client contract regressions, exercised through real composer selection.
use crate::{app::App, input::InputBuffer};
use protocol::*;

pub(crate) fn descriptor() -> FeatureInvocationDescriptor {
    FeatureInvocationDescriptor {
        identity: FeatureInvocationIdentity("plugin:test/run".into()),
        name: "run".into(),
        aliases: vec!["execute".into()],
        display_name: "Run".into(),
        description: "Run a declared action".into(),
        syntax: FeatureInvocationSyntax::Parenthesized,
        arguments: vec![
            InvocationArgumentDescriptor {
                name: "path".into(),
                position: Some(0),
                required: true,
                value_type: InvocationArgumentType::WorkerFile,
                completion: InvocationCompletion::WorkerFile,
                description: None,
            },
            InvocationArgumentDescriptor {
                name: "mode".into(),
                position: None,
                required: false,
                value_type: InvocationArgumentType::Enum {
                    values: vec!["safe".into(), "fast".into()],
                },
                completion: InvocationCompletion::Static {
                    values: vec!["safe".into(), "fast".into()],
                },
                description: None,
            },
            InvocationArgumentDescriptor {
                name: "count".into(),
                position: None,
                required: false,
                value_type: InvocationArgumentType::Integer,
                completion: InvocationCompletion::None,
                description: None,
            },
            InvocationArgumentDescriptor {
                name: "force".into(),
                position: None,
                required: false,
                value_type: InvocationArgumentType::Boolean,
                completion: InvocationCompletion::None,
                description: None,
            },
        ],
        client_adapter: None,
    }
}

pub(crate) fn attachment_descriptor() -> FeatureInvocationDescriptor {
    let mut descriptor = descriptor();
    // No hardcoded builtin identity, public name, or argument name in adapter dispatch.
    descriptor.identity = FeatureInvocationIdentity("plugin:files/add".into());
    descriptor.name = "add-file".into();
    descriptor.aliases.clear();
    descriptor.arguments.truncate(1);
    descriptor.arguments[0].name = "resource".into();
    descriptor.arguments[0].value_type = InvocationArgumentType::ClientFile;
    descriptor.arguments[0].completion = InvocationCompletion::ClientFile;
    descriptor.client_adapter = Some(InvocationClientAdapter::Attachment);
    descriptor
}

pub(crate) fn uploaded_file() -> UploadedFileRef {
    UploadedFileRef {
        artifact_id: "artifact-owned".into(),
        file_name: "資料 a.txt".into(),
        media_type: "text/plain".into(),
        created_at_ms: 1,
        availability: UploadedFileAvailability::Available,
        byte_len: 4,
        sha256: "a".repeat(64),
        source_entry_id: None,
    }
}

pub(crate) fn select(
    app: &mut App,
    descriptor: FeatureInvocationDescriptor,
    name: &str,
) -> Option<Method> {
    app.insert_char('/');
    assert!(matches!(
        app.refresh_completion(),
        Some(Method::ListCompletions {
            kind: CompletionKind::Feature,
            ..
        })
    ));
    app.handle_worker_event(Event::Completions {
        request_id: app.completion_request_id(),
        kind: CompletionKind::Feature,
        prefix: String::new(),
        context: None,
        entries: vec![CompletionEntry {
            value: name.into(),
            invocation: Some(descriptor),
            ..Default::default()
        }],
    });
    app.apply_completion_text()
}

fn complete(app: &mut App, value: &str, is_dir: bool) -> Option<Method> {
    let state = app.completion.as_ref().expect("argument completion");
    app.handle_worker_event(Event::Completions {
        request_id: app.completion_request_id(),
        kind: state.kind,
        prefix: state.prefix.clone(),
        context: state.context.clone(),
        entries: vec![CompletionEntry {
            value: value.into(),
            is_dir,
            ..Default::default()
        }],
    });
    app.apply_completion_text()
}

#[test]
fn feature_invocation_discovery_alias_selection_and_confirmation_are_occurrence_scoped() {
    let mut app = App::new("test".into());
    app.input.insert_str("説明 ");
    assert!(
        matches!(select(&mut app, descriptor(), "execute"), Some(Method::ListCompletions {
        kind: CompletionKind::FeatureArgument, context: Some(CompletionContext { argument: Some(argument), .. }), ..
    }) if argument == "path")
    );
    app.input
        .insert_str("\"資料/a b\") after /execute(\"not selected\")");
    assert!(
        app.submit_input().is_none(),
        "first Enter confirms typed chips, not Send"
    );
    let segments = app.input.submit_segments();
    assert!(
        matches!(&segments[1], Segment::FeatureInvoke { invocation } if invocation.identity == descriptor().identity && invocation.name == "execute")
    );
    assert!(
        matches!(segments.last(), Some(Segment::Text { content }) if content == " after /execute(\"not selected\")")
    );
    assert!(matches!(app.submit_input(), Some(Method::Submit { input, .. }) if input == segments));
}

#[test]
fn feature_invocation_raw_text_paste_and_unknown_payloads_do_not_dispatch() {
    let mut app = App::new("test".into());
    app.input
        .insert_str("/run(x) /attach(/tmp/a.txt) /clear-attachments");
    assert!(
        matches!(app.submit_input(), Some(Method::Submit { input, .. }) if matches!(input.as_slice(), [Segment::Text { .. }]))
    );
    let unknown: Segment =
        serde_json::from_str(r#"{"kind":"future_invocation","identity":"plugin:test/run"}"#)
            .unwrap();
    app.input.replace_with_segments(&[unknown]);
    assert_eq!(app.input.submit_segments(), vec![Segment::Unknown]);
    let mut input = InputBuffer::new();
    input.insert_paste("/run(x) ".repeat(20));
    input.finalize_feature_invocations().unwrap();
    assert!(matches!(
        input.submit_segments().as_slice(),
        [Segment::Paste { .. }]
    ));
}

#[test]
fn feature_invocation_argument_insertion_quotes_unicode_spaces_escapes_and_keeps_prose() {
    let mut app = App::new("test".into());
    app.input.insert_str("界 ");
    select(&mut app, descriptor(), "run");
    let value = "資料/a b/quote\"slash\\end";
    complete(&mut app, value, false);
    app.input.insert_str(", mode=sa");
    app.refresh_completion();
    complete(&mut app, "safe", false);
    app.input.insert_str(", count=1");
    app.refresh_completion();
    complete(&mut app, "12", false);
    app.input.insert_str(", force=f");
    app.refresh_completion();
    complete(&mut app, "false", false);
    app.input.insert_str(") trailing prose");
    assert!(app.submit_input().is_none());
    let segments = app.input.submit_segments();
    let Segment::FeatureInvoke { invocation } = &segments[1] else {
        panic!("typed chip expected")
    };
    assert_eq!(
        invocation.arguments[0].value,
        InvocationValue::String(value.into())
    );
    assert_eq!(invocation.arguments[2].value, InvocationValue::Integer(12));
    assert_eq!(
        invocation.arguments[3].value,
        InvocationValue::Boolean(false)
    );
    assert!(
        matches!(segments.last(), Some(Segment::Text { content }) if content == " trailing prose")
    );
}

#[test]
fn feature_invocation_argument_names_and_directory_drill_in_are_safe() {
    let mut app = App::new("test".into());
    select(&mut app, descriptor(), "run");
    assert!(
        matches!(complete(&mut app, "資料 dir", true), Some(Method::ListCompletions { prefix, .. }) if prefix == "資料 dir/")
    );
    complete(&mut app, "資料 dir/file name.txt", false);
    app.input.insert_str(", mo");
    app.refresh_completion();
    assert!(
        app.completion
            .as_ref()
            .unwrap()
            .context
            .as_ref()
            .unwrap()
            .argument
            .is_none()
    );
    assert!(
        matches!(complete(&mut app, "mode=", false), Some(Method::ListCompletions { context: Some(CompletionContext { argument: Some(argument), .. }), .. }) if argument == "mode")
    );
    complete(&mut app, "fast", false);
    app.input.insert_char(')');
    assert!(app.submit_input().is_none());
    assert!(matches!(
        app.input.submit_segments().as_slice(),
        [Segment::FeatureInvoke { .. }]
    ));
}

#[test]
fn feature_invocation_completion_replaces_middle_value_without_suffix_corruption() {
    let mut input = InputBuffer::new();
    input.insert_str("界 /run(path=  \"資料/old name\", mode=safe) prose");
    input.select_feature_invocation(2, descriptor());
    input.move_home();
    for _ in 0.."界 /run(path=  \"資料/ol".chars().count() {
        input.move_right();
    }
    let (_, before, prefix, context) = input
        .pending_feature_argument_completion(&[descriptor()])
        .unwrap();
    assert_eq!(prefix, "資料/ol");
    assert_eq!(context.argument.as_deref(), Some("path"));
    let start = before; // preceding atom is the opening quote
    input.replace_argument_completion(start, &quote_invocation_string("資料/new name"));
    assert_eq!(
        input.plain_text(),
        "界 /run(path=  \"資料/new name\", mode=safe) prose"
    );
    input.finalize_feature_invocations().unwrap();
    assert!(
        matches!(input.submit_segments().last(), Some(Segment::Text { content }) if content == " prose")
    );
}

#[test]
fn feature_invocation_validation_is_atomic_for_multiple_selected_calls() {
    for invalid in [
        "/run()",
        "/run(x, nope=y)",
        "/run(x, mode=unknown)",
        "/run(x, count=oops)",
        "/run(x, force=yes)",
        "/run(x, path=y)",
        "/run(\"unfinished",
    ] {
        let mut input = InputBuffer::new();
        input.insert_str("/run(ok) then ");
        let second = input.plain_text().chars().count();
        input.insert_str(invalid);
        input.select_feature_invocation(0, descriptor());
        input.select_feature_invocation(second, descriptor());
        let before = input.submit_segments();
        assert!(input.finalize_feature_invocations().is_err(), "{invalid}");
        assert_eq!(input.submit_segments(), before);
    }
}

#[test]
fn feature_invocation_selection_tracks_cursor_edits_and_does_not_survive_deleting_its_sigil() {
    let mut input = InputBuffer::new();
    input.insert_str("/run(x)");
    input.select_feature_invocation(0, descriptor());
    input.move_home();
    input.insert_str("prefix ");
    input.finalize_feature_invocations().unwrap();
    assert!(matches!(
        &input.submit_segments()[1],
        Segment::FeatureInvoke { .. }
    ));
    input.replace_with_segments(&[Segment::text("/run(y)")]);
    input.select_feature_invocation(0, descriptor());
    input.move_home();
    input.delete_after();
    input.insert_char('/');
    input.finalize_feature_invocations().unwrap();
    assert_eq!(input.submit_segments(), vec![Segment::text("/run(y)")]);
}

#[test]
fn feature_invocation_chip_reedit_preserves_identity_id_and_typed_arguments() {
    let parsed = parse_feature_invocation(
        "/execute(\"a b\", mode=safe, count=2)",
        0,
        &descriptor(),
        "stable-id",
    )
    .unwrap()
    .invocation;
    let segments = vec![
        Segment::FeatureInvoke {
            invocation: parsed.clone(),
        },
        Segment::UploadedFile {
            file: uploaded_file(),
        },
        Segment::Flow {
            selector: "review".into(),
        },
        Segment::FileRef {
            path: "src/".into(),
        },
    ];
    let mut input = InputBuffer::new();
    input.replace_with_segments(&segments);
    assert_eq!(input.submit_segments(), segments);
    assert!(input.edit_feature_invocation("stable-id", descriptor()));
    input.finalize_feature_invocations().unwrap();
    assert_eq!(input.submit_segments(), segments);
    assert!(!input.edit_feature_invocation("stable-id", attachment_descriptor()));
}

#[test]
fn feature_invocation_reedit_unknown_disabled_or_stale_keeps_original_chip() {
    let invocation = parse_feature_invocation("/run(x)", 0, &descriptor(), "id")
        .unwrap()
        .invocation;
    let mut app = App::new("test".into());
    let segments = vec![Segment::FeatureInvoke { invocation }];
    app.input.replace_with_segments(&segments);
    assert!(matches!(
        app.edit_adjacent_feature_invocation(),
        Some(Method::ListCompletions { .. })
    ));
    app.handle_worker_event(Event::Completions {
        request_id: app.completion_request_id(),
        kind: CompletionKind::Feature,
        prefix: "run".into(),
        context: None,
        entries: Vec::new(),
    });
    assert_eq!(app.input.submit_segments(), segments);
    app.edit_adjacent_feature_invocation();
    app.input.move_home();
    app.refresh_completion();
    app.handle_worker_event(Event::Completions {
        request_id: app.completion_request_id(),
        kind: CompletionKind::Feature,
        prefix: "run".into(),
        context: None,
        entries: vec![CompletionEntry {
            value: "run".into(),
            invocation: Some(descriptor()),
            ..Default::default()
        }],
    });
    assert_eq!(app.input.submit_segments(), segments);
}

#[test]
fn feature_invocation_generic_attachment_adapter_never_records_or_sends_local_path() {
    let mut app = App::new("test".into());
    select(&mut app, attachment_descriptor(), "add-file");
    app.input.insert_str("\"/tmp/資料 a.txt\") keep this prose");
    assert!(app.submit_input().is_none());
    assert_eq!(app.input_history_len(), 0);
    let adapters = app.attachment_invocations();
    assert_eq!(adapters.len(), 1);
    assert_eq!(adapters[0].1.to_str().unwrap(), "/tmp/資料 a.txt");
    app.input
        .stage_attachment(&adapters[0].0, "staging-id".into(), "資料 a.txt".into());
    assert!(app.submit_input().is_none());
    assert!(app.submit_notify_input().is_none());
    app.input
        .finish_attachment_stage("staging-id", Some(uploaded_file()));
    let Method::Submit { input, .. } = app.submit_input().unwrap() else {
        panic!("submit expected")
    };
    assert!(matches!(&input[0], Segment::UploadedFile { .. }));
    assert!(!serde_json::to_string(&input).unwrap().contains("/tmp/"));
    assert!(
        matches!(input.last(), Some(Segment::Text { content }) if content == " keep this prose")
    );
}

#[test]
fn feature_invocation_staging_deletion_and_late_completion_do_not_resurrect_chips() {
    for delete in [0, 1, 2] {
        let invocation = parse_feature_invocation("/run(x)", 0, &descriptor(), "invoke")
            .unwrap()
            .invocation;
        let mut input = InputBuffer::new();
        input.replace_with_segments(&[Segment::FeatureInvoke { invocation }]);
        input.stage_attachment("invoke", "stage".into(), "a.txt".into());
        match delete {
            0 => input.delete_before(),
            1 => {
                input.move_left();
                input.delete_after();
            }
            _ => input.delete_word_before(),
        }
        assert_eq!(input.take_removed_attachment_stages(), vec!["stage"]);
        assert!(!input.finish_attachment_stage("stage", Some(uploaded_file())));
        assert!(input.is_empty());
    }
}

#[test]
fn feature_invocation_typed_history_draft_transport_and_rewind_restore_without_flattening() {
    let invocation = parse_feature_invocation(
        "/run(\"資料/a b\", mode=safe)",
        0,
        &descriptor(),
        "stable-id",
    )
    .unwrap()
    .invocation;
    let segments = vec![
        Segment::text("before "),
        Segment::FeatureInvoke { invocation },
        Segment::UploadedFile {
            file: uploaded_file(),
        },
        Segment::Paste {
            id: 2,
            chars: 4,
            lines: 1,
            content: "data".into(),
        },
        Segment::FileRef {
            path: "src/".into(),
        },
        Segment::Flow {
            selector: "review".into(),
        },
    ];
    let mut app = App::new("test".into());
    app.input.replace_with_segments(&segments);
    let method = app.submit_input().unwrap();
    app.restore_unsent_run(&method);
    assert_eq!(app.input.submit_segments(), segments);
    app.input.clear();
    let draft = vec![
        Segment::text("draft "),
        Segment::UploadedFile {
            file: uploaded_file(),
        },
    ];
    app.input.replace_with_segments(&draft);
    app.input.move_home();
    assert!(app.browse_input_history_older());
    assert_eq!(app.input.submit_segments(), segments);
    assert!(app.browse_input_history_newer());
    assert_eq!(app.input.submit_segments(), draft);
    app.input.clear();
    app.handle_worker_event(Event::RewindApplied {
        session: SessionSnapshot {
            entries: vec![],
            pending_submissions: PendingSubmissionsSnapshot::default(),
        },
        input: segments.clone(),
        summary: RewindSummary {
            truncated_to_entries: 1,
            discarded_entries: 2,
            tool_side_effect_warning: false,
        },
    });
    assert_eq!(app.input.submit_segments(), segments);
}

#[test]
fn feature_invocation_stale_argument_context_and_unvalidated_discovery_entries_are_ignored() {
    let mut app = App::new("test".into());
    select(&mut app, descriptor(), "run");
    let state = app.completion.as_ref().unwrap();
    let prefix = state.prefix.clone();
    let mut context = state.context.clone().unwrap();
    context.invocation = FeatureInvocationIdentity("plugin:other/run".into());
    app.handle_worker_event(Event::Completions {
        request_id: app.completion_request_id(),
        kind: CompletionKind::FeatureArgument,
        prefix,
        context: Some(context),
        entries: vec![CompletionEntry {
            value: "unsafe".into(),
            ..Default::default()
        }],
    });
    assert!(app.completion.as_ref().unwrap().entries.is_empty());
    let mut app = App::new("test".into());
    app.insert_char('/');
    app.refresh_completion();
    let mut invalid = descriptor();
    invalid.identity = FeatureInvocationIdentity("unqualified".into());
    app.handle_worker_event(Event::Completions {
        request_id: app.completion_request_id(),
        kind: CompletionKind::Feature,
        prefix: String::new(),
        context: None,
        entries: vec![CompletionEntry {
            value: "run".into(),
            invocation: Some(invalid),
            ..Default::default()
        }],
    });
    assert!(app.apply_completion_text().is_none());
    assert_eq!(app.input.plain_text(), "/");
}

#[test]
fn feature_invocation_multiple_selected_calls_preserve_prose_and_source_qualified_identity() {
    let mut app = App::new("test".into());
    select(&mut app, descriptor(), "run");
    app.input.insert_str("x) prose ");
    let mut other = descriptor();
    other.identity = FeatureInvocationIdentity("plugin:other/second".into());
    other.name = "second".into();
    other.aliases.clear();
    select(&mut app, other.clone(), "second");
    app.input.insert_str("\"y z\") done");
    assert!(app.submit_input().is_none());
    let segments = app.input.submit_segments();
    assert!(
        matches!(&segments[0], Segment::FeatureInvoke { invocation } if invocation.identity == descriptor().identity)
    );
    assert!(matches!(&segments[1], Segment::Text { content } if content == " prose "));
    assert!(
        matches!(&segments[2], Segment::FeatureInvoke { invocation } if invocation.identity == other.identity)
    );
    assert!(matches!(&segments[3], Segment::Text { content } if content == " done"));
}

#[test]
fn feature_invocation_range_replacement_reports_each_removed_upload_and_stage() {
    let invocation = parse_feature_invocation("/run(x)", 0, &descriptor(), "id")
        .unwrap()
        .invocation;
    let mut input = InputBuffer::new();
    input.replace_with_segments(&[
        Segment::UploadedFile {
            file: uploaded_file(),
        },
        Segment::text(" "),
        Segment::FeatureInvoke { invocation },
    ]);
    input.stage_attachment("id", "stage".into(), "a.txt".into());
    input.replace_with_text_at(0, "replacement");
    assert_eq!(input.take_removed_uploaded_files(), vec![uploaded_file()]);
    assert_eq!(input.take_removed_attachment_stages(), vec!["stage"]);
    assert_eq!(input.plain_text(), "replacement");
}

#[test]
fn feature_invocation_argument_positions_ignore_previous_named_arguments() {
    let mut input = InputBuffer::new();
    input.insert_str("/run(mode=safe, \"資料/a");
    input.select_feature_invocation(0, descriptor());
    let (_, _, prefix, context) = input
        .pending_feature_argument_completion(&[descriptor()])
        .unwrap();
    assert_eq!(context.argument.as_deref(), Some("path"));
    assert_eq!(prefix, "資料/a");
}

#[test]
fn feature_invocation_name_completion_replaces_suffix_at_middle_cursor() {
    let mut app = App::new("test".into());
    app.input.insert_str("/run() prose");
    app.input.move_home();
    app.input.move_right();
    app.input.move_right();
    app.refresh_completion();
    app.handle_worker_event(Event::Completions {
        request_id: app.completion_request_id(),
        kind: CompletionKind::Feature,
        prefix: "r".into(),
        context: None,
        entries: vec![CompletionEntry {
            value: "run".into(),
            invocation: Some(descriptor()),
            ..Default::default()
        }],
    });
    app.apply_completion_text();
    assert_eq!(app.input.plain_text(), "/run() prose");
}

#[test]
fn feature_invocation_discovery_expands_aliases_from_host_metadata() {
    let mut app = App::new("test".into());
    app.input.insert_str("/exe");
    app.refresh_completion();
    app.handle_worker_event(Event::Completions {
        request_id: app.completion_request_id(),
        kind: CompletionKind::Feature,
        prefix: "exe".into(),
        context: None,
        entries: vec![CompletionEntry {
            value: "run".into(),
            invocation: Some(descriptor()),
            ..Default::default()
        }],
    });
    assert_eq!(app.completion.as_ref().unwrap().entries[0].value, "execute");
    app.apply_completion_text();
    assert_eq!(app.input.plain_text(), "/execute(");
}

#[test]
fn feature_invocation_named_arguments_are_suggested_alongside_positional_values() {
    let mut app = App::new("test".into());
    select(&mut app, descriptor(), "run");
    app.input.insert_str("pa");
    app.refresh_completion();
    let state = app.completion.as_ref().unwrap();
    app.handle_worker_event(Event::Completions {
        request_id: app.completion_request_id(),
        kind: state.kind,
        prefix: state.prefix.clone(),
        context: state.context.clone(),
        entries: Vec::new(),
    });
    assert_eq!(app.completion.as_ref().unwrap().entries[0].value, "path=");
    app.apply_completion_text();
    assert_eq!(app.input.plain_text(), "/run(path=");
    app.input.insert_str("\"資料/path\")");
    assert!(app.submit_input().is_none());
    assert!(matches!(
        app.input.submit_segments().as_slice(),
        [Segment::FeatureInvoke { .. }]
    ));
}

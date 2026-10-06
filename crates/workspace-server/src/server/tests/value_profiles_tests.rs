use super::*;
use crate::profile_settings::{
    build_virtual_profile_config_bundle, project_profiles_from_workspace_config,
};

const PROFILE: &str = r#"{
    slug = "alpha";
    worker = { mode = "wip"; };
    model = { ref = "codex-oauth/gpt-5.6-sol"; };
    feature = { task = { enabled = true; }; ticket = { enabled = false; }; };
}"#;

fn request(
    api: &WorkspaceApi,
    source: &str,
    extra: Vec<config_source::ConfigTreeChange>,
) -> ConfigCommitRequest {
    let state = api
        .config_store
        .load_workspace_config(TEST_WORKSPACE_ID)
        .unwrap()
        .unwrap();
    let main = state
        .snapshot
        .get(&config_source::VirtualPath::parse("main.dcdl").unwrap())
        .unwrap();
    let mut changes = vec![config_source::ConfigTreeChange::Update {
        path: main.path.clone(),
        expected_digest: main.content_digest.clone(),
        content: source.into(),
    }];
    changes.extend(extra);
    ConfigCommitRequest {
        base_revision: state.snapshot.revision,
        base_digest: state.snapshot.digest,
        entrypoints: state.contract.entrypoints,
        changes,
    }
}

fn source(value: &str) -> String {
    format!(
        r#"{{ profile = {{ default_profile = "project:alpha"; entries = [{{ selector = "project:alpha"; label = "Alpha"; description = "Value recipe"; profile = {value}; }}]; }}; }}"#
    )
}

#[tokio::test]
async fn value_profiles_builtin_http_config_tree_is_read_only_and_never_falls_back() {
    let dir = tempfile::tempdir().unwrap();
    let api = test_api(dir.path()).await;
    let token = seed_test_api_token(api.store.as_ref(), TEST_WORKSPACE_ID);
    let app = build_router(api.clone());
    let tree = format!("/api/w/{TEST_WORKSPACE_ID}/config/source-tree");
    let commit = format!("{tree}/commit");
    let initial = get_json_authenticated(app.clone(), &tree, &token).await;
    let saved = request_json_authenticated(
        app.clone(),
        "POST",
        &commit,
        Some(
            serde_json::to_value(request(
                &api,
                &source(r#"import "$builtin/profiles/companion.dcdl""#),
                vec![config_source::ConfigTreeChange::Create {
                    // An ordinary Workspace file at the matching suffix must not rescue
                    // an unknown builtin import.
                    path: config_source::VirtualPath::parse("profiles/private.dcdl").unwrap(),
                    content_type: config_source::ConfigContentType::Decodal,
                    content: PROFILE.into(),
                }],
            ))
            .unwrap(),
        ),
        &token,
        StatusCode::CREATED,
    )
    .await;
    assert_eq!(
        saved["snapshot"]["revision"].as_u64().unwrap(),
        initial["snapshot"]["revision"].as_u64().unwrap() + 1
    );
    let listed = get_json_authenticated(app.clone(), &tree, &token).await;
    assert_eq!(listed, saved);
    let entries = listed["snapshot"]["entries"].as_object().unwrap();
    assert!(entries.contains_key("profiles/private.dcdl"));
    assert!(
        entries
            .keys()
            .all(|key| key != "$builtin" && !key.starts_with("$builtin/"))
    );
    let shadow = get_json_authenticated(
        app.clone(),
        &format!("{tree}/entries/profiles%2Fprivate.dcdl"),
        &token,
    )
    .await;
    assert_eq!(shadow, entries["profiles/private.dcdl"]);
    assert!(
        shadow["content"]
            .as_str()
            .unwrap()
            .contains("slug = \"alpha\"")
    );
    let active = api
        .config_store
        .load_workspace_config(TEST_WORKSPACE_ID)
        .unwrap()
        .unwrap();
    let main_digest = entries["main.dcdl"]["content_digest"].as_str().unwrap();
    let base = json!({
        "base_revision": listed["snapshot"]["revision"],
        "base_digest": listed["snapshot"]["digest"],
        "entrypoints": listed["contract"]["entrypoints"],
    });
    let reserved = "$builtin/profiles/companion.dcdl";
    for change in [
        json!({ "kind": "create", "path": reserved, "content_type": "decodal", "content": PROFILE }),
        json!({ "kind": "create", "path": "$builtin", "content_type": "text", "content": "shadow" }),
        json!({ "kind": "update", "path": reserved, "expected_digest": "not-a-workspace-entry", "content": PROFILE }),
        json!({ "kind": "delete", "path": reserved, "expected_digest": "not-a-workspace-entry" }),
        json!({ "kind": "rename", "from": "profiles/private.dcdl", "to": reserved, "expected_digest": shadow["content_digest"] }),
        json!({ "kind": "rename", "from": reserved, "to": "profiles/copy.dcdl", "expected_digest": "not-a-workspace-entry" }),
    ] {
        let mut body = base.clone();
        body["changes"] = json!([change]);
        let error = request_json_authenticated(
            app.clone(),
            "POST",
            &commit,
            Some(body),
            &token,
            StatusCode::BAD_REQUEST,
        )
        .await;
        assert!(
            error["message"]
                .as_str()
                .unwrap()
                .contains("builtin namespace is read-only"),
            "{error}"
        );
        assert_eq!(
            get_json_authenticated(app.clone(), &tree, &token).await,
            listed
        );
    }
    let mut body = base;
    body["changes"] = json!([{
        "kind": "update", "path": "main.dcdl", "expected_digest": main_digest,
        "content": source(r#"import "$builtin/profiles/private.dcdl""#),
    }]);
    let error = request_json_authenticated(
        app.clone(),
        "POST",
        &commit,
        Some(body),
        &token,
        StatusCode::BAD_REQUEST,
    )
    .await;
    let message = error["message"].as_str().unwrap();
    assert!(
        message.contains(
            "unknown or non-public read-only builtin source: $builtin/profiles/private.dcdl"
        ),
        "{error}"
    );
    assert!(message.contains("\"kind\":\"import\""), "{error}");
    assert_eq!(get_json_authenticated(app, &tree, &token).await, listed);
    assert_eq!(
        api.config_store
            .load_workspace_config(TEST_WORKSPACE_ID)
            .unwrap()
            .unwrap(),
        active
    );
}

#[tokio::test]
async fn value_profiles_builtin_companion_composition_seals_observed_sources() {
    let dir = tempfile::tempdir().unwrap();
    let api = test_api(dir.path()).await;
    let recipe = r#"(import "$builtin/profiles/companion.dcdl") // {
        worker = { mode = "wip"; };
        feature = {
            task = { enabled = true; };
            ticket = { enabled = false; };
            workspace_config = { enabled = true; };
        };
    }"#;
    let state = commit_workspace_config_tree(
        &api,
        TEST_WORKSPACE_ID,
        &request(
            &api,
            &source(r#"import "./recipes/custom.dcdl""#),
            vec![
                config_source::ConfigTreeChange::Create {
                    path: config_source::VirtualPath::parse("recipes/custom.dcdl").unwrap(),
                    content_type: config_source::ConfigContentType::Decodal,
                    content: recipe.into(),
                },
                // Builtin relative imports must not be shadowed by Workspace files.
                config_source::ConfigTreeChange::Create {
                    path: config_source::VirtualPath::parse("profiles/base.dcdl").unwrap(),
                    content_type: config_source::ConfigContentType::Decodal,
                    content: "{ slug = \"workspace-shadow\"; }".into(),
                },
            ],
        ),
    )
    .unwrap();
    let projection = project_profiles_from_workspace_config(TEST_WORKSPACE_ID, &state).unwrap();
    let bundle = build_virtual_profile_config_bundle(
        &projection,
        &state,
        TEST_WORKSPACE_ID,
        "created",
        "project:alpha",
    )
    .unwrap()
    .unwrap();
    assert_eq!(bundle.metadata.digest, bundle.computed_digest());
    let archive = bundle.profile_source_archive.as_ref().unwrap();
    let verified = archive.verify().unwrap();
    let environment = config_source::SnapshotEnvironment::new(state.snapshot.clone());
    let builtin_sources = environment.builtin_import_sources(&state.contract).unwrap();
    assert_eq!(
        builtin_sources
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec![
            "$builtin/profiles/base.dcdl",
            "$builtin/profiles/companion.dcdl",
        ]
    );
    assert_eq!(
        archive.reference.source_graph.source_count,
        builtin_sources.len() + 1
    );
    assert_eq!(archive.reference.source_graph.import_count, 0);
    assert_eq!(
        verified.manifest().entrypoints["project:alpha"],
        "profiles/evaluated.json"
    );
    for (key, bytes) in &builtin_sources {
        let source = verified
            .manifest()
            .sources
            .iter()
            .find(|source| &source.source_key == key)
            .unwrap();
        assert_eq!(source.path, *key);
        assert_eq!(source.kind, "decodal");
        assert_eq!(
            source.digest,
            worker_runtime::profile_archive::sha256_hex(bytes.as_bytes())
        );
        assert_eq!(source.size_bytes, bytes.len() as u64);
    }
    assert!(
        verified
            .manifest()
            .sources
            .iter()
            .any(|source| source.path == "profiles/evaluated.json" && source.kind == "json")
    );
    // Resolve using the Runtime archive consumer, without access to the config tree.
    let resolved = verified
        .resolve_profile(
            "project:alpha",
            Path::new("/runtime-worker"),
            "builtin-value-worker",
        )
        .unwrap();
    assert_eq!(resolved.worker.mode, manifest::WorkerMode::Wip);
    assert!(resolved.feature.task.enabled);
    assert!(!resolved.feature.ticket.enabled);
    assert!(resolved.feature.workspace_config.enabled);
    let evaluation = environment.evaluate_contract(&state.contract).unwrap();
    assert_eq!(
        resolved.model.ref_.as_deref(),
        evaluation.projections[0]
            .data_json
            .pointer("/profile/entries/0/profile/model/ref")
            .and_then(serde_json::Value::as_str)
    );
    let persisted = manifest::write_persisted_worker_manifest_snapshot(&resolved).unwrap();
    let old_bundle_digest = bundle.metadata.digest.clone();

    let updated = commit_workspace_config_tree(
        &api,
        TEST_WORKSPACE_ID,
        &request(&api, &source(PROFILE), vec![]),
    )
    .unwrap();
    let projection = project_profiles_from_workspace_config(TEST_WORKSPACE_ID, &updated).unwrap();
    let updated_bundle = build_virtual_profile_config_bundle(
        &projection,
        &updated,
        TEST_WORKSPACE_ID,
        "created",
        "project:alpha",
    )
    .unwrap()
    .unwrap();
    assert_ne!(old_bundle_digest, updated_bundle.metadata.digest);
    assert_eq!(
        updated_bundle
            .profile_source_archive
            .as_ref()
            .unwrap()
            .reference
            .source_graph
            .source_count,
        1
    );
    // Editing config changes future snapshots, not a sealed bundle or persisted Worker.
    assert_eq!(bundle.metadata.digest, old_bundle_digest);
    assert_eq!(
        serde_json::to_value(manifest::read_persisted_worker_manifest_snapshot(persisted).unwrap())
            .unwrap(),
        serde_json::to_value(resolved).unwrap()
    );
}

#[tokio::test]
async fn value_profiles_save_project_and_runtime_consume_the_same_revision() {
    let dir = tempfile::tempdir().unwrap();
    let api = test_api(dir.path()).await;
    let forms = [
        PROFILE.to_string(),
        r#"import "./recipes/alpha.dcdl""#.into(),
        r#"(import "./recipes/base.dcdl") // { worker = { mode = "wip"; }; feature.task.enabled = true; }"#.into(),
    ];
    let mut manifests = Vec::new();
    let mut saved_manifest = None;
    for (index, form) in forms.iter().enumerate() {
        let extra = if index == 0 {
            vec![
                config_source::ConfigTreeChange::Create {
                    path: config_source::VirtualPath::parse("recipes/alpha.dcdl").unwrap(),
                    content_type: config_source::ConfigContentType::Decodal,
                    content: PROFILE.into(),
                },
                config_source::ConfigTreeChange::Create {
                    path: config_source::VirtualPath::parse("recipes/base.dcdl").unwrap(),
                    content_type: config_source::ConfigContentType::Decodal,
                    content: PROFILE
                        .replace("wip", "tools")
                        .replace("enabled = true", "enabled = false"),
                },
            ]
        } else {
            vec![]
        };
        let state = commit_workspace_config_tree(
            &api,
            TEST_WORKSPACE_ID,
            &request(&api, &source(form), extra),
        )
        .unwrap();
        let loaded = api
            .config_store
            .load_workspace_config(TEST_WORKSPACE_ID)
            .unwrap()
            .unwrap();
        assert_eq!(loaded, state);
        // The Browser receives the same fingerprint-bound authoring metadata through REST.
        let wire = workspace_config_state_to_api(loaded.clone());
        let decoded: config_source::WorkspaceConfigSchemaBundle =
            serde_json::from_value(serde_json::to_value(wire.contract.schema_bundle).unwrap())
                .unwrap();
        decoded.validate().unwrap();
        assert_eq!(decoded, loaded.contract.schema_bundle);
        assert!(
            decoded
                .contributions
                .iter()
                .find(|entry| entry.provider_id == "builtin:profile")
                .unwrap()
                .authoring_source
                .is_some()
        );
        let projection =
            project_profiles_from_workspace_config(TEST_WORKSPACE_ID, &loaded).unwrap();
        assert_eq!(
            projection.settings.default_profile.as_deref(),
            Some("project:alpha")
        );
        assert_eq!(
            projection.settings.config_revision,
            Some(state.snapshot.revision)
        );
        let selected = projection
            .settings
            .profiles
            .iter()
            .find(|entry| entry.selector == "project:alpha")
            .unwrap();
        assert_eq!(selected.label, "Alpha");
        assert_eq!(selected.description.as_deref(), Some("Value recipe"));
        assert!(selected.is_default);
        assert_eq!(selected.profile_source_id.as_deref(), Some("project:alpha"));
        let bundle = build_virtual_profile_config_bundle(
            &projection,
            &state,
            TEST_WORKSPACE_ID,
            "2026-01-01T00:00:00Z",
            "project:alpha",
        )
        .unwrap()
        .unwrap();
        assert_eq!(bundle.metadata.digest, bundle.computed_digest());
        assert_eq!(
            bundle.metadata.revision,
            state.snapshot.revision.to_string()
        );
        let archive = bundle.profile_source_archive.as_ref().unwrap();
        let verified = archive.verify().unwrap();
        // This is the Runtime's archive consumer. It needs neither the Workspace tree nor imports.
        let resolved = verified
            .resolve_profile(
                "project:alpha",
                Path::new("/runtime-worker"),
                "value-worker",
            )
            .unwrap();
        assert_eq!(resolved.worker.mode, manifest::WorkerMode::Wip);
        assert!(resolved.feature.task.enabled);
        assert!(!resolved.feature.ticket.enabled);
        assert!(!resolved.feature.ticket.authoring);
        assert!(!resolved.feature.sub_worker.enabled);
        assert_eq!(archive.reference.source_graph.import_count, 0);
        let evaluation = config_source::SnapshotEnvironment::new(state.snapshot.clone())
            .evaluate_contract(&state.contract)
            .unwrap();
        let value = evaluation.projections[0]
            .data_json
            .pointer("/profile/entries/0/profile")
            .unwrap();
        assert!(value.is_object());
        assert!(
            value.get("engine").is_none(),
            "omission must remain omission in the Profile value"
        );
        let mut snapshot = serde_json::to_value(&resolved).unwrap();
        snapshot.as_object_mut().unwrap().remove("profile"); // revision-specific provenance only
        manifests.push(snapshot);
        if index == 0 {
            saved_manifest =
                Some(manifest::write_persisted_worker_manifest_snapshot(&resolved).unwrap());
        }
        if index == 2 {
            let mut other_state = state.clone();
            other_state.snapshot.revision += 1;
            assert!(
                build_virtual_profile_config_bundle(
                    &projection,
                    &other_state,
                    TEST_WORKSPACE_ID,
                    "created",
                    "project:alpha"
                )
                .unwrap_err()
                .to_string()
                .contains("does not match")
            );
        }
    }
    assert_eq!(manifests[0], manifests[1]);
    assert_eq!(manifests[1], manifests[2]);
    // Saving a new recipe is not an operation on existing Workers or their persisted Manifest.
    let state = commit_workspace_config_tree(
        &api,
        TEST_WORKSPACE_ID,
        &request(
            &api,
            &source(
                &PROFILE
                    .replace("wip", "tools")
                    .replace("enabled = true", "enabled = false"),
            ),
            vec![],
        ),
    )
    .unwrap();
    let projection = project_profiles_from_workspace_config(TEST_WORKSPACE_ID, &state).unwrap();
    let bundle = build_virtual_profile_config_bundle(
        &projection,
        &state,
        TEST_WORKSPACE_ID,
        "created",
        "project:alpha",
    )
    .unwrap()
    .unwrap();
    let fresh = crate::profile_settings::resolve_profile_manifest_from_config_bundle(
        &bundle,
        "project:alpha",
    )
    .unwrap();
    assert_eq!(fresh.worker.mode, manifest::WorkerMode::Tools);
    assert!(!fresh.feature.task.enabled);
    let restored =
        manifest::read_persisted_worker_manifest_snapshot(saved_manifest.unwrap()).unwrap();
    assert_eq!(restored.worker.mode, manifest::WorkerMode::Wip);
    assert!(restored.feature.task.enabled);
}

#[tokio::test]
async fn value_profiles_scalar_and_table_scopes_analyze_save_and_resolve_equally() {
    let dir = tempfile::tempdir().unwrap();
    let api = test_api(dir.path()).await;
    let scalar = format!(
        r#"{PROFILE} // {{ scope = "workspace_read"; delegation_scope = "workspace_write"; }}"#
    );
    let table = format!(
        r#"{PROFILE} // {{ scope = {{ intent = "workspace_read"; }}; delegation_scope = {{ intent = "workspace_write"; }}; }}"#
    );
    let mut scopes = Vec::new();
    for (index, form) in [scalar.clone(), table, r#"import "./scoped.dcdl""#.into()]
        .iter()
        .enumerate()
    {
        let extra = if index == 0 {
            vec![config_source::ConfigTreeChange::Create {
                path: config_source::VirtualPath::parse("scoped.dcdl").unwrap(),
                content_type: config_source::ConfigContentType::Decodal,
                content: scalar.clone(),
            }]
        } else {
            vec![]
        };
        let authored = format!("{} as WorkspaceConfigSchema", source(form));
        let state =
            commit_workspace_config_tree(&api, TEST_WORKSPACE_ID, &request(&api, &authored, extra))
                .unwrap();
        let environment = config_source::SnapshotEnvironment::new(state.snapshot.clone())
            .with_schema_bundle(state.contract.schema_bundle.clone());
        assert!(
            environment
                .analyze(
                    &config_source::VirtualPath::parse("main.dcdl").unwrap(),
                    None
                )
                .is_empty(),
            "{authored}"
        );
        let projection = project_profiles_from_workspace_config(TEST_WORKSPACE_ID, &state).unwrap();
        let bundle = build_virtual_profile_config_bundle(
            &projection,
            &state,
            TEST_WORKSPACE_ID,
            "created",
            "project:alpha",
        )
        .unwrap()
        .unwrap();
        let resolved = bundle
            .profile_source_archive
            .as_ref()
            .unwrap()
            .verify()
            .unwrap()
            .resolve_profile(
                "project:alpha",
                Path::new("/runtime-worker"),
                "scoped-worker",
            )
            .unwrap();
        assert_eq!(resolved.scope.allow.len(), 1);
        assert_eq!(resolved.delegation_scope.allow.len(), 1);
        let scope = serde_json::to_value(&resolved.scope).unwrap();
        let delegation = serde_json::to_value(&resolved.delegation_scope).unwrap();
        assert_eq!(scope["allow"][0]["target"], "/runtime-worker");
        assert_eq!(scope["allow"][0]["permission"], "read");
        assert_eq!(delegation["allow"][0]["target"], "/runtime-worker");
        assert_eq!(delegation["allow"][0]["permission"], "write");
        scopes.push((scope, delegation));
    }
    assert_eq!(scopes[0], scopes[1]);
    assert_eq!(scopes[1], scopes[2]);
}

fn profile_fixture_decodal(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Object(fields) => format!(
            "{{ {} }}",
            fields
                .iter()
                .map(|(key, value)| format!("{key} = {};", profile_fixture_decodal(value)))
                .collect::<Vec<_>>()
                .join(" ")
        ),
        serde_json::Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(profile_fixture_decodal)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        serde_json::Value::Null => panic!("fixture omission must not be represented as null"),
        value => value.to_string(),
    }
}

#[tokio::test]
async fn value_profiles_authoring_corpus_preserves_effective_runtime_settings() {
    let cases: Vec<serde_json::Value> = serde_json::from_str(include_str!(
        "../../../../../resources/config-schema/profile-authoring-values.json"
    ))
    .unwrap();
    // The canonical token fixture must exercise every serializable setting,
    // so adding a field to CompactionConfigPartial cannot silently outgrow the
    // editor-shape coverage. Aliases/ratio helpers have separate corpus cases.
    let default = serde_json::to_value(manifest::CompactionConfigPartial::default()).unwrap();
    let canonical = cases
        .iter()
        .find(|case| case["name"] == "canonical_compaction_tokens")
        .unwrap();
    assert_eq!(
        default
            .as_object()
            .unwrap()
            .keys()
            .collect::<std::collections::BTreeSet<_>>(),
        canonical["profile"]["compaction"]
            .as_object()
            .unwrap()
            .keys()
            .filter(|name| name.as_str() != "kind")
            .collect()
    );
    for case in cases {
        let dir = tempfile::tempdir().unwrap();
        let api = test_api(dir.path()).await;
        let recipe = profile_fixture_decodal(&case["profile"]);
        for (index, (form, patched)) in [
            (recipe.clone(), false),
            (r#"import "./recipe.dcdl""#.into(), false),
            (
                r#"(import "./recipe.dcdl") // { description = "patched"; }"#.into(),
                true,
            ),
        ]
        .iter()
        .enumerate()
        {
            let extra = if index == 0 {
                vec![config_source::ConfigTreeChange::Create {
                    path: config_source::VirtualPath::parse("recipe.dcdl").unwrap(),
                    content_type: config_source::ConfigContentType::Decodal,
                    content: recipe.clone(),
                }]
            } else {
                vec![]
            };
            let authored = format!("{} as WorkspaceConfigSchema", source(form));
            let state = commit_workspace_config_tree(
                &api,
                TEST_WORKSPACE_ID,
                &request(&api, &authored, extra),
            )
            .unwrap();
            let environment = config_source::SnapshotEnvironment::new(state.snapshot.clone())
                .with_schema_bundle(state.contract.schema_bundle.clone());
            let diagnostics = environment.analyze(
                &config_source::VirtualPath::parse("main.dcdl").unwrap(),
                None,
            );
            assert!(diagnostics.is_empty(), "{}: {diagnostics:?}", case["name"]);
            let mut expected = case["profile"].clone();
            if *patched {
                expected["description"] = serde_json::json!("patched");
            }
            let evaluated = environment.evaluate_contract(&state.contract).unwrap();
            assert_eq!(
                evaluated.projections[0]
                    .data_json
                    .pointer("/profile/entries/0/profile"),
                Some(&expected)
            );
            let projection =
                project_profiles_from_workspace_config(TEST_WORKSPACE_ID, &state).unwrap();
            let bundle = build_virtual_profile_config_bundle(
                &projection,
                &state,
                TEST_WORKSPACE_ID,
                "created",
                "project:alpha",
            )
            .unwrap()
            .unwrap();
            let resolved = bundle
                .profile_source_archive
                .as_ref()
                .unwrap()
                .verify()
                .unwrap()
                .resolve_profile(
                    "project:alpha",
                    Path::new("/runtime-worker"),
                    "corpus-worker",
                )
                .unwrap();
            let manifest = serde_json::to_value(resolved).unwrap();
            for (pointer, value) in case["manifest_expect"].as_object().unwrap() {
                assert_eq!(
                    manifest.pointer(pointer),
                    Some(value),
                    "{}: {pointer}",
                    case["name"]
                );
            }
        }
    }
}

#[tokio::test]
async fn value_profiles_invalid_saves_preserve_the_active_config() {
    let dir = tempfile::tempdir().unwrap();
    let api = test_api(dir.path()).await;
    let before = commit_workspace_config_tree(
        &api,
        TEST_WORKSPACE_ID,
        &request(&api, &source(PROFILE), vec![]),
    )
    .unwrap();
    let invalid = [
        (source("42"), ""),
        (source(r#"{ worker = { mode = "not-a-mode"; }; }"#), "profile_value_invalid"),
        (source(r#"{ worker = { name = "runtime-only"; }; }"#), "profile_value_invalid"),
        (source(r#"{ scope = 42; }"#), "profile_value_invalid"),
        (source(r#"{ scope = "not-an-intent"; }"#), "profile_value_invalid"),
        (source(r#"{ delegation_scope = { intent = 42; }; }"#), "profile_value_invalid"),
        (source(r#"{ scope = { intent = "workspace_read"; unknown = true; }; }"#), "profile_value_invalid"),
        (source(r#"{ scope = { intent = "workspace_read"; deny_write = 42; }; }"#), "profile_value_invalid"),
        (source(r#"{ delegation_scope = { intent = "workspace_write"; symlink_policy = 42; }; }"#), "profile_value_invalid"),
        (source(r#"import "./missing.dcdl""#), ""),
        (source(r#"import "../outside.dcdl""#), ""),
        (r#"{ profile = { entries = [{ selector = "project:alpha"; profile = {}; }, { selector = "project:alpha"; profile = {}; }]; }; }"#.into(), "profile_selector_duplicate"),
        (r#"{ profile = { default_profile = "project:unknown"; }; }"#.into(), "unknown_default_profile"),
        (r#"{ profile = { entries = [{ selector = "project:"; profile = {}; }]; }; }"#.into(), "profile_selector_invalid"),
        (r#"{ profile = { entries = [{ selector = "project:alpha"; source = "profiles/alpha.dcdl"; }]; }; }"#.into(), ""),
    ];
    for (source, code) in invalid {
        let error =
            commit_workspace_config_tree(&api, TEST_WORKSPACE_ID, &request(&api, &source, vec![]))
                .unwrap_err();
        assert!(error.to_string().contains(code), "{error}");
        assert_eq!(
            api.config_store
                .load_workspace_config(TEST_WORKSPACE_ID)
                .unwrap()
                .unwrap(),
            before
        );
    }
}

#[tokio::test]
async fn value_profiles_legacy_source_config_requires_explicit_lossless_migration() {
    let dir = tempfile::tempdir().unwrap();
    let api = test_api(dir.path()).await;
    let mut contributions = api.config_schema_registry.compose().unwrap().contributions;
    let profile_schema = contributions
        .iter_mut()
        .find(|entry| entry.provider_id == "builtin:profile")
        .unwrap();
    *profile_schema = config_source::ConfigSchemaContribution::new("builtin:profile", "profile", "1", r#"{
        profile = { default_profile = String default "builtin:companion";
            entries = [...{ selector = String; source = String; label = String default ""; description = String default ""; }] default [];
        };
    }"#).unwrap();
    let old_schema = config_source::WorkspaceConfigSchemaBundle::compose(contributions).unwrap();
    let old_source = r#"{ profile = { default_profile = "project:alpha"; entries = [{ selector = "project:alpha"; label = "Alpha"; source = "recipes/alpha.dcdl"; }]; }; }"#;
    let candidate = api
        .config_store
        .evaluate_workspace_config_candidate_with_schema(
            TEST_WORKSPACE_ID,
            &request(
                &api,
                old_source,
                vec![config_source::ConfigTreeChange::Create {
                    path: config_source::VirtualPath::parse("recipes/alpha.dcdl").unwrap(),
                    content_type: config_source::ConfigContentType::Decodal,
                    content: PROFILE.into(),
                }],
            ),
            old_schema,
        )
        .unwrap();
    // Fixture simulates a previously persisted v1 schema/revision.
    let legacy = api
        .config_store
        .commit_evaluated_workspace_config(TEST_WORKSPACE_ID, &candidate)
        .unwrap();
    let error = project_profiles_from_workspace_config(TEST_WORKSPACE_ID, &legacy).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("profile_source_registration_removed")
    );
    assert!(error.to_string().contains("profile = import"));
    assert_eq!(
        api.config_store
            .load_workspace_config(TEST_WORKSPACE_ID)
            .unwrap()
            .unwrap(),
        legacy
    );
    assert!(
        commit_workspace_config_tree(&api, TEST_WORKSPACE_ID, &request(&api, old_source, vec![]))
            .is_err()
    );
    assert_eq!(
        api.config_store
            .load_workspace_config(TEST_WORKSPACE_ID)
            .unwrap()
            .unwrap(),
        legacy
    );
    let migrated_source = old_source.replace(
        r#"source = "recipes/alpha.dcdl""#,
        r#"profile = import "./recipes/alpha.dcdl""#,
    );
    let migrated = commit_workspace_config_tree(
        &api,
        TEST_WORKSPACE_ID,
        &request(&api, &migrated_source, vec![]),
    )
    .unwrap();
    let projection = project_profiles_from_workspace_config(TEST_WORKSPACE_ID, &migrated).unwrap();
    assert_eq!(
        projection.settings.default_profile.as_deref(),
        Some("project:alpha")
    );
    assert_eq!(
        migrated
            .snapshot
            .get(&config_source::VirtualPath::parse("recipes/alpha.dcdl").unwrap()),
        legacy
            .snapshot
            .get(&config_source::VirtualPath::parse("recipes/alpha.dcdl").unwrap())
    );
    assert_eq!(
        projection
            .settings
            .profiles
            .iter()
            .find(|entry| entry.selector == "project:alpha")
            .unwrap()
            .label,
        "Alpha"
    );
}

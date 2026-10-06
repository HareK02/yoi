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

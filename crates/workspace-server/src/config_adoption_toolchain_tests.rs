//! System reevaluation and owner repair preserve persisted evaluation evidence.
//! These component tests connect real evaluation, SQLite persistence and Prompt projection
//! using isolated temporary databases; only schema inputs and storage faults are test-owned.
//! Run with `cargo test -p yoi-workspace-server --lib toolchain_refresh`.
//! The legacy fixture models the old storage format and byte-wise lexer result; it does
//! not run the old evaluator (the independent version probe is a separate test surface).
use super::*;
use config_source::{
    CONFIG_SOURCE_CONTRACT_VERSION, ConfigContentType, ConfigEntry, DECODAL_VERSION,
    SnapshotEnvironment, ToolchainContract, digest_bytes,
};

const VALUE: &str = "retained 雪 café 😀 e\u{301}";
const SOURCE: &str = r#"{ prompts = { common = { language = "retained 雪 café 😀 é"; }; }; } as WorkspaceConfigSchema"#;

fn digest(value: &impl Serialize) -> String {
    digest_bytes(&serde_json::to_vec(value).unwrap())
}

// Reproduce the serialized 0.4.0 contract, rather than relabel a 0.4.1 fingerprint.
fn legacy_contract(mut contract: ToolchainContract) -> ToolchainContract {
    contract.decodal_version = "0.4.0".into();
    let bundle = &mut contract.schema_bundle;
    let mut input = serde_json::to_vec(&(
        CONFIG_SOURCE_CONTRACT_VERSION,
        "0.4.0",
        bundle
            .contributions
            .iter()
            .map(|c| {
                (
                    c.provider_id.as_str(),
                    c.namespace.as_str(),
                    c.version.as_str(),
                    c.source_digest.as_str(),
                    c.projection_validator.as_ref(),
                )
            })
            .collect::<Vec<_>>(),
        digest_bytes(bundle.source.as_bytes()),
    ))
    .unwrap();
    let authoring = bundle
        .contributions
        .iter()
        .filter_map(|c| {
            c.authoring_source
                .as_deref()
                .map(|source| (c.provider_id.as_str(), source))
        })
        .collect::<Vec<_>>();
    if !authoring.is_empty() {
        input.extend(serde_json::to_vec(&("authoring_sources", authoring)).unwrap());
    }
    bundle.fingerprint = digest_bytes(&input);
    contract.fingerprint = digest(&(
        contract.contract_version,
        &contract.decodal_version,
        contract.schema_version,
        &contract.entrypoints,
        contract.import_policy_version,
        &bundle.fingerprint,
    ));
    contract
}

struct ToolchainFixture {
    dir: tempfile::TempDir,
    db: SqliteWorkspaceStore,
    state: WorkspaceConfigState,
    original: ConfigurationAdoptionRequest,
    receipt: Option<ConfigurationAdoptionResult>,
}

fn legacy_fixture(published: bool) -> ToolchainFixture {
    assert_eq!(
        DECODAL_VERSION, "0.4.1",
        "run after the dependency/toolchain update"
    );
    let (dir, db) = fixture();
    let host = host(false);
    let original = request(&db, &host, "legacy-040", Some(SOURCE));
    let preview = db
        .preview_configuration_adoption(WORKSPACE, OWNER, &original, schema(), &host)
        .unwrap();
    let mut evaluated = SnapshotEnvironment::new(preview.state.snapshot.clone())
        .evaluate_contract(&preview.state.contract)
        .unwrap();
    assert_eq!(
        evaluated.projections[0].data_json["prompts"]["common"]["language"],
        VALUE
    );
    // 0.4.0's string lexer appended each UTF-8 byte as a char.
    let broken: String = VALUE.bytes().map(char::from).collect();
    evaluated.projections[0].data_json["prompts"]["common"]["language"] = broken.into();
    evaluated.projections[0].projection_digest = digest(&evaluated.projections[0].data_json);
    let state = WorkspaceConfigState {
        snapshot: preview.state.snapshot,
        contract: legacy_contract(preview.state.contract),
        projection_digest: digest(&evaluated.projections),
    };
    persisted_fixture(dir, db, state, original, published, "legacy-time")
}

fn persisted_fixture(
    dir: tempfile::TempDir,
    db: SqliteWorkspaceStore,
    state: WorkspaceConfigState,
    original: ConfigurationAdoptionRequest,
    published: bool,
    materialized_at: &str,
) -> ToolchainFixture {
    let receipt = db
        .with_immediate_transaction(|tx| {
            tx.execute(
                "DELETE FROM workspace_config_entries WHERE workspace_id=?1",
                [WORKSPACE],
            )?;
            crate::config_source::insert_materialized_state(
                tx,
                WORKSPACE,
                &state,
                materialized_at,
            )?;
            tx.execute(
                "UPDATE workspace_config_trees SET decodal_version=?2 WHERE workspace_id=?1",
                params![WORKSPACE, state.contract.decodal_version],
            )?;
            if !published {
                return Ok(None);
            }
            let authority = OwnerScope(WORKSPACE);
            let content = ScopedStore::new(tx, scope(WORKSPACE), &authority);
            let prepared = content_history_adapters::settings::prepare(&state.snapshot).unwrap();
            for (id, object) in &prepared.objects {
                content.put(*id, &object.encode()).unwrap();
            }
            let head = content.adopt(prepared.root, None, OWNER.into(), 1).unwrap();
            let receipt = ConfigurationAdoptionResult {
                workspace_id: WORKSPACE.into(),
                operation_id: original.operation_id.clone(),
                history_head: head.to_string(),
                state: state.clone(),
                source_stored: true,
                policy_committed: true,
                runtime_application: RuntimePolicyApplication::NextCurrentPolicyCheck,
            };
            tx.execute(
                "INSERT INTO workspace_configuration_receipts VALUES(?1,?2,?3,?4)",
                params![
                    WORKSPACE,
                    original.operation_id,
                    request_fingerprint(WORKSPACE, OWNER, &original)?,
                    json(&receipt)?,
                ],
            )?;
            Ok(Some(receipt))
        })
        .unwrap();
    ToolchainFixture {
        dir,
        db,
        state,
        original,
        receipt,
    }
}

fn default_schema(value: &str) -> WorkspaceConfigSchemaBundle {
    let value = serde_json::to_string(value).unwrap();
    WorkspaceConfigSchemaBundle::compose([
        PermissionConfigSchemaProvider.contribution().unwrap(),
        config_source::ConfigSchemaContribution::new(
            "builtin:prompts",
            "prompts",
            "1",
            format!(
                "{{ prompts = {{ common = {{ language = String default {value}; \
                 workspace = String default {value}; }} default {{}}; }} default {{}}; }}"
            ),
        )
        .unwrap(),
    ])
    .unwrap()
}

fn current_fixture(published: bool, source: &str) -> ToolchainFixture {
    let (dir, db) = fixture();
    let original = request(&db, &host(false), "current-schema", Some(source));
    // Install valid, unformatted source bytes so an accidental owner-normalization
    // path cannot masquerade as a source-preserving system reevaluation.
    let snapshot = ConfigTreeSnapshot::from_entries([ConfigEntry::new(
        VirtualPath::parse(MAIN_CONFIG_ENTRYPOINT).unwrap(),
        ConfigContentType::Decodal,
        source,
    )
    .unwrap()])
    .unwrap();
    let contract = ToolchainContract::with_schema_bundle(
        config_source::DEFAULT_SCHEMA_VERSION,
        vec![VirtualPath::parse(MAIN_CONFIG_ENTRYPOINT).unwrap()],
        config_source::DEFAULT_IMPORT_POLICY_VERSION,
        default_schema("old-default"),
    );
    contract.validate().unwrap();
    let evaluation = SnapshotEnvironment::new(snapshot.clone())
        .evaluate_contract(&contract)
        .unwrap();
    let state = WorkspaceConfigState {
        snapshot,
        contract,
        projection_digest: evaluation.projection_digest,
    };
    persisted_fixture(dir, db, state, original, published, "schema-time")
}

// Compare raw rows, including old timestamps, manifests, receipt bytes and content
// objects. Failed/read-only operations must not leave even partial provenance behind.
fn evidence(db: &SqliteWorkspaceStore) -> Vec<Vec<Vec<rusqlite::types::Value>>> {
    db.with_conn(|conn| {
        [
            "workspace_config_trees",
            "workspace_config_entries",
            "workspace_config_tree_history",
            "workspace_configuration_receipts",
            "workspace_configuration_policies",
            "content_objects",
            "content_scopes",
            "content_scope_objects",
            "content_reachable",
        ]
        .into_iter()
        .map(|table| {
            let mut query = conn.prepare(&format!("SELECT * FROM {table} ORDER BY 1,2"))?;
            let count = query.column_count();
            let rows = query
                .query_map([], |r| (0..count).map(|i| r.get(i)).collect())?
                .collect::<std::result::Result<Vec<Vec<rusqlite::types::Value>>, _>>()?;
            Ok(rows)
        })
        .collect()
    })
    .unwrap()
}

fn assert_retained(db: &SqliteWorkspaceStore, state: &WorkspaceConfigState) {
    db.with_conn(|conn| {
        let saved: (String, String, String) = conn.query_row(
            "SELECT schema_bundle_json,manifest_json,created_at FROM workspace_config_tree_history
             WHERE workspace_id=?1 AND content_digest=?2 AND toolchain_fingerprint=?3 AND projection_digest=?4",
            params![WORKSPACE, state.snapshot.digest, state.contract.fingerprint, state.projection_digest],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        assert_eq!(saved, (json(&state.contract.schema_bundle)?, json(&state.snapshot.entries)?, "legacy-time".into()));
        Ok(())
    }).unwrap();
    assert_eq!(
        db.load_workspace_config_history(WORKSPACE, &state.snapshot.digest)
            .unwrap(),
        Some(state.snapshot.clone())
    );
    assert_native_policy(db);
}

#[test]
fn schema_default_change_preserves_source_history_receipts_and_subject_policy() {
    for published in [false, true] {
        let f = current_fixture(published, "{} as WorkspaceConfigSchema");
        let before = evidence(&f.db);
        let head = f.db.configuration_head(WORKSPACE, OWNER).unwrap();
        assert_eq!(head.is_some(), published);
        let cache = crate::prompt_settings::WorkspacePromptProjectionCache::default();
        let previous = cache.resolve(WORKSPACE, &f.state).unwrap();
        assert_eq!(
            previous.catalog().templates["common.language"],
            "old-default"
        );

        let changed = default_schema("new-default");
        let refreshed =
            f.db.ensure_workspace_config_materialized_with_schema(
                WORKSPACE,
                "schema-update",
                changed.clone(),
            )
            .unwrap();
        assert_eq!(
            refreshed.contract.decodal_version,
            f.state.contract.decodal_version
        );
        assert_eq!(refreshed.contract.decodal_version, DECODAL_VERSION);
        refreshed.contract.validate().unwrap();
        assert_eq!(refreshed.contract.schema_bundle, changed);
        assert_ne!(refreshed.contract.fingerprint, f.state.contract.fingerprint);
        assert_ne!(refreshed.projection_digest, f.state.projection_digest);
        assert_eq!(refreshed.snapshot, f.state.snapshot);
        assert_eq!(f.db.configuration_head(WORKSPACE, OWNER).unwrap(), head);
        let saved = f.db.load_workspace_config(WORKSPACE).unwrap().unwrap();
        assert_eq!(saved, refreshed);
        let projected = cache.resolve(WORKSPACE, &saved).unwrap();
        assert_eq!(
            projected.catalog().templates["common.language"],
            "new-default"
        );
        assert_eq!(
            projected.catalog().templates["common.workspace"],
            "new-default"
        );
        assert_eq!(
            previous.catalog().templates["common.language"],
            "old-default"
        );
        let after = evidence(&f.db);
        for index in [1, 3, 4, 5, 6, 7, 8] {
            assert_eq!(
                after[index], before[index],
                "retained table {index}, published={published}"
            );
        }
        for row in &before[2] {
            assert!(
                after[2].contains(row),
                "old evaluation evidence must be retained"
            );
        }
        assert_eq!(after[2].len(), before[2].len() + 1);
        assert_eq!(
            f.db.load_workspace_config_history(WORKSPACE, &f.state.snapshot.digest)
                .unwrap(),
            Some(f.state.snapshot.clone())
        );
        assert_native_policy(&f.db);
        assert_eq!(
            f.db.ensure_workspace_config_materialized_with_schema(
                WORKSPACE,
                "repeat",
                changed.clone()
            )
            .unwrap(),
            refreshed
        );
        assert_eq!(evidence(&f.db), after);
        drop(f.db);
        let db = open(&f.dir.path().join("server.sqlite"));
        assert_eq!(
            db.ensure_workspace_config_materialized_with_schema(
                WORKSPACE,
                "reopen",
                changed.clone()
            )
            .unwrap(),
            refreshed
        );
        assert_eq!(db.configuration_head(WORKSPACE, OWNER).unwrap(), head);
        if let Some(receipt) = f.receipt {
            assert_eq!(
                db.adopt_configuration(WORKSPACE, OWNER, &f.original, changed, &host(false))
                    .unwrap(),
                receipt
            );
        }
        assert_eq!(
            db.load_workspace_config(WORKSPACE).unwrap(),
            Some(refreshed)
        );
        assert_native_policy(&db);
        assert_eq!(evidence(&db), after, "repeat/reopen/replay must not write");
    }
}

#[test]
fn schema_default_change_retains_explicit_and_empty_prompt_overrides() {
    for value in [VALUE, ""] {
        let source = format!(
            "{{ prompts = {{ common = {{ language = {}; }}; }}; }} as WorkspaceConfigSchema",
            serde_json::to_string(value).unwrap()
        );
        let f = current_fixture(true, &source);
        let refreshed =
            f.db.ensure_workspace_config_materialized_with_schema(
                WORKSPACE,
                "schema-update",
                default_schema("new-default"),
            )
            .unwrap();
        assert_eq!(refreshed.snapshot, f.state.snapshot);
        let catalog =
            crate::prompt_settings::project_prompts_from_workspace_config(&refreshed).unwrap();
        assert_eq!(catalog.templates["common.language"], value);
        assert_eq!(catalog.templates["common.workspace"], "new-default");
        assert_native_policy(&f.db);
    }
}

#[test]
fn schema_change_failed_reevaluation_preserves_rows_and_requires_owner_update() {
    let incompatible = WorkspaceConfigSchemaBundle::compose([PermissionConfigSchemaProvider
        .contribution()
        .unwrap()])
    .unwrap();
    for published in [false, true] {
        for changed in [
            incompatible.clone(),
            default_schema("{% include \"missing\" %}"),
        ] {
            let f = current_fixture(published, SOURCE);
            let before = evidence(&f.db);
            let error =
                f.db.ensure_workspace_config_materialized_with_schema(
                    WORKSPACE,
                    "rejected",
                    changed.clone(),
                )
                .unwrap_err();
            assert!(
                matches!(error, Error::ConfigurationUpdateRequired(_)),
                "{error:?}"
            );
            assert_eq!(evidence(&f.db), before);
            assert_eq!(
                f.db.load_workspace_config(WORKSPACE).unwrap(),
                Some(f.state.clone())
            );
            assert!(matches!(
                crate::config_source::evaluate_workspace_config_state(&f.state, changed.clone()),
                Err(Error::ConfigurationUpdateRequired(_))
            ));
            drop(f.db);
            let db = open(&f.dir.path().join("server.sqlite"));
            assert_eq!(evidence(&db), before);
            assert!(matches!(
                db.ensure_workspace_config_materialized_with_schema(
                    WORKSPACE,
                    "retry-rejected",
                    changed
                ),
                Err(Error::ConfigurationUpdateRequired(_))
            ));
            assert_eq!(evidence(&db), before);
            let current = db.configuration_current(WORKSPACE, OWNER).unwrap();
            assert_eq!(current.state, f.state);
            assert_eq!(
                current.history_head,
                f.receipt.map(|receipt| receipt.history_head)
            );
            assert_native_policy(&db);
            let repair = request(
                &db,
                &host(false),
                "repair-schema",
                Some("{} as WorkspaceConfigSchema"),
            );
            db.adopt_configuration(
                WORKSPACE,
                OWNER,
                &repair,
                default_schema("new-default"),
                &host(false),
            )
            .unwrap();
            assert_native_policy(&db);
        }
    }
}

#[test]
fn schema_default_change_storage_failure_rolls_back_without_requesting_owner_repair() {
    for published in [false, true] {
        for (table, event) in [
            ("workspace_config_trees", "UPDATE"),
            ("workspace_config_entries", "INSERT"),
            ("workspace_config_tree_history", "INSERT"),
        ] {
            let f = current_fixture(published, "{} as WorkspaceConfigSchema");
            let before = evidence(&f.db);
            f.db.with_conn(|conn| {
                conn.execute_batch(&format!("CREATE TEMP TRIGGER fail_schema_update BEFORE {event} ON {table} BEGIN SELECT RAISE(ABORT, 'injected schema write failure'); END;"))?;
                Ok(())
            }).unwrap();
            let error =
                f.db.ensure_workspace_config_materialized_with_schema(
                    WORKSPACE,
                    "schema-update",
                    default_schema("new-default"),
                )
                .unwrap_err();
            assert!(error.configuration_update_required().is_none(), "{error:?}");
            assert!(
                error.to_string().contains("injected schema write failure"),
                "{error:?}"
            );
            assert_eq!(evidence(&f.db), before, "{table}, published={published}");
            drop(f.db);
            let db = open(&f.dir.path().join("server.sqlite"));
            assert_eq!(evidence(&db), before, "{table} after reopen");
            assert_eq!(
                db.load_workspace_config(WORKSPACE).unwrap(),
                Some(f.state.clone())
            );
            let refreshed = db
                .ensure_workspace_config_materialized_with_schema(
                    WORKSPACE,
                    "retry",
                    default_schema("new-default"),
                )
                .unwrap();
            assert_eq!(refreshed.snapshot, f.state.snapshot);
            let catalog =
                crate::prompt_settings::project_prompts_from_workspace_config(&refreshed).unwrap();
            assert_eq!(catalog.templates["common.language"], "new-default");
            assert_native_policy(&db);
        }
    }
}

#[test]
fn schema_default_change_rejects_stale_candidate_with_unchanged_source() {
    let f = current_fixture(false, "{} as WorkspaceConfigSchema");
    let stale =
        f.db.evaluate_workspace_config_candidate_with_schema(
            WORKSPACE,
            &request(&f.db, &host(false), "stale-schema", None).source,
            default_schema("old-default"),
        )
        .unwrap();
    let refreshed =
        f.db.ensure_workspace_config_materialized_with_schema(
            WORKSPACE,
            "schema-update",
            default_schema("new-default"),
        )
        .unwrap();
    assert_eq!(refreshed.snapshot.digest, stale.base_digest);
    assert_ne!(
        refreshed.contract.fingerprint,
        stale.base_toolchain_fingerprint
    );
    let before = evidence(&f.db);
    let error =
        f.db.commit_evaluated_workspace_config(WORKSPACE, &stale)
            .unwrap_err();
    assert!(
        matches!(error, Error::WorkspaceConfigConflict(_)),
        "{error:?}"
    );
    assert_eq!(evidence(&f.db), before);
    assert_eq!(
        f.db.load_workspace_config(WORKSPACE).unwrap(),
        Some(refreshed)
    );
}

#[test]
fn system_reevaluation_preserves_source_owner_receipts_and_subject_policy() {
    // Include pre-adoption installations: no editing head is not owner consent.
    for published in [false, true] {
        let f = legacy_fixture(published);
        let before = evidence(&f.db);
        assert_eq!(
            f.db.load_workspace_config(WORKSPACE).unwrap(),
            Some(f.state.clone())
        );
        let head = f.db.configuration_head(WORKSPACE, OWNER).unwrap();
        let refreshed =
            f.db.ensure_workspace_config_materialized_with_schema(WORKSPACE, "later", schema())
                .unwrap();
        assert_eq!(refreshed.snapshot, f.state.snapshot);
        refreshed.contract.validate().unwrap();
        assert_ne!(refreshed.projection_digest, f.state.projection_digest);
        assert_eq!(f.db.configuration_head(WORKSPACE, OWNER).unwrap(), head);
        let after = evidence(&f.db);
        for index in [1, 3, 4, 5, 6, 7, 8] {
            assert_eq!(
                after[index], before[index],
                "source and authority evidence table {index}"
            );
        }
        assert_retained(&f.db, &f.state);
        assert_eq!(
            f.db.ensure_workspace_config_materialized_with_schema(WORKSPACE, "again", schema())
                .unwrap(),
            refreshed
        );
        assert_eq!(evidence(&f.db), after);
        drop(f.db);
        let db = open(&f.dir.path().join("server.sqlite"));
        assert_eq!(
            db.load_workspace_config(WORKSPACE).unwrap(),
            Some(refreshed)
        );
        assert_eq!(evidence(&db), after);
        assert_retained(&db, &f.state);
    }
}

#[test]
fn patch_version_change_reevaluates_even_when_saved_fingerprint_is_unchanged() {
    let (_dir, db) = fixture();
    db.with_conn(|conn| {
        conn.execute(
            "UPDATE workspace_config_trees SET decodal_version='0.4.0' WHERE workspace_id=?1",
            [WORKSPACE],
        )?;
        Ok(())
    })
    .unwrap();
    let before = evidence(&db);
    let saved = db.load_workspace_config(WORKSPACE).unwrap().unwrap();
    assert!(saved.contract.validate().is_err());
    assert!(
        crate::config_source::evaluate_workspace_config_state(&saved, schema())
            .unwrap_err()
            .configuration_update_required()
            .is_some()
    );
    let refreshed = db
        .ensure_workspace_config_materialized_with_schema(WORKSPACE, "patch", schema())
        .unwrap();
    assert_eq!(refreshed.contract.decodal_version, DECODAL_VERSION);
    assert_eq!(refreshed.snapshot, saved.snapshot);
    assert_eq!(refreshed.contract.fingerprint, saved.contract.fingerprint);
    assert_ne!(evidence(&db), before);
    let after = evidence(&db);
    assert_eq!(
        db.ensure_workspace_config_materialized_with_schema(WORKSPACE, "same-version", schema())
            .unwrap(),
        refreshed
    );
    assert_eq!(evidence(&db), after);
}

#[test]
fn system_reevaluation_storage_failure_rolls_back_without_requesting_owner_repair() {
    let f = legacy_fixture(true);
    let before = evidence(&f.db);
    f.db.with_conn(|conn| {
        conn.execute_batch("CREATE TRIGGER fail_reevaluation BEFORE UPDATE ON workspace_config_trees BEGIN SELECT RAISE(ABORT, 'injected write failure'); END;")?;
        Ok(())
    }).unwrap();
    let error =
        f.db.ensure_workspace_config_materialized_with_schema(WORKSPACE, "later", schema())
            .unwrap_err();
    assert!(error.configuration_update_required().is_none(), "{error:?}");
    assert_eq!(evidence(&f.db), before);
}

#[test]
fn decodal_040_owner_refresh_preserves_source_history_and_receipts_across_reopen() {
    let f = legacy_fixture(true);
    let host = host(false);
    let refresh = request(&f.db, &host, "refresh-041", None);
    let before = evidence(&f.db);
    let preview =
        f.db.preview_configuration_adoption(WORKSPACE, OWNER, &refresh, schema(), &host)
            .unwrap();
    assert!(preview.changed_paths.is_empty());
    assert!(preview.changed_subjects.is_empty());
    assert_eq!(evidence(&f.db), before);
    let result = adopt(&f.db, &host, &refresh).unwrap();
    assert_eq!(result.state, preview.state);
    assert_eq!(result.state.snapshot, f.state.snapshot);
    assert_eq!(result.state.contract.decodal_version, "0.4.1");
    result.state.contract.validate().unwrap();
    assert_ne!(
        result.state.contract.fingerprint,
        f.state.contract.fingerprint
    );
    assert_ne!(
        result.state.contract.schema_bundle.fingerprint,
        f.state.contract.schema_bundle.fingerprint
    );
    assert_ne!(result.state.projection_digest, f.state.projection_digest);
    assert_ne!(Some(result.history_head.clone()), refresh.expected_head);
    let evaluated =
        crate::config_source::evaluate_workspace_config_state(&result.state, schema()).unwrap();
    assert_eq!(evaluated.projection_digest, result.state.projection_digest);
    assert_eq!(
        evaluated.projections[0].data_json["prompts"]["common"]["language"],
        VALUE
    );
    assert_retained(&f.db, &f.state);
    f.db.with_conn(|conn| {
        let saved: (String, String) = conn.query_row(
            "SELECT schema_bundle_json,manifest_json FROM workspace_config_tree_history
             WHERE workspace_id=?1 AND content_digest=?2 AND toolchain_fingerprint=?3 AND projection_digest=?4",
            params![WORKSPACE, result.state.snapshot.digest, result.state.contract.fingerprint, result.state.projection_digest],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        assert_eq!(saved, (json(&result.state.contract.schema_bundle)?, json(&result.state.snapshot.entries)?));
        Ok(())
    }).unwrap();
    let saved = evidence(&f.db);
    // Adoption appends history/receipt/objects rather than rewriting old evidence.
    for index in [2, 3, 4, 5, 7, 8] {
        for row in &before[index] {
            assert!(
                saved[index].contains(row),
                "retained evidence table {index}"
            );
        }
    }
    drop(f.db);
    let db = open(&f.dir.path().join("server.sqlite"));
    assert_eq!(
        db.load_workspace_config(WORKSPACE).unwrap(),
        Some(result.state.clone())
    );
    assert_eq!(
        db.configuration_head(WORKSPACE, OWNER).unwrap(),
        Some(result.history_head.clone())
    );
    assert_eq!(
        db.configuration_adoption_result(WORKSPACE, OWNER, "refresh-041")
            .unwrap(),
        Some(result.clone())
    );
    assert_eq!(adopt(&db, &host, &refresh).unwrap(), result);
    assert_eq!(adopt(&db, &host, &f.original).unwrap(), f.receipt.unwrap());
    assert_eq!(
        evidence(&db),
        saved,
        "historical replay must not reactivate 0.4.0"
    );
    assert_retained(&db, &f.state);
}

#[test]
fn decodal_040_refresh_rejects_failed_evaluation_stale_head_and_same_source_aba() {
    let f = legacy_fixture(true);
    let host = host(false);
    let stale = request(&f.db, &host, "stale-refresh", None);
    let before = evidence(&f.db);
    let incompatible = WorkspaceConfigSchemaBundle::compose([PermissionConfigSchemaProvider
        .contribution()
        .unwrap()])
    .unwrap();
    let error =
        f.db.ensure_workspace_config_materialized_with_schema(
            WORKSPACE,
            "failed-update",
            incompatible.clone(),
        )
        .unwrap_err();
    assert!(
        matches!(error, Error::ConfigurationUpdateRequired(_)),
        "{error:?}"
    );
    assert!(error.to_string().contains("owner must update"));
    assert_eq!(evidence(&f.db), before);
    assert!(
        f.db.preview_configuration_adoption(WORKSPACE, OWNER, &stale, incompatible.clone(), &host)
            .is_err()
    );
    assert!(
        f.db.adopt_configuration(WORKSPACE, OWNER, &stale, incompatible, &host)
            .is_err()
    );
    assert_eq!(evidence(&f.db), before);
    // The stored source remains readable after failure; repair is an explicit edit.
    let changed = adopt(&f.db, &host, &request(&f.db, &host, "to-b", Some(B))).unwrap();
    assert_ne!(changed.state.snapshot, f.state.snapshot);
    let source = &f
        .state
        .snapshot
        .get(&VirtualPath::parse(MAIN_CONFIG_ENTRYPOINT).unwrap())
        .unwrap()
        .content;
    let back = adopt(&f.db, &host, &request(&f.db, &host, "back-a", Some(source))).unwrap();
    assert_eq!(back.state.snapshot, f.state.snapshot);
    assert_ne!(Some(back.history_head.clone()), stale.expected_head);
    let saved = evidence(&f.db);
    let error = adopt(&f.db, &host, &stale).unwrap_err();
    assert!(
        matches!(error, Error::Configuration(cause) if cause.details().reason == FailureReason::EditingHeadChanged)
    );
    let mut reused = f.original.clone();
    reused.source.changes.clear();
    assert!(
        matches!(adopt(&f.db, &host, &reused), Err(Error::Configuration(cause)) if cause.details().reason == FailureReason::RequestIdentityConflict)
    );
    assert_eq!(
        adopt(&f.db, &host, &f.original).unwrap(),
        f.receipt.unwrap()
    );
    assert_eq!(evidence(&f.db), saved);
    assert_retained(&f.db, &f.state);
}

#[test]
fn decodal_040_refresh_storage_failures_rollback_and_reopen_without_partial_adoption() {
    for (table, event) in [
        ("content_objects", "INSERT"),
        ("workspace_config_trees", "UPDATE"),
        ("workspace_config_entries", "INSERT"),
        ("workspace_config_tree_history", "INSERT"),
        ("content_scopes", "UPDATE"),
        ("workspace_configuration_receipts", "INSERT"),
    ] {
        let f = legacy_fixture(true);
        let host = host(false);
        let refresh = request(&f.db, &host, "retry-refresh", None);
        let before = evidence(&f.db);
        f.db.with_conn(|conn| {
            conn.execute_batch(&format!("CREATE TEMP TRIGGER injected_refresh_failure BEFORE {event} ON {table} BEGIN SELECT RAISE(ABORT,'injected {table}'); END;"))?;
            Ok(())
        }).unwrap();
        let error = adopt(&f.db, &host, &refresh).unwrap_err();
        assert!(
            error.to_string().contains(&format!("injected {table}")),
            "{error}"
        );
        assert_eq!(evidence(&f.db), before, "{table}");
        drop(f.db);
        let db = open(&f.dir.path().join("server.sqlite"));
        assert_eq!(evidence(&db), before, "{table} after reopen");
        assert_eq!(
            db.load_workspace_config(WORKSPACE).unwrap(),
            Some(f.state.clone())
        );
        assert_retained(&db, &f.state);
        assert_eq!(
            db.configuration_adoption_result(WORKSPACE, OWNER, "retry-refresh")
                .unwrap(),
            None
        );
        let result = adopt(&db, &host, &refresh).unwrap();
        assert_eq!(result.state.contract.decodal_version, "0.4.1");
        assert_eq!(result.state.snapshot, f.state.snapshot);
        assert_retained(&db, &f.state);
    }
}

#[test]
fn decodal_040_empty_provider_bundle_cannot_silently_evaluate_with_new_lexer() {
    let snapshot = ConfigTreeSnapshot::from_entries([ConfigEntry::new(
        VirtualPath::parse(MAIN_CONFIG_ENTRYPOINT).unwrap(),
        ConfigContentType::Decodal,
        "{ value = \"retained 雪\"; } as WorkspaceConfigSchema",
    )
    .unwrap()])
    .unwrap();
    let state = WorkspaceConfigState {
        snapshot,
        contract: legacy_contract(ToolchainContract::new(
            1,
            vec![VirtualPath::parse(MAIN_CONFIG_ENTRYPOINT).unwrap()],
            1,
        )),
        projection_digest: "retained-projection".into(),
    };
    let error = crate::config_source::evaluate_workspace_config_state(
        &state,
        WorkspaceConfigSchemaBundle::empty(),
    )
    .unwrap_err();
    assert!(error.configuration_update_required().is_some());
}

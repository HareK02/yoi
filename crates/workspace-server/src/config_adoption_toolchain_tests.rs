//! Persisted 0.4.0 evidence must survive an explicit, owner-authorized 0.4.1 refresh.
//! The fixture models the old storage format and byte-wise lexer result; it does not
//! run the old evaluator (the independent version probe is a separate test surface).
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

struct LegacyFixture {
    dir: tempfile::TempDir,
    db: SqliteWorkspaceStore,
    state: WorkspaceConfigState,
    original: ConfigurationAdoptionRequest,
    receipt: Option<ConfigurationAdoptionResult>,
}

fn legacy_fixture(published: bool) -> LegacyFixture {
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
    let receipt = db
        .with_immediate_transaction(|tx| {
            tx.execute(
                "DELETE FROM workspace_config_entries WHERE workspace_id=?1",
                [WORKSPACE],
            )?;
            crate::config_source::insert_materialized_state(tx, WORKSPACE, &state, "legacy-time")?;
            tx.execute(
                "UPDATE workspace_config_trees SET decodal_version='0.4.0' WHERE workspace_id=?1",
                [WORKSPACE],
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
    LegacyFixture {
        dir,
        db,
        state,
        original,
        receipt,
    }
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
fn decodal_040_read_and_ensure_preserve_provenance_until_owner_refresh() {
    // Include pre-adoption installations: no editing head is not owner consent.
    for published in [false, true] {
        let f = legacy_fixture(published);
        let before = evidence(&f.db);
        assert_eq!(
            f.db.load_workspace_config(WORKSPACE).unwrap(),
            Some(f.state.clone())
        );
        assert_eq!(
            f.db.ensure_workspace_config_materialized_with_schema(WORKSPACE, "later", schema())
                .unwrap(),
            f.state
        );
        assert!(crate::config_source::evaluate_workspace_config_state(&f.state, schema()).is_err());
        let refresh = request(&f.db, &host(false), "refresh", None);
        let candidate = f
            .db
            .evaluate_workspace_config_candidate_with_schema(WORKSPACE, &refresh.source, schema())
            .unwrap();
        assert!(
            f.db.commit_evaluated_workspace_config(WORKSPACE, &candidate)
                .is_err()
        );
        let error =
            f.db.adopt_configuration(WORKSPACE, "not-owner", &refresh, schema(), &host(false))
                .unwrap_err();
        assert!(
            matches!(error, Error::Configuration(cause) if cause.details().reason == FailureReason::OwnerRequired)
        );
        assert_eq!(evidence(&f.db), before);
        drop(f.db);
        let db = open(&f.dir.path().join("server.sqlite"));
        assert_eq!(
            db.load_workspace_config(WORKSPACE).unwrap(),
            Some(f.state.clone())
        );
        assert_eq!(evidence(&db), before);
        assert_retained(&db, &f.state);
        // Even without prior adoption, an explicit owner request can refresh.
        if !published {
            adopt(&db, &host(false), &refresh).unwrap();
        }
    }
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
    assert!(error.to_string().contains("owner-authorized"));
}

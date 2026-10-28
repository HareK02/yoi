//! Additional removal-copy erasure uses the real Runtime, retention provider,
//! Session tombstone, filesystem and signed HTTP router. Only execution is fake
//! (the parent fixture's AcceptingBackend); no Backend recovery is simulated here.
use super::*;
#[path = "subject_job_removal_purge_recovery_tests.rs"]
mod recovery;
use runtime_api::{SubjectDeleteWorkerRequest, SubjectJobRemovalPurgeRequest};
use std::collections::BTreeMap;
use std::path::Path;

fn tree(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(root: &Path, at: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
        if !at.exists() {
            return;
        }
        for entry in std::fs::read_dir(at).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                walk(root, &entry.path(), files);
            } else {
                files.insert(
                    entry.path().strip_prefix(root).unwrap().into(),
                    std::fs::read(entry.path()).unwrap(),
                );
            }
        }
    }
    let mut files = BTreeMap::new();
    walk(root, root, &mut files);
    files
}

impl Fixture {
    fn deletion_request(&self) -> SubjectDeleteWorkerRequest {
        SubjectDeleteWorkerRequest {
            workspace_id: self.scope.workspace_id.clone(),
            worker_id: self.worker.worker_id,
            create_fingerprint: task_request("identity only").create_fingerprint,
            operation_id: "subject-deletion".into(),
        }
    }

    fn removal_purge_request(
        &self,
        disposition: SessionDisposition,
    ) -> SubjectJobRemovalPurgeRequest {
        let removal = self.retention_request(disposition);
        SubjectJobRemovalPurgeRequest {
            deletion: self.deletion_request(),
            removal: serde_json::from_value(serde_json::to_value(removal).unwrap()).unwrap(),
        }
    }

    fn complete_removal(
        &self,
        request: &SubjectJobRemovalPurgeRequest,
    ) -> crate::retention::WorkerRetentionExecutionResult {
        let diagnostics = self.root().parent().unwrap().join("runs/attempt");
        std::fs::create_dir_all(&diagnostics).unwrap();
        std::fs::write(diagnostics.join("worker.out.log"), b"Subject Job output").unwrap();
        self.runtime
            .subject_delete_worker_scoped(&self.scope, request.deletion.clone())
            .unwrap();
        let removal =
            serde_json::from_value(serde_json::to_value(&request.removal).unwrap()).unwrap();
        self.runtime.execute_worker_retention(&removal).unwrap()
    }

    fn receipt_path(&self) -> PathBuf {
        self.temp
            .path()
            .join("retention/operations/remove-source.json")
    }

    fn copies(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        tree(&self.temp.path().join("archives"))
    }

    fn independent_worker(&self) -> WorkerRef {
        let mut request = task_request("independent Worker");
        request.config_bundle = None;
        request.memory_settings.as_mut().unwrap().workspace_id = self.scope.workspace_id.clone();
        self.runtime
            .create_worker_scoped(&self.scope, request)
            .unwrap()
            .worker_ref
    }

    // Independent successful snapshot made before the Subject deletion starts.
    fn prior_archive(&self) -> WorkerRef {
        let worker = self.independent_worker();
        let aggregate = self
            .temp
            .path()
            .join("workers")
            .join(worker.worker_id.to_string());
        let session = session_store::new_session_id();
        let segment = session_store::new_segment_id();
        WorkerSessionStore::new(aggregate.join("session"))
            .unwrap()
            .create_segment(
                session,
                segment,
                &[LogEntry::AnnotatedSegmentStart {
                    ts: 1,
                    session_id: session,
                    system_prompt: None,
                    config: Default::default(),
                    history: vec![],
                    forked_from: None,
                    compacted_from: None,
                }],
            )
            .unwrap();
        WorkerAggregateStore::new(&aggregate, format!("worker-runtime-{}", worker.worker_id))
            .unwrap()
            .write(
                &session_store::WorkerMetadata::new(
                    format!("worker-runtime-{}", worker.worker_id),
                    Some(session_store::WorkerActiveSegmentRef::active_segment(
                        session, segment,
                    )),
                )
                .with_workspace_id(&self.scope.workspace_id),
            )
            .unwrap();
        self.runtime
            .stop_worker_scoped(&self.scope, &worker, None)
            .unwrap();
        let mut removal = self.retention_request(SessionDisposition::Archive);
        removal.worker_id = worker.worker_id;
        removal.operation_id = "prior-removal".into();
        removal.archive_id = Some("prior-archive".into());
        removal.input_fingerprint = "prior-fingerprint".into();
        self.runtime.execute_worker_retention(&removal).unwrap();
        worker
    }
}

#[test]
fn subject_job_removal_purge_erases_copies_preserves_receipt_and_prior_success_after_reopen() {
    for disposition in [SessionDisposition::Archive, SessionDisposition::Purge] {
        let f = Fixture::new();
        f.prior_archive();
        let prior = tree(&f.temp.path().join("archives/workers/prior-archive"));
        assert!(!prior.is_empty());
        let unrelated = f.independent_worker();
        let unrelated_root = f
            .temp
            .path()
            .join("workers")
            .join(unrelated.worker_id.to_string());
        let unrelated_before = tree(&unrelated_root);
        let mut request = f.removal_purge_request(disposition);
        request.removal.diagnostics_disposition = runtime_api::DiagnosticsDisposition::Retain;
        let original_result = f.complete_removal(&request);
        let receipt = std::fs::read(f.receipt_path()).unwrap();
        let archive = f.temp.path().join("archives/workers/canonical-archive");
        let diagnostics = f.temp.path().join("archives/diagnostics/remove-source");
        assert!(diagnostics.is_dir());
        let manifest = (disposition == SessionDisposition::Archive)
            .then(|| std::fs::read(archive.join("manifest.json")).unwrap());
        let archive_staging = f
            .temp
            .path()
            .join("archives/workers/.canonical-archive.tmp-123-1");
        if let Some(manifest) = &manifest {
            std::fs::create_dir_all(archive_staging.join("session/segments")).unwrap();
            std::fs::write(archive_staging.join("manifest.json"), manifest).unwrap();
            std::fs::write(
                archive_staging.join("session/segments/interrupted.jsonl"),
                b"interrupted Subject Job copy",
            )
            .unwrap();
        }
        // Diagnostics copying precedes publication of its manifest. Interrupted
        // staging is still bound to the exact removal operation name.
        let diagnostics_staging = f
            .temp
            .path()
            .join("archives/diagnostics/.remove-source.tmp-123-2");
        std::fs::create_dir_all(&diagnostics_staging).unwrap();
        std::fs::write(
            diagnostics_staging.join("worker.out.log"),
            b"interrupted diagnostic copy",
        )
        .unwrap();
        for _ in 0..2 {
            let response = f
                .runtime
                .purge_subject_job_removal_scoped(&f.scope, request.clone())
                .unwrap();
            assert_eq!(response.operation_id, request.removal.operation_id);
            assert_eq!(
                response.input_fingerprint,
                request.removal.input_fingerprint
            );
            assert_eq!(response.worker_id, request.removal.worker_id);
            assert_eq!(response.workspace_id, request.removal.workspace_id);
            assert!(!diagnostics.exists());
            assert!(!diagnostics_staging.exists());
            assert!(!archive_staging.join("session").exists());
            assert!(!archive.join("session").exists());
            assert_eq!(std::fs::read(f.receipt_path()).unwrap(), receipt);
            if let Some(manifest) = &manifest {
                assert_eq!(
                    std::fs::read(archive.join("manifest.json")).unwrap(),
                    *manifest
                );
                assert!(matches!(
                    runtime_session_retention(f.temp.path()).ensure_live(f.session),
                    Err(session_store::StoreError::SessionErased(_))
                ));
            }
        }
        assert_eq!(
            tree(&f.temp.path().join("archives/workers/prior-archive")),
            prior
        );
        assert_eq!(tree(&unrelated_root), unrelated_before);
        let Fixture {
            temp,
            runtime,
            scope,
            store,
            session,
            ..
        } = f;
        drop(store);
        drop(runtime);
        let runtime = Runtime::with_fs_store_and_execution_backend(
            crate::FsRuntimeStoreOptions::new(temp.path()).with_runtime_id("runtime-test"),
            Arc::new(AcceptingBackend),
        )
        .unwrap();
        runtime.bind_runtime_identity("runtime-test").unwrap();
        // Lost response/restart is an exact retry, not a new erasure operation.
        runtime
            .purge_subject_job_removal_scoped(&scope, request.clone())
            .unwrap();
        let removal =
            serde_json::from_value(serde_json::to_value(&request.removal).unwrap()).unwrap();
        assert_eq!(
            runtime.execute_worker_retention(&removal).unwrap(),
            original_result
        );
        assert!(!archive.join("session").exists());
        assert!(!diagnostics.exists());
        assert_eq!(
            std::fs::read(temp.path().join("retention/operations/remove-source.json")).unwrap(),
            receipt
        );
        assert_eq!(
            tree(&temp.path().join("archives/workers/prior-archive")),
            prior
        );
        assert_eq!(tree(&unrelated_root), unrelated_before);
        if disposition == SessionDisposition::Archive {
            assert!(matches!(
                runtime_session_retention(temp.path()).lock_live(session),
                Err(session_store::StoreError::SessionErased(_))
            ));
            assert!(
                runtime
                    .session_public_material_reader_scoped(
                        &scope,
                        &Source::Archived {
                            archive_id: "canonical-archive".into()
                        },
                        &session.to_string()
                    )
                    .is_err()
            );
        }
    }
}

#[test]
fn subject_job_removal_purge_requires_complete_old_request_and_confirmed_exact_fence() {
    let f = Fixture::new();
    let request = f.removal_purge_request(SessionDisposition::Archive);
    let copies = f.copies();
    assert!(
        f.runtime
            .purge_subject_job_removal_scoped(&f.scope, request.clone())
            .is_err()
    );
    assert_eq!(f.copies(), copies);
    f.complete_removal(&request);
    let copies = f.copies();
    let receipt = std::fs::read(f.receipt_path()).unwrap();
    let original = serde_json::to_value(&request).unwrap();
    // Every saved request field is bound, not just operation/fingerprint. These
    // are external serialized requests, including the pre-policy-change values.
    for (section, field, value) in [
        (
            "deletion",
            "workspace_id",
            serde_json::json!("foreign-workspace"),
        ),
        (
            "deletion",
            "worker_id",
            serde_json::json!(WorkerId::now_v7()),
        ),
        (
            "deletion",
            "create_fingerprint",
            serde_json::json!("foreign-create"),
        ),
        (
            "deletion",
            "operation_id",
            serde_json::json!("foreign-deletion"),
        ),
        (
            "removal",
            "operation_id",
            serde_json::json!("unknown-removal"),
        ),
        (
            "removal",
            "input_fingerprint",
            serde_json::json!("foreign-fingerprint"),
        ),
        (
            "removal",
            "archive_id",
            serde_json::json!("foreign-archive"),
        ),
        (
            "removal",
            "workspace_id",
            serde_json::json!("foreign-workspace"),
        ),
        (
            "removal",
            "source_runtime_id",
            serde_json::json!("foreign-runtime"),
        ),
        (
            "removal",
            "worker_id",
            serde_json::json!(WorkerId::now_v7()),
        ),
        ("removal", "source_created_at", serde_json::json!("changed")),
        ("removal", "removed_at", serde_json::json!("changed")),
        ("removal", "effective_profile", serde_json::json!("changed")),
        ("removal", "retention_class", serde_json::json!("changed")),
        ("removal", "policy_id", serde_json::json!("new-policy")),
        ("removal", "session_disposition", serde_json::json!("purge")),
        (
            "removal",
            "diagnostics_disposition",
            serde_json::json!("retain"),
        ),
    ] {
        let mut changed = original.clone();
        changed[section][field] = value;
        let changed = serde_json::from_value(changed).unwrap();
        assert!(
            f.runtime
                .purge_subject_job_removal_scoped(&f.scope, changed)
                .is_err(),
            "{section}.{field}"
        );
        assert_eq!(f.copies(), copies, "{section}.{field}");
        assert_eq!(std::fs::read(f.receipt_path()).unwrap(), receipt);
    }
    for scope in [
        RuntimeWorkspaceScope::new("workspace-b", "https://backend.test"),
        RuntimeWorkspaceScope::new("workspace-a", "https://other-backend.test"),
    ] {
        assert!(
            f.runtime
                .purge_subject_job_removal_scoped(&scope, request.clone())
                .is_err()
        );
        assert_eq!(f.copies(), copies);
    }
}

#[test]
fn subject_job_removal_purge_missing_pending_or_foreign_receipt_preserves_copies() {
    for kind in [
        "missing",
        "pending",
        "foreign-worker",
        "foreign-disposition",
    ] {
        let f = Fixture::new();
        let mut request = f.removal_purge_request(SessionDisposition::Archive);
        request.removal.diagnostics_disposition = runtime_api::DiagnosticsDisposition::Retain;
        f.complete_removal(&request);
        let path = f.receipt_path();
        let mut receipt: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        match kind {
            "missing" => std::fs::remove_file(&path).unwrap(),
            _ => {
                match kind {
                    "pending" => receipt["result"]["source_removed"] = false.into(),
                    "foreign-worker" => {
                        receipt["result"]["worker_id"] = serde_json::json!(WorkerId::now_v7())
                    }
                    _ => receipt["result"]["session_disposition"] = "purge".into(),
                }
                std::fs::write(&path, serde_json::to_vec(&receipt).unwrap()).unwrap();
            }
        }
        let copies = f.copies();
        let retained_receipt = std::fs::read(&path).ok();
        assert!(
            f.runtime
                .purge_subject_job_removal_scoped(&f.scope, request)
                .is_err(),
            "{kind}"
        );
        assert_eq!(f.copies(), copies, "{kind}");
        assert_eq!(std::fs::read(path).ok(), retained_receipt);
        runtime_session_retention(f.temp.path())
            .ensure_live(f.session)
            .unwrap();
    }
}

#[tokio::test]
async fn subject_job_removal_purge_signed_http_requires_delete_and_matching_scope() {
    let f = Fixture::new();
    let mut request = f.removal_purge_request(SessionDisposition::Archive);
    request.removal.diagnostics_disposition = runtime_api::DiagnosticsDisposition::Retain;
    f.complete_removal(&request);
    let identity = RuntimeIdentityMaterial::generate("purge-key").unwrap();
    let trust = workspace_trust(&identity);
    let app = runtime_http_router_with_workspace_auth(
        f.runtime.clone(),
        None,
        workspace_test_auth(vec![trust.clone()]),
    );
    let path = "/v1/subject-job-removal-purge";
    let copies = f.copies();
    for operation in ["workers:read", "workers:stop", "workers:create"] {
        let body = serde_json::to_vec(&request).unwrap();
        let mut claims = signed_claims(&trust, "POST", path, &body);
        claims.operation = operation.into();
        let response = signed_request(app.clone(), &identity, &claims, body, "workspace-a").await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(f.copies(), copies);
    }
    let mut foreign = request.clone();
    foreign.deletion.workspace_id = "foreign-workspace".into();
    let body = serde_json::to_vec(&foreign).unwrap();
    let claims = signed_claims(&trust, "POST", path, &body);
    let response = signed_request(app.clone(), &identity, &claims, body, "workspace-a").await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(f.copies(), copies);
    for _ in 0..2 {
        let body = serde_json::to_vec(&request).unwrap();
        let claims = signed_claims(&trust, "POST", path, &body);
        assert_eq!(claims.operation, "workers:delete");
        let response = signed_request(app.clone(), &identity, &claims, body, "workspace-a").await;
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), 16384)
            .await
            .unwrap();
        let response: runtime_api::SubjectJobRemovalPurgeResponse =
            serde_json::from_slice(&bytes).unwrap();
        assert_eq!(response.operation_id, request.removal.operation_id);
        assert_eq!(
            response.input_fingerprint,
            request.removal.input_fingerprint
        );
        assert!(
            !f.temp
                .path()
                .join("archives/workers/canonical-archive/session")
                .exists()
        );
        assert!(
            !f.temp
                .path()
                .join("archives/diagnostics/remove-source")
                .exists()
        );
    }
}

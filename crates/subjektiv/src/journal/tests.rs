//! Component tests through the Journal store boundary and production FeatureStorage migrations.
//! Only Host-attested Session fragments and Job bindings are supplied by the fixture; no Session
//! runtime or provider is started. Temporary storage owns cleanup. Receipt insertion faults are
//! deterministic SQLite triggers, not a claim of arbitrary process-crash durability.

mod input_delivery {
    use super::*;
    use transport::{JournalJobRequest as Request, JournalJobResponse as Response};

    fn job_request(body: &str) -> ::job::JobRequest {
        ::job::JobRequest {
            job_id: "immutable-body-job".into(),
            purpose: "Journal input delivery gate".into(),
            input_ref: "journal:immutable-input".into(),
            input: serde_json::json!({ "raw_body": body }),
            instruction: "Record the immutable input body".into(),
            profile: "builtin:subjektiv-journal-recording".into(),
            serialization_key: None,
            limits: Default::default(),
        }
    }

    fn digest() -> String {
        job_request("保存済みの公開原文本文と経緯")
            .input_digest()
            .unwrap()
    }

    fn malformed_digests() -> Vec<String> {
        vec![
            String::new(),
            "sha256:not-a-digest".into(),
            "a".repeat(64),
            format!("sha256:{}", "A".repeat(64)),
            format!("sha256:{}", "g".repeat(64)),
            format!("sha256:{}", "a".repeat(63)),
            format!("sha256:{}", "a".repeat(65)),
            format!("SHA256:{}", "a".repeat(64)),
        ]
    }

    fn run(f: &Fixture, ctx: &JournalJobContext, request: Request) -> JournalResult<Response> {
        service::execute_snapshot(f.store(), ctx, &digest(), request)
    }

    fn attest(f: &Fixture, ctx: &JournalJobContext) {
        service::attest_input_delivery(f.store(), ctx, &digest(), &f.ranges).unwrap();
    }

    #[test]
    fn input_attestation_does_not_advance_journal_progress_and_replays_after_reopen() {
        let mut f = Fixture::new();
        let (generation, ctx) = f.generation("immutable-input", f.ranges.clone());
        let before = f.state();
        let sources = f.sources();
        attest(&f, &ctx);
        attest(&f, &ctx);
        assert_eq!(f.state(), before);
        assert_eq!(f.sources(), sources);
        assert!(f.entries().is_empty());
        assert!(matches!(
            service::finish(f.store(), &ctx),
            Err(JournalError::Conflict(_))
        ));
        assert!(f.store().journal_retention_required(&f.host).unwrap());
        f.reopen();
        attest(&f, &ctx);
        assert_eq!(f.state(), before);
        assert_eq!(f.sources(), sources);
        f.store()
            .database
            .try_with_connection(|tx| {
                let saved: (String, String) = tx.query_row(
                    "SELECT generation_id,input_digest FROM journal_generation_input_deliveries",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )?;
                assert_eq!(saved, (generation.id.clone(), digest()));
                let source_deliveries: u64 = tx.query_row(
                    "SELECT COUNT(*) FROM journal_generation_deliveries",
                    [],
                    |r| r.get(0),
                )?;
                let receipts: u64 =
                    tx.query_row("SELECT COUNT(*) FROM journal_operations", [], |r| r.get(0))?;
                assert_eq!((source_deliveries, receipts), (0, 0));
                Ok::<_, JournalError>(())
            })
            .unwrap();
    }

    #[test]
    fn snapshot_gate_requires_matching_digest_and_exact_generation_selection() {
        let mut f = Fixture::new();
        let (generation, ctx) = f.generation("digest-binding", f.ranges.clone());
        let record = f.operation("record", open_action("ongoing"), f.ranges.clone());
        for request in [
            Request::ReadState,
            Request::Execute {
                operation: record.clone(),
            },
        ] {
            assert!(matches!(
                run(&f, &ctx, request),
                Err(JournalError::Conflict(_))
            ));
        }
        let mut changed = f.ranges.clone();
        changed[0].bytes.start += 1;
        for selection in [
            changed,
            f.ranges[1..].to_vec(),
            f.ranges.iter().rev().cloned().collect(),
        ] {
            assert!(matches!(
                service::attest_input_delivery(f.store(), &ctx, &digest(), &selection),
                Err(JournalError::Conflict(_))
            ));
        }
        for malformed in malformed_digests() {
            assert!(matches!(
                service::attest_input_delivery(f.store(), &ctx, &malformed, &f.ranges),
                Err(JournalError::InvalidInput(_))
            ));
        }
        attest(&f, &ctx);
        f.reopen();
        let different = job_request("改変された公開原文本文")
            .input_digest()
            .unwrap();
        assert_ne!(different, digest());
        assert!(matches!(
            service::attest_input_delivery(f.store(), &ctx, &different, &f.ranges),
            Err(JournalError::Conflict(_))
        ));
        assert!(matches!(
            service::execute_snapshot(f.store(), &ctx, &different, Request::ReadState),
            Err(JournalError::Conflict(_))
        ));
        assert!(matches!(
            service::execute_snapshot(
                f.store(),
                &ctx,
                &different,
                Request::Execute {
                    operation: record.clone()
                }
            ),
            Err(JournalError::Conflict(_))
        ));
        assert!(matches!(
            run(&f, &ctx, Request::Execute { operation: record }),
            Ok(Response::Saved(_))
        ));
        f.close(&generation);
        let (_, replacement) = f.generation("replacement-input", f.ranges.clone());
        assert!(matches!(
            run(&f, &replacement, Request::ReadState),
            Err(JournalError::Conflict(_))
        ));
        attest(&f, &replacement);
        assert!(matches!(
            run(&f, &replacement, Request::ReadState),
            Ok(Response::State(_))
        ));
    }

    #[test]
    fn snapshot_execution_reads_journal_only_and_keeps_receipt_checkpoint_and_position_constraints()
    {
        let mut f = Fixture::new();
        let (_, ctx) = f.generation("source-free", f.ranges.clone());
        attest(&f, &ctx);
        let source = Request::ReadText {
            read: transport::JournalTextRead {
                target: transport::JournalTextTarget::Source {
                    pointer: f.ranges[0].source.pointer.clone(),
                },
                offset: 0,
                limit: MAX_JOURNAL_READ_BYTES,
                content_token: None,
            },
        };
        let error = run(&f, &ctx, source).unwrap_err();
        assert!(
            matches!(error, JournalError::PermissionDenied(ref message) if message.contains("Source target") && message.contains("immutable Job input"))
        );
        let record = f.operation("saved-record", open_action("ongoing"), f.ranges.clone());
        let request = Request::Execute {
            operation: record.clone(),
        };
        let receipt = run(&f, &ctx, request.clone()).unwrap();
        // The lost response is recovered from the same operation after closing/reopening SQLite.
        f.reopen();
        assert_eq!(run(&f, &ctx, request).unwrap(), receipt);
        assert_eq!(f.entries().len(), 1);
        assert_eq!(f.state().0.process_order, 0);
        assert!(matches!(
            service::finish(f.store(), &ctx),
            Err(JournalError::Conflict(_))
        ));
        let position = f.operation(
            "saved-position",
            JournalAction::UpdatePosition {
                current_position: "現在地".into(),
            },
            vec![],
        );
        run(
            &f,
            &ctx,
            Request::Execute {
                operation: position,
            },
        )
        .unwrap();
        for evidence in [
            vec![],
            vec!["missing-record".into()],
            vec!["saved-position".into()],
        ] {
            let operation = f.operation(
                "invalid-checkpoint",
                JournalAction::Checkpoint {
                    record_operation_ids: evidence,
                },
                f.ranges.clone(),
            );
            assert!(run(&f, &ctx, Request::Execute { operation }).is_err());
            assert_eq!(
                (f.state().0.process_order, f.state().0.process_byte),
                (0, 0)
            );
        }
        for (snapshot, expected) in [
            (transport::JournalSnapshot::Initial, ""),
            (transport::JournalSnapshot::Current, "現在地"),
        ] {
            let response = run(
                &f,
                &ctx,
                Request::ReadText {
                    read: transport::JournalTextRead {
                        target: transport::JournalTextTarget::Position { snapshot },
                        offset: 0,
                        limit: 100,
                        content_token: None,
                    },
                },
            )
            .unwrap();
            assert!(matches!(response, Response::Text(page) if page.text == expected));
        }
        for target in [
            transport::JournalTextTarget::OpenBody {
                snapshot: transport::JournalSnapshot::Current,
            },
            transport::JournalTextTarget::OpenOngoing {
                snapshot: transport::JournalSnapshot::Current,
            },
        ] {
            assert!(matches!(
                run(
                    &f,
                    &ctx,
                    Request::ReadText {
                        read: transport::JournalTextRead {
                            target,
                            offset: 0,
                            limit: 100,
                            content_token: None
                        }
                    }
                ),
                Ok(Response::Text(_))
            ));
        }
        assert!(matches!(
            run(
                &f,
                &ctx,
                Request::QueryOperation {
                    request_id: record.request_id.clone()
                }
            ),
            Ok(Response::Operation(Some(_)))
        ));
        assert!(
            matches!(run(&f, &ctx, Request::ListOperations { after: None, limit: 32 }), Ok(Response::Operations(page)) if page.items.len() == 2)
        );
        assert!(
            matches!(run(&f, &ctx, Request::ReadOperationCoverage { request_id: record.request_id.clone(), offset: 0, limit: 32 }), Ok(Response::Coverage(page)) if page.items == f.ranges)
        );
        let checkpoint = f.operation(
            "checkpoint",
            JournalAction::Checkpoint {
                record_operation_ids: vec![record.request_id],
            },
            f.ranges.clone(),
        );
        run(
            &f,
            &ctx,
            Request::Execute {
                operation: checkpoint,
            },
        )
        .unwrap();
        f.reopen();
        assert_eq!(
            (f.state().0.process_order, f.state().0.process_byte),
            (2, 0)
        );
        assert!(!f.store().journal_retention_required(&f.host).unwrap());
        let completion = service::finish(f.store(), &ctx).unwrap();
        assert_eq!((completion.process_order, completion.process_byte), (2, 0));
        assert!(matches!(
            run(&f, &ctx, Request::ReadState),
            Err(JournalError::Conflict(_))
        ));
        assert!(matches!(
            service::attest_input_delivery(f.store(), &ctx, &digest(), &f.ranges),
            Err(JournalError::Conflict(_))
        ));
        assert_eq!(service::finish(f.store(), &ctx).unwrap(), completion);
        let attr = f.host.attribution.as_ref().unwrap();
        let binding = f
            .store()
            .claim_journal_writer(&f.host.subject_id, &attr.runtime_id, &attr.worker_id)
            .unwrap();
        f.store().release_journal_writer(&binding).unwrap();
        assert!(matches!(
            service::attest_input_delivery(f.store(), &ctx, &digest(), &f.ranges),
            Err(JournalError::PermissionDenied(_))
        ));
        assert!(matches!(
            run(&f, &ctx, Request::ReadState),
            Err(JournalError::PermissionDenied(_))
        ));
    }

    #[test]
    fn retired_subject_rejects_late_attestation_and_snapshot_execution() {
        let mut f = Fixture::new();
        let (_, ctx) = f.generation("retired-input", f.ranges.clone());
        attest(&f, &ctx);
        let operation = f.operation(
            "late-retired-record",
            open_action("must not save"),
            f.ranges.clone(),
        );
        f.store().retire_subject(&f.host.subject_id).unwrap();
        f.reopen();
        assert!(matches!(
            service::attest_input_delivery(f.store(), &ctx, &digest(), &f.ranges),
            Err(JournalError::PermissionDenied(_))
        ));
        assert!(matches!(
            run(&f, &ctx, Request::ReadState),
            Err(JournalError::PermissionDenied(_))
        ));
        assert!(matches!(
            run(&f, &ctx, Request::Execute { operation }),
            Err(JournalError::PermissionDenied(_))
        ));
        assert!(f.entries().is_empty());
    }

    #[test]
    fn legacy_source_delivery_cannot_satisfy_immutable_input_gate() {
        let f = Fixture::with_sizes(&[8]);
        let (_, ctx) = f.generation("legacy-read", f.ranges.clone());
        service::execute_bounded(
            f.store(),
            &ctx,
            Request::ReadText {
                read: transport::JournalTextRead {
                    target: transport::JournalTextTarget::Source {
                        pointer: f.ranges[0].source.pointer.clone(),
                    },
                    offset: 0,
                    limit: 8,
                    content_token: None,
                },
            },
            |source, _, _| {
                Ok(recording::JournalMaterial {
                    range: JournalSourceRange {
                        source: source.clone(),
                        bytes: JournalByteRange { start: 0, end: 8 },
                    },
                    text: "12345678".into(),
                })
            },
        )
        .unwrap();
        assert!(matches!(
            run(&f, &ctx, Request::ReadState),
            Err(JournalError::Conflict(_))
        ));
        assert!(matches!(
            run(
                &f,
                &ctx,
                Request::Execute {
                    operation: f.operation("new-record", open_action("ongoing"), f.ranges.clone())
                }
            ),
            Err(JournalError::Conflict(_))
        ));
    }

    #[test]
    fn input_delivery_migration_preserves_existing_subjects_and_enforces_binding_constraints() {
        // Exercise the oldest Journal baseline, the immediate predecessor and fresh creation.
        for baseline in [
            &crate::MIGRATIONS[..7],
            &crate::MIGRATIONS[..12],
            crate::MIGRATIONS,
        ] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("migration");
            let manager = FeatureStorage::new(&root);
            let scope = manager.scope("input-delivery-upgrade").unwrap();
            let registration = scope
                .register(feature_storage::FeatureRegistration::new(
                    crate::SUBJEKTIV_FEATURE_ID,
                    baseline,
                ))
                .unwrap();
            let store = SubjektivStore::open(&scope, &registration).unwrap();
            let subject = store
                .create_subject_with_behavior(
                    SubjectRole::new("preserved").unwrap(),
                    "Existing behavior".into(),
                )
                .unwrap();
            drop(store);
            drop(scope);
            manager.shutdown().unwrap();
            drop(manager);

            let manager = FeatureStorage::new(&root);
            let scope = manager.scope("input-delivery-upgrade").unwrap();
            let registration = SubjektivStore::register(&scope).unwrap();
            let store = SubjektivStore::open(&scope, &registration).unwrap();
            assert_eq!(store.subject(&subject.id).unwrap(), Some(subject.clone()));
            let host = session_host(&store, &subject.id, "migration-session");
            let ranges = vec![source_range("migration-session", 0, 8)];
            let id = store
                .accept_journal_sources(&host, "Migration", &ranges)
                .unwrap();
            let binding = JobAttemptBinding {
                job_id: "migration-job".into(),
                attempt_id: "attempt".into(),
            };
            let generation = store
                .prepare_journal_generation(&host, &id, binding.clone(), ranges.clone())
                .unwrap();
            let ctx = JournalJobContext::validated(&store, &host, &generation.id, binding).unwrap();
            service::attest_input_delivery(&store, &ctx, &digest(), &ranges).unwrap();
            store
                .close_journal_generation(&host, &generation.id, &generation.job)
                .unwrap();
            let second = store
                .prepare_journal_generation(
                    &host,
                    &id,
                    JobAttemptBinding {
                        job_id: "next-migration-job".into(),
                        attempt_id: "attempt".into(),
                    },
                    ranges.clone(),
                )
                .unwrap();
            store.database.try_transaction(|tx| {
                let columns = tx.prepare("PRAGMA table_info(journal_generation_input_deliveries)")?.query_map([], |r| r.get::<_, String>(1))?.collect::<Result<Vec<_>, _>>()?;
                assert_eq!(columns, ["generation_id", "input_digest"]);
                assert!(tx.execute("INSERT INTO journal_generation_input_deliveries VALUES(?1,?2)", rusqlite::params![generation.id, digest()]).is_err());
                assert!(tx.execute("INSERT INTO journal_generation_input_deliveries VALUES('missing-generation',?1)", [digest()]).is_err());
                assert!(tx.execute("UPDATE journal_generation_input_deliveries SET input_digest=?1", [job_request("changed body").input_digest().unwrap()]).is_err());
                // Digest validation is a schema constraint as well as an API boundary check.
                for invalid in malformed_digests() {
                    assert!(tx.execute("INSERT INTO journal_generation_input_deliveries VALUES(?1,?2)", rusqlite::params![second.id, invalid]).is_err());
                }
                let saved: String = tx.query_row("SELECT input_digest FROM journal_generation_input_deliveries WHERE generation_id=?1", [&generation.id], |r| r.get(0))?;
                assert_eq!(saved, digest());
                Ok::<_, JournalError>(())
            }).unwrap();
            drop(store);
            drop(scope);
            manager.shutdown().unwrap();
        }
    }

    #[test]
    fn input_delivery_migration_failure_rolls_back_table_creation_and_retries() {
        // A colliding trigger is created before migration so failure occurs after its new table DDL.
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("rollback");
        let manager = FeatureStorage::new(&root);
        let scope = manager.scope("input-delivery-rollback").unwrap();
        let registration = scope
            .register(feature_storage::FeatureRegistration::new(
                crate::SUBJEKTIV_FEATURE_ID,
                &crate::MIGRATIONS[..12],
            ))
            .unwrap();
        let store = SubjektivStore::open(&scope, &registration).unwrap();
        let subject = store
            .create_subject_with_behavior(
                SubjectRole::new("preserved").unwrap(),
                "Retain on failure".into(),
            )
            .unwrap();
        store.database.try_transaction(|tx| {
            tx.execute_batch("CREATE TRIGGER journal_generation_input_delivery_no_update BEFORE UPDATE ON subjects BEGIN SELECT RAISE(ABORT, 'injected migration collision'); END;")?;
            Ok::<_, JournalError>(())
        }).unwrap();
        drop(store);
        drop(scope);
        manager.shutdown().unwrap();
        drop(manager);

        let manager = FeatureStorage::new(&root);
        let scope = manager.scope("input-delivery-rollback").unwrap();
        let registration = SubjektivStore::register(&scope).unwrap();
        assert!(matches!(
            SubjektivStore::open(&scope, &registration),
            Err(crate::SubjektivError::Storage(
                feature_storage::FeatureStorageError::MigrationFailed { .. }
            ))
        ));
        drop(scope);
        manager.shutdown().unwrap();
        drop(manager);

        let manager = FeatureStorage::new(&root);
        let scope = manager.scope("input-delivery-rollback").unwrap();
        let registration = scope
            .register(feature_storage::FeatureRegistration::new(
                crate::SUBJEKTIV_FEATURE_ID,
                &crate::MIGRATIONS[..12],
            ))
            .unwrap();
        let store = SubjektivStore::open(&scope, &registration).unwrap();
        assert_eq!(store.subject(&subject.id).unwrap(), Some(subject.clone()));
        store.database.try_transaction(|tx| {
            assert!(!tx.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='journal_generation_input_deliveries')", [], |r| r.get::<_, bool>(0))?);
            tx.execute_batch("DROP TRIGGER journal_generation_input_delivery_no_update")?;
            Ok::<_, JournalError>(())
        }).unwrap();
        drop(store);
        drop(scope);
        manager.shutdown().unwrap();
        drop(manager);

        let manager = FeatureStorage::new(&root);
        let scope = manager.scope("input-delivery-rollback").unwrap();
        let registration = SubjektivStore::register(&scope).unwrap();
        let store = SubjektivStore::open(&scope, &registration).unwrap();
        assert_eq!(store.subject(&subject.id).unwrap(), Some(subject));
        store.database.try_with_connection(|tx| {
            assert!(tx.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='journal_generation_input_deliveries')", [], |r| r.get::<_, bool>(0))?);
            Ok::<_, JournalError>(())
        }).unwrap();
        drop(store);
        drop(scope);
        manager.shutdown().unwrap();
    }

    #[test]
    fn late_input_attestation_and_execution_cannot_revive_cancelled_or_erased_generations() {
        for erased in [false, true] {
            let mut f = Fixture::new();
            let (generation, ctx) = f.generation("late-input", f.ranges.clone());
            attest(&f, &ctx);
            let record = f.operation(
                "late-record",
                open_action("must not save"),
                f.ranges.clone(),
            );
            if erased {
                f.store()
                    .erase_journal_session(&f.host, "erase-input")
                    .unwrap();
            } else {
                f.close(&generation);
                f.generation("replacement", f.ranges.clone());
            }
            f.reopen();
            assert!(service::attest_input_delivery(f.store(), &ctx, &digest(), &f.ranges).is_err());
            assert!(run(&f, &ctx, Request::ReadState).is_err());
            assert!(run(&f, &ctx, Request::Execute { operation: record }).is_err());
            f.store()
                .database
                .try_with_connection(|tx| {
                    let count: u64 = tx.query_row(
                        "SELECT COUNT(*) FROM journal_generation_input_deliveries",
                        [],
                        |r| r.get(0),
                    )?;
                    assert_eq!(count, u64::from(!erased));
                    let records: u64 =
                        tx.query_row("SELECT COUNT(*) FROM journal_operations", [], |r| r.get(0))?;
                    assert_eq!(records, 0);
                    Ok::<_, JournalError>(())
                })
                .unwrap();
        }
    }
}

mod snapshot_inputs {
    //! Component coverage joins shared canonical values, Journal input construction, SQLite
    //! generations, and the production snapshot service. Synthetic Host-attested captures and
    //! a saved Job file replace only capture/dispatch ownership; no Worker or Session reader runs.
    use super::*;
    use recording::JournalInput;
    use session_store::capture::{
        CaptureBody, CaptureDto, CaptureEntryDto, CaptureLimits, CaptureSource, CaptureUsePolicy,
        RecordingPosition,
    };
    use session_store::public_material::{
        PublicMaterialDescriptor, PublicMaterialIdentity, PublicMaterialOrigin,
    };
    use transport::{JournalJobRequest as Request, JournalJobResponse as Response};

    fn canonical_capture(session: &str, bodies: &[String]) -> CaptureDto {
        let capture = CaptureDto {
            version: 1,
            session_id: session.into(),
            segment_id: Uuid::from_u128(3).to_string(),
            view_entry_count: bodies.len() as u64,
            entries: bodies
                .iter()
                .enumerate()
                .map(|(index, body)| {
                    let segment_id = Uuid::from_u128(1 + (index / 50) as u128);
                    let entry_id = format!("synthetic-{index}");
                    let origin = session_store::LoggedSessionHistoryOrigin::LocalHumanInput;
                    CaptureEntryDto {
                        entry_id: entry_id.clone(),
                        order: index as u64,
                        source: CaptureSource {
                            segment_id: segment_id.to_string(),
                            entry_index: index as u64,
                            provenance: serde_json::from_value(serde_json::json!("human_input"))
                                .unwrap(),
                            history_origin: Some(origin.clone()),
                            derived_from: vec![],
                        },
                        policy: CaptureUsePolicy {
                            recording_material: true,
                            memory_evidence: false,
                            observation_readable: false,
                        },
                        body: CaptureBody::CanonicalPublic {
                            descriptor: PublicMaterialDescriptor {
                                identity: PublicMaterialIdentity {
                                    segment_id,
                                    entry_ref: format!("E{entry_id}"),
                                },
                                canonical_order: index as u64,
                                origin: PublicMaterialOrigin::History { origin },
                                sha256: Sha256::digest(body.as_bytes())
                                    .iter()
                                    .map(|b| format!("{b:02x}"))
                                    .collect(),
                                total_bytes: body.len() as u64,
                            },
                            byte_offset: 0,
                            text: body.clone(),
                            boundary: None,
                        },
                    }
                })
                .collect(),
        };
        capture.validate(CaptureLimits::default()).unwrap();
        capture
    }

    fn admitted_capture(
        f: &Fixture,
        bodies: &[String],
    ) -> (JournalHostContext, String, CaptureDto) {
        let host = session_host(f.store(), &f.host.subject_id, "canonical-input-session");
        let capture = canonical_capture("canonical-input-session", bodies);
        let mut journal_id = None;
        for ranges in recording::capture_ranges(&capture)
            .unwrap()
            .chunks(MAX_JOURNAL_RANGES)
        {
            let id = f
                .store()
                .accept_journal_sources(&host, "Canonical input", ranges)
                .unwrap();
            if let Some(prior) = &journal_id {
                assert_eq!(prior, &id);
            }
            journal_id = Some(id);
        }
        (host, journal_id.unwrap(), capture)
    }

    fn operation(
        f: &Fixture,
        host: &JournalHostContext,
        id: &str,
        request_id: &str,
        action: JournalAction,
        ranges: Vec<JournalSourceRange>,
    ) -> JournalOperation {
        JournalOperation {
            request_id: request_id.into(),
            expected_state: f.store().read_journal(host, id).unwrap().state_token,
            action,
            covered_ranges: ranges,
        }
    }

    fn seed_initial_state(
        f: &Fixture,
        host: &JournalHostContext,
        id: &str,
        ranges: Vec<JournalSourceRange>,
    ) {
        let binding = JobAttemptBinding {
            job_id: "seed-initial-state".into(),
            attempt_id: "seed-attempt".into(),
        };
        let generation = f
            .store()
            .prepare_journal_generation(host, id, binding.clone(), ranges)
            .unwrap();
        let ctx = JournalJobContext::validated(f.store(), host, &generation.id, binding).unwrap();
        f.store()
            .execute_journal(
                &ctx,
                operation(
                    f,
                    host,
                    id,
                    "seed-open",
                    open_action(&"Original full open body 日本語\n\"\\".repeat(300)),
                    vec![],
                ),
            )
            .unwrap();
        f.store()
            .execute_journal(
                &ctx,
                operation(
                    f,
                    host,
                    id,
                    "seed-position",
                    JournalAction::UpdatePosition {
                        current_position: "Initial position 日本語\n\"\\".repeat(200),
                    },
                    vec![],
                ),
            )
            .unwrap();
        f.store()
            .close_journal_generation(host, &generation.id, &generation.job)
            .unwrap();
    }

    fn job_request(job_id: &str, input: &JournalInput) -> ::job::JobRequest {
        let request = ::job::JobRequest {
            job_id: job_id.into(),
            purpose: "Journal canonical input".into(),
            input_ref: format!("journal:{}", input.journal_id),
            input: serde_json::to_value(input).unwrap(),
            instruction: "Process saved immutable bodies only".into(),
            profile: "builtin:subjektiv-journal-recording".into(),
            serialization_key: None,
            limits: Default::default(),
        };
        request.validate().unwrap();
        request
    }

    fn run(
        f: &Fixture,
        ctx: &JournalJobContext,
        request: &::job::JobRequest,
        action: Request,
    ) -> JournalResult<Response> {
        service::execute_snapshot(f.store(), ctx, &request.input_digest().unwrap(), action)
    }

    #[test]
    fn complete_input_budget_and_successive_canonical_prefixes_preserve_all_escaped_utf8_bytes() {
        let f = Fixture::new();
        let mut bodies: Vec<_> = (0..131)
            .map(|i| format!("[user {i}] 公開原文\n\"\\\u{1}\t").repeat(8))
            .collect();
        bodies[60] = "[long public result] 日本語🦀\n\"\\\u{1}\t".repeat(14_000);
        let (host, id, capture) = admitted_capture(&f, &bodies);
        let all_ranges = recording::capture_ranges(&capture).unwrap();
        seed_initial_state(&f, &host, &id, all_ranges[..1].to_vec());
        let mut reconstructed = vec![String::new(); bodies.len()];
        let mut jobs = 0;
        let mut partial = false;
        for step in 0..20 {
            let before = f.store().journal_resume_state(&host, &id).unwrap();
            if before.0.process_order == bodies.len() as u64 {
                break;
            }
            let callback_count = std::cell::Cell::new(0);
            let input = JournalInput::build(f.store(), &host, &id, |order, byte, budget| {
                callback_count.set(callback_count.get() + 1);
                assert_eq!(
                    (order, byte),
                    (before.0.process_order, before.0.process_byte)
                );
                Ok(capture
                    .recording_slice(
                        RecordingPosition {
                            canonical_order: order,
                            byte_offset: byte,
                        },
                        MAX_JOURNAL_RANGES,
                        budget,
                    )
                    .unwrap()
                    .capture)
            })
            .unwrap();
            assert_eq!(callback_count.get(), 1);
            let request = job_request(&format!("bounded-{step}"), &input);
            assert!(
                serde_json::to_vec(&request.input).unwrap().len() <= ::job::MAX_JOB_INPUT_BYTES
            );
            assert_eq!(JournalInput::decode(&request.input).unwrap(), input);
            assert_eq!(input.initial_state_token, before.0.state_token);
            assert_eq!(
                input.initial_open_id,
                before.1.as_ref().map(|e| e.id.clone())
            );
            let binding = JobAttemptBinding {
                job_id: request.job_id.clone(),
                attempt_id: "attempt".into(),
            };
            let generation = f
                .store()
                .prepare_journal_input(&host, &input, binding.clone())
                .unwrap();
            input.validate_generation(&generation).unwrap();
            let ctx =
                JournalJobContext::validated(f.store(), &host, &generation.id, binding).unwrap();
            service::attest_input_delivery(
                f.store(),
                &ctx,
                &request.input_digest().unwrap(),
                &input.selected_ranges,
            )
            .unwrap();
            // Capture, input construction, generation preparation and delivery cannot reflect bytes.
            assert_eq!(f.store().journal_resume_state(&host, &id).unwrap(), before);
            for range in &input.selected_ranges {
                let (_, offset, text) = input.capture.recording_text(range.source.order).unwrap();
                let target = &mut reconstructed[range.source.order as usize];
                assert_eq!(offset, target.len() as u64);
                assert_eq!(range.bytes.start, offset);
                assert_eq!(range.bytes.end, offset + text.len() as u64);
                assert_eq!(
                    text,
                    &bodies[range.source.order as usize]
                        [range.bytes.start as usize..range.bytes.end as usize]
                );
                target.push_str(text);
                partial |= range.bytes.end < range.source.total_bytes;
            }
            let record_id = format!("bounded-record-{step}");
            run(
                &f,
                &ctx,
                &request,
                Request::Execute {
                    operation: operation(
                        &f,
                        &host,
                        &id,
                        &record_id,
                        JournalAction::ConfirmUnchanged,
                        input.selected_ranges.clone(),
                    ),
                },
            )
            .unwrap();
            run(
                &f,
                &ctx,
                &request,
                Request::Execute {
                    operation: operation(
                        &f,
                        &host,
                        &id,
                        &format!("bounded-position-{step}"),
                        JournalAction::UpdatePosition {
                            current_position: format!("Processed prefix {step}"),
                        },
                        vec![],
                    ),
                },
            )
            .unwrap();
            run(
                &f,
                &ctx,
                &request,
                Request::Execute {
                    operation: operation(
                        &f,
                        &host,
                        &id,
                        &format!("bounded-checkpoint-{step}"),
                        JournalAction::Checkpoint {
                            record_operation_ids: vec![record_id],
                        },
                        input.selected_ranges,
                    ),
                },
            )
            .unwrap();
            service::finish(f.store(), &ctx).unwrap();
            jobs += 1;
        }
        assert!(
            jobs > 2,
            "the long escaped input and 131 entries must require multiple prefixes"
        );
        assert!(
            partial,
            "a long UTF-8 body must be split, not previewed or dropped"
        );
        assert_eq!(reconstructed, bodies);
        let journal = f.store().read_journal(&host, &id).unwrap();
        assert_eq!((journal.process_order, journal.process_byte), (131, 0));
        for range in all_ranges {
            assert_eq!(
                f.store()
                    .read_journal_source(&host, &id, range.source.order)
                    .unwrap()
                    .reflected_bytes,
                range.source.total_bytes
            );
        }
    }

    #[test]
    fn journal_input_decode_rejects_mismatched_ranges_policy_session_tampered_text_and_missing_body()
     {
        let f = Fixture::new();
        let (host, id, capture) = admitted_capture(
            &f,
            &["[user] 公開原文\n".into(), "[user] 次の原文\n".into()],
        );
        let input =
            JournalInput::build(f.store(), &host, &id, |_, _, _| Ok(capture.clone())).unwrap();
        let original = serde_json::to_value(input).unwrap();
        let cases: &[(&str, fn(&mut serde_json::Value))] = &[
            ("range", |v| {
                v["selected_ranges"][0]["bytes"]["end"] = serde_json::json!(1)
            }),
            ("recording policy", |v| {
                v["capture"]["entries"][0]["policy"]["recording_material"] =
                    serde_json::json!(false)
            }),
            ("cross-purpose policy", |v| {
                v["capture"]["entries"][0]["policy"]["memory_evidence"] = serde_json::json!(true)
            }),
            ("session", |v| {
                v["capture"]["session_id"] = serde_json::json!("different-session")
            }),
            ("full text digest", |v| {
                let text = &mut v["capture"]["entries"][0]["body"]["text"];
                *text = serde_json::json!("X".repeat(text.as_str().unwrap().len()));
            }),
            ("Journal version", |v| v["version"] = serde_json::json!(2)),
            ("capture version", |v| {
                v["capture"]["version"] = serde_json::json!(2)
            }),
            ("missing body", |v| {
                v["capture"]["entries"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("body");
            }),
            ("missing text", |v| {
                v["capture"]["entries"][0]["body"]
                    .as_object_mut()
                    .unwrap()
                    .remove("text");
            }),
        ];
        for (name, corrupt) in cases {
            let mut value = original.clone();
            corrupt(&mut value);
            assert!(
                JournalInput::decode(&value).is_err(),
                "accepted corrupt input: {name}"
            );
        }
    }

    #[test]
    fn released_writer_recovers_exact_saved_input_and_completion_without_renewing_mutation_authority()
     {
        let mut f = Fixture::new();
        // This Subject has only the tested Session; unrelated fixture history must not
        // participate in the writer's pending-source release protection.
        let subject = f
            .store()
            .create_subject_with_behavior(
                SubjectRole::new("completion-recovery").unwrap(),
                String::new(),
            )
            .unwrap();
        let host = session_host(f.store(), &subject.id, "released-completion-session");
        let writer = f
            .store()
            .claim_journal_writer(&subject.id, "runtime", "worker")
            .unwrap();
        let capture = canonical_capture(
            "released-completion-session",
            &["[user] 記録済みの公開本文\n".into()],
        );
        let ranges = recording::capture_ranges(&capture).unwrap();
        let id = f
            .store()
            .accept_journal_sources(&host, "Retained completion", &ranges)
            .unwrap();
        seed_initial_state(&f, &host, &id, ranges.clone());
        let input = JournalInput::build(f.store(), &host, &id, |_, _, _| Ok(capture)).unwrap();
        let request = job_request("retained-completion-job", &input);
        let saved_request = serde_json::to_vec(&request).unwrap();
        let binding = JobAttemptBinding {
            job_id: request.job_id.clone(),
            attempt_id: "original-attempt".into(),
        };
        let generation = f
            .store()
            .prepare_journal_input(&host, &input, binding.clone())
            .unwrap();
        let old_ctx =
            JournalJobContext::validated(f.store(), &host, &generation.id, binding.clone())
                .unwrap();
        service::attest_input_delivery(
            f.store(),
            &old_ctx,
            &request.input_digest().unwrap(),
            &input.selected_ranges,
        )
        .unwrap();
        run(
            &f,
            &old_ctx,
            &request,
            Request::Execute {
                operation: operation(
                    &f,
                    &host,
                    &id,
                    "completed-record",
                    JournalAction::ConfirmUnchanged,
                    ranges.clone(),
                ),
            },
        )
        .unwrap();
        run(
            &f,
            &old_ctx,
            &request,
            Request::Execute {
                operation: operation(
                    &f,
                    &host,
                    &id,
                    "completed-position",
                    JournalAction::UpdatePosition {
                        current_position: "Saved position after all selected input".into(),
                    },
                    vec![],
                ),
            },
        )
        .unwrap();
        run(
            &f,
            &old_ctx,
            &request,
            Request::Execute {
                operation: operation(
                    &f,
                    &host,
                    &id,
                    "completed-checkpoint",
                    JournalAction::Checkpoint {
                        record_operation_ids: vec!["completed-record".into()],
                    },
                    ranges,
                ),
            },
        )
        .unwrap();
        let saved_completion = service::finish(f.store(), &old_ctx).unwrap();
        let finished_journal = f.store().read_journal(&host, &id).unwrap();
        assert_eq!(
            (
                saved_completion.process_order,
                saved_completion.process_byte
            ),
            (1, 0)
        );
        f.store().release_journal_writer(&writer).unwrap();
        assert!(
            f.store()
                .journal_worker_was_released(&subject.id, "runtime", "worker")
                .unwrap()
        );
        f.reopen();

        // Reconciliation reads the same immutable Job input, not a new capture or the
        // current Journal projection. Release did not erase this retained completion.
        let request: ::job::JobRequest = serde_json::from_slice(&saved_request).unwrap();
        let input = JournalInput::decode(&request.input).unwrap();
        let retained = f
            .store()
            .prepare_journal_input(&host, &input, binding.clone())
            .unwrap();
        assert_eq!(retained, generation);
        let recovery =
            JournalJobContext::validated_recovery(f.store(), &host, &retained.id, binding.clone())
                .unwrap();
        assert_eq!(
            service::finish(f.store(), &recovery).unwrap(),
            saved_completion
        );
        assert_eq!(
            f.store().read_journal(&host, &id).unwrap(),
            finished_journal
        );
        assert!(matches!(
            JournalJobContext::validated(f.store(), &host, &retained.id, binding.clone()),
            Err(JournalError::PermissionDenied(_))
        ));
        assert!(matches!(
            f.store().prepare_journal_input(
                &host,
                &input,
                JobAttemptBinding {
                    job_id: binding.job_id.clone(),
                    attempt_id: "new-attempt".into()
                }
            ),
            Err(JournalError::PermissionDenied(_))
        ));
        let mut changed = input.clone();
        changed.initial_state_token.push('x');
        assert!(matches!(
            f.store().prepare_journal_input(&host, &changed, binding),
            Err(JournalError::Conflict(_))
        ));
        let late_mutation = operation(
            &f,
            &host,
            &id,
            "late-released-position",
            JournalAction::UpdatePosition {
                current_position: "Must not replace saved position".into(),
            },
            vec![],
        );
        assert!(matches!(
            run(
                &f,
                &old_ctx,
                &request,
                Request::Execute {
                    operation: late_mutation.clone()
                }
            ),
            Err(JournalError::PermissionDenied(_))
        ));
        assert!(matches!(
            f.store().execute_journal(&recovery, late_mutation),
            Err(JournalError::PermissionDenied(_))
        ));
        assert!(matches!(
            service::attest_input_delivery(
                f.store(),
                &old_ctx,
                &request.input_digest().unwrap(),
                &input.selected_ranges
            ),
            Err(JournalError::PermissionDenied(_))
        ));
        assert_eq!(
            f.store().read_journal(&host, &id).unwrap(),
            finished_journal
        );
        assert_eq!(
            service::finish(f.store(), &recovery).unwrap(),
            saved_completion
        );
    }

    #[test]
    fn journal_input_generation_validation_rejects_foreign_scope_and_changed_initial_proof() {
        let f = Fixture::new();
        let (host, id, capture) = admitted_capture(&f, &["[user] original canonical body".into()]);
        let ranges = recording::capture_ranges(&capture).unwrap();
        seed_initial_state(&f, &host, &id, ranges);
        let input = JournalInput::build(f.store(), &host, &id, |_, _, _| Ok(capture)).unwrap();
        let generation = f
            .store()
            .prepare_journal_input(
                &host,
                &input,
                JobAttemptBinding {
                    job_id: "generation-proof".into(),
                    attempt_id: "attempt".into(),
                },
            )
            .unwrap();
        input.validate_generation(&generation).unwrap();
        let cases: &[(&str, fn(&mut JournalGeneration))] = &[
            ("Journal", |g| g.journal_id.push('x')),
            ("Subject", |g| g.initial_journal.subject_id.push('x')),
            ("Session", |g| g.initial_journal.source_session_id.push('x')),
            ("initial content condition", |g| {
                g.initial_journal.state_token.push('x')
            }),
            ("initial open entry", |g| {
                g.initial_open.as_mut().unwrap().id.push('x')
            }),
            ("previous fixed boundary", |g| {
                g.previous_fixed_end = Some(g.selected_ranges[0].source.pointer.clone())
            }),
            ("selection", |g| g.selected_ranges[0].bytes.end -= 1),
        ];
        for (name, change) in cases {
            let mut changed = generation.clone();
            change(&mut changed);
            assert!(
                matches!(
                    input.validate_generation(&changed),
                    Err(JournalError::Conflict(_))
                ),
                "accepted changed generation proof: {name}"
            );
        }
    }

    #[test]
    fn same_saved_job_input_rebinds_full_initial_snapshot_and_reuses_partial_record_receipts_after_reopen()
     {
        let mut f = Fixture::new();
        let bodies = vec![
            "[user] 最初の原文は長いUTF8\n".repeat(30),
            "[user] 続きの確定原文\n".repeat(20),
        ];
        let (host, id, capture) = admitted_capture(&f, &bodies);
        let ranges = recording::capture_ranges(&capture).unwrap();
        seed_initial_state(&f, &host, &id, ranges.clone());
        let input = JournalInput::build(f.store(), &host, &id, |_, _, _| Ok(capture)).unwrap();
        let request = job_request("durable-saved-input", &input);
        let saved_path = f.temp.path().join("immutable-job.json");
        std::fs::write(&saved_path, serde_json::to_vec(&request).unwrap()).unwrap();
        let first_binding = JobAttemptBinding {
            job_id: request.job_id.clone(),
            attempt_id: "first".into(),
        };
        let first = f
            .store()
            .prepare_journal_input(&host, &input, first_binding.clone())
            .unwrap();
        assert_eq!(
            f.store()
                .prepare_journal_input(&host, &input, first_binding.clone())
                .unwrap(),
            first
        );
        input.validate_generation(&first).unwrap();
        let first_ctx =
            JournalJobContext::validated(f.store(), &host, &first.id, first_binding).unwrap();
        service::attest_input_delivery(
            f.store(),
            &first_ctx,
            &request.input_digest().unwrap(),
            &input.selected_ranges,
        )
        .unwrap();
        let record = operation(
            &f,
            &host,
            &id,
            "durable-record",
            JournalAction::Rewrite {
                entry_id: first.initial_open.as_ref().unwrap().id.clone(),
                title: "Saved activity".into(),
                body: "Both original ranges reflected in this record".into(),
                ongoing: "Continue work".into(),
            },
            ranges.clone(),
        );
        let saved_record = run(
            &f,
            &first_ctx,
            &request,
            Request::Execute {
                operation: record.clone(),
            },
        )
        .unwrap();
        let saved_position = run(
            &f,
            &first_ctx,
            &request,
            Request::Execute {
                operation: operation(
                    &f,
                    &host,
                    &id,
                    "first-position",
                    JournalAction::UpdatePosition {
                        current_position: "Position after the successful record".into(),
                    },
                    vec![],
                ),
            },
        )
        .unwrap();
        let mut cut = bodies[0].len() / 2;
        while !bodies[0].is_char_boundary(cut) {
            cut -= 1;
        }
        let saved_checkpoint = run(
            &f,
            &first_ctx,
            &request,
            Request::Execute {
                operation: operation(
                    &f,
                    &host,
                    &id,
                    "partial-checkpoint",
                    JournalAction::Checkpoint {
                        record_operation_ids: vec![record.request_id.clone()],
                    },
                    vec![fragment(&ranges[0], 0, cut as u64)],
                ),
            },
        )
        .unwrap();
        assert_eq!(
            (
                f.store().read_journal(&host, &id).unwrap().process_order,
                f.store().read_journal(&host, &id).unwrap().process_byte
            ),
            (0, cut as u64)
        );
        f.store()
            .close_journal_generation(&host, &first.id, &first.job)
            .unwrap();
        f.reopen();
        // Only the saved input and SQLite are reopened. There is no latest-source callback.
        let retry_request: ::job::JobRequest =
            serde_json::from_slice(&std::fs::read(saved_path).unwrap()).unwrap();
        assert_eq!(retry_request, request);
        let retry_input = JournalInput::decode(&retry_request.input).unwrap();
        let retry_binding = JobAttemptBinding {
            job_id: retry_request.job_id.clone(),
            attempt_id: "second".into(),
        };
        let mut changed_initial = retry_input.clone();
        changed_initial.initial_state_token.push('x');
        assert!(matches!(
            changed_initial.validate_generation(&first),
            Err(JournalError::Conflict(_))
        ));
        assert!(matches!(
            f.store()
                .prepare_journal_input(&host, &changed_initial, retry_binding.clone()),
            Err(JournalError::Conflict(_))
        ));
        assert!(matches!(
            f.store().prepare_journal_input(
                &host,
                &retry_input,
                JobAttemptBinding {
                    job_id: "different-job-cannot-borrow-initial-proof".into(),
                    attempt_id: "attempt".into()
                }
            ),
            Err(JournalError::Conflict(_))
        ));
        let retry = f
            .store()
            .prepare_journal_input(&host, &retry_input, retry_binding.clone())
            .unwrap();
        assert_ne!(retry.id, first.id);
        assert_eq!(retry.initial_journal, first.initial_journal);
        assert_eq!(retry.initial_open, first.initial_open);
        assert_eq!(retry.previous_fixed_end, first.previous_fixed_end);
        assert_eq!(retry.selected_ranges, first.selected_ranges);
        assert_ne!(
            retry.initial_journal.state_token,
            f.store().read_journal(&host, &id).unwrap().state_token
        );
        retry_input.validate_generation(&retry).unwrap();
        assert_eq!(
            f.store()
                .prepare_journal_input(&host, &retry_input, retry_binding.clone())
                .unwrap(),
            retry
        );
        let retry_ctx =
            JournalJobContext::validated(f.store(), &host, &retry.id, retry_binding).unwrap();
        service::attest_input_delivery(
            f.store(),
            &retry_ctx,
            &retry_request.input_digest().unwrap(),
            &retry_input.selected_ranges,
        )
        .unwrap();
        assert!(matches!(
            run(&f, &first_ctx, &request, Request::ReadState),
            Err(JournalError::Conflict(_))
        ));
        assert!(matches!(
            run(
                &f,
                &first_ctx,
                &request,
                Request::Execute {
                    operation: operation(
                        &f,
                        &host,
                        &id,
                        "late-first-attempt",
                        JournalAction::ConfirmUnchanged,
                        ranges.clone()
                    )
                }
            ),
            Err(JournalError::Conflict(_))
        ));
        for (request_id, saved) in [
            (record.request_id.as_str(), saved_record),
            ("first-position", saved_position),
            ("partial-checkpoint", saved_checkpoint),
        ] {
            let Response::Saved(receipt) = saved else {
                panic!("saved operation receipt expected")
            };
            assert_eq!(
                f.store()
                    .query_journal_operation(&host, &id, request_id)
                    .unwrap(),
                Some(receipt)
            );
        }
        assert!(
            matches!(run(&f, &retry_ctx, &retry_request, Request::ReadOperationCoverage { request_id: record.request_id.clone(), offset: 0, limit: 32 }), Ok(Response::Coverage(page)) if page.items == ranges)
        );
        for (target, expected) in [
            (
                transport::JournalTextTarget::Position {
                    snapshot: transport::JournalSnapshot::Initial,
                },
                first.initial_journal.current_position.clone(),
            ),
            (
                transport::JournalTextTarget::OpenBody {
                    snapshot: transport::JournalSnapshot::Initial,
                },
                first.initial_open.as_ref().unwrap().body.clone(),
            ),
            (
                transport::JournalTextTarget::OpenOngoing {
                    snapshot: transport::JournalSnapshot::Initial,
                },
                first.initial_open.as_ref().unwrap().ongoing.clone(),
            ),
        ] {
            assert!(
                matches!(run(&f, &retry_ctx, &retry_request, Request::ReadText { read: transport::JournalTextRead { target, offset: 0, limit: MAX_JOURNAL_READ_BYTES, content_token: None } }), Ok(Response::Text(page)) if page.text == expected)
            );
        }
        let remainder = vec![
            fragment(&ranges[0], cut as u64, ranges[0].bytes.end),
            ranges[1].clone(),
        ];
        let missing_position = operation(
            &f,
            &host,
            &id,
            "retry-checkpoint-needs-position",
            JournalAction::Checkpoint {
                record_operation_ids: vec![record.request_id.clone()],
            },
            remainder.clone(),
        );
        assert!(matches!(
            run(
                &f,
                &retry_ctx,
                &retry_request,
                Request::Execute {
                    operation: missing_position
                }
            ),
            Err(JournalError::Conflict(_))
        ));
        run(
            &f,
            &retry_ctx,
            &retry_request,
            Request::Execute {
                operation: operation(
                    &f,
                    &host,
                    &id,
                    "retry-position",
                    JournalAction::UpdatePosition {
                        current_position: "Retry confirmed saved activity".into(),
                    },
                    vec![],
                ),
            },
        )
        .unwrap();
        run(
            &f,
            &retry_ctx,
            &retry_request,
            Request::Execute {
                operation: operation(
                    &f,
                    &host,
                    &id,
                    "remainder-checkpoint",
                    JournalAction::Checkpoint {
                        record_operation_ids: vec![record.request_id],
                    },
                    remainder,
                ),
            },
        )
        .unwrap();
        let completion = service::finish(f.store(), &retry_ctx).unwrap();
        assert_eq!((completion.process_order, completion.process_byte), (2, 0));
        assert_eq!(
            f.store()
                .list_journal_entries(&host, &id, None, 32)
                .unwrap()
                .items
                .len(),
            1
        );
    }
}

use super::*;
use crate::SubjectRole;
use feature_storage::{FeatureStorage, ScopedFeatureStorage};
use sha2::{Digest, Sha256};
use std::sync::{Arc, Barrier};
use std::thread;

#[test]
fn explicit_erasure_releases_only_after_all_body_sessions_including_unadmitted_history_are_erased()
{
    let f = Fixture::new();
    let subject = f.host.subject_id.clone();
    let binding = f
        .store()
        .claim_journal_writer(&subject, "runtime", "worker")
        .unwrap();
    let attr = f
        .store()
        .record_session_attribution(
            SubjectSessionAttribution::new(&subject, "runtime", "worker", "saved-before-admission")
                .unwrap(),
        )
        .unwrap();
    let unadmitted = f.store().bind_journal_session(&binding, attr).unwrap();
    assert!(!f.store().release_erased_journal_body(&f.host).unwrap());
    f.store()
        .erase_journal_session(&f.host, "erase-first")
        .unwrap();
    assert!(!f.store().release_erased_journal_body(&f.host).unwrap());
    assert!(f.store().journal_writer_active(&binding).unwrap());
    f.store()
        .erase_journal_session(&unadmitted, "erase-last")
        .unwrap();
    assert!(f.store().release_erased_journal_body(&unadmitted).unwrap());
    assert!(f.store().release_erased_journal_body(&unadmitted).unwrap());
    assert!(!f.store().journal_writer_active(&binding).unwrap());
    assert!(
        f.store()
            .journal_worker_was_released(&subject, "runtime", "worker")
            .unwrap()
    );
    let next = f
        .store()
        .claim_journal_writer(&subject, "runtime", "replacement")
        .unwrap();
    assert!(f.store().release_erased_journal_body(&f.host).unwrap());
    assert!(
        f.store().journal_writer_active(&next).unwrap(),
        "late erase retry cannot revoke another body"
    );
}

#[test]
fn released_binding_preserves_open_and_receipts_but_never_revives_old_session_writes() {
    let mut f = Fixture::new();
    let subject = f.host.subject_id.clone();
    let binding = f
        .store()
        .claim_journal_writer(&subject, "runtime", "worker")
        .unwrap();
    assert!(
        f.store()
            .claim_journal_writer(&subject, "runtime", "other-worker")
            .is_err()
    );
    assert!(f.store().release_journal_writer(&binding).is_err());
    let (generation, context) = f.generation("first-body", f.ranges.clone());
    let create = f.operation(
        "create",
        open_action("A was proposed; user corrected a constraint; B selected and partly executed"),
        f.ranges.clone(),
    );
    let receipt = f.store().execute_journal(&context, create.clone()).unwrap();
    f.execute(
        &context,
        "position",
        JournalAction::UpdatePosition {
            current_position:
                "B: configuration changed; tests not executed; external result Unknown".into(),
        },
        vec![],
    );
    assert!(f.store().release_journal_writer(&binding).is_err());
    f.execute(
        &context,
        "checkpoint",
        JournalAction::Checkpoint {
            record_operation_ids: vec!["create".into()],
        },
        f.ranges.clone(),
    );
    // A saved checkpoint is not proof that the generation has stopped executing.
    assert!(f.store().release_journal_writer(&binding).is_err());
    f.close(&generation);
    f.store().release_journal_writer(&binding).unwrap();
    f.store().release_journal_writer(&binding).unwrap();
    f.reopen();
    assert!(matches!(
        f.store().journal_compaction_snapshot(&f.host),
        Err(JournalError::PermissionDenied(_))
    ));
    let old = f
        .store()
        .query_journal_operation(&f.host, &f.journal_id, "create")
        .unwrap();
    assert_eq!(old, Some(receipt.clone()));
    assert_eq!(
        f.store().execute_journal(&context, create).unwrap(),
        receipt
    );
    let reader = JournalHostContext::validated_reader(f.store(), &subject).unwrap();
    let snapshot = f
        .store()
        .journal_startup_snapshot(&reader, Some("source-session"))
        .unwrap();
    assert_eq!(
        snapshot.journals[0].entries[0].state,
        JournalEntryState::Open
    );
    assert!(snapshot.journals[0].current_position.contains("Unknown"));
    let new = f
        .store()
        .claim_journal_writer(&subject, "runtime", "worker")
        .unwrap();
    assert_ne!(new.binding_id, binding.binding_id);
    assert!(
        f.store()
            .bind_journal_session(&new, f.host.attribution.clone().unwrap())
            .is_err()
    );
    let late = source_range("source-session", 2, 64);
    assert!(matches!(
        f.store().accept_journal_sources(&f.host, "late", &[late]),
        Err(JournalError::PermissionDenied(_))
    ));
    assert!(
        f.store()
            .prepare_journal_generation(
                &f.host,
                &f.journal_id,
                JobAttemptBinding {
                    job_id: "late".into(),
                    attempt_id: "late".into()
                },
                f.ranges.clone()
            )
            .is_err()
    );
    let operation = JournalOperation {
        request_id: "late-position".into(),
        expected_state: snapshot.journals[0].state_token.clone(),
        action: JournalAction::UpdatePosition {
            current_position: "forged".into(),
        },
        covered_ranges: vec![],
    };
    assert!(f.store().execute_journal(&context, operation).is_err());
}

#[test]
fn release_rejects_unbound_legacy_body_sessions_even_without_admitted_sources() {
    let f = Fixture::new();
    let (generation, context) = f.generation("recorded-body", f.ranges.clone());
    f.execute(
        &context,
        "record",
        JournalAction::ConfirmUnchanged,
        f.ranges.clone(),
    );
    save_position(f.store(), &context, "position");
    f.execute(
        &context,
        "checkpoint",
        checkpoint(&["record"]),
        f.ranges.clone(),
    );
    f.close(&generation);
    let binding = f
        .store()
        .claim_journal_writer(&f.host.subject_id, "runtime", "worker")
        .unwrap();
    let legacy = f
        .store()
        .record_session_attribution(
            SubjectSessionAttribution::new(
                &f.host.subject_id,
                "runtime",
                "worker",
                "legacy-not-admitted",
            )
            .unwrap(),
        )
        .unwrap();
    assert!(matches!(
        f.store().release_journal_writer(&binding),
        Err(JournalError::Conflict(_))
    ));
    assert!(f.store().journal_writer_active(&binding).unwrap());
    // The Host must bind and reconcile this original Session, even if its canonical
    // history proves empty and therefore creates no Journal.
    f.store().bind_journal_session(&binding, legacy).unwrap();
    f.store().release_journal_writer(&binding).unwrap();
}

#[test]
fn predecessor_prefers_latest_journal_activity_not_session_attachment_order() {
    let f = Fixture::new();
    let binding = f
        .store()
        .claim_journal_writer(&f.host.subject_id, "runtime", "worker")
        .unwrap();
    let second = session_host(f.store(), &f.host.subject_id, "later-attached-session");
    let second_ranges = vec![source_range("later-attached-session", 0, 8)];
    let second_id = f
        .store()
        .accept_journal_sources(&second, "Earlier activity", &second_ranges)
        .unwrap();
    for (host, journal_id, ranges) in [
        (&f.host, &f.journal_id, &f.ranges),
        (&second, &second_id, &second_ranges),
    ] {
        let job = JobAttemptBinding {
            job_id: journal_id.clone(),
            attempt_id: "recording".into(),
        };
        let generation = f
            .store()
            .prepare_journal_generation(host, journal_id, job.clone(), ranges.clone())
            .unwrap();
        let context = JournalJobContext::validated(f.store(), host, &generation.id, job).unwrap();
        let state = f
            .store()
            .read_journal(host, journal_id)
            .unwrap()
            .state_token;
        f.store()
            .execute_journal(
                &context,
                JournalOperation {
                    request_id: "record".into(),
                    expected_state: state,
                    action: open_action("Unfinished activity"),
                    covered_ranges: ranges.clone(),
                },
            )
            .unwrap();
        let position = save_position(f.store(), &context, "position");
        f.store()
            .checkpoint_journal(
                &context,
                JournalOperation {
                    request_id: "checkpoint".into(),
                    expected_state: position.state_token,
                    action: checkpoint(&["record"]),
                    covered_ranges: ranges.clone(),
                },
            )
            .unwrap();
        f.store()
            .close_journal_generation(host, &generation.id, &generation.job)
            .unwrap();
    }
    // Stored timestamps model a later update to the earlier-attached Session without
    // using elapsed wall time as a synchronization or ordering assertion.
    f.store()
        .database
        .try_transaction::<_, JournalError>(|tx| {
            tx.execute(
                "UPDATE journals SET updated_at='2001-01-01T00:00:00Z' WHERE id=?1",
                [&f.journal_id],
            )?;
            tx.execute(
                "UPDATE journals SET updated_at='2000-01-01T00:00:00Z' WHERE id=?1",
                [&second_id],
            )?;
            Ok(())
        })
        .unwrap();
    let _empty = session_host(
        f.store(),
        &f.host.subject_id,
        "latest-attachment-no-journal",
    );
    f.store().release_journal_writer(&binding).unwrap();
    let replacement = f
        .store()
        .claim_journal_writer(&f.host.subject_id, "runtime", "replacement")
        .unwrap();
    assert_eq!(
        f.store()
            .journal_predecessor_session(&replacement)
            .unwrap()
            .as_deref(),
        Some("source-session")
    );
}

#[test]
fn concurrent_body_claims_from_independent_connections_bind_only_one_worker() {
    let f = Fixture::new();
    let subject = f
        .store()
        .create_subject(SubjectRole::new("claim-race").unwrap())
        .unwrap();
    let manager = FeatureStorage::new(f.temp.path().join("storage"));
    let workspace = manager.workspace("journal-tests").unwrap();
    let registration = SubjektivStore::register(&workspace).unwrap();
    let second = SubjektivStore::open(&workspace, &registration).unwrap();
    let barrier = Arc::new(Barrier::new(3));
    let spawn = |store: SubjektivStore, worker: &'static str| {
        let barrier = barrier.clone();
        let subject = subject.id.clone();
        thread::spawn(move || {
            barrier.wait();
            store.claim_journal_writer(&subject, "runtime", worker)
        })
    };
    let first = spawn(f.store().clone(), "first");
    let second = spawn(second, "second");
    barrier.wait();
    let outcomes = [first.join().unwrap(), second.join().unwrap()];
    assert_eq!(outcomes.iter().filter(|r| r.is_ok()).count(), 1);
    let winner = outcomes.into_iter().find_map(Result::ok).unwrap();
    assert!(f.store().journal_writer_active(&winner).unwrap());
    let loser = if winner.worker_id == "first" {
        "second"
    } else {
        "first"
    };
    assert!(
        f.store()
            .claim_journal_writer(&subject.id, "runtime", loser)
            .is_err()
    );
    drop(workspace);
    manager.shutdown().unwrap();
}

#[test]
fn release_and_late_admission_are_serialized_by_the_persisted_binding_check() {
    let f = Fixture::new();
    let binding = f
        .store()
        .claim_journal_writer(&f.host.subject_id, "runtime", "worker")
        .unwrap();
    let (generation, context) = f.generation("release-race", f.ranges.clone());
    f.execute(
        &context,
        "record",
        open_action("unfinished"),
        f.ranges.clone(),
    );
    f.execute(
        &context,
        "position",
        JournalAction::UpdatePosition {
            current_position: "remaining work".into(),
        },
        vec![],
    );
    f.execute(
        &context,
        "checkpoint",
        JournalAction::Checkpoint {
            record_operation_ids: vec!["record".into()],
        },
        f.ranges.clone(),
    );
    f.close(&generation);
    let manager = FeatureStorage::new(f.temp.path().join("storage"));
    let workspace = manager.workspace("journal-tests").unwrap();
    let registration = SubjektivStore::register(&workspace).unwrap();
    let second = SubjektivStore::open(&workspace, &registration).unwrap();
    let barrier = Arc::new(Barrier::new(3));
    let b = barrier.clone();
    let store = f.store().clone();
    let released = binding.clone();
    let release = thread::spawn(move || {
        b.wait();
        store.release_journal_writer(&released)
    });
    let b = barrier.clone();
    let host = f.host.clone();
    let late = source_range("source-session", 2, 16);
    let admission = thread::spawn(move || {
        b.wait();
        second.accept_journal_sources(&host, "late", &[late])
    });
    barrier.wait();
    let release = release.join().unwrap();
    let admission = admission.join().unwrap();
    assert_ne!(
        release.is_ok(),
        admission.is_ok(),
        "exactly one ordered effect may commit"
    );
    assert_eq!(
        f.store().journal_writer_active(&binding).unwrap(),
        admission.is_ok()
    );
    assert_eq!(
        f.store()
            .read_journal_source(&f.host, &f.journal_id, 2)
            .is_ok(),
        admission.is_ok()
    );
    drop(workspace);
    manager.shutdown().unwrap();
}

#[test]
fn compaction_snapshot_prioritizes_current_session_without_waiting_or_mutating_recording() {
    let f = Fixture::new();
    let (generation, context) = f.generation("in-flight", f.ranges.clone());
    f.execute(
        &context,
        "open",
        open_action(&"日本語\\\"\\n".repeat(6000)),
        f.ranges.clone(),
    );
    f.execute(
        &context,
        "position",
        JournalAction::UpdatePosition {
            current_position: "partial activity; checkpoint still behind".into(),
        },
        vec![],
    );
    let mut latest = String::new();
    for n in 0..5 {
        let session = format!("supplement-{n}");
        let host = session_host(f.store(), &f.host.subject_id, &session);
        f.store()
            .accept_journal_sources(&host, "recent", &[source_range(&session, 0, 10)])
            .unwrap();
        latest = session;
    }
    let reader = JournalHostContext::validated_reader(f.store(), &f.host.subject_id).unwrap();
    let startup = f
        .store()
        .journal_startup_snapshot(&reader, Some(&latest))
        .unwrap();
    assert_eq!(startup.journals[0].source_session_id, latest);
    let before = f.state();
    let snapshot = f.store().journal_compaction_snapshot(&f.host).unwrap();
    assert_eq!(snapshot.journals[0].source_session_id, "source-session");
    assert_eq!(snapshot.journals[0].process_order, before.0.process_order);
    assert_eq!(snapshot.journals[0].process_byte, before.0.process_byte);
    assert_eq!(
        snapshot.journals[0].entries[0].state,
        JournalEntryState::Open
    );
    assert!(snapshot.journals[0].entries[0].omitted);
    assert!(snapshot.omitted);
    assert_eq!(snapshot.journals.len(), 3);
    assert!(serde_json::to_vec(&snapshot).unwrap().len() <= startup::STARTUP_MAX_BYTES);
    assert_eq!(
        f.store().journal_compaction_snapshot(&f.host).unwrap(),
        snapshot
    );
    assert_eq!(f.state(), before);
    assert_eq!(
        f.store()
            .active_journal_generation(&f.host, &f.journal_id)
            .unwrap(),
        Some(generation)
    );
    assert!(matches!(
        f.store().journal_compaction_snapshot(&reader),
        Err(JournalError::PermissionDenied(_))
    ));
    f.execute(
        &context,
        "new-position",
        JournalAction::UpdatePosition {
            current_position: "background recording progressed after snapshot".into(),
        },
        vec![],
    );
    let refreshed = f.store().journal_compaction_snapshot(&f.host).unwrap();
    assert_ne!(
        refreshed.journals[0].state_token,
        snapshot.journals[0].state_token
    );
    assert_ne!(
        refreshed.journals[0].current_position,
        snapshot.journals[0].current_position
    );
    let empty_host = session_host(f.store(), &f.host.subject_id, "not-yet-admitted");
    assert!(
        !f.store()
            .journal_compaction_snapshot(&empty_host)
            .unwrap()
            .journals
            .is_empty()
    );
    let other = f
        .store()
        .create_subject_with_behavior(SubjectRole::new("other").unwrap(), "unchanged".into())
        .unwrap();
    let other_host = session_host(f.store(), &other.id, "other-session");
    let empty = f.store().journal_compaction_snapshot(&other_host).unwrap();
    assert!(empty.journals.is_empty());
    assert!(!empty.omitted);
}

#[test]
fn startup_snapshot_distinguishes_none_is_subject_local_and_bounds_utf8_at_storage() {
    let f = Fixture::new();
    let (_, context) = f.generation("snapshot", f.ranges.clone());
    f.execute(
        &context,
        "large-open",
        open_action(&"日本語\\\"\\n".repeat(6000)),
        f.ranges.clone(),
    );
    let reader = JournalHostContext::validated_reader(f.store(), &f.host.subject_id).unwrap();
    let snapshot = f
        .store()
        .journal_startup_snapshot(&reader, Some("source-session"))
        .unwrap();
    let encoded = serde_json::to_vec(&snapshot).unwrap();
    assert!(encoded.len() <= startup::STARTUP_MAX_BYTES);
    assert!(snapshot.journals[0].entries[0].omitted);
    assert!(snapshot.journals[0].entries[0].body.len() <= 2048);
    assert_eq!(snapshot.journals[0].source_session_id, "source-session");
    let other = f
        .store()
        .create_subject_with_behavior(
            SubjectRole::new("other").unwrap(),
            "behavior unchanged".into(),
        )
        .unwrap();
    let other_reader = JournalHostContext::validated_reader(f.store(), &other.id).unwrap();
    assert!(matches!(
        f.store()
            .journal_startup_snapshot(&other_reader, Some("source-session")),
        Err(JournalError::PermissionDenied(_))
    ));
    let none = f
        .store()
        .journal_startup_snapshot(&other_reader, None)
        .unwrap();
    assert!(none.journals.is_empty());
    assert!(!none.omitted);
    assert_eq!(
        f.store().subject(&other.id).unwrap().unwrap().behavior_md,
        "behavior unchanged"
    );
}

#[test]
fn stable_legacy_run_references_are_accepted_without_relaxing_character_or_size_limits() {
    let f = Fixture::new();
    let mut range = source_range("source-session", 2, 64);
    range.source.pointer.entry_ref =
        format!("El-run-{}-{}-123456789", Uuid::now_v7(), Uuid::now_v7());
    assert!(range.source.pointer.entry_ref.len() > 65);
    f.store()
        .accept_journal_sources(&f.host, "Journal", &[range.clone()])
        .unwrap();
    assert_eq!(
        f.store()
            .read_journal_source(&f.host, &f.journal_id, 2)
            .unwrap()
            .source,
        range.source
    );
    let mut next = source_range("source-session", 3, 64);
    for reference in [
        format!("E{}", "a".repeat(129)),
        "Einvalid_ref".into(),
        "E".into(),
    ] {
        next.source.pointer.entry_ref = reference;
        assert!(matches!(
            f.store()
                .accept_journal_sources(&f.host, "Journal", &[next.clone()]),
            Err(JournalError::InvalidInput(_))
        ));
        assert!(matches!(
            f.store().read_journal_source(&f.host, &f.journal_id, 3),
            Err(JournalError::NotFound(_))
        ));
    }
    next.source.pointer.entry_ref = format!("E{}", "a".repeat(128));
    f.store()
        .accept_journal_sources(&f.host, "Journal", &[next])
        .unwrap();
}

#[test]
fn descriptor_only_job_cannot_record_unread_bytes_and_delivery_survives_reopen() {
    let mut f = Fixture::new();
    let text = "公開履歴から実際に供給された全文";
    let (host, id, ranges) = recording_sources(&f, &[text]);
    let binding = JobAttemptBinding {
        job_id: "descriptor-only".into(),
        attempt_id: "attempt".into(),
    };
    let generation = f
        .store()
        .prepare_journal_generation(&host, &id, binding.clone(), ranges.clone())
        .unwrap();
    let context =
        JournalJobContext::validated(f.store(), &host, &generation.id, binding.clone()).unwrap();
    let operation = JournalOperation {
        request_id: "save-only-after-reading".into(),
        expected_state: f.store().read_journal(&host, &id).unwrap().state_token,
        action: open_action("ongoing"),
        covered_ranges: ranges.clone(),
    };
    let execute = || transport::JournalJobRequest::Execute {
        operation: operation.clone(),
    };
    assert!(matches!(
        service::execute(f.store(), &context, execute(), |_| unreachable!()),
        Err(JournalError::Conflict(_))
    ));
    let first = service::execute(
        f.store(),
        &context,
        transport::JournalJobRequest::ReadText {
            read: transport::JournalTextRead {
                target: transport::JournalTextTarget::Source {
                    pointer: ranges[0].source.pointer.clone(),
                },
                offset: 0,
                limit: 6,
                content_token: None,
            },
        },
        |_| Ok(text.into()),
    )
    .unwrap();
    let transport::JournalJobResponse::Text(first) = first else {
        panic!("source page required")
    };
    assert_eq!(first.text, "公開");
    assert!(matches!(
        service::execute(f.store(), &context, execute(), |_| unreachable!()),
        Err(JournalError::Conflict(_))
    ));
    f.reopen();
    let context = JournalJobContext::validated(f.store(), &host, &generation.id, binding).unwrap();
    let remainder = transport::JournalJobRequest::ReadText {
        read: transport::JournalTextRead {
            target: transport::JournalTextTarget::Source {
                pointer: ranges[0].source.pointer.clone(),
            },
            offset: first.next_byte.unwrap(),
            limit: MAX_JOURNAL_READ_BYTES,
            content_token: Some(first.content_token),
        },
    };
    // A failed source fetch must not turn allowed descriptors into delivered input.
    assert!(
        service::execute(f.store(), &context, remainder.clone(), |_| Err(
            JournalError::NotFound("original source missing".into())
        ))
        .is_err()
    );
    assert!(service::execute(f.store(), &context, execute(), |_| unreachable!()).is_err());
    service::execute(f.store(), &context, remainder, |_| Ok(text.into())).unwrap();
    let response = service::execute(f.store(), &context, execute(), |_| unreachable!()).unwrap();
    assert!(matches!(response, transport::JournalJobResponse::Saved(_)));
    assert_eq!(
        f.store()
            .list_journal_entries(&host, &id, None, 32)
            .unwrap()
            .items
            .len(),
        1
    );
    // Original bytes are not duplicated into the durable delivery ledger.
    f.store()
        .database
        .try_with_connection(|tx| {
            let delivered: String = tx.query_row(
                "SELECT ranges_json FROM journal_generation_deliveries WHERE generation_id=?1",
                [&generation.id],
                |r| r.get(0),
            )?;
            assert!(!delivered.contains(text));
            assert_eq!(
                serde_json::from_str::<Vec<JournalByteRange>>(&delivered).unwrap(),
                vec![ranges[0].bytes]
            );
            Ok::<_, JournalError>(())
        })
        .unwrap();
}

#[test]
fn oversized_source_response_does_not_attest_undelivered_bytes() {
    let f = Fixture::new();
    let text = "\u{1}".repeat(MAX_JOURNAL_READ_BYTES);
    let (host, id, ranges) = recording_sources(&f, &[&text]);
    let binding = JobAttemptBinding {
        job_id: "escaped-source".into(),
        attempt_id: "attempt".into(),
    };
    let generation = f
        .store()
        .prepare_journal_generation(&host, &id, binding.clone(), ranges.clone())
        .unwrap();
    let context = JournalJobContext::validated(f.store(), &host, &generation.id, binding).unwrap();
    assert!(matches!(
        service::execute(
            f.store(),
            &context,
            transport::JournalJobRequest::ReadText {
                read: transport::JournalTextRead {
                    target: transport::JournalTextTarget::Source {
                        pointer: ranges[0].source.pointer.clone()
                    },
                    offset: 0,
                    limit: MAX_JOURNAL_READ_BYTES,
                    content_token: None,
                },
            },
            |_| Ok(text.clone())
        ),
        Err(JournalError::InvalidInput(_))
    ));
    let operation = JournalOperation {
        request_id: "unread-create".into(),
        expected_state: f.store().read_journal(&host, &id).unwrap().state_token,
        action: open_action("must not save"),
        covered_ranges: ranges,
    };
    assert!(matches!(
        service::execute(
            f.store(),
            &context,
            transport::JournalJobRequest::Execute { operation },
            |_| unreachable!()
        ),
        Err(JournalError::Conflict(_))
    ));
    f.store()
        .database
        .try_with_connection(|tx| {
            let count: u64 = tx.query_row(
                "SELECT COUNT(*) FROM journal_generation_deliveries WHERE generation_id=?1",
                [&generation.id],
                |r| r.get(0),
            )?;
            assert_eq!(count, 0);
            Ok::<_, JournalError>(())
        })
        .unwrap();
}

#[test]
fn job_state_keeps_initial_position_and_bounds_current_text_with_stale_continuation_rejection() {
    let f = Fixture::new();
    let (_, context) = f.generation("state-reading-job", f.ranges.clone());
    let current = "現在地".repeat(8192);
    f.store()
        .execute_journal(
            &context,
            f.operation(
                "position-one",
                JournalAction::UpdatePosition {
                    current_position: current.clone(),
                },
                vec![],
            ),
        )
        .unwrap();
    let state = service::execute(
        f.store(),
        &context,
        transport::JournalJobRequest::ReadState,
        |_| unreachable!(),
    )
    .unwrap();
    let transport::JournalJobResponse::State(state) = state else {
        panic!("missing state")
    };
    assert_eq!(state.initial_journal.current_position_bytes, 0);
    assert_eq!(state.current_journal.current_position_bytes, current.len());
    assert!(serde_json::to_vec(&state).unwrap().len() < 4096);
    let read = transport::JournalTextRead {
        target: transport::JournalTextTarget::Position {
            snapshot: transport::JournalSnapshot::Current,
        },
        offset: 0,
        limit: 100,
        content_token: None,
    };
    let page = service::execute(
        f.store(),
        &context,
        transport::JournalJobRequest::ReadText { read: read.clone() },
        |_| unreachable!(),
    )
    .unwrap();
    let transport::JournalJobResponse::Text(page) = page else {
        panic!("missing text")
    };
    assert!(page.text.len() <= 100);
    assert_eq!(page.text, current[..page.text.len()]);
    let next = page.next_byte.unwrap();
    f.store()
        .execute_journal(
            &context,
            f.operation(
                "position-two",
                JournalAction::UpdatePosition {
                    current_position: "変更".repeat(8192),
                },
                vec![],
            ),
        )
        .unwrap();
    let continued = transport::JournalTextRead {
        offset: next,
        content_token: Some(page.content_token),
        ..read
    };
    assert!(matches!(
        service::execute(
            f.store(),
            &context,
            transport::JournalJobRequest::ReadText { read: continued },
            |_| unreachable!()
        ),
        Err(JournalError::Conflict(_))
    ));
    let initial = transport::JournalTextRead {
        target: transport::JournalTextTarget::Position {
            snapshot: transport::JournalSnapshot::Initial,
        },
        offset: 0,
        limit: 100,
        content_token: None,
    };
    let initial = service::execute(
        f.store(),
        &context,
        transport::JournalJobRequest::ReadText { read: initial },
        |_| unreachable!(),
    )
    .unwrap();
    let transport::JournalJobResponse::Text(initial) = initial else {
        panic!("missing initial text")
    };
    assert!(initial.text.is_empty());
    assert_eq!(initial.next_byte, None);
}

#[test]
fn bounded_material_selection_fetches_only_a_budgeted_fragment_of_a_large_source() {
    let f = Fixture::with_sizes(&[16 * 1024 * 1024]);
    let materials = recording::select_materials_bounded(
        f.store(),
        &f.host,
        &f.journal_id,
        1024,
        |source, start, limit| {
            assert_eq!(start, 0);
            assert!(limit < 1024);
            Ok(recording::JournalMaterial {
                range: JournalSourceRange {
                    source: source.clone(),
                    bytes: JournalByteRange {
                        start,
                        end: start + limit as u64,
                    },
                },
                text: "x".repeat(limit),
            })
        },
    )
    .unwrap();
    assert_eq!(materials.len(), 1);
    assert!(serde_json::to_vec(&materials).unwrap().len() <= 1024);
    assert!(materials[0].range.bytes.end < f.ranges[0].source.total_bytes);
    assert_eq!(f.state().0.process_order, 0);
}

#[test]
fn bounded_material_selection_rejects_wrong_identity_range_and_unusable_input_budget() {
    let f = Fixture::new();
    for defect in 0..3 {
        let result = recording::select_materials_bounded(
            f.store(),
            &f.host,
            &f.journal_id,
            4096,
            |source, start, _| {
                let mut material = recording::JournalMaterial {
                    range: JournalSourceRange {
                        source: source.clone(),
                        bytes: JournalByteRange {
                            start,
                            end: start + 4,
                        },
                    },
                    text: "text".into(),
                };
                match defect {
                    0 => material.range.source.content_sha256 = "0".repeat(64),
                    1 => material.range.bytes.start += 1,
                    _ => material.range.bytes.end += 1,
                }
                Ok(material)
            },
        );
        assert!(matches!(result, Err(JournalError::Conflict(_))));
    }
    assert!(matches!(
        recording::select_materials_bounded(
            f.store(),
            &f.host,
            &f.journal_id,
            16,
            |_, _, _| panic!("unusable budget must not fetch")
        ),
        Err(JournalError::InvalidInput(_))
    ));
}

#[test]
fn erasure_failure_rolls_back_tombstone_and_content_then_exact_retry_succeeds() {
    let f = Fixture::new();
    let (_, job) = f.generation("erasure-fault", f.ranges.clone());
    let operation = f.operation(
        "record-before-fault",
        open_action("preserved until erase commits"),
        f.ranges.clone(),
    );
    f.store().execute_journal(&job, operation).unwrap();
    f.store().database.try_with_connection::<_,JournalError>(|tx| {
        tx.execute_batch("CREATE TRIGGER erase_fault BEFORE DELETE ON journal_entries BEGIN SELECT RAISE(ABORT,'erase fault'); END;")?;
        Ok(())
    }).unwrap();
    assert!(
        f.store()
            .erase_journal_session(&f.host, "erasure-fault-request")
            .is_err()
    );
    assert!(f.store().journal_for_session(&f.host).unwrap().is_some());
    assert!(f.state().1.is_some());
    assert!(
        f.store()
            .query_journal_operation(&f.host, &f.journal_id, "record-before-fault")
            .unwrap()
            .is_some()
    );
    f.store()
        .database
        .try_with_connection::<_, JournalError>(|tx| {
            tx.execute_batch("DROP TRIGGER erase_fault;")?;
            Ok(())
        })
        .unwrap();
    f.store()
        .erase_journal_session(&f.host, "erasure-fault-request")
        .unwrap();
    assert!(
        f.store()
            .list_journals(&f.host, None, 32)
            .unwrap()
            .items
            .is_empty()
    );
}

#[test]
fn checkpointed_generation_rejects_late_activity_but_retains_saved_receipts() {
    let f = Fixture::new();
    let (generation, context) = f.generation("checkpointed-job", f.ranges.clone());
    let operation = f.operation("record", open_action("B partly executed"), f.ranges.clone());
    let receipt = f
        .store()
        .execute_journal(&context, operation.clone())
        .unwrap();
    let entry_id = receipt.entry_id.clone().unwrap();
    save_position(f.store(), &context, "position");
    f.execute(
        &context,
        "checkpoint",
        checkpoint(&["record"]),
        f.ranges.clone(),
    );
    let before = f.state();
    for (index, action) in [
        open_action("late create"),
        JournalAction::Rewrite {
            entry_id: entry_id.clone(),
            title: "late rewrite".into(),
            body: "late".into(),
            ongoing: "late".into(),
        },
        JournalAction::Fix {
            entry_id,
            source_end: f.ranges.last().unwrap().source.pointer.clone(),
        },
        JournalAction::ConfirmUnchanged,
    ]
    .into_iter()
    .enumerate()
    {
        let late = f.operation(&format!("late-{index}"), action, f.ranges.clone());
        assert!(matches!(
            f.store().execute_journal(&context, late),
            Err(JournalError::Conflict(_))
        ));
        assert_eq!(f.state(), before);
    }
    assert_eq!(
        f.store().execute_journal(&context, operation).unwrap(),
        receipt
    );
    f.close(&generation);
    let binding = f
        .store()
        .claim_journal_writer(&f.host.subject_id, "runtime", "worker")
        .unwrap();
    f.store().release_journal_writer(&binding).unwrap();
}

#[test]
fn checkpoint_without_current_position_leaves_failed_job_input_recoverable_after_reopen() {
    let mut f = Fixture::new();
    let (failed, context) = f.generation("incomplete-position", f.ranges.clone());
    save_position(f.store(), &context, "outdated-position");
    f.execute(
        &context,
        "record",
        open_action("B selected; tests not run"),
        f.ranges.clone(),
    );
    let checkpoint_operation = f.operation("checkpoint", checkpoint(&["record"]), f.ranges.clone());
    let before = f.state();
    assert!(matches!(
        f.store().execute_journal(&context, checkpoint_operation),
        Err(JournalError::Conflict(_))
    ));
    assert_eq!(f.state(), before);
    assert!(f.sources().iter().all(|source| source.reflected_bytes == 0));
    assert!(f.receipt("checkpoint").is_none());
    f.close(&failed);
    f.reopen();
    let (replacement, context) = f.generation("recover-position", f.ranges.clone());
    f.execute(
        &context,
        "recovered-position",
        JournalAction::UpdatePosition {
            current_position: "B partly performed; tests unperformed; external result Unknown"
                .into(),
        },
        vec![],
    );
    // Use the saved record receipt rather than duplicating the old activity entry.
    f.execute(
        &context,
        "checkpoint",
        checkpoint(&["record"]),
        f.ranges.clone(),
    );
    f.close(&replacement);
    let binding = f
        .store()
        .claim_journal_writer(&f.host.subject_id, "runtime", "worker")
        .unwrap();
    assert_eq!(f.entries().len(), 1);
    assert_eq!(f.entries()[0].state, JournalEntryState::Open);
    assert!(f.state().0.current_position.contains("Unknown"));
    f.store().release_journal_writer(&binding).unwrap();
    assert!(!f.store().journal_writer_active(&binding).unwrap());
}

#[test]
fn job_service_completion_requires_checkpoint_and_replays_saved_completion_after_new_generation() {
    let mut f = Fixture::new();
    let (generation, context) = f.generation("service-job", f.ranges.clone());
    for range in &f.ranges {
        service::execute_bounded(
            f.store(),
            &context,
            transport::JournalJobRequest::ReadText {
                read: transport::JournalTextRead {
                    target: transport::JournalTextTarget::Source {
                        pointer: range.source.pointer.clone(),
                    },
                    offset: 0,
                    limit: MAX_JOURNAL_READ_BYTES,
                    content_token: None,
                },
            },
            |source, start, limit| {
                let end = source.total_bytes.min(start + limit as u64);
                Ok(recording::JournalMaterial {
                    range: JournalSourceRange {
                        source: source.clone(),
                        bytes: JournalByteRange { start, end },
                    },
                    text: "x".repeat((end - start) as usize),
                })
            },
        )
        .unwrap();
    }
    let create = f.operation("service-create", open_action("ongoing"), f.ranges.clone());
    let response = service::execute(
        f.store(),
        &context,
        transport::JournalJobRequest::Execute { operation: create },
        |_| panic!("record operation has no source fetch"),
    )
    .unwrap();
    let transport::JournalJobResponse::Saved(receipt) = response else {
        panic!("missing receipt")
    };
    assert!(matches!(
        service::finish(f.store(), &context),
        Err(JournalError::Conflict(_))
    ));
    let checkpoint_operation = JournalOperation {
        request_id: "service-checkpoint".into(),
        expected_state: receipt.state_token,
        action: JournalAction::Checkpoint {
            record_operation_ids: vec!["service-create".into()],
        },
        covered_ranges: f.ranges.clone(),
    };
    // Reject the checkpoint atomically: failed/terminal Jobs must leave pending input
    // recoverable rather than stranding a fully reflected prefix without a position memo.
    assert!(matches!(
        service::execute(
            f.store(),
            &context,
            transport::JournalJobRequest::Execute {
                operation: checkpoint_operation
            },
            |_| unreachable!(),
        ),
        Err(JournalError::Conflict(_))
    ));
    assert!(f.sources().iter().all(|source| source.reflected_bytes == 0));
    assert!(f.receipt("service-checkpoint").is_none());
    f.execute(
        &context,
        "service-position",
        JournalAction::UpdatePosition {
            current_position: "ongoing; work remains".into(),
        },
        vec![],
    );
    f.execute(
        &context,
        "service-checkpoint",
        checkpoint(&["service-create"]),
        f.ranges.clone(),
    );
    let completion = service::finish(f.store(), &context).unwrap();
    assert_eq!(completion.generation_id, generation.id);
    assert_eq!((completion.process_order, completion.process_byte), (2, 0));
    assert!(
        f.store()
            .active_journal_generation(&f.host, &f.journal_id)
            .unwrap()
            .is_none()
    );
    f.reopen();
    assert!(
        JournalJobContext::validated(f.store(), &f.host, &generation.id, generation.job.clone())
            .is_err()
    );
    let context = JournalJobContext::validated_recovery(
        f.store(),
        &f.host,
        &generation.id,
        generation.job.clone(),
    )
    .unwrap();
    assert_eq!(service::finish(f.store(), &context).unwrap(), completion);
    let next_range = source_range("source-session", 2, 256);
    f.store()
        .accept_journal_sources(&f.host, "Journal", &[next_range.clone()])
        .unwrap();
    let (_, next) = f.generation("next-service-job", vec![next_range]);
    let update = f.operation(
        "later-position",
        JournalAction::UpdatePosition {
            current_position: "new generation".into(),
        },
        vec![],
    );
    f.store().execute_journal(&next, update.clone()).unwrap();
    assert_eq!(service::finish(f.store(), &context).unwrap(), completion);
    assert!(
        service::execute(
            f.store(),
            &context,
            transport::JournalJobRequest::Execute { operation: update },
            |_| unreachable!()
        )
        .is_err()
    );
}

#[test]
fn job_service_source_reads_are_generation_bound_and_stop_at_supplied_byte_end() {
    let f = Fixture::new();
    let text = "public source with tail";
    let (host, id, ranges) = recording_sources(&f, &[text]);
    let mut selected = ranges[0].clone();
    selected.bytes.end = 6;
    let binding = JobAttemptBinding {
        job_id: "bounded-service".into(),
        attempt_id: "attempt".into(),
    };
    let generation = f
        .store()
        .prepare_journal_generation(&host, &id, binding.clone(), vec![selected.clone()])
        .unwrap();
    let context = JournalJobContext::validated(f.store(), &host, &generation.id, binding).unwrap();
    let read = transport::JournalTextRead {
        target: transport::JournalTextTarget::Source {
            pointer: selected.source.pointer.clone(),
        },
        offset: 0,
        limit: 100,
        content_token: None,
    };
    let response = service::execute(
        f.store(),
        &context,
        transport::JournalJobRequest::ReadText { read },
        |_| Ok(text.into()),
    )
    .unwrap();
    let transport::JournalJobResponse::Text(page) = response else {
        panic!("missing text page")
    };
    assert_eq!(page.text, "public");
    assert_eq!(page.next_byte, None);
    let read = transport::JournalTextRead {
        target: transport::JournalTextTarget::Source {
            pointer: f.ranges[0].source.pointer.clone(),
        },
        offset: 0,
        limit: 100,
        content_token: None,
    };
    assert!(matches!(
        service::execute(
            f.store(),
            &context,
            transport::JournalJobRequest::ReadText { read },
            |_| panic!("foreign source must not be fetched")
        ),
        Err(JournalError::PermissionDenied(_))
    ));
}

#[test]
fn explicit_erasure_remains_available_for_retired_subject_without_reopening_recording() {
    let f = Fixture::new();
    f.store().retire_subject(&f.host.subject_id).unwrap();
    assert!(f.store().journal_retention_required(&f.host).unwrap());
    f.store()
        .erase_journal_session(&f.host, "erase-retired")
        .unwrap();
    assert!(!f.store().journal_retention_required(&f.host).unwrap());
    assert!(
        f.store()
            .list_journals(&f.host, None, 32)
            .unwrap()
            .items
            .is_empty()
    );
    assert!(
        f.store()
            .accept_journal_sources(&f.host, "late", &f.ranges)
            .is_err()
    );
}

#[test]
fn explicit_erasure_removes_saved_content_and_fences_late_admission_after_reopen() {
    let mut f = Fixture::new();
    let (generation, job) = f.generation("erase-job", f.ranges.clone());
    let operation = f.operation(
        "created-before-erasure",
        open_action("private content"),
        f.ranges.clone(),
    );
    f.store().execute_journal(&job, operation.clone()).unwrap();
    assert!(f.store().journal_retention_required(&f.host).unwrap());
    assert!(!f.store().journal_session_erased(&f.host).unwrap());
    f.store()
        .erase_journal_session(&f.host, "erase-request")
        .unwrap();
    f.reopen();
    f.store()
        .erase_journal_session(&f.host, "erase-request")
        .unwrap();
    assert!(!f.store().journal_retention_required(&f.host).unwrap());
    assert!(f.store().journal_session_erased(&f.host).unwrap());
    let reader = JournalHostContext::validated_reader(f.store(), &f.host.subject_id).unwrap();
    assert!(matches!(
        f.store().journal_session_erased(&reader),
        Err(JournalError::PermissionDenied(_))
    ));
    assert!(
        f.store()
            .list_journals(&f.host, None, 32)
            .unwrap()
            .items
            .is_empty()
    );
    assert!(matches!(
        f.store()
            .accept_journal_sources(&f.host, "late retry", &f.ranges),
        Err(JournalError::PermissionDenied(_))
    ));
    assert!(f.store().execute_journal(&job, operation).is_err());
    assert!(
        JournalJobContext::validated(f.store(), &f.host, &generation.id, generation.job).is_err()
    );
}

#[test]
fn explicit_erasure_before_admission_prevents_later_creation_and_cross_session_key_reuse() {
    let f = Fixture::new();
    let other = session_host(f.store(), &f.host.subject_id, "not-yet-admitted");
    f.store()
        .erase_journal_session(&other, "erase-unadmitted")
        .unwrap();
    assert!(matches!(
        f.store()
            .accept_journal_sources(&other, "late", &[source_range("not-yet-admitted", 0, 8)]),
        Err(JournalError::PermissionDenied(_))
    ));
    assert!(matches!(
        f.store().erase_journal_session(&f.host, "erase-unadmitted"),
        Err(JournalError::Conflict(_))
    ));
    assert!(f.store().read_journal(&f.host, &f.journal_id).is_ok());
}

#[test]
fn retention_release_requires_checkpoint_not_saved_entry_or_unavailability() {
    let f = Fixture::new();
    let (_, job) = f.generation("retained-job", f.ranges.clone());
    let operation = f.operation("record", open_action("ongoing activity"), f.ranges.clone());
    f.store().execute_journal(&job, operation).unwrap();
    assert!(f.store().journal_retention_required(&f.host).unwrap());
    f.store()
        .mark_journal_sources_unavailable(&f.host, &f.journal_id, &f.ranges)
        .unwrap();
    assert!(f.store().journal_retention_required(&f.host).unwrap());
    f.store()
        .mark_journal_sources_available(&f.host, &f.journal_id, &f.ranges)
        .unwrap();
    let receipt = save_position(f.store(), &job, "retained-position");
    f.store()
        .checkpoint_journal(
            &job,
            JournalOperation {
                request_id: "retained-checkpoint".into(),
                expected_state: receipt.state_token,
                action: JournalAction::Checkpoint {
                    record_operation_ids: vec!["record".into()],
                },
                covered_ranges: f.ranges.clone(),
            },
        )
        .unwrap();
    assert!(!f.store().journal_retention_required(&f.host).unwrap());
    assert!(f.state().1.is_some());
}

fn recording_sources(
    f: &Fixture,
    texts: &[&str],
) -> (JournalHostContext, String, Vec<JournalSourceRange>) {
    let host = session_host(f.store(), &f.host.subject_id, "recording-session");
    let ranges = texts
        .iter()
        .enumerate()
        .map(|(order, text)| {
            let mut range = source_range("recording-session", order as u64, text.len() as u64);
            range.source.content_sha256 = Sha256::digest(text.as_bytes())
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            range
        })
        .collect::<Vec<_>>();
    let id = f
        .store()
        .accept_journal_sources(&host, "Recording", &ranges)
        .unwrap();
    (host, id, ranges)
}

#[test]
fn recording_materials_bound_escaped_utf8_without_advancing_reflection() {
    let f = Fixture::new();
    let text = "日本語\n\"\\".repeat(4000);
    let (host, id, ranges) = recording_sources(&f, &[&text]);
    let materials =
        recording::select_materials(f.store(), &host, &id, 1024, |_| Ok(text.clone())).unwrap();
    assert_eq!(materials.len(), 1);
    assert!(serde_json::to_vec(&materials).unwrap().len() <= 1024);
    let range = &materials[0].range;
    assert_eq!(range.bytes.start, 0);
    assert!(range.bytes.end < ranges[0].source.total_bytes);
    assert_eq!(materials[0].text, text[..range.bytes.end as usize]);
    let status = f.store().read_journal_source(&host, &id, 0).unwrap();
    assert_eq!(status.reflected_bytes, 0);
    assert_eq!(status.accepted, vec![ranges[0].bytes]);
}

#[test]
fn recording_materials_reject_changed_bytes_instead_of_using_preview_identity() {
    let f = Fixture::new();
    let (host, id, _) = recording_sources(&f, &["canonical public text"]);
    assert!(matches!(
        recording::select_materials(f.store(), &host, &id, 4096, |_| Ok("preview".into())),
        Err(JournalError::Conflict(_))
    ));
    assert_eq!(
        f.store()
            .read_journal_source(&host, &id, 0)
            .unwrap()
            .reflected_bytes,
        0
    );
}

#[test]
fn recording_materials_stop_at_unavailable_old_source_without_reading_newer_input() {
    let f = Fixture::new();
    let (host, id, ranges) = recording_sources(&f, &["old", "new"]);
    f.store()
        .mark_journal_sources_unavailable(&host, &id, &ranges[..1])
        .unwrap();
    let materials = recording::select_materials(f.store(), &host, &id, 4096, |_| {
        panic!("blocked prefix must not fetch newer input")
    })
    .unwrap();
    assert!(materials.is_empty());
}

#[test]
fn recording_materials_resume_exact_saved_byte_prefix_after_reopen() {
    let mut f = Fixture::new();
    let text = "first 日本語 second";
    let (host, id, ranges) = recording_sources(&f, &[text]);
    let selected =
        recording::select_materials(f.store(), &host, &id, 512, |_| Ok(text.into())).unwrap();
    let selected = vec![JournalSourceRange {
        bytes: JournalByteRange { start: 0, end: 6 },
        ..selected[0].range.clone()
    }];
    let binding = JobAttemptBinding {
        job_id: "recording-job".into(),
        attempt_id: "attempt".into(),
    };
    let generation = f
        .store()
        .prepare_journal_generation(&host, &id, binding.clone(), selected.clone())
        .unwrap();
    let job = JournalJobContext::validated(f.store(), &host, &generation.id, binding).unwrap();
    let state = f.store().read_journal(&host, &id).unwrap().state_token;
    f.store()
        .execute_journal(
            &job,
            JournalOperation {
                request_id: "unchanged".into(),
                expected_state: state,
                action: JournalAction::ConfirmUnchanged,
                covered_ranges: selected.clone(),
            },
        )
        .unwrap();
    let receipt = save_position(f.store(), &job, "recording-position");
    f.store()
        .checkpoint_journal(
            &job,
            JournalOperation {
                request_id: "checkpoint".into(),
                expected_state: receipt.state_token,
                action: JournalAction::Checkpoint {
                    record_operation_ids: vec!["unchanged".into()],
                },
                covered_ranges: selected,
            },
        )
        .unwrap();
    f.reopen();
    let remaining =
        recording::select_materials(f.store(), &host, &id, 4096, |_| Ok(text.into())).unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(
        remaining[0].range.bytes,
        JournalByteRange {
            start: 6,
            end: ranges[0].source.total_bytes
        }
    );
    assert_eq!(remaining[0].text, text[6..]);
}

struct Fixture {
    store: Option<SubjektivStore>,
    workspace: Option<ScopedFeatureStorage>,
    manager: Option<FeatureStorage>,
    temp: tempfile::TempDir,
    host: JournalHostContext,
    journal_id: String,
    ranges: Vec<JournalSourceRange>,
}

impl Fixture {
    fn new() -> Self {
        Self::with_sizes(&[256, 256])
    }

    fn with_sizes(sizes: &[u64]) -> Self {
        Self::with_schema(sizes, false)
    }

    fn with_schema(sizes: &[u64], upgrade: bool) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let manager = FeatureStorage::new(temp.path().join("storage"));
        let workspace = manager.workspace("journal-tests").unwrap();
        let registration = if upgrade {
            workspace
                .register(feature_storage::FeatureRegistration::new(
                    crate::SUBJEKTIV_FEATURE_ID,
                    &crate::MIGRATIONS[..crate::MIGRATIONS
                        .iter()
                        .position(|migration| migration.version() == 8)
                        .expect("Journal introduction migration")],
                ))
                .unwrap()
        } else {
            SubjektivStore::register(&workspace).unwrap()
        };
        let store = SubjektivStore::open(&workspace, &registration).unwrap();
        let subject = store
            .create_subject_with_behavior(
                SubjectRole::new("companion").unwrap(),
                if upgrade {
                    "Preserve existing Subject behavior".into()
                } else {
                    String::new()
                },
            )
            .unwrap();
        let attribution =
            SubjectSessionAttribution::new(&subject.id, "runtime", "worker", "source-session")
                .unwrap();
        store
            .record_session_attribution(attribution.clone())
            .unwrap();
        let (manager, workspace, store) = if upgrade {
            let has_journals: bool = store.database.try_with_connection::<_, JournalError>(|connection| {
                Ok(connection.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='journals')", [], |row| row.get(0))?)
            }).unwrap();
            assert!(!has_journals);
            drop(store);
            drop(workspace);
            manager.shutdown().unwrap();
            drop(manager);
            let manager = FeatureStorage::new(temp.path().join("storage"));
            let workspace = manager.workspace("journal-tests").unwrap();
            let registration = SubjektivStore::register(&workspace).unwrap();
            let store = SubjektivStore::open(&workspace, &registration).unwrap();
            assert_eq!(store.subject(&subject.id).unwrap(), Some(subject.clone()));
            assert_eq!(
                store.session_attribution("source-session").unwrap(),
                Some(attribution)
            );
            (manager, workspace, store)
        } else {
            (manager, workspace, store)
        };
        let host = session_host(&store, &subject.id, "source-session");
        let ranges = sizes
            .iter()
            .enumerate()
            .map(|(order, size)| source_range("source-session", order as u64, *size))
            .collect::<Vec<_>>();
        let journal_id = store
            .accept_journal_sources(&host, "Development journal", &ranges)
            .unwrap();
        Self {
            store: Some(store),
            workspace: Some(workspace),
            manager: Some(manager),
            temp,
            host,
            journal_id,
            ranges,
        }
    }

    fn store(&self) -> &SubjektivStore {
        self.store.as_ref().unwrap()
    }

    fn reopen(&mut self) {
        self.store.take();
        self.workspace.take();
        self.manager.take().unwrap().shutdown().unwrap();
        let manager = FeatureStorage::new(self.temp.path().join("storage"));
        let workspace = manager.workspace("journal-tests").unwrap();
        let registration = SubjektivStore::register(&workspace).unwrap();
        self.store = Some(SubjektivStore::open(&workspace, &registration).unwrap());
        self.workspace = Some(workspace);
        self.manager = Some(manager);
    }

    fn state(&self) -> (Journal, Option<JournalEntry>) {
        self.store()
            .journal_resume_state(&self.host, &self.journal_id)
            .unwrap()
    }

    fn generation(
        &self,
        name: &str,
        ranges: Vec<JournalSourceRange>,
    ) -> (JournalGeneration, JournalJobContext) {
        let binding = JobAttemptBinding {
            job_id: name.into(),
            attempt_id: "attempt-1".into(),
        };
        let generation = self
            .store()
            .prepare_journal_generation(&self.host, &self.journal_id, binding.clone(), ranges)
            .unwrap();
        let context =
            JournalJobContext::validated(self.store(), &self.host, &generation.id, binding)
                .unwrap();
        (generation, context)
    }

    fn operation(
        &self,
        id: &str,
        action: JournalAction,
        ranges: Vec<JournalSourceRange>,
    ) -> JournalOperation {
        JournalOperation {
            request_id: id.into(),
            expected_state: self.state().0.state_token,
            action,
            covered_ranges: ranges,
        }
    }

    fn execute(
        &self,
        ctx: &JournalJobContext,
        id: &str,
        action: JournalAction,
        ranges: Vec<JournalSourceRange>,
    ) -> JournalReceipt {
        self.store()
            .execute_journal(ctx, self.operation(id, action, ranges))
            .unwrap()
    }

    fn close(&self, generation: &JournalGeneration) {
        self.store()
            .close_journal_generation(&self.host, &generation.id, &generation.job)
            .unwrap();
    }

    fn sources(&self) -> Vec<JournalSourceStatus> {
        self.store()
            .list_journal_sources(&self.host, &self.journal_id, None, MAX_JOURNAL_PAGE)
            .unwrap()
            .items
    }

    fn entries(&self) -> Vec<JournalEntrySummary> {
        self.store()
            .list_journal_entries(&self.host, &self.journal_id, None, MAX_JOURNAL_PAGE)
            .unwrap()
            .items
    }

    fn receipt(&self, id: &str) -> Option<JournalReceipt> {
        self.store()
            .query_journal_operation(&self.host, &self.journal_id, id)
            .unwrap()
    }

    fn sql(&self, sql: &str) -> JournalResult<()> {
        self.store().database.try_transaction(|tx| {
            tx.execute_batch(sql)?;
            Ok(())
        })
    }

    fn execute_and_reopen(
        &mut self,
        ctx: &JournalJobContext,
        id: &str,
        action: JournalAction,
        ranges: Vec<JournalSourceRange>,
    ) -> JournalReceipt {
        let operation = self.operation(id, action, ranges);
        let receipt = self
            .store()
            .execute_journal(ctx, operation.clone())
            .unwrap();
        let state = self.state();
        self.reopen();
        assert_eq!(self.state(), state);
        assert_eq!(self.receipt(id), Some(receipt.clone()));
        assert_eq!(
            self.store().execute_journal(ctx, operation).unwrap(),
            receipt
        );
        receipt
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.store.take();
        self.workspace.take();
        if let Some(manager) = self.manager.take() {
            let _ = manager.shutdown();
        }
    }
}

fn session_host(store: &SubjektivStore, subject: &str, session: &str) -> JournalHostContext {
    let attribution =
        SubjectSessionAttribution::new(subject, "runtime", "worker", session).unwrap();
    let attribution = store.record_session_attribution(attribution).unwrap();
    let binding = store
        .claim_journal_writer(subject, "runtime", "worker")
        .unwrap();
    store.bind_journal_session(&binding, attribution).unwrap()
}

fn source_range(session: &str, order: u64, total_bytes: u64) -> JournalSourceRange {
    let digest = Sha256::digest(format!(
        "canonical-public-entry:{session}:{order}:{total_bytes}"
    ));
    JournalSourceRange {
        source: JournalSource {
            session_id: session.into(),
            pointer: JournalSourcePointer {
                segment_id: Uuid::now_v7(),
                entry_ref: format!("E{order}"),
            },
            order,
            content_sha256: digest.iter().map(|byte| format!("{byte:02x}")).collect(),
            total_bytes,
        },
        bytes: JournalByteRange {
            start: 0,
            end: total_bytes,
        },
    }
}

fn fragment(range: &JournalSourceRange, start: u64, end: u64) -> JournalSourceRange {
    JournalSourceRange {
        source: range.source.clone(),
        bytes: JournalByteRange { start, end },
    }
}

fn open_action(body: &str) -> JournalAction {
    JournalAction::Create {
        title: "Investigation".into(),
        body: body.into(),
        ongoing: "Verify the next boundary".into(),
        state: JournalEntryState::Open,
        source_end: None,
        correction_refs: vec![],
    }
}

fn fixed_action(body: &str, range: &JournalSourceRange, corrections: Vec<String>) -> JournalAction {
    JournalAction::Create {
        title: "Verified outcome".into(),
        body: body.into(),
        ongoing: String::new(),
        state: JournalEntryState::Fixed,
        source_end: Some(range.source.pointer.clone()),
        correction_refs: corrections,
    }
}

fn save_position(
    store: &SubjektivStore,
    context: &JournalJobContext,
    request_id: &str,
) -> JournalReceipt {
    let generation = store
        .read_journal_generation(&context.host, &context.generation_id)
        .unwrap();
    let (journal, _) = store
        .journal_resume_state(&context.host, &generation.journal_id)
        .unwrap();
    store
        .execute_journal(
            context,
            JournalOperation {
                request_id: request_id.into(),
                expected_state: journal.state_token,
                action: JournalAction::UpdatePosition {
                    current_position: journal.current_position,
                },
                covered_ranges: vec![],
            },
        )
        .unwrap()
}

fn checkpoint(ids: &[&str]) -> JournalAction {
    JournalAction::Checkpoint {
        record_operation_ids: ids.iter().map(|id| (*id).into()).collect(),
    }
}

#[test]
fn sequential_open_rewrite_fix_and_correction_preserve_fixed_history() {
    let fixture = Fixture::new();
    let (_, ctx) = fixture.generation("sequence", fixture.ranges.clone());
    let first = fixture.execute(
        &ctx,
        "create",
        open_action("Initial finding"),
        vec![fixture.ranges[0].clone()],
    );
    let first_id = first.entry_id.unwrap();
    let before = fixture.state();
    let blocked = fixture.operation(
        "append-before-fix",
        fixed_action("Must not append", &fixture.ranges[1], vec![]),
        vec![],
    );
    assert!(matches!(
        fixture.store().execute_journal(&ctx, blocked),
        Err(JournalError::Conflict(_))
    ));
    assert_eq!(fixture.state(), before);
    assert!(fixture.receipt("append-before-fix").is_none());

    fixture.execute(
        &ctx,
        "rewrite",
        JournalAction::Rewrite {
            entry_id: first_id.clone(),
            title: "Evidence checked".into(),
            body: "Revised finding".into(),
            ongoing: "One final check".into(),
        },
        vec![fixture.ranges[0].clone()],
    );
    let rewritten = fixture.state().1.unwrap();
    assert_eq!(rewritten.id, first_id);
    assert_eq!(rewritten.position, 0);
    assert_eq!(rewritten.body, "Revised finding");
    fixture.execute(
        &ctx,
        "fix",
        JournalAction::Fix {
            entry_id: first_id.clone(),
            source_end: fixture.ranges[0].source.pointer.clone(),
        },
        vec![fixture.ranges[0].clone()],
    );
    assert!(fixture.state().1.is_none());
    let fixed = fixture.entries()[0].clone();
    assert_eq!(fixed.state, JournalEntryState::Fixed);
    assert!(fixed.fixed_at.is_some());
    assert!(fixed.previous_fixed_end.is_none());
    assert!(fixed.source_available);
    assert_eq!(fixture.state().0.process_order, 0);

    let immutable = fixture.operation(
        "rewrite-fixed",
        JournalAction::Rewrite {
            entry_id: first_id.clone(),
            title: "Tampered".into(),
            body: "Tampered".into(),
            ongoing: String::new(),
        },
        vec![],
    );
    assert!(matches!(
        fixture.store().execute_journal(&ctx, immutable),
        Err(JournalError::Conflict(_))
    ));
    let repeated_end = fixture.operation(
        "non-advancing-end",
        fixed_action(
            "Cannot reuse the earlier endpoint",
            &fixture.ranges[0],
            vec![],
        ),
        vec![],
    );
    assert!(matches!(
        fixture.store().execute_journal(&ctx, repeated_end),
        Err(JournalError::InvalidInput(_))
    ));
    let missing_correction = fixture.operation(
        "missing-correction",
        fixed_action(
            "No such target",
            &fixture.ranges[1],
            vec!["missing-entry".into()],
        ),
        vec![],
    );
    assert!(matches!(
        fixture.store().execute_journal(&ctx, missing_correction),
        Err(JournalError::PermissionDenied(_))
    ));
    assert_eq!(fixture.entries(), vec![fixed.clone()]);
    assert!(fixture.receipt("missing-correction").is_none());
    let second = fixture.execute(
        &ctx,
        "correction",
        fixed_action(
            "The earlier finding needs qualification",
            &fixture.ranges[1],
            vec![first_id.clone()],
        ),
        vec![fixture.ranges[1].clone()],
    );
    let entries = fixture.entries();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0], fixed);
    assert_eq!(entries[1].id, second.entry_id.unwrap());
    assert_eq!(entries[1].position, 1);
    assert_eq!(entries[1].correction_refs, vec![first_id]);
    assert_eq!(entries[1].previous_fixed_end, fixed.source_end);
    assert_eq!(fixture.state().0.process_order, 0);
    save_position(fixture.store(), &ctx, "final-position");
    let cp = fixture.execute(
        &ctx,
        "checkpoint",
        checkpoint(&["rewrite", "correction"]),
        fixture.ranges.clone(),
    );
    assert_eq!((cp.process_order, cp.process_byte), (2, 0));
}

#[test]
fn same_attempt_keeps_immutable_input_while_next_attempt_carries_current_state() {
    let fixture = Fixture::new();
    let selected = vec![fixture.ranges[0].clone()];
    let (generation, ctx) = fixture.generation("first-job", selected.clone());
    fixture.execute(
        &ctx,
        "create",
        open_action("Pending investigation"),
        selected.clone(),
    );
    fixture.execute(
        &ctx,
        "position",
        JournalAction::UpdatePosition {
            current_position: "Investigating receipt recovery".into(),
        },
        vec![],
    );
    fixture.execute(
        &ctx,
        "checkpoint",
        checkpoint(&["create"]),
        selected.clone(),
    );
    let current = fixture.state();
    let new_source = source_range("source-session", 2, 400);
    assert_eq!(
        fixture
            .store()
            .accept_journal_sources(
                &fixture.host,
                "Ignored replacement title",
                &[new_source.clone()]
            )
            .unwrap(),
        fixture.journal_id
    );
    assert_eq!(fixture.state(), current);
    assert_eq!(
        fixture
            .store()
            .read_journal_generation(&fixture.host, &generation.id)
            .unwrap(),
        generation
    );
    assert_eq!(
        fixture
            .store()
            .prepare_journal_generation(
                &fixture.host,
                &fixture.journal_id,
                generation.job.clone(),
                selected
            )
            .unwrap(),
        generation
    );
    assert!(matches!(
        fixture.store().prepare_journal_generation(
            &fixture.host,
            &fixture.journal_id,
            generation.job.clone(),
            fixture.ranges.clone()
        ),
        Err(JournalError::Conflict(_))
    ));
    let changed_input = fixture.operation(
        "outside-snapshot",
        JournalAction::ConfirmUnchanged,
        vec![fixture.ranges[1].clone()],
    );
    assert!(matches!(
        fixture.store().execute_journal(&ctx, changed_input),
        Err(JournalError::InvalidInput(_))
    ));
    assert!(matches!(
        fixture.store().prepare_journal_generation(
            &fixture.host,
            &fixture.journal_id,
            JobAttemptBinding {
                job_id: "another-job".into(),
                attempt_id: "attempt-1".into()
            },
            vec![fixture.ranges[1].clone()]
        ),
        Err(JournalError::Conflict(_))
    ));
    fixture.close(&generation);
    let (next, _) = fixture.generation("next-job", vec![fixture.ranges[1].clone(), new_source]);
    assert_eq!(next.initial_journal, current.0);
    assert_eq!(next.initial_open, current.1);
    assert!(next.previous_fixed_end.is_none());
    assert_eq!(
        fixture
            .store()
            .read_journal_generation(&fixture.host, &generation.id)
            .unwrap(),
        generation
    );
}

#[test]
fn lost_response_reopen_returns_exact_receipt_and_rejects_changed_retry() {
    let mut fixture = Fixture::new();
    let (generation, ctx) = fixture.generation("response-loss", fixture.ranges.clone());
    let operation = fixture.operation(
        "stable-request",
        open_action("Saved before response loss"),
        vec![fixture.ranges[0].clone()],
    );
    // The caller discards the successful response and recovers from the durable receipt.
    fixture
        .store()
        .execute_journal(&ctx, operation.clone())
        .unwrap();
    fixture.reopen();
    let recovered = fixture.receipt("stable-request").unwrap();
    let state = fixture.state();
    assert_eq!(
        fixture
            .store()
            .execute_journal(&ctx, operation.clone())
            .unwrap(),
        recovered
    );
    assert_eq!(fixture.state(), state);
    assert_eq!(fixture.entries().len(), 1);
    let mut changed = operation.clone();
    changed.action = open_action("Different body using the same request ID");
    assert!(matches!(
        fixture.store().execute_journal(&ctx, changed),
        Err(JournalError::Conflict(_))
    ));
    let mut changed = operation.clone();
    changed.expected_state = recovered.state_token.clone();
    assert!(matches!(
        fixture.store().execute_journal(&ctx, changed),
        Err(JournalError::Conflict(_))
    ));
    let mut changed = operation.clone();
    changed.covered_ranges = vec![fixture.ranges[1].clone()];
    assert!(matches!(
        fixture.store().execute_journal(&ctx, changed),
        Err(JournalError::Conflict(_))
    ));
    fixture.close(&generation);
    fixture.reopen();
    assert_eq!(
        fixture
            .store()
            .execute_journal(&ctx, operation.clone())
            .unwrap(),
        recovered
    );
    let (_, replacement) = fixture.generation("replacement-job", fixture.ranges.clone());
    assert!(matches!(
        fixture.store().execute_journal(&replacement, operation),
        Err(JournalError::Conflict(_))
    ));
    assert_eq!(fixture.receipt("stable-request"), Some(recovered));
    assert_eq!(fixture.state(), state);
}

#[test]
fn cancellation_preserves_open_and_receipts_but_fences_late_writes_and_checkpoint() {
    let mut fixture = Fixture::new();
    let (generation, ctx) = fixture.generation("cancelled", fixture.ranges.clone());
    let saved = fixture.execute(
        &ctx,
        "create",
        open_action("Saved but not reflected"),
        fixture.ranges.clone(),
    );
    let late = fixture.operation(
        "late-checkpoint",
        checkpoint(&["create"]),
        fixture.ranges.clone(),
    );
    let before = fixture.state();
    fixture.close(&generation);
    fixture.close(&generation);
    fixture.reopen();
    assert!(matches!(
        fixture.store().execute_journal(&ctx, late),
        Err(JournalError::Conflict(_))
    ));
    let late_rewrite = fixture.operation(
        "late-rewrite",
        JournalAction::Rewrite {
            entry_id: saved.entry_id.clone().unwrap(),
            title: "Late".into(),
            body: "Late".into(),
            ongoing: String::new(),
        },
        vec![],
    );
    assert!(matches!(
        fixture.store().execute_journal(&ctx, late_rewrite),
        Err(JournalError::Conflict(_))
    ));
    assert!(matches!(
        JournalJobContext::validated(
            fixture.store(),
            &fixture.host,
            &generation.id,
            generation.job.clone()
        ),
        Err(JournalError::Conflict(_))
    ));
    assert_eq!(fixture.state(), before);
    assert_eq!(fixture.receipt("create"), Some(saved));
    assert!(fixture.receipt("late-checkpoint").is_none());
    assert!(
        fixture
            .sources()
            .iter()
            .all(|source| source.reflected_bytes == 0)
    );
}

#[test]
fn stale_cas_is_rejected_even_after_position_returns_to_same_text() {
    let mut fixture = Fixture::new();
    let (_, ctx) = fixture.generation("causal-state", fixture.ranges.clone());
    fixture.execute(
        &ctx,
        "position-a",
        JournalAction::UpdatePosition {
            current_position: "A".into(),
        },
        vec![],
    );
    let at_a = fixture.state().0;
    let stale = fixture.operation(
        "stale-create",
        open_action("Must not apply against a later A"),
        vec![],
    );
    fixture.execute(
        &ctx,
        "position-b",
        JournalAction::UpdatePosition {
            current_position: "B".into(),
        },
        vec![],
    );
    fixture.execute(
        &ctx,
        "position-a-again",
        JournalAction::UpdatePosition {
            current_position: "A".into(),
        },
        vec![],
    );
    fixture.reopen();
    let current = fixture.state().0;
    assert_eq!(current.current_position, at_a.current_position);
    assert_ne!(current.state_token, at_a.state_token);
    assert!(matches!(
        fixture.store().execute_journal(&ctx, stale),
        Err(JournalError::Conflict(_))
    ));
    assert!(fixture.entries().is_empty());
    assert!(fixture.receipt("stale-create").is_none());
    assert_eq!(fixture.state().0, current);
}

#[test]
fn foreign_scope_subject_session_job_and_journal_cannot_mutate_or_read_receipts() {
    let fixture = Fixture::new();
    let (generation, ctx) = fixture.generation("authorized-job", fixture.ranges.clone());
    let created = fixture.execute(&ctx, "create", open_action("Subject-owned record"), vec![]);
    let entry_id = created.entry_id.unwrap();
    let before = fixture.state();
    let foreign_subject = fixture
        .store()
        .create_subject(SubjectRole::new("other").unwrap())
        .unwrap();
    let foreign_host = session_host(fixture.store(), &foreign_subject.id, "foreign-session");
    assert!(matches!(
        fixture
            .store()
            .read_journal(&foreign_host, &fixture.journal_id),
        Err(JournalError::NotFound(_))
    ));
    assert!(matches!(
        fixture
            .store()
            .query_journal_operation(&foreign_host, &fixture.journal_id, "create"),
        Err(JournalError::NotFound(_))
    ));
    assert!(matches!(
        JournalJobContext::validated(
            fixture.store(),
            &foreign_host,
            &generation.id,
            generation.job.clone()
        ),
        Err(JournalError::NotFound(_))
    ));
    let other_session = session_host(fixture.store(), &fixture.host.subject_id, "other-session");
    assert!(matches!(
        fixture
            .store()
            .journal_resume_state(&other_session, &fixture.journal_id),
        Err(JournalError::PermissionDenied(_))
    ));
    assert!(matches!(
        JournalJobContext::validated(
            fixture.store(),
            &other_session,
            &generation.id,
            generation.job.clone()
        ),
        Err(JournalError::PermissionDenied(_))
    ));
    let wrong_attempt = JobAttemptBinding {
        job_id: generation.job.job_id.clone(),
        attempt_id: "attempt-2".into(),
    };
    assert!(matches!(
        JournalJobContext::validated(
            fixture.store(),
            &fixture.host,
            &generation.id,
            wrong_attempt.clone()
        ),
        Err(JournalError::PermissionDenied(_))
    ));
    assert!(matches!(
        fixture
            .store()
            .close_journal_generation(&fixture.host, &generation.id, &wrong_attempt),
        Err(JournalError::PermissionDenied(_))
    ));

    let other_manager = FeatureStorage::new(fixture.temp.path().join("foreign-storage"));
    let other_workspace = other_manager.workspace("foreign-scope").unwrap();
    let registration = SubjektivStore::register(&other_workspace).unwrap();
    let other_store = SubjektivStore::open(&other_workspace, &registration).unwrap();
    assert!(matches!(
        other_store.read_journal(&fixture.host, &fixture.journal_id),
        Err(JournalError::PermissionDenied(_))
    ));
    assert!(matches!(
        other_store.execute_journal(
            &ctx,
            fixture.operation("wrong-scope", open_action("No"), vec![])
        ),
        Err(JournalError::PermissionDenied(_))
    ));
    let reader =
        JournalHostContext::validated_reader(fixture.store(), &fixture.host.subject_id).unwrap();
    assert!(matches!(
        fixture
            .store()
            .accept_journal_sources(&reader, "No authority", &fixture.ranges),
        Err(JournalError::PermissionDenied(_))
    ));
    let mut forged_attribution = fixture.host.attribution.clone().unwrap();
    forged_attribution.worker_id = "different-worker".into();
    assert!(matches!(
        JournalHostContext::validated_session(fixture.store(), forged_attribution),
        Err(JournalError::PermissionDenied(_))
    ));

    let other_range = source_range("other-session", 0, 256);
    let other_id = fixture
        .store()
        .accept_journal_sources(&other_session, "Another journal", &[other_range.clone()])
        .unwrap();
    let other_generation = fixture
        .store()
        .prepare_journal_generation(
            &other_session,
            &other_id,
            JobAttemptBinding {
                job_id: "other-journal-job".into(),
                attempt_id: "attempt-1".into(),
            },
            vec![other_range],
        )
        .unwrap();
    let other_ctx = JournalJobContext::validated(
        fixture.store(),
        &other_session,
        &other_generation.id,
        other_generation.job,
    )
    .unwrap();
    let other_state = fixture
        .store()
        .journal_resume_state(&other_session, &other_id)
        .unwrap();
    let cross_journal = JournalOperation {
        request_id: "cross-journal-rewrite".into(),
        expected_state: other_state.0.state_token.clone(),
        action: JournalAction::Rewrite {
            entry_id,
            title: "No".into(),
            body: "No".into(),
            ongoing: "No".into(),
        },
        covered_ranges: vec![],
    };
    assert!(matches!(
        fixture.store().execute_journal(&other_ctx, cross_journal),
        Err(JournalError::NotFound(_))
    ));
    assert_eq!(
        fixture
            .store()
            .journal_resume_state(&other_session, &other_id)
            .unwrap(),
        other_state
    );
    assert_eq!(fixture.state(), before);
}

#[test]
fn corrections_allow_same_subject_other_journal_but_reject_foreign_subject_atomically() {
    let fixture = Fixture::new();
    let (generation, ctx) = fixture.generation("first", fixture.ranges.clone());
    let target = fixture
        .execute(
            &ctx,
            "fixed",
            fixed_action("Original record", &fixture.ranges[0], vec![]),
            vec![],
        )
        .entry_id
        .unwrap();
    fixture.close(&generation);
    let same_subject = session_host(fixture.store(), &fixture.host.subject_id, "later-session");
    let later_source = source_range("later-session", 0, 128);
    let later_id = fixture
        .store()
        .accept_journal_sources(&same_subject, "Later journal", &[later_source.clone()])
        .unwrap();
    let later_generation = fixture
        .store()
        .prepare_journal_generation(
            &same_subject,
            &later_id,
            JobAttemptBinding {
                job_id: "later".into(),
                attempt_id: "one".into(),
            },
            vec![later_source.clone()],
        )
        .unwrap();
    let later_ctx = JournalJobContext::validated(
        fixture.store(),
        &same_subject,
        &later_generation.id,
        later_generation.job,
    )
    .unwrap();
    let operation = JournalOperation {
        request_id: "cross-journal-correction".into(),
        expected_state: later_generation.initial_journal.state_token,
        action: fixed_action(
            "Qualification from a later Session",
            &later_source,
            vec![target.clone()],
        ),
        covered_ranges: vec![later_source],
    };
    fixture
        .store()
        .execute_journal(&later_ctx, operation)
        .unwrap();
    let later_entries = fixture
        .store()
        .list_journal_entries(&same_subject, &later_id, None, 1)
        .unwrap();
    assert_eq!(later_entries.items[0].correction_refs, vec![target.clone()]);

    let foreign_subject = fixture
        .store()
        .create_subject(SubjectRole::new("foreign").unwrap())
        .unwrap();
    let foreign_host = session_host(fixture.store(), &foreign_subject.id, "foreign-session");
    let foreign_source = source_range("foreign-session", 0, 128);
    let foreign_id = fixture
        .store()
        .accept_journal_sources(&foreign_host, "Foreign", &[foreign_source.clone()])
        .unwrap();
    let foreign_generation = fixture
        .store()
        .prepare_journal_generation(
            &foreign_host,
            &foreign_id,
            JobAttemptBinding {
                job_id: "foreign".into(),
                attempt_id: "one".into(),
            },
            vec![foreign_source.clone()],
        )
        .unwrap();
    let foreign_ctx = JournalJobContext::validated(
        fixture.store(),
        &foreign_host,
        &foreign_generation.id,
        foreign_generation.job,
    )
    .unwrap();
    let rejected = JournalOperation {
        request_id: "foreign-correction".into(),
        expected_state: foreign_generation.initial_journal.state_token.clone(),
        action: fixed_action("Forbidden correction", &foreign_source, vec![target]),
        covered_ranges: vec![foreign_source],
    };
    assert!(matches!(
        fixture.store().execute_journal(&foreign_ctx, rejected),
        Err(JournalError::PermissionDenied(_))
    ));
    assert!(
        fixture
            .store()
            .list_journal_entries(&foreign_host, &foreign_id, None, 1)
            .unwrap()
            .items
            .is_empty()
    );
    assert!(
        fixture
            .store()
            .query_journal_operation(&foreign_host, &foreign_id, "foreign-correction")
            .unwrap()
            .is_none()
    );
    assert_eq!(
        fixture
            .store()
            .read_journal(&foreign_host, &foreign_id)
            .unwrap()
            .state_token,
        foreign_generation.initial_journal.state_token
    );
}

#[test]
fn retired_subject_keeps_reader_access_but_rejects_existing_write_context() {
    let fixture = Fixture::new();
    let (_, ctx) = fixture.generation("retired-subject", fixture.ranges.clone());
    let receipt = fixture.execute(&ctx, "saved", open_action("Historical record"), vec![]);
    let reader =
        JournalHostContext::validated_reader(fixture.store(), &fixture.host.subject_id).unwrap();
    let late = fixture.operation(
        "late",
        JournalAction::UpdatePosition {
            current_position: "No longer authorized".into(),
        },
        vec![],
    );
    fixture
        .store()
        .retire_subject(&fixture.host.subject_id)
        .unwrap();
    assert!(matches!(
        fixture.store().execute_journal(&ctx, late),
        Err(JournalError::PermissionDenied(_))
    ));
    assert_eq!(
        fixture
            .store()
            .query_journal_operation(&reader, &fixture.journal_id, "saved")
            .unwrap(),
        Some(receipt)
    );
    assert_eq!(
        fixture
            .store()
            .list_journal_entries(&reader, &fixture.journal_id, None, 1)
            .unwrap()
            .items
            .len(),
        1
    );
}

#[test]
fn concurrent_creates_from_independent_connections_save_exactly_one_open_and_receipt() {
    let fixture = Fixture::new();
    let (_, ctx) = fixture.generation("concurrent", fixture.ranges.clone());
    let manager = FeatureStorage::new(fixture.temp.path().join("storage"));
    let workspace = manager.workspace("journal-tests").unwrap();
    let registration = SubjektivStore::register(&workspace).unwrap();
    let second_store = SubjektivStore::open(&workspace, &registration).unwrap();
    let barrier = Arc::new(Barrier::new(3));
    let first_op = fixture.operation(
        "writer-a",
        open_action("A"),
        vec![fixture.ranges[0].clone()],
    );
    let second_op = fixture.operation(
        "writer-b",
        open_action("B"),
        vec![fixture.ranges[0].clone()],
    );
    let spawn = |store: SubjektivStore, operation: JournalOperation| {
        let barrier = barrier.clone();
        let ctx = ctx.clone();
        thread::spawn(move || {
            barrier.wait();
            store.execute_journal(&ctx, operation)
        })
    };
    let first = spawn(fixture.store().clone(), first_op);
    let second = spawn(second_store, second_op);
    barrier.wait();
    let results = [first.join().unwrap(), second.join().unwrap()];
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(JournalError::Conflict(_))))
            .count(),
        1
    );
    let winner = results.into_iter().find_map(Result::ok).unwrap();
    assert_eq!(fixture.entries().len(), 1);
    assert_eq!(fixture.state().1.unwrap().id, winner.entry_id.unwrap());
    let receipts = fixture
        .store()
        .list_journal_operations(&fixture.host, &fixture.journal_id, None, 2)
        .unwrap();
    assert_eq!(receipts.items.len(), 1);
    assert_eq!(receipts.items[0].request_id, winner.request_id);
    assert_eq!(fixture.state().0.process_order, 0);
    drop(workspace);
    manager.shutdown().unwrap();
}

#[test]
fn checkpoint_requires_saved_record_correspondence_and_never_position_only_evidence() {
    let fixture = Fixture::new();
    let (_, ctx) = fixture.generation("checkpoint-evidence", fixture.ranges.clone());
    fixture.execute(
        &ctx,
        "position",
        JournalAction::UpdatePosition {
            current_position: "A summary is not a receipt for source reflection".into(),
        },
        vec![],
    );
    let before = fixture.state();
    let position_only = fixture.operation(
        "position-checkpoint",
        checkpoint(&["position"]),
        vec![fixture.ranges[0].clone()],
    );
    assert!(matches!(
        fixture.store().execute_journal(&ctx, position_only),
        Err(JournalError::InvalidInput(_))
    ));
    let missing = fixture.operation(
        "missing-checkpoint",
        checkpoint(&["missing-record"]),
        vec![fixture.ranges[0].clone()],
    );
    assert!(matches!(
        fixture.store().execute_journal(&ctx, missing),
        Err(JournalError::NotFound(_))
    ));
    let false_position = fixture.operation(
        "false-position",
        JournalAction::UpdatePosition {
            current_position: "Pretend processed".into(),
        },
        vec![fixture.ranges[0].clone()],
    );
    assert!(matches!(
        fixture.store().execute_journal(&ctx, false_position),
        Err(JournalError::InvalidInput(_))
    ));
    assert_eq!(fixture.state(), before);
    fixture.execute(
        &ctx,
        "record",
        open_action("Only the first source is represented"),
        vec![fixture.ranges[0].clone()],
    );
    let after_record = fixture.state();
    let excess = fixture.operation(
        "excess-checkpoint",
        checkpoint(&["record"]),
        fixture.ranges.clone(),
    );
    assert!(matches!(
        fixture.store().execute_journal(&ctx, excess),
        Err(JournalError::InvalidInput(_))
    ));
    assert_eq!(fixture.state(), after_record);
    assert!(
        fixture
            .sources()
            .iter()
            .all(|source| source.reflected_bytes == 0)
    );
    fixture.execute(
        &ctx,
        "checked-unchanged",
        JournalAction::ConfirmUnchanged,
        vec![fixture.ranges[1].clone()],
    );
    save_position(fixture.store(), &ctx, "checked-position");
    let receipt = fixture.execute(
        &ctx,
        "valid-checkpoint",
        checkpoint(&["record", "checked-unchanged"]),
        fixture.ranges.clone(),
    );
    assert_eq!((receipt.process_order, receipt.process_byte), (2, 0));
    assert!(
        fixture
            .sources()
            .iter()
            .all(|source| source.reflected_bytes == source.source.total_bytes)
    );
    let before = fixture.state();
    let replay_prefix = fixture.operation(
        "rewind-checkpoint",
        checkpoint(&["record"]),
        vec![fixture.ranges[0].clone()],
    );
    assert!(matches!(
        fixture.store().execute_journal(&ctx, replay_prefix),
        Err(JournalError::InvalidInput(_))
    ));
    assert_eq!(fixture.state(), before);
}

#[test]
fn huge_source_partial_generations_checkpoint_only_read_prefix_and_fix_only_at_entry_end() {
    let mut fixture = Fixture::with_sizes(&[8 * 1024 * 1024]);
    let source = fixture.ranges[0].clone();
    let head = fragment(&source, 0, 16 * 1024);
    let rest = fragment(&source, head.bytes.end, source.source.total_bytes);
    let (first_generation, first_ctx) = fixture.generation("huge-head", vec![head.clone()]);
    let created = fixture.execute(
        &first_ctx,
        "partial-record",
        open_action("Only the initial fragment was checked"),
        vec![head.clone()],
    );
    let unread_end = fixture.operation(
        "premature-fix",
        JournalAction::Fix {
            entry_id: created.entry_id.clone().unwrap(),
            source_end: source.source.pointer.clone(),
        },
        vec![],
    );
    assert!(matches!(
        fixture.store().execute_journal(&first_ctx, unread_end),
        Err(JournalError::InvalidInput(_))
    ));
    save_position(fixture.store(), &first_ctx, "partial-position");
    let cp = fixture.execute(
        &first_ctx,
        "partial-checkpoint",
        checkpoint(&["partial-record"]),
        vec![head.clone()],
    );
    assert_eq!((cp.process_order, cp.process_byte), (0, head.bytes.end));
    assert_eq!(fixture.sources()[0].reflected_bytes, head.bytes.end);
    fixture.close(&first_generation);
    fixture.reopen();
    let gap = fragment(&source, head.bytes.end + 1, source.source.total_bytes);
    let binding = JobAttemptBinding {
        job_id: "gap-job".into(),
        attempt_id: "one".into(),
    };
    assert!(matches!(
        fixture.store().prepare_journal_generation(
            &fixture.host,
            &fixture.journal_id,
            binding,
            vec![gap]
        ),
        Err(JournalError::InvalidInput(_))
    ));
    let overlap = fragment(&source, head.bytes.end - 1, source.source.total_bytes);
    let binding = JobAttemptBinding {
        job_id: "overlap-job".into(),
        attempt_id: "one".into(),
    };
    assert!(matches!(
        fixture.store().prepare_journal_generation(
            &fixture.host,
            &fixture.journal_id,
            binding,
            vec![overlap]
        ),
        Err(JournalError::InvalidInput(_))
    ));
    let (next, next_ctx) = fixture.generation("huge-tail", vec![rest.clone()]);
    assert_eq!(next.initial_journal.process_byte, head.bytes.end);
    assert_eq!(
        next.initial_open.as_ref().unwrap().id,
        created.entry_id.clone().unwrap()
    );
    fixture.execute(
        &next_ctx,
        "tail-record",
        JournalAction::Rewrite {
            entry_id: created.entry_id.clone().unwrap(),
            title: "Complete input checked".into(),
            body: "A bounded record of a huge source".into(),
            ongoing: "Ready to fix".into(),
        },
        vec![rest.clone()],
    );
    fixture.execute(
        &next_ctx,
        "fix",
        JournalAction::Fix {
            entry_id: created.entry_id.unwrap(),
            source_end: source.source.pointer.clone(),
        },
        vec![],
    );
    assert_eq!(fixture.state().0.process_byte, head.bytes.end);
    save_position(fixture.store(), &next_ctx, "tail-position");
    let cp = fixture.execute(
        &next_ctx,
        "tail-checkpoint",
        checkpoint(&["tail-record"]),
        vec![rest],
    );
    assert_eq!((cp.process_order, cp.process_byte), (1, 0));
    assert_eq!(
        fixture.sources()[0].reflected_bytes,
        source.source.total_bytes
    );
}

#[test]
fn accepted_overlap_merges_but_gaps_and_overlapping_selected_prefixes_cannot_skip_input() {
    let fixture = Fixture::new();
    let ranges = vec![
        fragment(&fixture.ranges[0], 0, 150),
        fragment(&fixture.ranges[0], 100, 256),
    ];
    fixture
        .store()
        .accept_journal_sources(&fixture.host, "Same journal", &ranges)
        .unwrap();
    assert_eq!(
        fixture.sources()[0].accepted,
        vec![JournalByteRange { start: 0, end: 256 }]
    );
    let binding = JobAttemptBinding {
        job_id: "overlapping-selection".into(),
        attempt_id: "one".into(),
    };
    assert!(matches!(
        fixture.store().prepare_journal_generation(
            &fixture.host,
            &fixture.journal_id,
            binding,
            ranges
        ),
        Err(JournalError::InvalidInput(_))
    ));
    let binding = JobAttemptBinding {
        job_id: "missing-order-zero".into(),
        attempt_id: "one".into(),
    };
    assert!(matches!(
        fixture.store().prepare_journal_generation(
            &fixture.host,
            &fixture.journal_id,
            binding,
            vec![fixture.ranges[1].clone()]
        ),
        Err(JournalError::InvalidInput(_))
    ));

    let host = session_host(fixture.store(), &fixture.host.subject_id, "gapped-session");
    let full = source_range("gapped-session", 0, 1000);
    let head = fragment(&full, 0, 100);
    let tail = fragment(&full, 200, 1000);
    let id = fixture
        .store()
        .accept_journal_sources(&host, "Gapped admission", &[head.clone(), tail.clone()])
        .unwrap();
    let binding = JobAttemptBinding {
        job_id: "gapped-source".into(),
        attempt_id: "one".into(),
    };
    assert!(matches!(
        fixture
            .store()
            .prepare_journal_generation(&host, &id, binding.clone(), vec![full.clone()]),
        Err(JournalError::InvalidInput(_))
    ));
    assert!(matches!(
        fixture.store().prepare_journal_generation(
            &host,
            &id,
            binding.clone(),
            vec![head.clone(), tail.clone()]
        ),
        Err(JournalError::InvalidInput(_))
    ));
    fixture
        .store()
        .accept_journal_sources(&host, "Fill only the gap", &[fragment(&full, 100, 200)])
        .unwrap();
    let generation = fixture
        .store()
        .prepare_journal_generation(&host, &id, binding, vec![full])
        .unwrap();
    assert_eq!(generation.initial_journal.process_byte, 0);
    assert!(fixture.entries().is_empty());
}

#[test]
fn canonical_compaction_identity_is_idempotent_and_rewind_replacement_rolls_back_admission() {
    let fixture = Fixture::new();
    let before = fixture.state();
    let sources = fixture.sources();
    // A retained compaction copy supplies the original canonical identity, not a new source.
    assert_eq!(
        fixture
            .store()
            .accept_journal_sources(&fixture.host, "Compacted copy", &fixture.ranges)
            .unwrap(),
        fixture.journal_id
    );
    assert_eq!(fixture.sources(), sources);
    assert_eq!(fixture.state(), before);
    let original = fixture.ranges[0].clone();
    let mut replacements = Vec::new();
    let mut changed = original.clone();
    changed.source.content_sha256 = "f".repeat(64);
    replacements.push(changed);
    let mut changed = original.clone();
    changed.source.pointer.segment_id = Uuid::now_v7();
    replacements.push(changed);
    let mut changed = original.clone();
    changed.source.pointer.entry_ref = "Ereplacement".into();
    replacements.push(changed);
    let mut changed = original.clone();
    changed.source.total_bytes += 1;
    replacements.push(changed);
    let mut changed = original.clone();
    changed.source.order = 2;
    replacements.push(changed);
    for replacement in replacements {
        let new_source = source_range("source-session", 3, 64);
        let result = fixture.store().accept_journal_sources(
            &fixture.host,
            "Must roll back both",
            &[new_source, replacement],
        );
        assert!(
            matches!(result, Err(JournalError::Conflict(_))),
            "{result:?}"
        );
        assert_eq!(fixture.sources(), sources);
        assert_eq!(fixture.state(), before);
    }
    let wrong_session = source_range("another-session", 2, 100);
    assert!(matches!(
        fixture
            .store()
            .accept_journal_sources(&fixture.host, "No", &[wrong_session]),
        Err(JournalError::PermissionDenied(_))
    ));
    let mut derived_summary = source_range("source-session", 2, 100);
    derived_summary.source.pointer.entry_ref = "summary-2".into();
    assert!(matches!(
        fixture.store().accept_journal_sources(
            &fixture.host,
            "No derived source",
            &[derived_summary]
        ),
        Err(JournalError::InvalidInput(_))
    ));
    assert_eq!(fixture.sources(), sources);
}

#[test]
fn unavailable_source_preserves_records_and_receipts_without_advancing_processing() {
    let mut fixture = Fixture::new();
    let (generation, ctx) = fixture.generation("retention", fixture.ranges.clone());
    let record = fixture.execute(
        &ctx,
        "record",
        fixed_action("Retained outcome", &fixture.ranges[0], vec![]),
        vec![fixture.ranges[0].clone()],
    );
    let before = fixture.state();
    fixture
        .store()
        .mark_journal_sources_unavailable(
            &fixture.host,
            &fixture.journal_id,
            &[fixture.ranges[0].clone()],
        )
        .unwrap();
    fixture.reopen();
    assert_eq!(fixture.state(), before);
    assert_eq!(fixture.receipt("record"), Some(record));
    assert_eq!(
        fixture.entries()[0].source_end,
        Some(fixture.ranges[0].source.pointer.clone())
    );
    assert!(!fixture.entries()[0].source_available);
    assert_eq!(fixture.sources()[0].reflected_bytes, 0);
    let unread = fixture.operation(
        "unavailable-checkpoint",
        checkpoint(&["record"]),
        vec![fixture.ranges[0].clone()],
    );
    assert!(matches!(
        fixture.store().execute_journal(&ctx, unread),
        Err(JournalError::Conflict(_))
    ));
    let unavailable = fixture.operation(
        "unavailable-confirmation",
        JournalAction::ConfirmUnchanged,
        vec![fixture.ranges[0].clone()],
    );
    assert!(matches!(
        fixture.store().execute_journal(&ctx, unavailable),
        Err(JournalError::Conflict(_))
    ));
    fixture.close(&generation);
    let binding = JobAttemptBinding {
        job_id: "retention-replacement".into(),
        attempt_id: "one".into(),
    };
    assert!(matches!(
        fixture.store().prepare_journal_generation(
            &fixture.host,
            &fixture.journal_id,
            binding,
            fixture.ranges.clone()
        ),
        Err(JournalError::Conflict(_))
    ));
    let reader =
        JournalHostContext::validated_reader(fixture.store(), &fixture.host.subject_id).unwrap();
    let page = fixture
        .store()
        .read_journal_text(
            &reader,
            &fixture.journal_id,
            Some(fixture.entries()[0].id.as_str()),
            JournalTextField::Body,
            None,
            0,
            100,
        )
        .unwrap();
    assert_eq!(page.text, "Retained outcome");
    assert!(fixture.receipt("unavailable-checkpoint").is_none());
}

#[test]
fn unicode_and_escaped_long_text_pages_reconstruct_exactly_and_reject_stale_continuation() {
    let fixture = Fixture::new();
    let (_, ctx) = fixture.generation("text-pages", fixture.ranges.clone());
    let body = "界🙂\"\\\n\t".repeat(12_000);
    let ongoing = "次の確認🙂\n".repeat(3000);
    let action = JournalAction::Create {
        title: "Unicode".into(),
        body: body.clone(),
        ongoing: ongoing.clone(),
        state: JournalEntryState::Open,
        source_end: None,
        correction_refs: vec![],
    };
    let id = fixture
        .execute(&ctx, "create", action, vec![])
        .entry_id
        .unwrap();
    fixture.execute(
        &ctx,
        "position",
        JournalAction::UpdatePosition {
            current_position: body.clone(),
        },
        vec![],
    );
    for (field, entry, expected) in [
        (JournalTextField::Body, Some(id.as_str()), body.as_str()),
        (
            JournalTextField::Ongoing,
            Some(id.as_str()),
            ongoing.as_str(),
        ),
        (JournalTextField::CurrentPosition, None, body.as_str()),
    ] {
        let mut offset = 0;
        let mut token = None::<String>;
        let mut reconstructed = String::new();
        loop {
            let page = fixture
                .store()
                .read_journal_text(
                    &fixture.host,
                    &fixture.journal_id,
                    entry,
                    field,
                    token.as_deref(),
                    offset,
                    257,
                )
                .unwrap();
            assert!(page.text.len() <= 257);
            assert!(!page.text.is_empty());
            assert!(serde_json::to_vec(&page).unwrap().len() < MAX_JOURNAL_READ_BYTES);
            if let Some(token) = &token {
                assert_eq!(&page.content_token, token);
            }
            reconstructed.push_str(&page.text);
            token = Some(page.content_token);
            match page.next_byte {
                Some(next) => {
                    assert!(next > offset);
                    assert!(expected.is_char_boundary(next));
                    offset = next;
                }
                None => break,
            }
        }
        assert_eq!(reconstructed, expected);
    }
    let first = fixture
        .store()
        .read_journal_text(
            &fixture.host,
            &fixture.journal_id,
            Some(&id),
            JournalTextField::Body,
            None,
            0,
            257,
        )
        .unwrap();
    let position = fixture
        .store()
        .read_journal_text(
            &fixture.host,
            &fixture.journal_id,
            None,
            JournalTextField::CurrentPosition,
            None,
            0,
            257,
        )
        .unwrap();
    assert!(matches!(
        fixture.store().read_journal_text(
            &fixture.host,
            &fixture.journal_id,
            Some(&id),
            JournalTextField::Body,
            None,
            first.next_byte.unwrap(),
            257
        ),
        Err(JournalError::Conflict(_))
    ));
    assert!(matches!(
        fixture.store().read_journal_text(
            &fixture.host,
            &fixture.journal_id,
            Some(&id),
            JournalTextField::Body,
            Some(&first.content_token),
            1,
            257
        ),
        Err(JournalError::InvalidInput(_))
    ));
    fixture.execute(
        &ctx,
        "rewrite",
        JournalAction::Rewrite {
            entry_id: id.clone(),
            title: "Changed".into(),
            body: "Different body".into(),
            ongoing: "Changed ongoing".into(),
        },
        vec![],
    );
    assert!(matches!(
        fixture.store().read_journal_text(
            &fixture.host,
            &fixture.journal_id,
            Some(&id),
            JournalTextField::Body,
            Some(&first.content_token),
            first.next_byte.unwrap(),
            257
        ),
        Err(JournalError::Conflict(_))
    ));
    fixture.execute(
        &ctx,
        "position-rewrite",
        JournalAction::UpdatePosition {
            current_position: "Different position".into(),
        },
        vec![],
    );
    assert!(matches!(
        fixture.store().read_journal_text(
            &fixture.host,
            &fixture.journal_id,
            None,
            JournalTextField::CurrentPosition,
            Some(&position.content_token),
            position.next_byte.unwrap(),
            257
        ),
        Err(JournalError::Conflict(_))
    ));
    for invalid_limit in [0, 3, MAX_JOURNAL_READ_BYTES + 1] {
        assert!(matches!(
            fixture.store().read_journal_text(
                &fixture.host,
                &fixture.journal_id,
                Some(&id),
                JournalTextField::Body,
                None,
                0,
                invalid_limit
            ),
            Err(JournalError::InvalidInput(_))
        ));
    }
    let summary = &fixture.entries()[0];
    let json = serde_json::to_value(summary).unwrap();
    assert!(json.get("body").is_none());
    assert!(json.get("ongoing").is_none());
}

#[test]
fn metadata_and_operation_keysets_do_not_skip_appends_or_sort_by_request_id() {
    let fixture = Fixture::with_sizes(&[100, 100, 100]);
    let (_, ctx) = fixture.generation("metadata-pages", fixture.ranges.clone());
    fixture.execute(
        &ctx,
        "z-request",
        fixed_action("First", &fixture.ranges[0], vec![]),
        vec![],
    );
    fixture.execute(
        &ctx,
        "a-request",
        fixed_action("Second", &fixture.ranges[1], vec![]),
        vec![],
    );
    let first = fixture
        .store()
        .list_journal_entries(&fixture.host, &fixture.journal_id, None, 1)
        .unwrap();
    let receipts = fixture
        .store()
        .list_journal_operations(&fixture.host, &fixture.journal_id, None, 1)
        .unwrap();
    assert_eq!(first.items[0].position, 0);
    assert_eq!(receipts.items[0].request_id, "z-request");
    fixture.execute(
        &ctx,
        "m-request",
        fixed_action("Appended between pages", &fixture.ranges[2], vec![]),
        vec![],
    );
    let second = fixture
        .store()
        .list_journal_entries(
            &fixture.host,
            &fixture.journal_id,
            Some(first.next.unwrap().parse().unwrap()),
            1,
        )
        .unwrap();
    let third = fixture
        .store()
        .list_journal_entries(
            &fixture.host,
            &fixture.journal_id,
            Some(second.next.unwrap().parse().unwrap()),
            1,
        )
        .unwrap();
    assert_eq!(second.items[0].position, 1);
    assert_eq!(third.items[0].position, 2);
    assert!(third.next.is_none());
    let second_receipt = fixture
        .store()
        .list_journal_operations(
            &fixture.host,
            &fixture.journal_id,
            receipts.next.as_deref(),
            1,
        )
        .unwrap();
    let third_receipt = fixture
        .store()
        .list_journal_operations(
            &fixture.host,
            &fixture.journal_id,
            second_receipt.next.as_deref(),
            1,
        )
        .unwrap();
    assert_eq!(second_receipt.items[0].request_id, "a-request");
    assert_eq!(third_receipt.items[0].request_id, "m-request");
    assert!(third_receipt.next.is_none());
    let source_page = fixture
        .store()
        .list_journal_sources(&fixture.host, &fixture.journal_id, None, 1)
        .unwrap();
    let next_source = fixture
        .store()
        .list_journal_sources(
            &fixture.host,
            &fixture.journal_id,
            source_page.next.as_deref(),
            2,
        )
        .unwrap();
    assert_eq!(
        next_source
            .items
            .iter()
            .map(|s| s.source.order)
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
    for limit in [0, MAX_JOURNAL_PAGE + 1] {
        assert!(matches!(
            fixture
                .store()
                .list_journal_entries(&fixture.host, &fixture.journal_id, None, limit),
            Err(JournalError::InvalidInput(_))
        ));
        assert!(matches!(
            fixture.store().list_journal_operations(
                &fixture.host,
                &fixture.journal_id,
                None,
                limit
            ),
            Err(JournalError::InvalidInput(_))
        ));
        assert!(matches!(
            fixture
                .store()
                .list_journal_sources(&fixture.host, &fixture.journal_id, None, limit),
            Err(JournalError::InvalidInput(_))
        ));
        assert!(matches!(
            fixture.store().list_journals(&fixture.host, None, limit),
            Err(JournalError::InvalidInput(_))
        ));
    }
}

#[test]
fn journal_metadata_pages_are_subject_scoped_and_reader_context_needs_no_session() {
    let fixture = Fixture::new();
    let later_host = session_host(
        fixture.store(),
        &fixture.host.subject_id,
        "metadata-later-session",
    );
    let later_source = source_range("metadata-later-session", 0, 100);
    let later_id = fixture
        .store()
        .accept_journal_sources(&later_host, "Later", &[later_source])
        .unwrap();
    let foreign_subject = fixture
        .store()
        .create_subject(SubjectRole::new("foreign").unwrap())
        .unwrap();
    let foreign_host = session_host(
        fixture.store(),
        &foreign_subject.id,
        "metadata-foreign-session",
    );
    let foreign_source = source_range("metadata-foreign-session", 0, 100);
    let foreign_id = fixture
        .store()
        .accept_journal_sources(&foreign_host, "Not visible", &[foreign_source])
        .unwrap();
    let reader =
        JournalHostContext::validated_reader(fixture.store(), &fixture.host.subject_id).unwrap();
    let first = fixture.store().list_journals(&reader, None, 1).unwrap();
    assert_eq!(first.items.len(), 1);
    let second = fixture
        .store()
        .list_journals(&reader, first.next.as_deref(), 1)
        .unwrap();
    assert_eq!(second.items.len(), 1);
    assert!(second.next.is_none());
    let mut expected = vec![fixture.journal_id.clone(), later_id];
    expected.sort();
    assert_eq!(
        vec![first.items[0].id.clone(), second.items[0].id.clone()],
        expected
    );
    assert!(
        first
            .items
            .iter()
            .chain(&second.items)
            .all(|item| item.subject_id == fixture.host.subject_id)
    );
    assert!(matches!(
        fixture.store().read_journal(&reader, &foreign_id),
        Err(JournalError::NotFound(_))
    ));
    let json = serde_json::to_value(&first.items[0]).unwrap();
    assert!(json.get("current_position").is_none());
    assert!(json.get("entries").is_none());
    assert!(json.get("body").is_none());
}

#[test]
fn backup_restore_preserves_generation_records_corrections_sources_position_and_receipts() {
    let mut fixture = Fixture::new();
    let (generation, ctx) = fixture.generation("backup", fixture.ranges.clone());
    let first = fixture
        .execute(
            &ctx,
            "fixed",
            fixed_action("Durable outcome", &fixture.ranges[0], vec![]),
            vec![fixture.ranges[0].clone()],
        )
        .entry_id
        .unwrap();
    let mut correction = open_action("Further qualification is ongoing");
    if let JournalAction::Create {
        correction_refs, ..
    } = &mut correction
    {
        correction_refs.push(first);
    }
    fixture.execute(
        &ctx,
        "correction",
        correction,
        vec![fixture.ranges[1].clone()],
    );
    fixture.execute(
        &ctx,
        "position",
        JournalAction::UpdatePosition {
            current_position: "Correction awaits verification".into(),
        },
        vec![],
    );
    fixture.execute(
        &ctx,
        "checkpoint",
        checkpoint(&["fixed"]),
        vec![fixture.ranges[0].clone()],
    );
    fixture
        .store()
        .mark_journal_sources_unavailable(
            &fixture.host,
            &fixture.journal_id,
            &[fragment(&fixture.ranges[0], 0, 10)],
        )
        .unwrap();
    let expected_state = fixture.state();
    let expected_entries = fixture.entries();
    let expected_sources = fixture.sources();
    let expected_receipts = fixture
        .store()
        .list_journal_operations(&fixture.host, &fixture.journal_id, None, 32)
        .unwrap();
    let snapshot = fixture.temp.path().join("snapshot");
    fixture
        .workspace
        .as_ref()
        .unwrap()
        .backup(&snapshot)
        .unwrap();
    fixture.execute(
        &ctx,
        "after-backup",
        JournalAction::UpdatePosition {
            current_position: "Not part of snapshot".into(),
        },
        vec![],
    );
    fixture.close(&generation);
    fixture.workspace.as_ref().unwrap().delete().unwrap();
    fixture.store.take();
    fixture.workspace.take();
    fixture.manager.take().unwrap().shutdown().unwrap();
    let manager = FeatureStorage::new(fixture.temp.path().join("storage"));
    let workspace = manager.workspace("journal-tests").unwrap();
    workspace.restore(&snapshot).unwrap();
    let registration = SubjektivStore::register(&workspace).unwrap();
    fixture.store = Some(SubjektivStore::open(&workspace, &registration).unwrap());
    fixture.workspace = Some(workspace);
    fixture.manager = Some(manager);
    assert_eq!(fixture.state(), expected_state);
    assert_eq!(fixture.entries(), expected_entries);
    assert_eq!(fixture.sources(), expected_sources);
    assert_eq!(
        fixture
            .store()
            .read_journal_generation(&fixture.host, &generation.id)
            .unwrap(),
        generation
    );
    assert_eq!(
        fixture
            .store()
            .list_journal_operations(&fixture.host, &fixture.journal_id, None, 32)
            .unwrap(),
        expected_receipts
    );
    assert!(fixture.receipt("after-backup").is_none());
    assert!(
        JournalJobContext::validated(
            fixture.store(),
            &fixture.host,
            &generation.id,
            generation.job
        )
        .is_ok()
    );
    fixture.reopen();
    assert_eq!(fixture.state(), expected_state);
    assert_eq!(fixture.entries(), expected_entries);
}

#[test]
fn migrated_schema_rejects_null_fixed_order_open_tail_mutation_and_receipt_tampering() {
    let mut fixture = Fixture::with_schema(&[256, 256], true);
    let (_, ctx) = fixture.generation("schema", fixture.ranges.clone());
    let fixed_id = fixture
        .execute(
            &ctx,
            "fixed",
            fixed_action("Immutable record", &fixture.ranges[0], vec![]),
            vec![],
        )
        .entry_id
        .unwrap();
    let open_id = fixture
        .execute(&ctx, "open", open_action("Mutable tail"), vec![])
        .entry_id
        .unwrap();
    let foreign_subject = fixture
        .store()
        .create_subject(SubjectRole::new("foreign").unwrap())
        .unwrap();
    let foreign_host = session_host(
        fixture.store(),
        &foreign_subject.id,
        "schema-foreign-session",
    );
    let foreign_source = source_range("schema-foreign-session", 0, 100);
    let foreign_journal = fixture
        .store()
        .accept_journal_sources(&foreign_host, "Foreign journal", &[foreign_source])
        .unwrap();
    let before = fixture.state();
    let entries = fixture.entries();
    let journal = &fixture.journal_id;
    let subject = &fixture.host.subject_id;
    let invalid_sql = [
        format!(
            "INSERT INTO journal_entries(id,subject_id,journal_id,position,state,title,body,ongoing,created_at,updated_at) VALUES('foreign-owner','{subject}','{foreign_journal}',0,'open','','','','now','now')"
        ),
        format!("UPDATE journal_entries SET body='changed' WHERE id='{fixed_id}'"),
        format!("UPDATE journal_entries SET position=10 WHERE id='{open_id}'"),
        format!("UPDATE journal_entries SET subject_id='missing-subject' WHERE id='{open_id}'"),
        format!(
            "UPDATE journal_entries SET state='fixed', source_end_json='{{}}', source_order=NULL, fixed_at='now', ongoing='' WHERE id='{open_id}'"
        ),
        format!(
            "UPDATE journal_entries SET state='fixed', source_end_json='{{}}', source_order=-1, fixed_at='now', ongoing='' WHERE id='{open_id}'"
        ),
        format!(
            "UPDATE journal_entries SET state='fixed', source_end_json='{{}}', source_order=1, fixed_at='now', ongoing='still open' WHERE id='{open_id}'"
        ),
        format!("UPDATE journal_entries SET source_end_json='{{}}' WHERE id='{open_id}'"),
        format!(
            "INSERT INTO journal_entries(id,subject_id,journal_id,position,state,title,body,ongoing,created_at,updated_at) VALUES('second-open','{subject}','{journal}',2,'open','','','','now','now')"
        ),
        format!(
            "INSERT INTO journal_entries(id,subject_id,journal_id,position,state,title,body,ongoing,source_end_json,source_order,created_at,updated_at,fixed_at) VALUES('after-open','{subject}','{journal}',2,'fixed','','','','{{}}',1,'now','now','now')"
        ),
        format!(
            "UPDATE journal_operations SET receipt_json='{{}}' WHERE journal_id='{journal}' AND request_id='fixed'"
        ),
        format!(
            "DELETE FROM journal_operations WHERE journal_id='{journal}' AND request_id='fixed'"
        ),
        format!("DELETE FROM subjects WHERE subject_id='{subject}'"),
        format!(
            "INSERT INTO journal_corrections(subject_id,entry_id,target_id) VALUES('{subject}','{open_id}','{open_id}')"
        ),
    ];
    for sql in invalid_sql {
        assert!(
            matches!(fixture.sql(&sql), Err(JournalError::Storage(_))),
            "schema accepted {sql}"
        );
        assert_eq!(fixture.state(), before);
        assert_eq!(fixture.entries(), entries);
    }
    fixture.reopen();
    assert_eq!(fixture.state(), before);
    assert_eq!(fixture.entries(), entries);
    assert!(fixture.receipt("fixed").is_some());
    assert!(fixture.receipt("open").is_some());
}

#[test]
fn failure_before_receipt_rolls_back_record_and_checkpoint_then_exact_retry_succeeds() {
    let mut fixture = Fixture::new();
    let (_, ctx) = fixture.generation("transaction-fault", fixture.ranges.clone());
    fixture.sql("CREATE TRIGGER fail_test_receipt BEFORE INSERT ON journal_operations WHEN NEW.request_id IN ('fault-record','fault-checkpoint') BEGIN SELECT RAISE(ABORT, 'injected before receipt'); END;").unwrap();
    let record = fixture.operation(
        "fault-record",
        open_action("Must commit with its receipt"),
        fixture.ranges.clone(),
    );
    let before = fixture.state();
    let sources = fixture.sources();
    assert!(matches!(
        fixture.store().execute_journal(&ctx, record.clone()),
        Err(JournalError::Storage(_))
    ));
    fixture.reopen();
    assert_eq!(fixture.state(), before);
    assert_eq!(fixture.sources(), sources);
    assert!(fixture.entries().is_empty());
    assert!(fixture.receipt("fault-record").is_none());
    fixture.sql("DROP TRIGGER fail_test_receipt;").unwrap();
    let saved = fixture
        .store()
        .execute_journal(&ctx, record.clone())
        .unwrap();
    assert_eq!(
        fixture.store().execute_journal(&ctx, record).unwrap(),
        saved
    );
    save_position(fixture.store(), &ctx, "fault-position");
    let cp = fixture.operation(
        "fault-checkpoint",
        checkpoint(&["fault-record"]),
        fixture.ranges.clone(),
    );
    fixture.sql("CREATE TRIGGER fail_test_receipt BEFORE INSERT ON journal_operations WHEN NEW.request_id='fault-checkpoint' BEGIN SELECT RAISE(ABORT, 'injected before receipt'); END;").unwrap();
    let before_checkpoint = fixture.state();
    assert!(matches!(
        fixture.store().execute_journal(&ctx, cp.clone()),
        Err(JournalError::Storage(_))
    ));
    fixture.reopen();
    assert_eq!(fixture.state(), before_checkpoint);
    assert_eq!(fixture.sources(), sources);
    assert_eq!(fixture.receipt("fault-record"), Some(saved));
    assert!(fixture.receipt("fault-checkpoint").is_none());
    fixture.sql("DROP TRIGGER fail_test_receipt;").unwrap();
    let reflected = fixture.store().execute_journal(&ctx, cp.clone()).unwrap();
    assert_eq!((reflected.process_order, reflected.process_byte), (2, 0));
    fixture.reopen();
    assert_eq!(
        fixture.store().execute_journal(&ctx, cp).unwrap(),
        reflected
    );
    assert_eq!(fixture.entries().len(), 1);
    assert!(
        fixture
            .sources()
            .iter()
            .all(|source| source.reflected_bytes == source.source.total_bytes)
    );
}

#[test]
fn per_operation_reopen_recovers_a_realistic_investigation_and_later_correction_without_job_result()
{
    let mut fixture = Fixture::new();
    let first_range = fixture.ranges[0].clone();
    let second_range = fixture.ranges[1].clone();
    let (first_generation, first_ctx) =
        fixture.generation("investigation-job", vec![first_range.clone()]);
    let created = fixture.execute_and_reopen(
        &first_ctx,
        "investigation-create",
        open_action("Observed a gap in checkpoint evidence; reproducing it"),
        vec![first_range.clone()],
    );
    let first_id = created.entry_id.unwrap();
    fixture.execute_and_reopen(
        &first_ctx,
        "investigation-rewrite",
        JournalAction::Rewrite {
            entry_id: first_id.clone(),
            title: "Checkpoint evidence verified".into(),
            body: "Position-only updates cannot prove source reflection; record receipts can."
                .into(),
            ongoing: "Verify reopen before fixing the outcome".into(),
        },
        vec![first_range.clone()],
    );
    fixture.execute_and_reopen(
        &first_ctx,
        "investigation-position",
        JournalAction::UpdatePosition {
            current_position: "Record saved; checkpoint not yet committed".into(),
        },
        vec![],
    );
    assert_eq!(fixture.state().0.process_order, 0);
    fixture.execute_and_reopen(
        &first_ctx,
        "investigation-fix",
        JournalAction::Fix {
            entry_id: first_id.clone(),
            source_end: first_range.source.pointer.clone(),
        },
        vec![],
    );
    assert!(fixture.state().1.is_none());
    assert_eq!(fixture.state().0.process_order, 0);
    let fixed_before_correction = fixture.entries()[0].clone();
    save_position(fixture.store(), &first_ctx, "investigation-fixed-position");
    fixture.execute_and_reopen(
        &first_ctx,
        "investigation-checkpoint",
        checkpoint(&["investigation-rewrite"]),
        vec![first_range.clone()],
    );
    fixture.close(&first_generation);
    fixture.reopen();
    let (second_generation, second_ctx) =
        fixture.generation("followup-job", vec![second_range.clone()]);
    assert_eq!(
        second_generation.previous_fixed_end,
        Some(first_range.source.pointer)
    );
    assert_eq!(second_generation.initial_journal.process_order, 1);
    assert_eq!(
        second_generation.initial_journal.current_position,
        "Record saved; checkpoint not yet committed"
    );
    assert!(second_generation.initial_open.is_none());
    let mut correction =
        open_action("Clarify that explicit no-change receipts also count as evidence");
    if let JournalAction::Create {
        correction_refs, ..
    } = &mut correction
    {
        correction_refs.push(first_id);
    }
    let correction_id = fixture
        .execute_and_reopen(
            &second_ctx,
            "followup-create",
            correction,
            vec![second_range.clone()],
        )
        .entry_id
        .unwrap();
    fixture.execute_and_reopen(
        &second_ctx,
        "followup-fix",
        JournalAction::Fix {
            entry_id: correction_id,
            source_end: second_range.source.pointer.clone(),
        },
        vec![],
    );
    fixture.execute_and_reopen(
        &second_ctx,
        "followup-confirm",
        JournalAction::ConfirmUnchanged,
        vec![second_range.clone()],
    );
    fixture.execute_and_reopen(
        &second_ctx,
        "followup-position",
        JournalAction::UpdatePosition {
            current_position: "Follow-up recorded; all admitted input has been checked".into(),
        },
        vec![],
    );
    fixture.execute_and_reopen(
        &second_ctx,
        "followup-checkpoint",
        checkpoint(&["followup-create", "followup-confirm"]),
        vec![second_range],
    );
    fixture.close(&second_generation);
    fixture.reopen();
    let entries = fixture.entries();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0], fixed_before_correction);
    assert_eq!(entries[1].correction_refs, vec![entries[0].id.clone()]);
    assert!(
        entries
            .iter()
            .all(|entry| entry.state == JournalEntryState::Fixed)
    );
    assert_eq!(
        (
            fixture.state().0.process_order,
            fixture.state().0.process_byte
        ),
        (2, 0)
    );
    assert!(fixture.state().1.is_none());
}

#[test]
fn split_input_and_replacement_job_reconcile_old_record_coverage_without_duplicate_create() {
    let mut fixture = Fixture::with_sizes(&[8]);
    let whole = fixture.ranges[0].clone();
    let (generation, ctx) = fixture.generation(
        "split-job",
        vec![fragment(&whole, 0, 4), fragment(&whole, 4, 8)],
    );
    let saved = fixture.execute(
        &ctx,
        "record",
        fixed_action("Confirmed from both UTF-8 input fragments", &whole, vec![]),
        vec![whole.clone()],
    );
    fixture.close(&generation);
    fixture.reopen();
    assert!(
        fixture
            .store()
            .active_journal_generation(&fixture.host, &fixture.journal_id)
            .unwrap()
            .is_none()
    );
    let status = fixture
        .store()
        .read_journal_operation_status(&fixture.host, &fixture.journal_id, "record")
        .unwrap();
    assert_eq!(status.kind, JournalOperationKind::Create);
    assert_eq!(status.receipt, saved);
    assert_eq!(status.covered_range_count, 1);
    let coverage = fixture
        .store()
        .read_journal_operation_coverage(&fixture.host, &fixture.journal_id, "record", 0, 1)
        .unwrap();
    assert_eq!(coverage.items, vec![whole.clone()]);
    let (_, replacement) = fixture.generation("replacement-job", vec![whole.clone()]);
    save_position(fixture.store(), &replacement, "replacement-position");
    let cp = fixture.operation("recover-reflection", checkpoint(&["record"]), vec![whole]);
    let result = fixture
        .store()
        .checkpoint_journal(&replacement, cp)
        .unwrap();
    assert_eq!((result.process_order, result.process_byte), (1, 0));
    assert_eq!(fixture.entries().len(), 1);
    assert_eq!(fixture.entries()[0].id, saved.entry_id.unwrap());
    assert!(fixture.state().1.is_none());
}

#[test]
fn serialized_metadata_and_text_budgets_preserve_continuation_and_availability_recovery() {
    let fixture = Fixture::with_sizes(&[8; 64]);
    let (_, ctx) = fixture.generation("large-pages", fixture.ranges.clone());
    for (i, r) in fixture.ranges.iter().enumerate() {
        let action = JournalAction::Create {
            title: "\u{0001}".repeat(1024),
            body: "\u{0001}".repeat(65536),
            ongoing: String::new(),
            state: JournalEntryState::Fixed,
            source_end: Some(r.source.pointer.clone()),
            correction_refs: vec![],
        };
        fixture.execute(&ctx, &format!("record-{i}"), action, vec![r.clone()]);
    }
    let mut after = None;
    let mut ids = Vec::new();
    loop {
        let page = fixture
            .store()
            .list_journal_entries(&fixture.host, &fixture.journal_id, after, 32)
            .unwrap();
        assert!(serde_json::to_string_pretty(&page).unwrap().len() <= MAX_JOURNAL_RESPONSE_BYTES);
        assert!(!page.items.is_empty());
        ids.extend(page.items.iter().map(|e| e.id.clone()));
        let Some(next) = page.next else { break };
        after = Some(next.parse().unwrap());
    }
    assert_eq!(ids.len(), 64);
    assert_eq!(
        ids.iter().collect::<std::collections::HashSet<_>>().len(),
        64
    );
    let mut offset = 0;
    let mut token = None;
    let mut body = String::new();
    loop {
        let page = fixture
            .store()
            .read_journal_text(
                &fixture.host,
                &fixture.journal_id,
                Some(&ids[0]),
                JournalTextField::Body,
                token.as_deref(),
                offset,
                MAX_JOURNAL_READ_BYTES,
            )
            .unwrap();
        assert!(serde_json::to_string_pretty(&page).unwrap().len() <= MAX_JOURNAL_RESPONSE_BYTES);
        body.push_str(&page.text);
        token = Some(page.content_token);
        let Some(next) = page.next_byte else { break };
        assert!(next > offset);
        offset = next;
    }
    assert_eq!(body, "\u{0001}".repeat(65536));
    fixture
        .store()
        .mark_journal_sources_unavailable(&fixture.host, &fixture.journal_id, &fixture.ranges[..1])
        .unwrap();
    assert!(!fixture.entries()[0].source_available);
    fixture
        .store()
        .mark_journal_sources_available(&fixture.host, &fixture.journal_id, &fixture.ranges[..1])
        .unwrap();
    assert!(fixture.entries()[0].source_available);
    assert_eq!(fixture.state().0.process_order, 0);
}

#[test]
fn journal_migration_failure_rolls_back_ddl_and_retries_without_changing_existing_subject() {
    for (baseline, collision, rolled_back) in [
        (&crate::MIGRATIONS[..7], "journal_entries", "journals"),
        (
            &crate::MIGRATIONS[..9],
            "journal_session_bindings",
            "journal_writer_bindings",
        ),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("storage");
        let manager = FeatureStorage::new(&root);
        let scope = manager.scope("migration-rollback").unwrap();
        let registration = scope
            .register(feature_storage::FeatureRegistration::new(
                crate::SUBJEKTIV_FEATURE_ID,
                baseline,
            ))
            .unwrap();
        let store = SubjektivStore::open(&scope, &registration).unwrap();
        let subject = store
            .create_subject_with_behavior(
                SubjectRole::new("preserved").unwrap(),
                "Original behavior".into(),
            )
            .unwrap();
        store
            .database
            .try_transaction::<_, JournalError>(|tx| {
                tx.execute_batch(&format!("CREATE TABLE {collision}(injected INTEGER)"))?;
                Ok(())
            })
            .unwrap();
        drop(store);
        drop(scope);
        manager.shutdown().unwrap();
        drop(manager);
        let manager = FeatureStorage::new(&root);
        let scope = manager.scope("migration-rollback").unwrap();
        let registration = SubjektivStore::register(&scope).unwrap();
        assert!(matches!(
            SubjektivStore::open(&scope, &registration),
            Err(crate::SubjektivError::Storage(
                feature_storage::FeatureStorageError::MigrationFailed { .. }
            ))
        ));
        drop(scope);
        manager.shutdown().unwrap();
        drop(manager);
        let manager = FeatureStorage::new(&root);
        let scope = manager.scope("migration-rollback").unwrap();
        let registration = scope
            .register(feature_storage::FeatureRegistration::new(
                crate::SUBJEKTIV_FEATURE_ID,
                baseline,
            ))
            .unwrap();
        let store = SubjektivStore::open(&scope, &registration).unwrap();
        assert_eq!(store.subject(&subject.id).unwrap(), Some(subject.clone()));
        store
            .database
            .try_transaction::<_, JournalError>(|tx| {
                assert!(!tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name=?1)",
                    [rolled_back],
                    |r| r.get::<_, bool>(0)
                )?);
                tx.execute_batch(&format!("DROP TABLE {collision}"))?;
                Ok(())
            })
            .unwrap();
        drop(store);
        drop(scope);
        manager.shutdown().unwrap();
        drop(manager);
        let manager = FeatureStorage::new(&root);
        let scope = manager.scope("migration-rollback").unwrap();
        let registration = SubjektivStore::register(&scope).unwrap();
        let store = SubjektivStore::open(&scope, &registration).unwrap();
        assert_eq!(store.subject(&subject.id).unwrap(), Some(subject.clone()));
        let host = session_host(&store, &subject.id, "post-migration-session");
        let range = source_range("post-migration-session", 0, 8);
        let id = store
            .accept_journal_sources(&host, "Recovered migration", &[range])
            .unwrap();
        assert_eq!(
            store.read_journal(&host, &id).unwrap().source_session_id,
            "post-migration-session"
        );
    }
}

#[test]
fn late_gap_admission_and_partial_updates_reject_stale_source_page_cursors() {
    let fixture = Fixture::with_sizes(&[8]);
    let second = source_range("source-session", 1, 8);
    let third = source_range("source-session", 2, 8);
    let fourth = source_range("source-session", 3, 8);
    fixture
        .store()
        .accept_journal_sources(&fixture.host, "", &[third, fourth])
        .unwrap();
    let first = fixture
        .store()
        .list_journal_sources(&fixture.host, &fixture.journal_id, None, 2)
        .unwrap();
    assert_eq!(
        first
            .items
            .iter()
            .map(|s| s.source.order)
            .collect::<Vec<_>>(),
        vec![0, 2]
    );
    let cursor = first.next.unwrap();
    fixture
        .store()
        .accept_journal_sources(&fixture.host, "", &[second])
        .unwrap();
    let next =
        fixture
            .store()
            .list_journal_sources(&fixture.host, &fixture.journal_id, Some(&cursor), 2);
    assert!(
        matches!(next, Err(JournalError::Conflict(_))),
        "gap completion must not silently skip newly admitted order 1: {next:?}"
    );
    let fresh = fixture
        .store()
        .list_journal_sources(&fixture.host, &fixture.journal_id, None, 2)
        .unwrap();
    assert_eq!(
        fresh
            .items
            .iter()
            .map(|s| s.source.order)
            .collect::<Vec<_>>(),
        vec![0, 1]
    );
    let next = fixture
        .store()
        .list_journal_sources(&fixture.host, &fixture.journal_id, fresh.next.as_deref(), 2)
        .unwrap();
    assert_eq!(
        next.items
            .iter()
            .map(|s| s.source.order)
            .collect::<Vec<_>>(),
        vec![2, 3]
    );
    // Duplicate notifications do not change the content condition.
    fixture
        .store()
        .accept_journal_sources(&fixture.host, "", &fixture.ranges)
        .unwrap();
    assert!(
        fixture
            .store()
            .list_journal_sources(&fixture.host, &fixture.journal_id, fresh.next.as_deref(), 2)
            .is_ok()
    );
    let selected = fixture.ranges[0].clone();
    let (_, job) = fixture.generation("reflect-during-list", vec![selected.clone()]);
    fixture.execute(
        &job,
        "no-change",
        JournalAction::ConfirmUnchanged,
        vec![selected.clone()],
    );
    assert!(
        fixture
            .store()
            .list_journal_sources(&fixture.host, &fixture.journal_id, fresh.next.as_deref(), 2)
            .is_ok()
    );
    save_position(fixture.store(), &job, "listing-position");
    fixture.execute(
        &job,
        "checkpoint",
        checkpoint(&["no-change"]),
        vec![selected],
    );
    assert!(matches!(
        fixture.store().list_journal_sources(
            &fixture.host,
            &fixture.journal_id,
            fresh.next.as_deref(),
            2
        ),
        Err(JournalError::Conflict(_))
    ));
    assert_eq!(
        fixture
            .store()
            .read_journal_source(&fixture.host, &fixture.journal_id, 0)
            .unwrap()
            .reflected_bytes,
        8
    );

    let host = session_host(fixture.store(), &fixture.host.subject_id, "partial-session");
    let range = source_range("partial-session", 0, 16);
    let later = source_range("partial-session", 1, 8);
    let id = fixture
        .store()
        .accept_journal_sources(&host, "Partial source", &[fragment(&range, 0, 4), later])
        .unwrap();
    let page = fixture
        .store()
        .list_journal_sources(&host, &id, None, 1)
        .unwrap();
    fixture
        .store()
        .accept_journal_sources(&host, "", &[fragment(&range, 4, 8)])
        .unwrap();
    assert!(matches!(
        fixture
            .store()
            .list_journal_sources(&host, &id, page.next.as_deref(), 1),
        Err(JournalError::Conflict(_))
    ));
    let page = fixture
        .store()
        .list_journal_sources(&host, &id, None, 1)
        .unwrap();
    fixture
        .store()
        .mark_journal_sources_unavailable(&host, &id, &[fragment(&range, 0, 8)])
        .unwrap();
    assert!(matches!(
        fixture
            .store()
            .list_journal_sources(&host, &id, page.next.as_deref(), 1),
        Err(JournalError::Conflict(_))
    ));
    let page = fixture
        .store()
        .list_journal_sources(&host, &id, None, 1)
        .unwrap();
    fixture
        .store()
        .mark_journal_sources_available(&host, &id, &[fragment(&range, 0, 4)])
        .unwrap();
    assert!(matches!(
        fixture
            .store()
            .list_journal_sources(&host, &id, page.next.as_deref(), 1),
        Err(JournalError::Conflict(_))
    ));
    let page = fixture
        .store()
        .list_journal_sources(&host, &id, None, 1)
        .unwrap();
    assert!(matches!(
        fixture
            .store()
            .list_journal_sources(&host, &fixture.journal_id, page.next.as_deref(), 1),
        Err(JournalError::InvalidInput(_))
    ));
    let mut cursor: serde_json::Value =
        serde_json::from_str(page.next.as_deref().unwrap()).unwrap();
    cursor["unexpected_field"] = true.into();
    assert!(matches!(
        fixture
            .store()
            .list_journal_sources(&host, &id, Some(&cursor.to_string()), 1),
        Err(JournalError::InvalidInput(_))
    ));
}

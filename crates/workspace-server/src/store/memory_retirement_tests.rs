// Included in store::tests. Real retained schema, ordered migration and isolated
// FeatureStorage prove retirement does not reinterpret Subject content.
mod memory_retirement_contract {
    use super::*;

    const OLD_TABLES: &[&str] = &[
        "workspace_memory_documents",
        "memory_staging_records",
        "memory_staging_resolutions",
        "workspace_memory_settings",
    ];

    fn populate(conn: &Connection, subject: &str) -> (String, String) {
        conn.execute_batch(r#"INSERT INTO accounts(account_id,kind,handle,display_name,created_at,updated_at) VALUES('retirement-owner','user','retirement-owner','Owner','1','1');
            INSERT INTO workspaces(workspace_id,owner_account_id,display_name,state,created_at,updated_at) VALUES('retirement-ws','retirement-owner','Workspace','active','1','1');
            INSERT INTO workspace_memory_settings VALUES('retirement-ws','日本語','1','1');
            INSERT INTO workspace_memory_documents VALUES('retirement-ws','# legacy document','1','1');
            INSERT INTO memory_staging_records VALUES('retirement-ws','candidate-legacy','{"claim":"legacy candidate"}',NULL,'1');
            INSERT INTO memory_staging_resolutions VALUES('retirement-ws','closed-legacy','apply','legacy resolution','[]','{"claim":"legacy copy"}',NULL,'1','2');
            INSERT INTO objectives(workspace_id,objective_id,title,state,body_md,created_at,updated_at) VALUES('retirement-ws','objective-keep','Keep','active','unrelated body','1','1');
            INSERT INTO audit_events(workspace_id,audit_event_id,created_at,actor_kind,actor_key,actor_display,action,target_kind,outcome) VALUES('retirement-ws','audit-keep','1','user','retirement-owner','Owner','memoryXsubject-event','subject','success');"#).unwrap();
        let subject_initialization = FeatureInitializations::new(vec![
            protocol::feature_initialization::FeatureInitialization::encode(
                subjektiv_api::worker_initialization::FEATURE_ID,
                subjektiv_api::worker_initialization::VERSION,
                &subjektiv_api::worker_initialization::ResolvedWorkerConnection {
                    workspace_id: "retirement-ws".into(),
                    subject_id: Some(subject.into()),
                    kind: subjektiv_api::worker_initialization::WorkerConnectionKind::SubjectBody,
                },
            )
            .unwrap(),
        ])
        .unwrap();
        let mut fingerprints = Vec::new();
        for (key, initialization) in [
            ("subject-create", subject_initialization),
            ("ordinary-create", FeatureInitializations::default()),
        ] {
            let json = serde_json::to_string(&initialization).unwrap();
            let mut digest = Sha256::new();
            digest.update(b"workspace-worker-create-v4\0");
            for part in [key, "retirement-ws", "日本語"] {
                digest.update(part.as_bytes());
                digest.update(b"\0");
            }
            digest.update(json.as_bytes());
            let fingerprint = format!(
                "sha256:{}",
                digest
                    .finalize()
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>()
            );
            conn.execute("INSERT INTO worker_create_reservations(workspace_id,allocation_key,worker_id,runtime_id,create_fingerprint,state,created_at,updated_at,request_fingerprint,memory_language,feature_initialization_json) VALUES('retirement-ws',?1,?2,'retirement-runtime',?3,'reserved','1','1',?1,'日本語',?4)",params![key,WorkerId::now_v7().to_string(),fingerprint,json]).unwrap();
            fingerprints.push(fingerprint);
        }
        (fingerprints.remove(0), fingerprints.remove(0))
    }

    #[test]
    fn retirement_deletes_legacy_content_preserves_subject_history_and_frozen_creation() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("workspace.db");
        prepare_retained_schema(&path, 143);
        let storage = feature_storage::FeatureStorage::new(temp.path().join("features"));
        let scope = storage.scope("retirement-ws").unwrap();
        let registration = subjektiv::SubjektivStore::register(&scope).unwrap();
        let subjects = subjektiv::SubjektivStore::open(&scope, &registration).unwrap();
        let subject = subjects
            .create_subject(subjektiv::SubjectRole::new("companion").unwrap())
            .unwrap();
        let memory = subjects
            .create_memory(
                &subject.id,
                subjektiv::MemoryDraft::active(
                    memory::extract::CandidateKind::Lesson,
                    "Keep",
                    "Subject-owned body",
                    "reason",
                    "created",
                ),
            )
            .unwrap();
        let conn = Connection::open(&path).unwrap();
        configure_sqlite(&conn).unwrap();
        let (subject_hash, ordinary_hash) = populate(&conn, &subject.id);
        drop(conn);
        let store = SqliteWorkspaceStore::open_for_test(&path).unwrap();
        store
            .with_conn(|conn| {
                for table in OLD_TABLES {
                    assert!(!table_exists(conn, table)?);
                }
                assert!(!column_exists(
                    conn,
                    "worker_create_reservations",
                    "memory_language"
                )?);
                assert_eq!(
                    read_subjektiv_settings(conn, "retirement-ws")?.language,
                    "日本語"
                );
                assert_eq!(
                    conn.query_row(
                        "SELECT body_md FROM objectives WHERE objective_id='objective-keep'",
                        [],
                        |r| r.get::<_, String>(0)
                    )?,
                    "unrelated body"
                );
                assert_eq!(
                    conn.query_row(
                        "SELECT action FROM audit_events WHERE audit_event_id='audit-keep'",
                        [],
                        |r| r.get::<_, String>(0)
                    )?,
                    "memoryXsubject-event"
                );
                Ok(())
            })
            .unwrap();
        let subject_reservation=store.reserve_worker_create_with_initialization("retirement-ws","retirement-runtime","subject-create","subject-create",None,&FeatureInitializations::new(vec![protocol::feature_initialization::FeatureInitialization::encode(subjektiv_api::worker_initialization::FEATURE_ID,subjektiv_api::worker_initialization::VERSION,&subjektiv_api::worker_initialization::ResolvedWorkerConnection {workspace_id:"retirement-ws".into(),subject_id:Some(subject.id.clone()),kind:subjektiv_api::worker_initialization::WorkerConnectionKind::SubjectBody}).unwrap()]).unwrap()).unwrap();
        assert_eq!(subject_reservation.create_fingerprint, subject_hash);
        assert_eq!(
            subject_reservation
                .subjektiv_memory_settings
                .as_ref()
                .unwrap()
                .language,
            "日本語"
        );
        let ordinary = store
            .reserve_worker_create(
                "retirement-ws",
                "retirement-runtime",
                "ordinary-create",
                "ordinary-create",
                None,
            )
            .unwrap();
        assert_eq!(ordinary.create_fingerprint, ordinary_hash);
        assert!(ordinary.subjektiv_memory_settings.is_none());
        store
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE subjektiv_memory_settings SET language='English'",
                    [],
                )?;
                Ok(())
            })
            .unwrap();
        assert_eq!(
            store
                .reserve_worker_create_with_initialization(
                    "retirement-ws",
                    "retirement-runtime",
                    "subject-create",
                    "subject-create",
                    None,
                    &subject_reservation.feature_initialization
                )
                .unwrap(),
            subject_reservation
        );
        for (column, value) in [
            ("subjektiv_memory_language", "tampered"),
            ("create_fingerprint", "tampered"),
        ] {
            let original = if column == "subjektiv_memory_language" {
                "日本語"
            } else {
                subject_hash.as_str()
            };
            store.with_conn(|conn| {conn.execute(&format!("UPDATE worker_create_reservations SET {column}=?1 WHERE allocation_key='subject-create'"),[value])?;Ok(())}).unwrap();
            assert!(
                store
                    .reserve_worker_create_with_initialization(
                        "retirement-ws",
                        "retirement-runtime",
                        "subject-create",
                        "subject-create",
                        None,
                        &subject_reservation.feature_initialization
                    )
                    .is_err()
            );
            store.with_conn(|conn| {conn.execute(&format!("UPDATE worker_create_reservations SET {column}=?1 WHERE allocation_key='subject-create'"),[original])?;Ok(())}).unwrap();
        }
        let fresh = SqliteWorkspaceStore::in_memory_for_test().unwrap();
        fresh
            .with_conn(|new| {
                for table in OLD_TABLES {
                    assert!(!table_exists(new, table)?);
                }
                store.with_conn(|upgraded| {
                    for table in ["worker_create_reservations", "subjektiv_memory_settings"] {
                        assert_eq!(
                            state_contract_schema_signature(new, table),
                            state_contract_schema_signature(upgraded, table)
                        );
                    }
                    Ok(())
                })
            })
            .unwrap();
        drop(store);
        let reopened = SqliteWorkspaceStore::open_for_test(&path).unwrap();
        reopened
            .with_conn(|conn| {
                for table in OLD_TABLES {
                    assert!(!table_exists(conn, table)?);
                }
                Ok(())
            })
            .unwrap();
        assert_eq!(subjects.subject(&subject.id).unwrap().unwrap(), subject);
        assert_eq!(
            serde_json::to_value(subjects.memory(&subject.id, &memory.id).unwrap().unwrap())
                .unwrap(),
            serde_json::to_value(&memory).unwrap()
        );
        let changes = subjects
            .list_memory_changes(&subject.id, &memory.id)
            .unwrap();
        assert!(!changes.is_empty());
        storage.shutdown().unwrap();
    }

    #[test]
    fn retirement_rolls_back_deletions_and_binding_changes_when_receipt_fails() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("workspace.db");
        prepare_retained_schema(&path, 143);
        let conn = Connection::open(&path).unwrap();
        configure_sqlite(&conn).unwrap();
        populate(&conn, "subject-fixture");
        conn.execute_batch("CREATE TRIGGER reject_retirement BEFORE INSERT ON __yoi_schema_migrations WHEN NEW.version=144 BEGIN SELECT RAISE(ABORT,'injected retirement receipt failure'); END;").unwrap();
        assert!(memory_retirement::migrate(&conn).is_err());
        assert_eq!(current_schema_version(&conn).unwrap(), 143);
        for table in OLD_TABLES {
            assert!(table_exists(&conn, table).unwrap());
        }
        assert!(
            !column_exists(
                &conn,
                "worker_create_reservations",
                "initialization_binding"
            )
            .unwrap()
        );
        assert_eq!(
            conn.query_row("SELECT body_md FROM workspace_memory_documents", [], |r| {
                r.get::<_, String>(0)
            })
            .unwrap(),
            "# legacy document"
        );
        conn.execute_batch("DROP TRIGGER reject_retirement;")
            .unwrap();
        memory_retirement::migrate(&conn).unwrap();
        assert_eq!(current_schema_version(&conn).unwrap(), 144);
    }
}

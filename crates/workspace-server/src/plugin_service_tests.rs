//! Real SQLite consumer/cursor/receipt owner tests. Outbox rows are boundary
//! fixtures here; Ticket's own suite exercises actual domain commits and rollback.
use super::*;
use crate::plugin_plan::*;
use tempfile::TempDir;

fn digest(s: &str) -> String {
    format!(
        "sha256:{}",
        Sha256::digest(s)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    )
}
struct Fixture {
    dir: TempDir,
    plan: StoredPluginPlan,
    now: i64,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let conn = Connection::open(dir.path().join("service.db")).unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE workspaces(workspace_id TEXT PRIMARY KEY); INSERT INTO workspaces VALUES('w');").unwrap();
        conn.execute_batch(include_str!("plugin_catalog_schema.sql"))
            .unwrap();
        conn.execute_batch(crate::plugin_plan::SCHEMA_SQL).unwrap();
        conn.execute_batch(plugin_receipt::SCHEMA_SQL).unwrap();
        conn.execute_batch(ticket::service_outbox::OUTBOX_SCHEMA_SQL)
            .unwrap();
        conn.execute_batch(super::SCHEMA_SQL).unwrap();
        conn.execute(
            "INSERT INTO plugin_installations VALUES('w','i','alias','package','install',0)",
            [],
        )
        .unwrap();
        let artifact = ArtifactIdentityProjection {
            package_id: "package".into(),
            version: "1".into(),
            package_digest: digest("package"),
            artifact_digest: digest("artifact"),
            artifact_size: 1,
            schema_version: manifest::plugin::SUPPORTED_PLUGIN_API_VERSION,
            world: "yoi:plugin/ticket-service-plugin@2.2.0".into(),
            required_imports: vec![],
            tools: vec![],
            services: vec!["watch".into()],
            connections: vec![],
            requested_grants: vec![PluginGrant::TicketEvents],
            config_schema: PluginSchemaIdentity {
                identity: "schema".into(),
                version: "1".into(),
                digest: digest("schema"),
                namespace: "package".into(),
            },
            revoked: false,
        };
        let pin = WorkerPluginPin::from_artifact(
            "i".into(),
            "install".into(),
            &artifact,
            PluginPinProjection {
                config: serde_json::json!({}),
                surfaces: vec![PluginSurface::Service],
                tools: vec![],
                services: vec!["watch".into()],
                grants: vec![PluginGrant::TicketEvents],
                bindings: vec![],
            },
        )
        .unwrap();
        let plan = WorkerPluginExecutionPlan::canonicalize(WorkerPluginPlanInput {
            workspace_id: "w".into(),
            config: ConfigProvenance {
                content_digest: digest("config"),
                toolchain_fingerprint: "toolchain".into(),
            },
            profile: ProfileProvenance {
                identity: "profile".into(),
                projection_digest: digest("profile"),
            },
            execution: HostExecutionIdentity {
                runtime_id: "runtime".into(),
                worker_id: "worker".into(),
                principal_id: "principal".into(),
                delegation_id: None,
            },
            plugins: vec![pin],
        })
        .unwrap();
        let stored = StoredPluginPlan {
            plan_id: "plan".into(),
            fingerprint: plan.fingerprint().unwrap(),
            pin_state: PlanPinState::Active,
            plan,
        };
        conn.execute("INSERT INTO plugin_execution_plans(workspace_id,plan_id,actor,operation_key,fingerprint,plan_json,pin_state) VALUES('w','plan','owner','plan',?1,?2,'active')",params![stored.fingerprint,stored.plan.canonical_json().unwrap()]).unwrap();
        Self {
            dir,
            plan: stored,
            now: chrono::Utc::now().timestamp_millis(),
        }
    }
    fn conn(&self) -> Connection {
        let c = Connection::open(self.dir.path().join("service.db")).unwrap();
        c.execute_batch("PRAGMA foreign_keys=ON").unwrap();
        c
    }
    fn write<T>(&self, f: impl FnOnce(&Transaction<'_>) -> Result<T>) -> Result<T> {
        let mut c = self.conn();
        let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let result = f(&tx)?;
        tx.commit()?;
        Ok(result)
    }
    fn acquire(&self, now: i64) -> PluginServiceLease {
        self.write(|tx| {
            acquire(
                tx,
                &self.plan,
                "i",
                "watch",
                &[
                    PluginTicketEventType::Changed,
                    PluginTicketEventType::Deleted,
                ],
                now,
            )
        })
        .unwrap()
    }
    fn event(&self, n: u32) -> String {
        let id = format!("{n:032x}");
        let occurred_at = chrono::DateTime::from_timestamp_millis(self.now)
            .unwrap()
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        self.conn().execute("INSERT INTO typed_ticket_service_outbox(event_id,workspace_id,ticket_id,event_type,occurred_at,domain_event_ref,content_digest,correlation_id,causation_id) VALUES(?1,'w','ticket','changed',?4,?2,?3,?1,?2)",params![id,format!("ticket:{n}"),"a".repeat(64),occurred_at]).unwrap();
        id
    }
    fn poll(&self, l: &PluginServiceLease, now: i64) -> PluginServiceResponse {
        self.write(|tx| poll(tx, "w", l, &self.plan, now)).unwrap()
    }
    fn delivery(&self, l: &PluginServiceLease, now: i64) -> (PluginTicketEventEnvelope, String) {
        match self.poll(l, now) {
            PluginServiceResponse::Delivery {
                envelope,
                invocation_id,
                ..
            } => (envelope, invocation_id),
            other => panic!("expected delivery: {other:?}"),
        }
    }
    fn released_proofs(&self) -> Vec<(String, String, String, i64)> {
        let conn = self.conn();
        let mut stmt = conn
            .prepare("SELECT workspace_id,consumer_id,lease_token,released_at_ms FROM plugin_service_released_proofs ORDER BY workspace_id,consumer_id,lease_token")
            .unwrap();
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap();
        rows.collect::<rusqlite::Result<Vec<_>>>().unwrap()
    }
    fn pause(&self) -> Option<PluginServicePauseReason> {
        load(&self.conn(), "w", &self.consumer()).unwrap().pause
    }
    fn consumer(&self) -> String {
        self.conn()
            .query_row(
                "SELECT consumer_id FROM plugin_service_consumers",
                [],
                |r| r.get(0),
            )
            .unwrap()
    }
}

#[test]
fn no_backfill_restore_stable_identity_and_new_plan_new_consumer() {
    let f = Fixture::new();
    f.event(1);
    let lease = f.acquire(f.now);
    assert!(matches!(f.poll(&lease, f.now), PluginServiceResponse::Idle));
    let id = f.event(2);
    let (event, inv) = f.delivery(&lease, f.now);
    assert_eq!(event.event_id, id);
    assert!(
        f.write(|tx| acquire(
            tx,
            &f.plan,
            "i",
            "watch",
            &[
                PluginTicketEventType::Changed,
                PluginTicketEventType::Deleted
            ],
            f.now
        ))
        .is_err()
    );
    f.write(|tx| release(tx, "w", &lease, f.now)).unwrap();
    let restored = f.acquire(f.now + 1);
    assert_eq!(lease.consumer_id, restored.consumer_id);
    assert_ne!(lease.lease_token, restored.lease_token);
    assert!(
        f.write(|tx| complete(
            tx,
            "w",
            &lease,
            &id,
            &PluginServiceEventResult::Ack,
            f.now + 1
        ))
        .is_err()
    );
    let (again, again_inv) = f.delivery(&restored, f.now + 30_001);
    assert_eq!(again.event_id, id);
    assert_eq!(again_inv, inv);
    assert_eq!(again.attempt, 2);
    f.write(|tx| {
        complete(
            tx,
            "w",
            &restored,
            &id,
            &PluginServiceEventResult::Ack,
            f.now + 30_001,
        )
    })
    .unwrap();
    f.write(|tx| {
        complete(
            tx,
            "w",
            &restored,
            &id,
            &PluginServiceEventResult::Ack,
            f.now + 30_002,
        )
    })
    .unwrap();
    assert!(matches!(
        f.poll(&restored, f.now + 30_002),
        PluginServiceResponse::Idle
    ));
    let mut replacement = f.plan.clone();
    replacement.plan_id = "replacement".into();
    f.conn().execute("INSERT INTO plugin_execution_plans(workspace_id,plan_id,actor,operation_key,fingerprint,plan_json,pin_state) VALUES('w','replacement','owner','replacement',?1,?2,'active')",params![replacement.fingerprint,replacement.plan.canonical_json().unwrap()]).unwrap();
    let fresh = f
        .write(|tx| {
            acquire(
                tx,
                &replacement,
                "i",
                "watch",
                &[
                    PluginTicketEventType::Changed,
                    PluginTicketEventType::Deleted,
                ],
                f.now,
            )
        })
        .unwrap();
    assert_ne!(fresh.consumer_id, lease.consumer_id);
    assert!(matches!(
        f.write(|tx| poll(tx, "w", &fresh, &replacement, f.now))
            .unwrap(),
        PluginServiceResponse::Idle
    ));
}

#[test]
fn release_replay_after_rotation_preserves_successor_and_original_proof() {
    let f = Fixture::new();
    let old = f.acquire(f.now);
    f.write(|tx| release(tx, "w", &old, f.now)).unwrap();
    let proof = vec![(
        "w".to_owned(),
        old.consumer_id.clone(),
        old.lease_token.clone(),
        f.now,
    )];
    assert_eq!(f.released_proofs(), proof);
    // Reconcile a lost response both before and after a successor takes over.
    f.write(|tx| release(tx, "w", &old, f.now + 1)).unwrap();
    let successor = f.acquire(f.now + 2);
    assert_eq!(old.consumer_id, successor.consumer_id);
    assert_ne!(old.lease_token, successor.lease_token);
    let event = f.event(1);
    f.delivery(&successor, f.now + 2);
    // Any UPDATE (even one writing identical values) would violate replay's
    // no-mutation guarantee. The real release owner must return from its proof.
    f.conn()
        .execute_batch(
            "CREATE TRIGGER forbid_replay_consumer_update BEFORE UPDATE ON plugin_service_consumers
             BEGIN SELECT RAISE(ABORT,'replay mutated successor'); END;",
        )
        .unwrap();
    for now in [f.now + 3, f.now + LEASE_MS + 3] {
        f.write(|tx| release(tx, "w", &old, now)).unwrap();
        let saved = load(&f.conn(), "w", &successor.consumer_id).unwrap();
        assert_eq!(
            saved.lease_token.as_deref(),
            Some(successor.lease_token.as_str())
        );
        assert_eq!(saved.lease_until_ms, f.now + 2 + LEASE_MS);
        assert_eq!(saved.current_event_id.as_deref(), Some(event.as_str()));
        assert_eq!(saved.attempt, 1);
        assert_eq!(f.released_proofs(), proof);
    }
}

#[test]
fn expired_matching_release_clears_lease_but_never_authorizes_work() {
    let f = Fixture::new();
    let lease = f.acquire(f.now);
    let expired = f.now + LEASE_MS;
    assert!(fence(&f.conn(), "w", &lease, expired).is_err());
    f.write(|tx| release(tx, "w", &lease, expired)).unwrap();
    let saved = load(&f.conn(), "w", &lease.consumer_id).unwrap();
    assert!(saved.lease_token.is_none());
    assert_eq!(saved.lease_until_ms, 0);
    assert_eq!(
        f.released_proofs(),
        vec![(
            "w".to_owned(),
            lease.consumer_id.clone(),
            lease.lease_token.clone(),
            expired,
        )]
    );
    assert!(fence(&f.conn(), "w", &lease, expired + 1).is_err());
    f.write(|tx| release(tx, "w", &lease, expired + 1)).unwrap();
    assert!(f.acquire(expired + 2).lease_token != lease.lease_token);
}

#[test]
fn release_wrong_or_unrecorded_stale_token_cannot_clear_successor_or_match_null() {
    let f = Fixture::new();
    let old = f.acquire(f.now);
    let successor = f.acquire(f.now + LEASE_MS);
    let wrong = PluginServiceLease {
        consumer_id: successor.consumer_id.clone(),
        lease_token: "never-issued".into(),
    };
    for lease in [&old, &wrong] {
        let error = f
            .write(|tx| release(tx, "w", lease, f.now + LEASE_MS + 1))
            .unwrap_err();
        assert_eq!(
            crate::plugin_host::map_error(error).code,
            PluginHostErrorCode::Conflict
        );
        let saved = load(&f.conn(), "w", &successor.consumer_id).unwrap();
        assert_eq!(
            saved.lease_token.as_deref(),
            Some(successor.lease_token.as_str())
        );
        assert_eq!(saved.lease_until_ms, f.now + 2 * LEASE_MS);
        assert!(f.released_proofs().is_empty());
    }
    f.write(|tx| release(tx, "w", &successor, f.now + LEASE_MS + 2))
        .unwrap();
    let proof = f.released_proofs();
    for lease in [&old, &wrong] {
        let error = f
            .write(|tx| release(tx, "w", lease, f.now + LEASE_MS + 3))
            .unwrap_err();
        assert_eq!(
            crate::plugin_host::map_error(error).code,
            PluginHostErrorCode::Conflict
        );
        let saved = load(&f.conn(), "w", &successor.consumer_id).unwrap();
        assert!(saved.lease_token.is_none());
        assert_eq!(saved.lease_until_ms, 0);
        assert_eq!(f.released_proofs(), proof);
    }
}

#[test]
fn released_proof_only_reconciles_exact_workspace_consumer_and_token() {
    let f = Fixture::new();
    let old = f.acquire(f.now);
    f.write(|tx| release(tx, "w", &old, f.now)).unwrap();
    let other = f
        .write(|tx| {
            acquire(
                tx,
                &f.plan,
                "i",
                "other-service",
                &[PluginTicketEventType::Changed],
                f.now,
            )
        })
        .unwrap();
    let wrong_consumer = PluginServiceLease {
        consumer_id: other.consumer_id.clone(),
        lease_token: old.lease_token.clone(),
    };
    let proof = f.released_proofs();
    for (workspace, lease) in [("other-workspace", &old), ("w", &wrong_consumer)] {
        let error = f
            .write(|tx| release(tx, workspace, lease, f.now + 1))
            .unwrap_err();
        assert_eq!(
            crate::plugin_host::map_error(error).code,
            PluginHostErrorCode::Conflict
        );
        let saved = load(&f.conn(), "w", &other.consumer_id).unwrap();
        assert_eq!(
            saved.lease_token.as_deref(),
            Some(other.lease_token.as_str())
        );
        assert_eq!(saved.lease_until_ms, f.now + LEASE_MS);
        assert_eq!(f.released_proofs(), proof);
    }
}

#[test]
fn release_rejects_unbounded_or_control_character_tokens_without_mutation() {
    let f = Fixture::new();
    let current = f.acquire(f.now);
    for token in [String::new(), "x".repeat(513), "invalid\nproof".into()] {
        let lease = PluginServiceLease {
            consumer_id: current.consumer_id.clone(),
            lease_token: token,
        };
        let error = f.write(|tx| release(tx, "w", &lease, f.now)).unwrap_err();
        assert_eq!(
            crate::plugin_host::map_error(error).code,
            PluginHostErrorCode::InvalidInput
        );
        assert_eq!(
            load(&f.conn(), "w", &current.consumer_id)
                .unwrap()
                .lease_token,
            Some(current.lease_token.clone())
        );
        assert!(f.released_proofs().is_empty());
    }
}

#[test]
fn release_receipt_insert_failure_rolls_back_exact_clear_and_retry_commits_both() {
    let f = Fixture::new();
    let lease = f.acquire(f.now);
    f.conn()
        .execute_batch(
            "CREATE TRIGGER fail_release_receipt BEFORE INSERT ON plugin_service_released_proofs
             BEGIN SELECT RAISE(ABORT,'injected receipt failure'); END;",
        )
        .unwrap();
    assert!(f.write(|tx| release(tx, "w", &lease, f.now + 1)).is_err());
    let saved = load(&f.conn(), "w", &lease.consumer_id).unwrap();
    assert_eq!(
        saved.lease_token.as_deref(),
        Some(lease.lease_token.as_str())
    );
    assert_eq!(saved.lease_until_ms, f.now + LEASE_MS);
    assert!(f.released_proofs().is_empty());
    f.conn()
        .execute_batch("DROP TRIGGER fail_release_receipt")
        .unwrap();
    // Failure after both writes must also roll back the proof and clear together.
    assert!(
        f.write(|tx| {
            release(tx, "w", &lease, f.now + 2)?;
            Err::<(), _>(failure(PluginHostErrorCode::Unavailable))
        })
        .is_err()
    );
    let saved = load(&f.conn(), "w", &lease.consumer_id).unwrap();
    assert_eq!(
        saved.lease_token.as_deref(),
        Some(lease.lease_token.as_str())
    );
    assert_eq!(saved.lease_until_ms, f.now + LEASE_MS);
    assert!(f.released_proofs().is_empty());
    f.write(|tx| release(tx, "w", &lease, f.now + 3)).unwrap();
    let saved = load(&f.conn(), "w", &lease.consumer_id).unwrap();
    assert!(saved.lease_token.is_none());
    assert_eq!(saved.lease_until_ms, 0);
    assert_eq!(
        f.released_proofs(),
        vec![("w".into(), lease.consumer_id, lease.lease_token, f.now + 3)]
    );
}

#[test]
fn released_proof_is_immutable_and_retained_until_consumer_deletion() {
    let f = Fixture::new();
    let lease = f.acquire(f.now);
    f.write(|tx| release(tx, "w", &lease, f.now)).unwrap();
    let proof = f.released_proofs();
    assert!(
        f.conn()
            .execute(
                "UPDATE plugin_service_released_proofs SET released_at_ms=released_at_ms+1",
                [],
            )
            .is_err()
    );
    assert!(
        f.conn()
            .execute("DELETE FROM plugin_service_released_proofs", [])
            .is_err()
    );
    assert!(
        f.conn()
            .execute(
                "INSERT INTO plugin_service_released_proofs VALUES('w','missing','token',0)",
                [],
            )
            .is_err()
    );
    assert_eq!(f.released_proofs(), proof);
    f.conn()
        .execute(
            "DELETE FROM plugin_service_consumers WHERE workspace_id='w' AND consumer_id=?1",
            [&lease.consumer_id],
        )
        .unwrap();
    assert!(f.released_proofs().is_empty());
}

#[test]
fn unknown_pauses_without_advancing_cursor() {
    let f = Fixture::new();
    let lease = f.acquire(f.now);
    f.event(1);
    let (event, id) = f.delivery(&lease, f.now);
    let mut record = plugin_receipt::lookup(&f.conn(), "w", &id).unwrap();
    record.service_lease = Some(lease.clone());
    f.write(|tx| plugin_receipt::effect_admit(tx, &record, "effect", &digest("request"), f.now))
        .unwrap();
    // Host must not turn even a guest ack into cursor success with unknown effects.
    f.write(|tx| {
        complete(
            tx,
            "w",
            &lease,
            &event.event_id,
            &PluginServiceEventResult::Ack,
            f.now,
        )
    })
    .unwrap();
    assert_eq!(f.pause(), Some(PluginServicePauseReason::Indeterminate));
    assert_eq!(load(&f.conn(), "w", &lease.consumer_id).unwrap().cursor, 0);
    assert!(
        f.write(|tx| plugin_receipt::effect_complete(
            tx,
            &record,
            "effect",
            &digest("request"),
            &Ok(PluginHttpResponse {
                status: 200,
                headers: vec![],
                body: vec![]
            })
        ))
        .is_err()
    );
}

#[test]
fn receipt_admission_and_completion_reject_replaced_lease_in_same_transaction() {
    let f = Fixture::new();
    let lease = f.acquire(f.now);
    f.event(1);
    let (_, id) = f.delivery(&lease, f.now);
    let mut old = plugin_receipt::lookup(&f.conn(), "w", &id).unwrap();
    old.service_lease = Some(lease.clone());
    assert!(matches!(
        f.write(|tx| plugin_receipt::effect_admit(tx, &old, "effect", &digest("request"), f.now))
            .unwrap(),
        plugin_receipt::EffectAdmission::Dispatch
    ));
    f.write(|tx| release(tx, "w", &lease, f.now)).unwrap();
    let new = f.acquire(f.now + 1);
    assert!(
        f.write(|tx| plugin_receipt::effect_admit(tx, &old, "other", &digest("other"), f.now + 1))
            .is_err()
    );
    assert!(
        f.write(|tx| plugin_receipt::effect_complete(
            tx,
            &old,
            "effect",
            &digest("request"),
            &Ok(PluginHttpResponse {
                status: 200,
                headers: vec![],
                body: vec![]
            })
        ))
        .is_err()
    );
    assert_ne!(new.lease_token, lease.lease_token);
    let count: i64 = f
        .conn()
        .query_row("SELECT count(*) FROM plugin_effect_receipts", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn retry_bound_max_attempt_reject_and_explicit_audited_skip() {
    let f = Fixture::new();
    let lease = f.acquire(f.now);
    let event = f.event(1);
    f.delivery(&lease, f.now);
    assert!(
        f.write(|tx| complete(
            tx,
            "w",
            &lease,
            &event,
            &PluginServiceEventResult::RetryAfter {
                delay_ms: MAX_RETRY_MS + 1
            },
            f.now
        ))
        .is_err()
    );
    for attempt in 1..=5 {
        let now = f.now + i64::from(attempt - 1);
        f.write(|tx| {
            complete(
                tx,
                "w",
                &lease,
                &event,
                &PluginServiceEventResult::RetryAfter { delay_ms: 1 },
                now,
            )
        })
        .unwrap();
        if attempt < 5 {
            let (e, _) = f.delivery(&lease, now + 1);
            assert_eq!(e.attempt, attempt + 1);
        }
    }
    assert_eq!(f.pause(), Some(PluginServicePauseReason::AttemptsExhausted));
    f.write(|tx| release(tx, "w", &lease, f.now + 10)).unwrap();
    let action = PluginServiceRecoveryAction::Skip {
        event_id: event.clone(),
    };
    f.write(|tx| {
        recover(
            tx,
            "w",
            &lease.consumer_id,
            PluginServicePauseReason::AttemptsExhausted,
            &action,
            "skip",
            "owner",
            "reviewed failed event",
            f.now + 11,
        )
    })
    .unwrap();
    f.write(|tx| {
        recover(
            tx,
            "w",
            &lease.consumer_id,
            PluginServicePauseReason::AttemptsExhausted,
            &action,
            "skip",
            "owner",
            "reviewed failed event",
            f.now + 11,
        )
    })
    .unwrap();
    assert_eq!(
        f.conn()
            .query_row(
                "SELECT count(*) FROM plugin_service_recovery_audit",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
    assert!(load(&f.conn(), "w", &lease.consumer_id).unwrap().cursor > 0);
    let restored = f.acquire(f.now + 12);
    let second = f.event(2);
    f.delivery(&restored, f.now + 12);
    f.write(|tx| {
        complete(
            tx,
            "w",
            &restored,
            &second,
            &PluginServiceEventResult::Reject,
            f.now + 12,
        )
    })
    .unwrap();
    assert_eq!(f.pause(), Some(PluginServicePauseReason::Rejected));
}

#[test]
fn queue_and_retention_pause_without_drops_or_automatic_tail_reset() {
    let f = Fixture::new();
    let lease = f.acquire(f.now);
    for n in 1..=MAX_QUEUE {
        f.event(n);
    }
    assert!(matches!(
        f.poll(&lease, f.now),
        PluginServiceResponse::Diagnostics { .. }
    ));
    assert_eq!(f.pause(), Some(PluginServicePauseReason::QueueFull));
    assert_eq!(load(&f.conn(), "w", &lease.consumer_id).unwrap().cursor, 0);
    let other = Fixture::new();
    let lease = other.acquire(other.now);
    other.event(1);
    assert_eq!(
        other
            .write(|tx| prune_outbox(tx, "w", i64::MAX, other.now + RETENTION_MS + 1_000))
            .unwrap(),
        1
    );
    assert!(matches!(
        other.poll(&lease, other.now),
        PluginServiceResponse::Diagnostics { .. }
    ));
    assert_eq!(other.pause(), Some(PluginServicePauseReason::RetentionGap));
    assert_eq!(
        load(&other.conn(), "w", &lease.consumer_id).unwrap().cursor,
        0
    );
    other
        .write(|tx| release(tx, "w", &lease, other.now))
        .unwrap();
    other
        .write(|tx| {
            recover(
                tx,
                "w",
                &lease.consumer_id,
                PluginServicePauseReason::RetentionGap,
                &PluginServiceRecoveryAction::AcceptGap,
                "gap",
                "owner",
                "accepted expired history",
                other.now,
            )
        })
        .unwrap();
    assert_eq!(
        load(&other.conn(), "w", &lease.consumer_id).unwrap().cursor,
        1
    );
}

#[test]
fn response_before_ack_crash_replays_same_receipt_and_then_commits_cursor() {
    let f = Fixture::new();
    let lease = f.acquire(f.now);
    let event = f.event(1);
    let (_, id) = f.delivery(&lease, f.now);
    let mut record = plugin_receipt::lookup(&f.conn(), "w", &id).unwrap();
    record.service_lease = Some(lease.clone());
    let fp = digest("request");
    f.write(|tx| plugin_receipt::effect_admit(tx, &record, "effect", &fp, f.now))
        .unwrap();
    let response = Ok(PluginHttpResponse {
        status: 200,
        headers: vec![],
        body: b"observed".to_vec(),
    });
    f.write(|tx| plugin_receipt::effect_complete(tx, &record, "effect", &fp, &response))
        .unwrap();
    // Losing the driver after effect success does not change event/invocation.
    f.write(|tx| release(tx, "w", &lease, f.now)).unwrap();
    let restored = f.acquire(f.now + 1);
    let (envelope, again) = f.delivery(&restored, f.now + 30_000);
    assert_eq!(again, id);
    assert_eq!(envelope.event_id, event);
    record.service_lease = Some(restored.clone());
    assert_eq!(
        f.write(|tx| plugin_receipt::effect_admit(tx, &record, "effect", &fp, f.now + 30_000))
            .unwrap(),
        plugin_receipt::EffectAdmission::Replay(response)
    );
    f.write(|tx| {
        complete(
            tx,
            "w",
            &restored,
            &event,
            &PluginServiceEventResult::Ack,
            f.now + 30_000,
        )
    })
    .unwrap();
    assert!(load(&f.conn(), "w", &restored.consumer_id).unwrap().cursor > 0);
}

#[test]
fn explicit_queue_recovery_drains_without_skip_and_does_not_resume_other_consumer() {
    let f = Fixture::new();
    let lease = f.acquire(f.now);
    let mut second_plan = f.plan.clone();
    second_plan.plan_id = "second-plan".into();
    f.conn().execute("INSERT INTO plugin_execution_plans(workspace_id,plan_id,actor,operation_key,fingerprint,plan_json,pin_state) VALUES('w','second-plan','owner','second-plan',?1,?2,'active')",params![second_plan.fingerprint,second_plan.plan.canonical_json().unwrap()]).unwrap();
    let second = f
        .write(|tx| {
            acquire(
                tx,
                &second_plan,
                "i",
                "watch",
                &[PluginTicketEventType::Changed],
                f.now,
            )
        })
        .unwrap();
    for n in 1..=MAX_QUEUE {
        f.event(n);
    }
    assert!(matches!(
        f.poll(&lease, f.now),
        PluginServiceResponse::Diagnostics { .. }
    ));
    assert!(matches!(
        f.write(|tx| poll(tx, "w", &second, &second_plan, f.now))
            .unwrap(),
        PluginServiceResponse::Diagnostics { .. }
    ));
    f.write(|tx| release(tx, "w", &lease, f.now)).unwrap();
    f.write(|tx| {
        recover(
            tx,
            "w",
            &lease.consumer_id,
            PluginServicePauseReason::QueueFull,
            &PluginServiceRecoveryAction::Resume,
            "drain",
            "owner",
            "process the observed backlog serially",
            f.now,
        )
    })
    .unwrap();
    let restored = f.acquire(f.now + 1);
    let (event, _) = f.delivery(&restored, f.now + 1);
    assert_eq!(event.event_id, format!("{:032x}", 1));
    assert_eq!(
        load(&f.conn(), "w", &lease.consumer_id).unwrap().cursor,
        0,
        "recovery is not skip"
    );
    assert_eq!(
        load(&f.conn(), "w", &second.consumer_id).unwrap().pause,
        Some(PluginServicePauseReason::QueueFull)
    );
    f.write(|tx| {
        complete(
            tx,
            "w",
            &restored,
            &event.event_id,
            &PluginServiceEventResult::Ack,
            f.now + 1,
        )
    })
    .unwrap();
    assert_eq!(load(&f.conn(), "w", &lease.consumer_id).unwrap().cursor, 1);
    assert_eq!(
        f.conn()
            .query_row(
                "SELECT count(*) FROM plugin_service_recovery_audit",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
}

#[test]
fn queue_full_before_claim_only_exact_observed_head_can_be_skipped_with_one_audit() {
    let f = Fixture::new();
    let lease = f.acquire(f.now);
    let mut other_plan = f.plan.clone();
    other_plan.plan_id = "other-plan".into();
    f.conn().execute("INSERT INTO plugin_execution_plans(workspace_id,plan_id,actor,operation_key,fingerprint,plan_json,pin_state) VALUES('w','other-plan','owner','other-plan',?1,?2,'active')",params![other_plan.fingerprint,other_plan.plan.canonical_json().unwrap()]).unwrap();
    let other = f
        .write(|tx| {
            acquire(
                tx,
                &other_plan,
                "i",
                "watch",
                &[PluginTicketEventType::Changed],
                f.now,
            )
        })
        .unwrap();
    let events: Vec<_> = (1..=MAX_QUEUE).map(|n| f.event(n)).collect();
    for (proof, plan) in [(&lease, &f.plan), (&other, &other_plan)] {
        let PluginServiceResponse::Diagnostics { diagnostics } =
            f.write(|tx| poll(tx, "w", proof, plan, f.now)).unwrap()
        else {
            panic!("full queue must pause before claiming an invocation");
        };
        assert_eq!(
            diagnostics.pause_reason,
            Some(PluginServicePauseReason::QueueFull)
        );
        assert_eq!(
            diagnostics.current_event_id.as_deref(),
            Some(events[0].as_str())
        );
        assert_eq!(diagnostics.queued, MAX_QUEUE);
        assert_eq!(diagnostics.attempt, 0);
        let saved = load(&f.conn(), "w", &proof.consumer_id).unwrap();
        assert!(saved.current_event_id.is_none());
        assert_eq!(saved.cursor, 0);
    }
    assert_eq!(
        f.conn()
            .query_row("SELECT count(*) FROM plugin_invocations", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    f.write(|tx| release(tx, "w", &lease, f.now)).unwrap();
    let recover_head = |action: &PluginServiceRecoveryAction| {
        f.write(|tx| {
            recover(
                tx,
                "w",
                &lease.consumer_id,
                PluginServicePauseReason::QueueFull,
                action,
                "skip-queue-head",
                "reviewer",
                "explicitly abandon observed queue head",
                f.now,
            )
        })
    };
    assert!(
        recover_head(&PluginServiceRecoveryAction::Skip {
            event_id: events[1].clone()
        })
        .is_err()
    );
    assert_eq!(load(&f.conn(), "w", &lease.consumer_id).unwrap().cursor, 0);
    assert_eq!(
        f.conn()
            .query_row(
                "SELECT count(*) FROM plugin_service_recovery_audit",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    let action = PluginServiceRecoveryAction::Skip {
        event_id: events[0].clone(),
    };
    recover_head(&action).unwrap();
    recover_head(&action).unwrap();
    let saved = load(&f.conn(), "w", &lease.consumer_id).unwrap();
    assert_eq!(
        saved.cursor, 1,
        "skip advances exactly one observed head, not the whole backlog"
    );
    assert!(saved.pause.is_none());
    assert!(saved.current_event_id.is_none());
    assert_eq!(
        f.conn()
            .query_row(
                "SELECT count(*) FROM plugin_service_recovery_audit",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
    let (consumer, actor, reason, raw): (String, String, String, String) = f.conn().query_row(
        "SELECT consumer_id,actor,reason,action_json FROM plugin_service_recovery_audit WHERE operation_key='skip-queue-head'", [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    ).unwrap();
    assert_eq!(consumer, lease.consumer_id);
    assert_eq!(actor, "reviewer");
    assert_eq!(reason, "explicitly abandon observed queue head");
    assert_eq!(
        serde_json::from_str::<PluginServiceRecoveryAction>(&raw).unwrap(),
        action
    );
    let untouched = load(&f.conn(), "w", &other.consumer_id).unwrap();
    assert_eq!(untouched.cursor, 0);
    assert_eq!(untouched.pause, Some(PluginServicePauseReason::QueueFull));
    assert!(untouched.current_event_id.is_none());
    assert_eq!(
        untouched.lease_token.as_deref(),
        Some(other.lease_token.as_str())
    );
    assert_eq!(
        f.conn()
            .query_row(
                "SELECT count(*) FROM typed_ticket_service_outbox",
                [],
                |r| r.get::<_, u32>(0)
            )
            .unwrap(),
        MAX_QUEUE
    );
    let restored = f.acquire(f.now + 1);
    assert_eq!(f.delivery(&restored, f.now + 1).0.event_id, events[1]);
}

#[test]
fn expired_old_driver_cannot_complete_pending_http_receipt_after_lease_rotation() {
    let f = Fixture::new();
    let old = f.acquire(f.now);
    f.event(1);
    let (_, id) = f.delivery(&old, f.now);
    let mut record = plugin_receipt::lookup(&f.conn(), "w", &id).unwrap();
    record.service_lease = Some(old.clone());
    let fingerprint = digest("request");
    f.write(|tx| plugin_receipt::effect_admit(tx, &record, "pending", &fingerprint, f.now))
        .unwrap();
    f.write(|tx| release(tx, "w", &old, f.now)).unwrap();
    let current = f.acquire(f.now + 1);
    let observed = Ok(PluginHttpResponse {
        status: 200,
        headers: vec![],
        body: b"late".to_vec(),
    });
    assert!(
        f.write(|tx| plugin_receipt::effect_complete(
            tx,
            &record,
            "pending",
            &fingerprint,
            &observed
        ))
        .is_err()
    );
    assert_eq!(
        f.conn()
            .query_row(
                "SELECT state FROM plugin_effect_receipts WHERE invocation_id=?1",
                [&id],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        "pending"
    );
    assert!(matches!(
        f.poll(&current, f.now + 1),
        PluginServiceResponse::Diagnostics { .. }
    ));
    assert_eq!(f.pause(), Some(PluginServicePauseReason::Indeterminate));
    assert_eq!(
        load(&f.conn(), "w", &current.consumer_id).unwrap().cursor,
        0
    );
}

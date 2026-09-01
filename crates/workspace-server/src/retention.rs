//! Workspace DB authority for deterministic Worker retention planning.
//! Runtime receives only resolved dispositions and stable ids; policy authority
//! never comes from prompts, profiles, or model input.

use crate::{Error as StoreError, store::SqliteWorkspaceStore};
use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use worker_runtime::identity::RuntimeWorkerRef;
use worker_runtime::retention::{
    DiagnosticsDisposition, RuntimeWorkerAggregateDiagnostic, SessionDisposition,
    WorkerRetentionExecutionRequest, WorkerRetentionExecutionResult, WorkerRetentionInventory,
    WorkerRetentionInventorySnapshot,
};

pub const CONSERVATIVE_POLICY_ID: &str = "workspace-default-conservative";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetadataDisposition {
    Tombstone,
    Purge,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ArchiveRetention {
    Forever,
    ForSeconds { seconds: u64 },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerRetentionPolicy {
    pub workspace_id: String,
    pub policy_id: String,
    pub revision: u64,
    pub session_disposition: SessionDisposition,
    pub metadata_disposition: MetadataDisposition,
    pub archive_retention: ArchiveRetention,
    pub diagnostics_disposition: DiagnosticsDisposition,
    pub diagnostics_retention_seconds: Option<u64>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerRetentionPolicyUpdate {
    pub policy_id: String,
    pub session_disposition: SessionDisposition,
    pub metadata_disposition: MetadataDisposition,
    pub archive_retention: ArchiveRetention,
    pub diagnostics_disposition: DiagnosticsDisposition,
    pub diagnostics_retention_seconds: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerRemovalPlanRequest {
    pub workspace_id: String,
    pub worker: RuntimeWorkerRef,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkerRemovalBlocker {
    Hold,
    CurrentAssignment {
        assignment_id: String,
        ticket_id: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerRemovalPlanState {
    Planned,
    Blocked,
    Executing,
    Failed,
    Stale,
    Succeeded,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerRemovalPlan {
    pub plan_id: String,
    pub operation_id: String,
    pub input_fingerprint: String,
    pub workspace_id: String,
    pub worker: RuntimeWorkerRef,
    pub worker_revision: String,
    pub run_generation: u64,
    pub policy_id: String,
    pub policy_revision: u64,
    pub session_disposition: SessionDisposition,
    pub metadata_disposition: MetadataDisposition,
    pub archive_retention: ArchiveRetention,
    pub diagnostics_disposition: DiagnosticsDisposition,
    pub diagnostics_retention_seconds: Option<u64>,
    pub archive_id: Option<String>,
    pub blockers: Vec<WorkerRemovalBlocker>,
    pub state: WorkerRemovalPlanState,
    pub reason: String,
    pub created_at: String,
    pub updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_category: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparedWorkerRemoval {
    pub plan: WorkerRemovalPlan,
    pub runtime_request: WorkerRetentionExecutionRequest,
    pub prior_failure_category: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerTombstone {
    pub workspace_id: String,
    pub worker: RuntimeWorkerRef,
    pub display_name: String,
    pub profile: Option<String>,
    pub created_at: String,
    pub removed_at: String,
    pub archive_id: Option<String>,
    pub policy_id: String,
    pub policy_revision: u64,
    pub operation_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerOrphanDiagnostic {
    pub diagnostic_id: String,
    pub workspace_id: String,
    pub runtime_id: String,
    pub worker_id: String,
    pub category: String,
    pub detail: String,
    pub observed_at: String,
}

#[derive(thiserror::Error, Debug)]
pub enum WorkerRetentionError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("Worker retention policy is not configured for Workspace {workspace_id}")]
    PolicyMissing { workspace_id: String },
    #[error("Worker retention policy revision conflict: expected {expected}, current {actual}")]
    PolicyRevisionConflict { expected: u64, actual: u64 },
    #[error("Worker was not found in the requested Workspace")]
    WorkerNotFound,
    #[error("Worker belongs to a different Workspace")]
    CrossWorkspace,
    #[error("Worker removal is blocked: {0:?}")]
    Blocked(Vec<WorkerRemovalBlocker>),
    #[error("Worker removal plan {plan_id} is stale: {reason}")]
    StalePlan { plan_id: String, reason: String },
    #[error("Worker removal operation {operation_id} was reused with different input")]
    OperationFingerprintConflict { operation_id: String },
    #[error("invalid Worker retention input: {0}")]
    Invalid(String),
}

pub(crate) fn repair_worker_diagnostics_archive_table(conn: &Connection) -> crate::Result<bool> {
    let existed: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='worker_diagnostics_archives')",
        [],
        |row| row.get(0),
    )?;
    if !existed {
        conn.execute_batch(
            "CREATE TABLE worker_diagnostics_archives (
              operation_id TEXT PRIMARY KEY, workspace_id TEXT NOT NULL, runtime_id TEXT NOT NULL,
              worker_id TEXT NOT NULL, policy_id TEXT NOT NULL, policy_revision INTEGER NOT NULL,
              committed_at TEXT NOT NULL, expires_at TEXT NOT NULL,
              FOREIGN KEY(operation_id) REFERENCES worker_removal_operations(operation_id),
              FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE);",
        )?;
    }
    Ok(!existed)
}

pub(crate) fn create_worker_retention_tables(conn: &Connection) -> crate::Result<()> {
    conn.execute_batch(r#"
      CREATE TABLE workspace_worker_retention_policy_revisions (
        workspace_id TEXT NOT NULL, policy_id TEXT NOT NULL, revision INTEGER NOT NULL CHECK(revision>0),
        session_disposition TEXT NOT NULL CHECK(session_disposition IN ('archive','purge')),
        metadata_disposition TEXT NOT NULL CHECK(metadata_disposition IN ('tombstone','purge')),
        archive_retention_kind TEXT NOT NULL CHECK(archive_retention_kind IN ('forever','for_seconds')),
        archive_retention_seconds INTEGER,
        diagnostics_disposition TEXT NOT NULL CHECK(diagnostics_disposition IN ('purge','retain')),
        diagnostics_retention_seconds INTEGER, created_at TEXT NOT NULL,
        PRIMARY KEY(workspace_id,policy_id,revision),
        FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE);
      CREATE TABLE workspace_worker_retention_policies (
        workspace_id TEXT PRIMARY KEY, policy_id TEXT NOT NULL, revision INTEGER NOT NULL, updated_at TEXT NOT NULL,
        FOREIGN KEY(workspace_id,policy_id,revision) REFERENCES workspace_worker_retention_policy_revisions(workspace_id,policy_id,revision),
        FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE);
      CREATE TABLE worker_removal_operations (
        operation_id TEXT PRIMARY KEY, plan_id TEXT NOT NULL UNIQUE, input_fingerprint TEXT NOT NULL,
        workspace_id TEXT NOT NULL, runtime_id TEXT NOT NULL, worker_id TEXT NOT NULL,
        worker_revision TEXT NOT NULL, run_generation INTEGER NOT NULL CHECK(run_generation>=0),
        policy_id TEXT NOT NULL, policy_revision INTEGER NOT NULL,
        session_disposition TEXT NOT NULL, metadata_disposition TEXT NOT NULL,
        archive_retention_kind TEXT NOT NULL, archive_retention_seconds INTEGER,
        diagnostics_disposition TEXT NOT NULL,
        diagnostics_retention_seconds INTEGER, archive_id TEXT UNIQUE, blockers_json TEXT NOT NULL,
        state TEXT NOT NULL CHECK(state IN ('planned','blocked','executing','failed','stale','succeeded')),
        reason TEXT NOT NULL, failure_category TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
        FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE);
      CREATE INDEX worker_removal_operations_worker_idx ON worker_removal_operations(workspace_id,runtime_id,worker_id,created_at);
      CREATE TABLE worker_session_archives (
        archive_id TEXT PRIMARY KEY, workspace_id TEXT NOT NULL, runtime_id TEXT NOT NULL, worker_id TEXT NOT NULL,
        session_id TEXT NOT NULL, checksum_sha256 TEXT NOT NULL, content_bytes INTEGER NOT NULL,
        policy_id TEXT NOT NULL, policy_revision INTEGER NOT NULL, operation_id TEXT NOT NULL UNIQUE,
        committed_at TEXT NOT NULL, expires_at TEXT,
        FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
        FOREIGN KEY(operation_id) REFERENCES worker_removal_operations(operation_id));
      CREATE TABLE worker_diagnostics_archives (
        operation_id TEXT PRIMARY KEY, workspace_id TEXT NOT NULL, runtime_id TEXT NOT NULL,
        worker_id TEXT NOT NULL, policy_id TEXT NOT NULL, policy_revision INTEGER NOT NULL,
        committed_at TEXT NOT NULL, expires_at TEXT NOT NULL,
        FOREIGN KEY(operation_id) REFERENCES worker_removal_operations(operation_id),
        FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE);
      CREATE TABLE worker_tombstones (
        workspace_id TEXT NOT NULL, runtime_id TEXT NOT NULL, worker_id TEXT NOT NULL,
        display_name TEXT NOT NULL, profile TEXT, worker_created_at TEXT NOT NULL, removed_at TEXT NOT NULL,
        archive_id TEXT, policy_id TEXT NOT NULL, policy_revision INTEGER NOT NULL, operation_id TEXT NOT NULL UNIQUE,
        PRIMARY KEY(workspace_id,runtime_id,worker_id),
        FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
        FOREIGN KEY(archive_id) REFERENCES worker_session_archives(archive_id),
        FOREIGN KEY(operation_id) REFERENCES worker_removal_operations(operation_id));
      CREATE TABLE worker_orphan_diagnostics (
        diagnostic_id TEXT PRIMARY KEY, workspace_id TEXT NOT NULL, runtime_id TEXT NOT NULL, worker_id TEXT NOT NULL,
        category TEXT NOT NULL, detail TEXT NOT NULL, observed_at TEXT NOT NULL,
        FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE);
      CREATE TABLE worker_retention_audit_events (
        event_id TEXT PRIMARY KEY, operation_id TEXT NOT NULL, workspace_id TEXT NOT NULL,
        event_kind TEXT NOT NULL, detail TEXT NOT NULL, created_at TEXT NOT NULL,
        FOREIGN KEY(operation_id) REFERENCES worker_removal_operations(operation_id),
        FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE);
      CREATE TRIGGER seed_worker_retention_policy_after_workspace_insert AFTER INSERT ON workspaces BEGIN
        INSERT INTO workspace_worker_retention_policy_revisions
          (workspace_id,policy_id,revision,session_disposition,metadata_disposition,archive_retention_kind,archive_retention_seconds,diagnostics_disposition,diagnostics_retention_seconds,created_at)
          VALUES(NEW.workspace_id,'workspace-default-conservative',1,'archive','tombstone','forever',NULL,'purge',NULL,NEW.created_at);
        INSERT INTO workspace_worker_retention_policies(workspace_id,policy_id,revision,updated_at)
          VALUES(NEW.workspace_id,'workspace-default-conservative',1,NEW.created_at);
      END;
    "#)?;
    let now = Utc::now().to_rfc3339();
    conn.execute("INSERT OR IGNORE INTO workspace_worker_retention_policy_revisions
      (workspace_id,policy_id,revision,session_disposition,metadata_disposition,archive_retention_kind,archive_retention_seconds,diagnostics_disposition,diagnostics_retention_seconds,created_at)
      SELECT workspace_id,?1,1,'archive','tombstone','forever',NULL,'purge',NULL,?2 FROM workspaces", params![CONSERVATIVE_POLICY_ID,now])?;
    conn.execute("INSERT OR IGNORE INTO workspace_worker_retention_policies(workspace_id,policy_id,revision,updated_at)
      SELECT workspace_id,?1,1,?2 FROM workspaces", params![CONSERVATIVE_POLICY_ID,now])?;
    Ok(())
}

impl SqliteWorkspaceStore {
    pub fn worker_retention_policy(
        &self,
        workspace_id: &str,
    ) -> crate::Result<Option<WorkerRetentionPolicy>> {
        self.with_conn(|conn| load_policy(conn, workspace_id))
    }

    pub fn update_worker_retention_policy(
        &self,
        workspace_id: &str,
        expected: u64,
        update: &WorkerRetentionPolicyUpdate,
    ) -> Result<WorkerRetentionPolicy, WorkerRetentionError> {
        validate_policy(update)?;
        self.with_conn_mut(|conn| {
            let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let current=load_policy(&tx,workspace_id)?.ok_or_else(|| StoreError::InvalidInput(format!("policy-missing:{workspace_id}")))?;
            if current.revision!=expected { return Err(StoreError::InvalidInput(format!("policy-conflict:{expected}:{}",current.revision))); }
            let revision=current.revision+1; let now=Utc::now().to_rfc3339();
            tx.execute("INSERT INTO workspace_worker_retention_policy_revisions
              (workspace_id,policy_id,revision,session_disposition,metadata_disposition,archive_retention_kind,archive_retention_seconds,diagnostics_disposition,diagnostics_retention_seconds,created_at)
              VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",params![workspace_id,update.policy_id,revision,sess(update.session_disposition),meta(update.metadata_disposition),archive_kind(update.archive_retention),archive_seconds(update.archive_retention),diag(update.diagnostics_disposition),update.diagnostics_retention_seconds,now])?;
            let changed=tx.execute("UPDATE workspace_worker_retention_policies SET policy_id=?1,revision=?2,updated_at=?3 WHERE workspace_id=?4 AND revision=?5",
              params![update.policy_id,revision,now,workspace_id,expected])?;
            if changed!=1 { return Err(StoreError::InvalidInput(format!("policy-conflict:{expected}:{revision}"))); }
            tx.commit()?; load_policy(conn,workspace_id)?.ok_or_else(|| StoreError::InvalidInput("updated policy missing".into()))
        }).map_err(map_error)
    }

    pub fn plan_worker_removal(
        &self,
        req: &WorkerRemovalPlanRequest,
        inv: &WorkerRetentionInventory,
    ) -> Result<WorkerRemovalPlan, WorkerRetentionError> {
        validate_plan(req, inv)?;
        let now = Utc::now().to_rfc3339();
        self.with_conn_mut(|conn| {
            let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let policy=load_policy(&tx,&req.workspace_id)?.ok_or_else(|| StoreError::InvalidInput(format!("policy-missing:{}",req.workspace_id)))?;
            let worker=match load_worker(&tx,&req.workspace_id,&req.worker)? {
                Some(v)=>v,
                None=>{
                    let other:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM worker_registry WHERE runtime_id=?1 AND worker_id=?2 AND workspace_id!=?3)",params![req.worker.runtime_id,req.worker.worker_id,req.workspace_id],|r|r.get(0))?;
                    return Err(StoreError::InvalidInput(if other{"cross-workspace".into()}else{"worker-missing".into()}));
                }
            };
            let mut blockers=Vec::new();
            if worker.retention_state=="pinned" { blockers.push(WorkerRemovalBlocker::Hold); }
            if let Some((assignment_id,ticket_id))=tx.query_row("SELECT a.assignment_id,a.ticket_id FROM ticket_current_worker_assignments c JOIN ticket_worker_assignments a ON a.workspace_id=c.workspace_id AND a.ticket_id=c.ticket_id AND a.assignment_id=c.assignment_id WHERE a.workspace_id=?1 AND a.runtime_id=?2 AND a.worker_id=?3",params![req.workspace_id,req.worker.runtime_id,req.worker.worker_id],|r|Ok((r.get(0)?,r.get(1)?))).optional()? {
                blockers.push(WorkerRemovalBlocker::CurrentAssignment{assignment_id,ticket_id});
            }
            let fp=fingerprint(req,&worker.updated_at,inv,&policy,&blockers)?;
            let plan_id=stable("wrp",&fp); let operation_id=stable("wro",&fp);
            let archive_id=(policy.session_disposition==SessionDisposition::Archive).then(||stable("wra",&fp));
            let state=if blockers.is_empty(){WorkerRemovalPlanState::Planned}else{WorkerRemovalPlanState::Blocked};
            tx.execute("INSERT OR IGNORE INTO worker_removal_operations(operation_id,plan_id,input_fingerprint,workspace_id,runtime_id,worker_id,worker_revision,run_generation,policy_id,policy_revision,session_disposition,metadata_disposition,archive_retention_kind,archive_retention_seconds,diagnostics_disposition,diagnostics_retention_seconds,archive_id,blockers_json,state,reason,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?21)",params![operation_id,plan_id,fp,req.workspace_id,req.worker.runtime_id,req.worker.worker_id,worker.updated_at,inv.run_generation,policy.policy_id,policy.revision,sess(policy.session_disposition),meta(policy.metadata_disposition),archive_kind(policy.archive_retention),archive_seconds(policy.archive_retention),diag(policy.diagnostics_disposition),policy.diagnostics_retention_seconds,archive_id,serde_json::to_string(&blockers).map_err(|e|StoreError::InvalidInput(e.to_string()))?,state_s(state),req.reason,now])?;
            let plan=load_plan(&tx,&plan_id)?.ok_or_else(||StoreError::InvalidInput("plan missing".into()))?;
            if plan.input_fingerprint!=fp{return Err(StoreError::InvalidInput(format!("fingerprint:{}",plan.operation_id)));}
            tx.commit()?; Ok(plan)
        }).map_err(map_error)
    }

    pub fn begin_worker_removal(
        &self,
        workspace_id: &str,
        plan_id: &str,
        fp: &str,
    ) -> Result<WorkerRemovalPlan, WorkerRetentionError> {
        self.with_conn_mut(|conn|{
            let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let mut plan=load_plan(&tx,plan_id)?.ok_or_else(||StoreError::InvalidInput(format!("stale:{plan_id}:plan missing")))?;
            if plan.workspace_id!=workspace_id{return Err(StoreError::InvalidInput("cross-workspace".into()));}
            if plan.input_fingerprint!=fp{return Err(StoreError::InvalidInput(format!("fingerprint:{}",plan.operation_id)));}
            if plan.state==WorkerRemovalPlanState::Succeeded{tx.commit()?;return Ok(plan);}
            if plan.state==WorkerRemovalPlanState::Blocked {
                return Err(StoreError::InvalidInput(format!("blocked:{}",serde_json::to_string(&plan.blockers).unwrap())));
            }
            if !matches!(plan.state, WorkerRemovalPlanState::Planned | WorkerRemovalPlanState::Failed | WorkerRemovalPlanState::Executing) {
                return Err(StoreError::InvalidInput(format!("stale:{plan_id}:plan state {} is not executable", state_s(plan.state))));
            }
            if !plan.blockers.is_empty(){return Err(StoreError::InvalidInput(format!("blocked:{}",serde_json::to_string(&plan.blockers).unwrap())));}
            let policy=load_policy(&tx,workspace_id)?.ok_or_else(||StoreError::InvalidInput(format!("policy-missing:{workspace_id}")))?;
            if policy.policy_id!=plan.policy_id||policy.revision!=plan.policy_revision{
                mark_stale(&tx,&plan,"policy revision changed")?;tx.commit()?;
                return Err(stale_error(&plan,"policy revision changed"));
            }
            let worker=load_worker(&tx,workspace_id,&plan.worker)?.ok_or_else(||StoreError::InvalidInput(format!("stale:{plan_id}:Worker missing")))?;
            if worker.updated_at!=plan.worker_revision{
                mark_stale(&tx,&plan,"Worker revision changed")?;tx.commit()?;
                return Err(stale_error(&plan,"Worker revision changed"));
            }
            if worker.retention_state=="pinned"{
                mark_stale(&tx,&plan,"hold added")?;tx.commit()?;
                return Err(stale_error(&plan,"hold added"));
            }
            let assigned:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM ticket_current_worker_assignments c JOIN ticket_worker_assignments a ON a.workspace_id=c.workspace_id AND a.ticket_id=c.ticket_id AND a.assignment_id=c.assignment_id WHERE a.workspace_id=?1 AND a.runtime_id=?2 AND a.worker_id=?3)",params![workspace_id,plan.worker.runtime_id,plan.worker.worker_id],|r|r.get(0))?;
            if assigned{
                mark_stale(&tx,&plan,"current assignment added")?;tx.commit()?;
                return Err(stale_error(&plan,"current assignment added"));
            }
            let now=Utc::now().to_rfc3339();
            tx.execute("UPDATE worker_removal_operations SET state='executing',updated_at=?1 WHERE operation_id=?2",params![now,plan.operation_id])?;
            plan.state=WorkerRemovalPlanState::Executing;plan.updated_at=now;tx.commit()?;Ok(plan)
        }).map_err(map_error)
    }

    /// Revalidates Backend authority and derives the complete Runtime request
    /// from the immutable plan. Callers cannot substitute generation or
    /// dispositions without causing a fingerprint/manifest mismatch.
    pub fn prepare_worker_removal_execution(
        &self,
        workspace_id: &str,
        plan_id: &str,
        input_fingerprint: &str,
    ) -> Result<PreparedWorkerRemoval, WorkerRetentionError> {
        let plan = self.begin_worker_removal(workspace_id, plan_id, input_fingerprint)?;
        let worker = self
            .with_conn(|conn| load_worker(conn, workspace_id, &plan.worker))?
            .ok_or_else(|| WorkerRetentionError::StalePlan {
                plan_id: plan.plan_id.clone(),
                reason: "Worker disappeared after execution fence".to_string(),
            })?;
        let worker_id = plan
            .worker
            .worker_id
            .parse::<worker_runtime::identity::WorkerId>()
            .map_err(|_| {
                WorkerRetentionError::Invalid("Worker id must be a canonical UUIDv7".to_string())
            })?;
        let removed_at = plan.created_at.clone();
        let prior_failure_category = plan.failure_category.clone();
        Ok(PreparedWorkerRemoval {
            runtime_request: WorkerRetentionExecutionRequest {
                operation_id: plan.operation_id.clone(),
                input_fingerprint: plan.input_fingerprint.clone(),
                archive_id: plan.archive_id.clone(),
                workspace_id: plan.workspace_id.clone(),
                source_runtime_id: plan.worker.runtime_id.clone(),
                worker_id: worker_id,
                expected_worker_revision: plan.worker_revision.clone(),
                expected_run_generation: plan.run_generation,
                source_created_at: worker.created_at,
                removed_at,
                effective_profile: worker.profile,
                retention_class: None,
                policy_id: plan.policy_id.clone(),
                policy_revision: plan.policy_revision,
                session_disposition: plan.session_disposition,
                diagnostics_disposition: plan.diagnostics_disposition,
            },
            plan,
            prior_failure_category,
        })
    }

    pub fn recover_worker_removal_execution(
        &self,
        workspace_id: &str,
        worker: &RuntimeWorkerRef,
    ) -> Result<Option<PreparedWorkerRemoval>, WorkerRetentionError> {
        bounded("workspace", workspace_id, 160)?;
        let plan = self.with_conn(|conn| {
            conn.query_row(
                "SELECT plan_id FROM worker_removal_operations
                 WHERE workspace_id=?1 AND runtime_id=?2 AND worker_id=?3
                   AND state IN ('planned','executing','failed','succeeded')
                   AND (
                     state='succeeded' OR worker_revision=(
                       SELECT updated_at FROM worker_registry
                       WHERE workspace_id=?1 AND runtime_id=?2 AND worker_id=?3
                     )
                   )
                 ORDER BY CASE state WHEN 'succeeded' THEN 0 ELSE 1 END,
                          created_at DESC LIMIT 1",
                params![workspace_id, worker.runtime_id, worker.worker_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(StoreError::from)
            .and_then(|plan_id| match plan_id {
                Some(plan_id) => load_plan(conn, &plan_id),
                None => Ok(None),
            })
        })?;
        let Some(plan) = plan else {
            return Ok(None);
        };
        let prior_failure_category = plan.failure_category.clone();
        let worker_id = plan
            .worker
            .worker_id
            .parse::<worker_runtime::identity::WorkerId>()
            .map_err(|_| {
                WorkerRetentionError::Invalid("Worker id must be a canonical UUIDv7".to_string())
            })?;
        let worker = if plan.state == WorkerRemovalPlanState::Succeeded {
            None
        } else {
            Some(
                self.with_conn(|conn| load_worker(conn, workspace_id, &plan.worker))?
                    .ok_or(WorkerRetentionError::WorkerNotFound)?,
            )
        };
        Ok(Some(PreparedWorkerRemoval {
            runtime_request: WorkerRetentionExecutionRequest {
                operation_id: plan.operation_id.clone(),
                input_fingerprint: plan.input_fingerprint.clone(),
                archive_id: plan.archive_id.clone(),
                workspace_id: plan.workspace_id.clone(),
                source_runtime_id: plan.worker.runtime_id.clone(),
                worker_id: worker_id,
                expected_worker_revision: plan.worker_revision.clone(),
                expected_run_generation: plan.run_generation,
                source_created_at: worker
                    .as_ref()
                    .map(|worker| worker.created_at.clone())
                    .unwrap_or_else(|| plan.created_at.clone()),
                removed_at: plan.created_at.clone(),
                effective_profile: worker
                    .as_ref()
                    .map(|worker| worker.profile.clone())
                    .unwrap_or_else(|| Some("removed".to_string())),
                retention_class: None,
                policy_id: plan.policy_id.clone(),
                policy_revision: plan.policy_revision,
                session_disposition: plan.session_disposition,
                diagnostics_disposition: plan.diagnostics_disposition,
            },
            plan,
            prior_failure_category,
        }))
    }

    pub fn fail_worker_removal(
        &self,
        workspace_id: &str,
        operation_id: &str,
        fp: &str,
        category: &str,
    ) -> Result<(), WorkerRetentionError> {
        bounded("failure category", category, 160)?;
        self.with_conn_mut(|conn|{
            let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let changed=tx.execute("UPDATE worker_removal_operations SET state='failed',failure_category=?1,updated_at=?2 WHERE workspace_id=?3 AND operation_id=?4 AND input_fingerprint=?5 AND state IN ('planned','executing','failed')",params![category,Utc::now().to_rfc3339(),workspace_id,operation_id,fp])?;
            if changed!=1{return Err(StoreError::InvalidInput("active operation mismatch".into()));}
            tx.commit()?;Ok(())
        })?;
        Ok(())
    }

    pub fn commit_worker_removal(
        &self,
        workspace_id: &str,
        operation_id: &str,
        fp: &str,
        result: &WorkerRetentionExecutionResult,
    ) -> Result<WorkerRemovalPlan, WorkerRetentionError> {
        self.with_conn_mut(|conn|{
            let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let mut plan=load_plan_op(&tx,operation_id)?.ok_or_else(||StoreError::InvalidInput("operation missing".into()))?;
            if plan.workspace_id!=workspace_id{return Err(StoreError::InvalidInput("cross-workspace".into()));}
            if plan.input_fingerprint!=fp||result.input_fingerprint!=fp||result.operation_id!=operation_id{return Err(StoreError::InvalidInput(format!("fingerprint:{operation_id}")));}
            if plan.state==WorkerRemovalPlanState::Succeeded{tx.commit()?;return Ok(plan);}
            if plan.state != WorkerRemovalPlanState::Executing {
                return Err(StoreError::InvalidInput(format!("stale:{}:plan state {} is not committable", plan.plan_id, state_s(plan.state))));
            }
            if result.expected_worker_revision != plan.worker_revision
                || result.worker_id.to_string() != plan.worker.worker_id
                || result.session_disposition != plan.session_disposition
                || result.diagnostics_disposition != plan.diagnostics_disposition
            {
                return Err(StoreError::InvalidInput("Runtime retention result does not match removal plan".into()));
            }
            if !result.source_removed{return Err(StoreError::InvalidInput("Runtime source was not removed".into()));}
            let worker=load_worker(&tx,workspace_id,&plan.worker)?.ok_or_else(||StoreError::InvalidInput("Worker missing before commit".into()))?;
            if worker.updated_at!=plan.worker_revision{return Err(StoreError::InvalidInput(format!("stale:{}:Worker revision changed",plan.plan_id)));}
            if worker.retention_state=="pinned" { return Err(StoreError::InvalidInput(format!("stale:{}:hold added",plan.plan_id))); }
            let assigned:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM ticket_current_worker_assignments c JOIN ticket_worker_assignments a ON a.workspace_id=c.workspace_id AND a.ticket_id=c.ticket_id AND a.assignment_id=c.assignment_id WHERE a.workspace_id=?1 AND a.runtime_id=?2 AND a.worker_id=?3)",params![workspace_id,plan.worker.runtime_id,plan.worker.worker_id],|row|row.get(0))?;
            if assigned { return Err(StoreError::InvalidInput(format!("stale:{}:current assignment added",plan.plan_id))); }
            let now=Utc::now().to_rfc3339();
            if let Some(a)=&result.archive{
                if plan.archive_id.as_deref()!=Some(&a.archive_id)||a.workspace_id!=workspace_id||a.source_runtime_id!=plan.worker.runtime_id||a.source_worker_id.to_string()!=plan.worker.worker_id||a.policy_id!=plan.policy_id||a.policy_revision!=plan.policy_revision{return Err(StoreError::InvalidInput("archive manifest mismatch".into()));}
                let expires_at=match plan.archive_retention { ArchiveRetention::Forever=>None, ArchiveRetention::ForSeconds{seconds}=>{let seconds=i64::try_from(seconds).map_err(|_|StoreError::InvalidInput("archive retention deadline overflow".into()))?;Some((Utc::now()+chrono::Duration::seconds(seconds)).to_rfc3339())} };
                tx.execute("INSERT OR IGNORE INTO worker_session_archives(archive_id,workspace_id,runtime_id,worker_id,session_id,checksum_sha256,content_bytes,policy_id,policy_revision,operation_id,committed_at,expires_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",params![a.archive_id,workspace_id,plan.worker.runtime_id,plan.worker.worker_id,a.source_session_id,a.content_checksum_sha256,a.content_bytes,plan.policy_id,plan.policy_revision,operation_id,now,expires_at])?;
            }else if plan.session_disposition==SessionDisposition::Archive{return Err(StoreError::InvalidInput("archive manifest missing".into()));}
            match plan.diagnostics_disposition {
                DiagnosticsDisposition::Purge if result.diagnostics_retained => return Err(StoreError::InvalidInput("Runtime retained diagnostics for purge disposition".into())),
                DiagnosticsDisposition::Retain if !result.diagnostics_retained => return Err(StoreError::InvalidInput("Runtime did not retain diagnostics".into())),
                DiagnosticsDisposition::Retain => {
                    let seconds=plan.diagnostics_retention_seconds.ok_or_else(||StoreError::InvalidInput("diagnostics retention deadline missing".into()))?;
                    let seconds=i64::try_from(seconds).map_err(|_|StoreError::InvalidInput("diagnostics retention deadline overflow".into()))?;
                    let expires_at=(Utc::now()+chrono::Duration::seconds(seconds)).to_rfc3339();
                    tx.execute("INSERT OR IGNORE INTO worker_diagnostics_archives(operation_id,workspace_id,runtime_id,worker_id,policy_id,policy_revision,committed_at,expires_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![operation_id,workspace_id,plan.worker.runtime_id,plan.worker.worker_id,plan.policy_id,plan.policy_revision,now,expires_at])?;
                }
                DiagnosticsDisposition::Purge => {}
            }
            if plan.metadata_disposition==MetadataDisposition::Tombstone{
                tx.execute("INSERT OR IGNORE INTO worker_tombstones(workspace_id,runtime_id,worker_id,display_name,profile,worker_created_at,removed_at,archive_id,policy_id,policy_revision,operation_id) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",params![workspace_id,plan.worker.runtime_id,plan.worker.worker_id,worker.display_name,worker.profile,worker.created_at,now,plan.archive_id,plan.policy_id,plan.policy_revision,operation_id])?;
            }
            let deleted=tx.execute("DELETE FROM worker_registry WHERE workspace_id=?1 AND runtime_id=?2 AND worker_id=?3 AND updated_at=?4",params![workspace_id,plan.worker.runtime_id,plan.worker.worker_id,plan.worker_revision])?;
            if deleted!=1{return Err(StoreError::InvalidInput(format!("stale:{}:removal fence changed",plan.plan_id)));}
            tx.execute("UPDATE worker_removal_operations SET state='succeeded',failure_category=NULL,updated_at=?1 WHERE operation_id=?2",params![now,operation_id])?;
            tx.execute("INSERT OR IGNORE INTO worker_retention_audit_events(event_id,operation_id,workspace_id,event_kind,detail,created_at) VALUES(?1,?2,?3,'worker_removed',?4,?5)",params![stable("wre",operation_id),operation_id,workspace_id,format!("runtime_id={} worker_id={} session={} metadata={} diagnostics={}",plan.worker.runtime_id,plan.worker.worker_id,sess(plan.session_disposition),meta(plan.metadata_disposition),diag(plan.diagnostics_disposition)),now])?;
            tx.commit()?;plan.state=WorkerRemovalPlanState::Succeeded;plan.updated_at=now;Ok(plan)
        }).map_err(map_error)
    }

    /// Compare trusted Runtime inventory with the Backend worker registry and
    /// persist bounded diagnostics for both orphan directions. This operation
    /// is diagnostic-only: a Runtime aggregate without Backend authority is
    /// never assigned an implicit purge disposition.
    pub fn reconcile_worker_retention_inventory(
        &self,
        snapshot: &WorkerRetentionInventorySnapshot,
    ) -> Result<Vec<WorkerOrphanDiagnostic>, WorkerRetentionError> {
        self.reconcile_worker_retention_inventory_parts(
            snapshot.workspace_id(),
            snapshot.runtime_id(),
            snapshot.workers(),
            snapshot.diagnostics(),
        )
    }

    fn reconcile_worker_retention_inventory_parts(
        &self,
        workspace_id: &str,
        runtime_id: &str,
        inventory: &[WorkerRetentionInventory],
        runtime_diagnostics: &[RuntimeWorkerAggregateDiagnostic],
    ) -> Result<Vec<WorkerOrphanDiagnostic>, WorkerRetentionError> {
        bounded("Workspace id", workspace_id, 160)?;
        bounded("Runtime id", runtime_id, 160)?;
        let mut runtime_workers = BTreeMap::new();
        for item in inventory {
            if item.workspace_id != workspace_id || item.runtime_id != runtime_id {
                return Err(WorkerRetentionError::CrossWorkspace);
            }
            let worker_id = item.worker_id.to_string();
            if runtime_workers.insert(worker_id.clone(), item).is_some() {
                return Err(WorkerRetentionError::Invalid(format!(
                    "duplicate Runtime inventory for Worker {worker_id}"
                )));
            }
        }
        self.with_conn_mut(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let policy_configured = load_policy(&tx, workspace_id)?.is_some();
            let mut statement = tx.prepare(
                "SELECT CAST(worker_id AS TEXT), retention_state
                 FROM worker_registry WHERE workspace_id=?1 AND runtime_id=?2",
            )?;
            let registry = statement
                .query_map(params![workspace_id, runtime_id], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<Result<BTreeMap<_, _>, _>>()?;
            drop(statement);
            let runtime_ids = runtime_workers.keys().cloned().collect::<BTreeSet<_>>();
            let registry_ids = registry.keys().cloned().collect::<BTreeSet<_>>();
            let observed_at = Utc::now().to_rfc3339();
            let mut diagnostics = runtime_diagnostics
                .iter()
                .map(|diagnostic| {
                    orphan_diagnostic(
                        workspace_id,
                        runtime_id,
                        diagnostic.worker_id(),
                        diagnostic.category(),
                        diagnostic.detail(),
                        &observed_at,
                    )
                })
                .collect::<Vec<_>>();
            for worker_id in runtime_ids.difference(&registry_ids) {
                let category = if policy_configured {
                    "runtime_aggregate_without_backend_registry"
                } else {
                    "runtime_aggregate_policy_missing_fail_closed"
                };
                diagnostics.push(orphan_diagnostic(
                    workspace_id,
                    runtime_id,
                    worker_id,
                    category,
                    "Runtime canonical aggregate is absent from Backend worker_registry; removal is blocked pending reconciliation",
                    &observed_at,
                ));
            }
            for worker_id in registry_ids.difference(&runtime_ids) {
                let category = if registry.get(worker_id).map(String::as_str) == Some("pinned") {
                    "backend_registry_without_runtime_aggregate_pinned"
                } else {
                    "backend_registry_without_runtime_aggregate"
                };
                diagnostics.push(orphan_diagnostic(
                    workspace_id,
                    runtime_id,
                    worker_id,
                    category,
                    "Backend worker_registry record has no Runtime canonical aggregate",
                    &observed_at,
                ));
            }
            for diagnostic in &diagnostics {
                insert_orphan_diagnostic(&tx, diagnostic)?;
            }
            tx.commit()?;
            Ok(diagnostics)
        })
        .map_err(WorkerRetentionError::Store)
    }

    pub fn worker_tombstone(
        &self,
        workspace_id: &str,
        worker: &RuntimeWorkerRef,
    ) -> crate::Result<Option<WorkerTombstone>> {
        self.with_conn(|conn|conn.query_row("SELECT display_name,profile,worker_created_at,removed_at,archive_id,policy_id,policy_revision,operation_id FROM worker_tombstones WHERE workspace_id=?1 AND runtime_id=?2 AND worker_id=?3",params![workspace_id,worker.runtime_id,worker.worker_id],|r|Ok(WorkerTombstone{workspace_id:workspace_id.into(),worker:worker.clone(),display_name:r.get(0)?,profile:r.get(1)?,created_at:r.get(2)?,removed_at:r.get(3)?,archive_id:r.get(4)?,policy_id:r.get(5)?,policy_revision:r.get::<_,i64>(6)? as u64,operation_id:r.get(7)?})).optional().map_err(StoreError::from))
    }
}

fn orphan_diagnostic(
    workspace_id: &str,
    runtime_id: &str,
    worker_id: &str,
    category: &str,
    detail: &str,
    observed_at: &str,
) -> WorkerOrphanDiagnostic {
    let identity = format!("{workspace_id}\0{runtime_id}\0{worker_id}\0{category}");
    WorkerOrphanDiagnostic {
        diagnostic_id: stable("wod", &identity),
        workspace_id: workspace_id.to_string(),
        runtime_id: runtime_id.to_string(),
        worker_id: worker_id.to_string(),
        category: category.to_string(),
        detail: detail.to_string(),
        observed_at: observed_at.to_string(),
    }
}

fn insert_orphan_diagnostic(
    conn: &Connection,
    diagnostic: &WorkerOrphanDiagnostic,
) -> crate::Result<()> {
    conn.execute(
        "INSERT INTO worker_orphan_diagnostics
         (diagnostic_id,workspace_id,runtime_id,worker_id,category,detail,observed_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7)
         ON CONFLICT(diagnostic_id) DO UPDATE SET
           detail=excluded.detail, observed_at=excluded.observed_at",
        params![
            diagnostic.diagnostic_id,
            diagnostic.workspace_id,
            diagnostic.runtime_id,
            diagnostic.worker_id,
            diagnostic.category,
            diagnostic.detail,
            diagnostic.observed_at
        ],
    )?;
    Ok(())
}

#[derive(Clone)]
struct WorkerRow {
    display_name: String,
    profile: Option<String>,
    retention_state: String,
    created_at: String,
    updated_at: String,
}
fn load_worker(c: &Connection, w: &str, r: &RuntimeWorkerRef) -> crate::Result<Option<WorkerRow>> {
    c.query_row("SELECT display_name,profile,retention_state,created_at,updated_at FROM worker_registry WHERE workspace_id=?1 AND runtime_id=?2 AND worker_id=?3",params![w,r.runtime_id,r.worker_id],|x|Ok(WorkerRow{display_name:x.get(0)?,profile:x.get(1)?,retention_state:x.get(2)?,created_at:x.get(3)?,updated_at:x.get(4)?})).optional().map_err(StoreError::from)
}
fn load_policy(c: &Connection, w: &str) -> crate::Result<Option<WorkerRetentionPolicy>> {
    c.query_row(
        "SELECT p.policy_id,p.revision,r.session_disposition,r.metadata_disposition,
                r.archive_retention_kind,r.archive_retention_seconds,
                r.diagnostics_disposition,r.diagnostics_retention_seconds,r.created_at,p.updated_at
         FROM workspace_worker_retention_policies p
         JOIN workspace_worker_retention_policy_revisions r
           ON r.workspace_id=p.workspace_id AND r.policy_id=p.policy_id AND r.revision=p.revision
         WHERE p.workspace_id=?1",
        params![w],
        |row| {
            let session: String = row.get(2)?;
            let metadata: String = row.get(3)?;
            let archive_kind: String = row.get(4)?;
            let archive_seconds: Option<i64> = row.get(5)?;
            let diagnostics: String = row.get(6)?;
            Ok(WorkerRetentionPolicy {
                workspace_id: w.into(),
                policy_id: row.get(0)?,
                revision: row.get::<_, i64>(1)? as u64,
                session_disposition: parse_s(&session)?,
                metadata_disposition: parse_m(&metadata)?,
                archive_retention: parse_archive(&archive_kind, archive_seconds)?,
                diagnostics_disposition: parse_d(&diagnostics)?,
                diagnostics_retention_seconds: row.get::<_, Option<i64>>(7)?.map(|v| v as u64),
                created_at: row.get(8)?,
                updated_at: row.get(9)?,
            })
        },
    )
    .optional()
    .map_err(StoreError::from)
}
fn load_plan(c: &Connection, id: &str) -> crate::Result<Option<WorkerRemovalPlan>> {
    load_plan_q(c, "plan_id", id)
}
fn load_plan_op(c: &Connection, id: &str) -> crate::Result<Option<WorkerRemovalPlan>> {
    load_plan_q(c, "operation_id", id)
}
fn load_plan_q(c: &Connection, key: &str, id: &str) -> crate::Result<Option<WorkerRemovalPlan>> {
    let query = format!(
        "SELECT plan_id,operation_id,input_fingerprint,workspace_id,runtime_id,worker_id,
                worker_revision,run_generation,policy_id,policy_revision,session_disposition,
                metadata_disposition,archive_retention_kind,archive_retention_seconds,
                diagnostics_disposition,diagnostics_retention_seconds,archive_id,blockers_json,
                state,reason,created_at,updated_at,failure_category
         FROM worker_removal_operations WHERE {key}=?1"
    );
    c.query_row(&query, params![id], |row| {
        let session: String = row.get(10)?;
        let metadata: String = row.get(11)?;
        let archive_kind: String = row.get(12)?;
        let archive_seconds: Option<i64> = row.get(13)?;
        let diagnostics: String = row.get(14)?;
        let blockers: String = row.get(17)?;
        let state: String = row.get(18)?;
        Ok(WorkerRemovalPlan {
            plan_id: row.get(0)?,
            operation_id: row.get(1)?,
            input_fingerprint: row.get(2)?,
            workspace_id: row.get(3)?,
            worker: RuntimeWorkerRef {
                runtime_id: row.get(4)?,
                worker_id: row.get(5)?,
            },
            worker_revision: row.get(6)?,
            run_generation: row.get::<_, i64>(7)? as u64,
            policy_id: row.get(8)?,
            policy_revision: row.get::<_, i64>(9)? as u64,
            session_disposition: parse_s(&session)?,
            metadata_disposition: parse_m(&metadata)?,
            archive_retention: parse_archive(&archive_kind, archive_seconds)?,
            diagnostics_disposition: parse_d(&diagnostics)?,
            diagnostics_retention_seconds: row.get::<_, Option<i64>>(15)?.map(|v| v as u64),
            archive_id: row.get(16)?,
            blockers: serde_json::from_str(&blockers).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    17,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?,
            state: parse_state(&state)?,
            reason: row.get(19)?,
            created_at: row.get(20)?,
            updated_at: row.get(21)?,
            failure_category: row.get(22)?,
        })
    })
    .optional()
    .map_err(StoreError::from)
}
fn mark_stale(
    tx: &rusqlite::Transaction<'_>,
    plan: &WorkerRemovalPlan,
    reason: &str,
) -> crate::Result<()> {
    tx.execute("UPDATE worker_removal_operations SET state='stale',failure_category=?1,updated_at=?2 WHERE operation_id=?3",params![reason,Utc::now().to_rfc3339(),plan.operation_id])?;
    Ok(())
}
fn stale_error(plan: &WorkerRemovalPlan, reason: &str) -> StoreError {
    StoreError::InvalidInput(format!("stale:{}:{reason}", plan.plan_id))
}
fn fingerprint(
    r: &WorkerRemovalPlanRequest,
    worker_revision: &str,
    i: &WorkerRetentionInventory,
    p: &WorkerRetentionPolicy,
    b: &[WorkerRemovalBlocker],
) -> crate::Result<String> {
    serde_json::to_vec(&serde_json::json!([
        r.workspace_id,
        r.worker.runtime_id,
        r.worker.worker_id,
        worker_revision,
        i.run_generation,
        i.session_id,
        i.segment_ids,
        p.policy_id,
        p.revision,
        p.session_disposition,
        p.metadata_disposition,
        p.archive_retention,
        p.diagnostics_disposition,
        p.diagnostics_retention_seconds,
        b,
        r.reason
    ]))
    .map(|v| hash(&v))
    .map_err(|e| StoreError::InvalidInput(e.to_string()))
}
fn hash(b: &[u8]) -> String {
    Sha256::digest(b)
        .iter()
        .map(|v| format!("{v:02x}"))
        .collect()
}
fn stable(p: &str, v: &str) -> String {
    format!("{p}_{}", &hash(v.as_bytes())[..32])
}
fn validate_plan(
    r: &WorkerRemovalPlanRequest,
    i: &WorkerRetentionInventory,
) -> Result<(), WorkerRetentionError> {
    bounded("workspace", &r.workspace_id, 160)?;
    bounded("reason", &r.reason, 2000)?;
    if i.workspace_id != r.workspace_id
        || i.runtime_id != r.worker.runtime_id
        || i.worker_id.to_string() != r.worker.worker_id
    {
        return Err(WorkerRetentionError::CrossWorkspace);
    }
    Ok(())
}
fn validate_policy(u: &WorkerRetentionPolicyUpdate) -> Result<(), WorkerRetentionError> {
    bounded("policy id", &u.policy_id, 160)?;
    if matches!(
        u.archive_retention,
        ArchiveRetention::ForSeconds { seconds: 0 }
    ) {
        return Err(WorkerRetentionError::Invalid(
            "archive retention seconds must be positive".to_string(),
        ));
    }
    match (u.diagnostics_disposition, u.diagnostics_retention_seconds) {
        (DiagnosticsDisposition::Purge, None) | (DiagnosticsDisposition::Retain, Some(1..)) => {
            Ok(())
        }
        _ => Err(WorkerRetentionError::Invalid(
            "diagnostics retention/disposition mismatch".into(),
        )),
    }
}
fn bounded(k: &str, v: &str, n: usize) -> Result<(), WorkerRetentionError> {
    if v.trim().is_empty() || v.len() > n {
        Err(WorkerRetentionError::Invalid(format!(
            "{k} must be non-empty and at most {n} bytes"
        )))
    } else {
        Ok(())
    }
}
fn map_error(e: StoreError) -> WorkerRetentionError {
    let StoreError::InvalidInput(m) = &e else {
        return WorkerRetentionError::Store(e);
    };
    if let Some(x) = m.strip_prefix("policy-missing:") {
        return WorkerRetentionError::PolicyMissing {
            workspace_id: x.into(),
        };
    }
    if let Some(x) = m.strip_prefix("policy-conflict:") {
        let mut s = x.split(':');
        return WorkerRetentionError::PolicyRevisionConflict {
            expected: s.next().and_then(|v| v.parse().ok()).unwrap_or(0),
            actual: s.next().and_then(|v| v.parse().ok()).unwrap_or(0),
        };
    }
    if m == "cross-workspace" {
        return WorkerRetentionError::CrossWorkspace;
    }
    if m == "worker-missing" {
        return WorkerRetentionError::WorkerNotFound;
    }
    if let Some(x) = m.strip_prefix("fingerprint:") {
        return WorkerRetentionError::OperationFingerprintConflict {
            operation_id: x.into(),
        };
    }
    if let Some(x) = m.strip_prefix("blocked:") {
        return WorkerRetentionError::Blocked(serde_json::from_str(x).unwrap_or_default());
    }
    if let Some(x) = m.strip_prefix("stale:") {
        let mut s = x.splitn(2, ':');
        return WorkerRetentionError::StalePlan {
            plan_id: s.next().unwrap_or_default().into(),
            reason: s.next().unwrap_or_default().into(),
        };
    }
    WorkerRetentionError::Store(e)
}
fn archive_kind(value: ArchiveRetention) -> &'static str {
    match value {
        ArchiveRetention::Forever => "forever",
        ArchiveRetention::ForSeconds { .. } => "for_seconds",
    }
}
fn archive_seconds(value: ArchiveRetention) -> Option<u64> {
    match value {
        ArchiveRetention::Forever => None,
        ArchiveRetention::ForSeconds { seconds } => Some(seconds),
    }
}
fn parse_archive(kind: &str, seconds: Option<i64>) -> rusqlite::Result<ArchiveRetention> {
    match (kind, seconds) {
        ("forever", None) => Ok(ArchiveRetention::Forever),
        ("for_seconds", Some(seconds)) if seconds > 0 => Ok(ArchiveRetention::ForSeconds {
            seconds: seconds as u64,
        }),
        _ => Err(bad("archive retention", kind)),
    }
}
fn sess(v: SessionDisposition) -> &'static str {
    match v {
        SessionDisposition::Archive => "archive",
        SessionDisposition::Purge => "purge",
    }
}
fn meta(v: MetadataDisposition) -> &'static str {
    match v {
        MetadataDisposition::Tombstone => "tombstone",
        MetadataDisposition::Purge => "purge",
    }
}
fn diag(v: DiagnosticsDisposition) -> &'static str {
    match v {
        DiagnosticsDisposition::Purge => "purge",
        DiagnosticsDisposition::Retain => "retain",
    }
}
fn state_s(v: WorkerRemovalPlanState) -> &'static str {
    match v {
        WorkerRemovalPlanState::Planned => "planned",
        WorkerRemovalPlanState::Blocked => "blocked",
        WorkerRemovalPlanState::Executing => "executing",
        WorkerRemovalPlanState::Failed => "failed",
        WorkerRemovalPlanState::Stale => "stale",
        WorkerRemovalPlanState::Succeeded => "succeeded",
    }
}
fn bad(k: &str, v: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        format!("invalid {k}: {v}").into(),
    )
}
fn parse_s(v: &str) -> rusqlite::Result<SessionDisposition> {
    match v {
        "archive" => Ok(SessionDisposition::Archive),
        "purge" => Ok(SessionDisposition::Purge),
        _ => Err(bad("session", v)),
    }
}
fn parse_m(v: &str) -> rusqlite::Result<MetadataDisposition> {
    match v {
        "tombstone" => Ok(MetadataDisposition::Tombstone),
        "purge" => Ok(MetadataDisposition::Purge),
        _ => Err(bad("metadata", v)),
    }
}
fn parse_d(v: &str) -> rusqlite::Result<DiagnosticsDisposition> {
    match v {
        "purge" => Ok(DiagnosticsDisposition::Purge),
        "retain" => Ok(DiagnosticsDisposition::Retain),
        _ => Err(bad("diagnostics", v)),
    }
}
fn parse_state(v: &str) -> rusqlite::Result<WorkerRemovalPlanState> {
    match v {
        "planned" => Ok(WorkerRemovalPlanState::Planned),
        "blocked" => Ok(WorkerRemovalPlanState::Blocked),
        "executing" => Ok(WorkerRemovalPlanState::Executing),
        "failed" => Ok(WorkerRemovalPlanState::Failed),
        "stale" => Ok(WorkerRemovalPlanState::Stale),
        "succeeded" => Ok(WorkerRemovalPlanState::Succeeded),
        _ => Err(bad("state", v)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{ControlPlaneStore, TicketCoderAssignmentRecord, WorkerRegistryRecord};
    use worker_runtime::identity::WorkerId;
    fn worker_id() -> WorkerId {
        WorkerId::from_legacy_u64(1)
    }
    fn setup() -> SqliteWorkspaceStore {
        let s = SqliteWorkspaceStore::in_memory().unwrap();
        s.with_conn(|c| {
            c.execute(
                "INSERT INTO accounts(account_id,kind,handle,display_name,created_at,updated_at) \
                 VALUES('owner-account','user','owner-account','Owner Account','t','t')",
                [],
            )?;
            c.execute(
                "INSERT INTO workspaces(workspace_id,display_name,state,created_at,updated_at,owner_account_id) \
                 VALUES('w','W','active','t','t','owner-account')",
                [],
            )?;
            c.execute(
                "INSERT INTO worker_registry(\
                    workspace_id,worker_id,runtime_id,display_name,profile,retention_state,created_at,updated_at\
                 ) VALUES('w',?1,'r','one','builtin:coder','normal','created','rev1')",
                [worker_id().to_string()],
            )?;
            c.execute(
                "INSERT INTO typed_tickets (workspace_id, ticket_id, slug, title, status, kind, priority, body, workflow_state, workflow_state_explicit) \
                 VALUES ('w', 'ticket', 'ticket', 'Ticket', 'open', 'task', 'normal', '', 'planning', 1)",
                [],
            )?;
            Ok(())
        })
        .unwrap();
        s
    }
    fn inv() -> WorkerRetentionInventory {
        WorkerRetentionInventory {
            workspace_id: "w".into(),
            runtime_id: "r".into(),
            worker_id: worker_id(),
            run_generation: 2,
            session_id: Some("s".into()),
            segment_ids: vec!["a".into()],
            session_bytes: 1,
            diagnostics_bytes: 0,
        }
    }
    fn req() -> WorkerRemovalPlanRequest {
        WorkerRemovalPlanRequest {
            workspace_id: "w".into(),
            worker: RuntimeWorkerRef {
                runtime_id: "r".into(),
                worker_id: worker_id().to_string(),
            },
            reason: "cleanup".into(),
        }
    }
    #[test]
    fn conservative_seed_and_policy_conflict() {
        let s = setup();
        let p = s.worker_retention_policy("w").unwrap().unwrap();
        assert_eq!(p.session_disposition, SessionDisposition::Archive);
        assert_eq!(p.archive_retention, ArchiveRetention::Forever);
        let u = WorkerRetentionPolicyUpdate {
            policy_id: "p".into(),
            session_disposition: SessionDisposition::Archive,
            metadata_disposition: MetadataDisposition::Purge,
            archive_retention: ArchiveRetention::ForSeconds { seconds: 3_600 },
            diagnostics_disposition: DiagnosticsDisposition::Purge,
            diagnostics_retention_seconds: None,
        };
        let updated = s.update_worker_retention_policy("w", 1, &u).unwrap();
        assert_eq!(updated.revision, 2);
        assert_eq!(
            updated.archive_retention,
            ArchiveRetention::ForSeconds { seconds: 3_600 }
        );
        assert!(matches!(
            s.update_worker_retention_policy("w", 1, &u),
            Err(WorkerRetentionError::PolicyRevisionConflict { .. })
        ));
    }
    #[test]
    fn deterministic_plan_hold_and_cross_workspace() {
        let s = setup();
        let a = s.plan_worker_removal(&req(), &inv()).unwrap();
        let b = s.plan_worker_removal(&req(), &inv()).unwrap();
        assert_eq!(a.plan_id, b.plan_id);
        assert_eq!(a.worker_revision, "rev1");
        s.with_conn(|c| {
            c.execute(
                "UPDATE worker_registry SET retention_state='pinned' WHERE workspace_id='w'",
                [],
            )?;
            Ok(())
        })
        .unwrap();
        let p = s.plan_worker_removal(&req(), &inv()).unwrap();
        assert_eq!(p.blockers, vec![WorkerRemovalBlocker::Hold]);
        assert!(matches!(
            s.begin_worker_removal("w", &p.plan_id, &p.input_fingerprint),
            Err(WorkerRetentionError::Blocked(_))
        ));
        let mut i = inv();
        i.workspace_id = "other".into();
        assert!(matches!(
            s.plan_worker_removal(&req(), &i),
            Err(WorkerRetentionError::CrossWorkspace)
        ));
        i.workspace_id = "w".into();
        i.runtime_id = "other-runtime".into();
        assert!(matches!(
            s.plan_worker_removal(&req(), &i),
            Err(WorkerRetentionError::CrossWorkspace)
        ));
    }
    #[test]
    fn stale_policy_and_failed_retry_restore_fence() {
        let s = setup();
        let p = s.plan_worker_removal(&req(), &inv()).unwrap();
        let u = WorkerRetentionPolicyUpdate {
            policy_id: "new".into(),
            session_disposition: SessionDisposition::Purge,
            metadata_disposition: MetadataDisposition::Purge,
            archive_retention: ArchiveRetention::Forever,
            diagnostics_disposition: DiagnosticsDisposition::Purge,
            diagnostics_retention_seconds: None,
        };
        s.update_worker_retention_policy("w", 1, &u).unwrap();
        assert!(matches!(
            s.begin_worker_removal("w", &p.plan_id, &p.input_fingerprint),
            Err(WorkerRetentionError::StalePlan { .. })
        ));
        let state: String = s
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT state FROM worker_removal_operations WHERE plan_id=?1",
                    params![p.plan_id],
                    |row| row.get(0),
                )
                .map_err(StoreError::from)
            })
            .unwrap();
        assert_eq!(state, "stale");
        assert!(matches!(
            s.begin_worker_removal("w", &p.plan_id, &p.input_fingerprint),
            Err(WorkerRetentionError::StalePlan { .. })
        ));
    }
    #[test]
    fn failed_retry_keeps_cleanup_stage_durable() {
        let s = setup();
        let p = s.plan_worker_removal(&req(), &inv()).unwrap();
        s.prepare_worker_removal_execution("w", &p.plan_id, &p.input_fingerprint)
            .unwrap();
        s.fail_worker_removal(
            "w",
            &p.operation_id,
            &p.input_fingerprint,
            "workdir_attachment_release_failed",
        )
        .unwrap();
        let retry = s
            .prepare_worker_removal_execution("w", &p.plan_id, &p.input_fingerprint)
            .unwrap();
        assert_eq!(retry.plan.state, WorkerRemovalPlanState::Executing);
        assert_eq!(
            retry.prior_failure_category.as_deref(),
            Some("workdir_attachment_release_failed")
        );
        let persisted: Option<String> = s
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT failure_category FROM worker_removal_operations WHERE operation_id=?1",
                    params![p.operation_id],
                    |row| row.get(0),
                )
                .map_err(StoreError::from)
            })
            .unwrap();
        assert_eq!(
            persisted.as_deref(),
            Some("workdir_attachment_release_failed")
        );
    }

    #[test]
    fn prepared_execution_is_derived_from_pinned_plan_generation() {
        let s = setup();
        let plan = s.plan_worker_removal(&req(), &inv()).unwrap();
        let prepared = s
            .prepare_worker_removal_execution("w", &plan.plan_id, &plan.input_fingerprint)
            .unwrap();
        assert_eq!(prepared.runtime_request.expected_run_generation, 2);
        assert_eq!(
            prepared.runtime_request.session_disposition,
            SessionDisposition::Archive
        );
        assert_eq!(prepared.runtime_request.policy_revision, 1);
        assert_eq!(
            prepared.runtime_request.worker_id,
            WorkerId::from_legacy_u64(1)
        );
        let retry = s
            .prepare_worker_removal_execution("w", &plan.plan_id, &plan.input_fingerprint)
            .unwrap();
        assert_eq!(retry.runtime_request, prepared.runtime_request);
    }

    #[test]
    fn purge_tombstone_commit_is_idempotent() {
        let s = setup();
        s.with_conn(|conn| {
            conn.execute("INSERT INTO typed_tickets(workspace_id,ticket_id,slug,title,status,kind,priority,body,workflow_state,workflow_state_explicit) VALUES('w','ticket-old','ticket-old','Old Ticket','open','task','normal','','planning',1)", [])?;
            conn.execute("INSERT INTO worker_registry(workspace_id,worker_id,runtime_id,display_name,profile,retention_state,created_at,updated_at) VALUES('w','1','r','old worker','builtin:coder','normal','created','rev1')", [])?;
            conn.execute("INSERT INTO ticket_worker_assignments(workspace_id,ticket_id,assignment_id,runtime_id,worker_id,assigned_by,assigned_at) VALUES('w','ticket-old','assignment-old','r','1','test','t')", [])?;
            conn.execute("DELETE FROM worker_registry WHERE workspace_id='w' AND runtime_id='r' AND worker_id='1'", [])?;
            conn.execute("DELETE FROM typed_tickets WHERE workspace_id='w' AND ticket_id='ticket-old'", [])?;
            Ok(())
        }).unwrap();
        let p = s.plan_worker_removal(&req(), &inv()).unwrap();
        s.begin_worker_removal("w", &p.plan_id, &p.input_fingerprint)
            .unwrap();
        let result = WorkerRetentionExecutionResult {
            operation_id: p.operation_id.clone(),
            input_fingerprint: p.input_fingerprint.clone(),
            expected_worker_revision: p.worker_revision.clone(),
            worker_id: worker_id(),
            session_disposition: p.session_disposition,
            diagnostics_disposition: p.diagnostics_disposition,
            archive: Some(worker_runtime::retention::WorkerSessionArchiveManifest {
                schema_version: 1,
                archive_id: p.archive_id.clone().unwrap(),
                workspace_id: "w".into(),
                source_runtime_id: "r".into(),
                source_worker_id: worker_id(),
                source_session_id: "s".into(),
                segment_ids: vec!["a".into()],
                source_created_at: "created".into(),
                removed_at: "removed".into(),
                archived_at_unix_seconds: 1,
                effective_profile: None,
                retention_class: None,
                content_checksum_sha256: "sum".into(),
                content_bytes: 1,
                content_file_count: 1,
                policy_id: p.policy_id.clone(),
                policy_revision: p.policy_revision,
                operation_id: p.operation_id.clone(),
                input_fingerprint: p.input_fingerprint.clone(),
            }),
            source_removed: true,
            diagnostics_retained: false,
        };
        assert_eq!(
            s.commit_worker_removal("w", &p.operation_id, &p.input_fingerprint, &result)
                .unwrap()
                .state,
            WorkerRemovalPlanState::Succeeded
        );
        assert!(s.worker_tombstone("w", &p.worker).unwrap().is_some());
        assert_eq!(
            s.commit_worker_removal("w", &p.operation_id, &p.input_fingerprint, &result)
                .unwrap()
                .state,
            WorkerRemovalPlanState::Succeeded
        );
        let historical: i64 = s.with_conn(|conn| conn.query_row(
            "SELECT COUNT(*) FROM ticket_worker_assignments WHERE workspace_id='w' AND assignment_id='assignment-old'",
            [],
            |row| row.get(0),
        ).map_err(StoreError::from)).unwrap();
        assert_eq!(historical, 1);
    }
    #[test]
    fn assignment_and_orphan_are_authoritative() {
        let s = setup();
        s.with_conn(|c| {
            let stable_worker_id = worker_id().to_string();
            c.execute(
                "INSERT INTO ticket_worker_assignments(\
                    workspace_id,ticket_id,assignment_id,runtime_id,worker_id,assigned_by,assigned_at\
                 ) VALUES('w','ticket','assignment','r',?1,'test','t')",
                [&stable_worker_id],
            )?;
            c.execute(
                "INSERT INTO ticket_current_worker_assignments(\
                    workspace_id,ticket_id,assignment_id,runtime_id,worker_id,updated_at\
                 ) VALUES('w','ticket','assignment','r',?1,'t')",
                [&stable_worker_id],
            )?;
            Ok(())
        })
        .unwrap();
        let p = s.plan_worker_removal(&req(), &inv()).unwrap();
        assert!(
            matches!(&p.blockers[..],[WorkerRemovalBlocker::CurrentAssignment{assignment_id,ticket_id}] if assignment_id=="assignment"&&ticket_id=="ticket")
        );
        let runtime_only = WorkerRetentionInventory {
            workspace_id: "w".into(),
            runtime_id: "r".into(),
            worker_id: WorkerId::from_legacy_u64(2),
            run_generation: 1,
            session_id: Some("orphan-session".into()),
            segment_ids: vec![],
            session_bytes: 10,
            diagnostics_bytes: 0,
        };
        let diagnostics = s
            .reconcile_worker_retention_inventory_parts("w", "r", &[runtime_only], &[])
            .unwrap();
        assert_eq!(diagnostics.len(), 2);
        assert!(diagnostics.iter().any(|item| {
            item.worker_id == WorkerId::from_legacy_u64(2).to_string()
                && item.category == "runtime_aggregate_without_backend_registry"
        }));
        assert!(diagnostics.iter().any(|item| {
            item.worker_id == worker_id().to_string()
                && item.category == "backend_registry_without_runtime_aggregate"
        }));
        let count: i64 = s
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM worker_orphan_diagnostics WHERE workspace_id='w' AND runtime_id='r'",
                    [],
                    |row| row.get(0),
                )
                .map_err(StoreError::from)
            })
            .unwrap();
        assert_eq!(count, 2);
        let mut wrong_scope = inv();
        wrong_scope.workspace_id = "other".into();
        assert!(matches!(
            s.reconcile_worker_retention_inventory_parts("w", "r", &[wrong_scope], &[]),
            Err(WorkerRetentionError::CrossWorkspace)
        ));
        let unchanged: i64 = s
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM worker_orphan_diagnostics WHERE workspace_id='w' AND runtime_id='r'",
                    [],
                    |row| row.get(0),
                )
                .map_err(StoreError::from)
            })
            .unwrap();
        assert_eq!(unchanged, 2);
    }
    #[test]
    fn concurrent_plan_converges_and_purge_omits_tombstone() {
        let s = std::sync::Arc::new(setup());
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        let handles = (0..2)
            .map(|_| {
                let s = s.clone();
                let b = barrier.clone();
                std::thread::spawn(move || {
                    b.wait();
                    s.plan_worker_removal(&req(), &inv()).unwrap()
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        let plans = handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(plans[0].plan_id, plans[1].plan_id);
        let s = setup();
        let u = WorkerRetentionPolicyUpdate {
            policy_id: "purge".into(),
            session_disposition: SessionDisposition::Purge,
            metadata_disposition: MetadataDisposition::Purge,
            archive_retention: ArchiveRetention::Forever,
            diagnostics_disposition: DiagnosticsDisposition::Purge,
            diagnostics_retention_seconds: None,
        };
        s.update_worker_retention_policy("w", 1, &u).unwrap();
        let p = s.plan_worker_removal(&req(), &inv()).unwrap();
        s.begin_worker_removal("w", &p.plan_id, &p.input_fingerprint)
            .unwrap();
        let r = WorkerRetentionExecutionResult {
            operation_id: p.operation_id.clone(),
            input_fingerprint: p.input_fingerprint.clone(),
            expected_worker_revision: p.worker_revision.clone(),
            worker_id: worker_id(),
            session_disposition: SessionDisposition::Purge,
            diagnostics_disposition: DiagnosticsDisposition::Purge,
            archive: None,
            source_removed: true,
            diagnostics_retained: false,
        };
        s.commit_worker_removal("w", &p.operation_id, &p.input_fingerprint, &r)
            .unwrap();
        assert!(s.worker_tombstone("w", &p.worker).unwrap().is_none());
    }
    #[test]
    fn commit_requires_executing_state_and_exact_runtime_result() {
        let store = setup();
        let plan = store.plan_worker_removal(&req(), &inv()).unwrap();
        let mut result = WorkerRetentionExecutionResult {
            operation_id: plan.operation_id.clone(),
            input_fingerprint: plan.input_fingerprint.clone(),
            expected_worker_revision: plan.worker_revision.clone(),
            worker_id: worker_id(),
            session_disposition: plan.session_disposition,
            diagnostics_disposition: plan.diagnostics_disposition,
            archive: None,
            source_removed: true,
            diagnostics_retained: false,
        };
        assert!(matches!(
            store.commit_worker_removal("w", &plan.operation_id, &plan.input_fingerprint, &result),
            Err(WorkerRetentionError::StalePlan { .. })
        ));
        store
            .begin_worker_removal("w", &plan.plan_id, &plan.input_fingerprint)
            .unwrap();
        result.worker_id = WorkerId::from_legacy_u64(2);
        assert!(
            store
                .commit_worker_removal("w", &plan.operation_id, &plan.input_fingerprint, &result)
                .is_err()
        );
        let count: i64 = store.with_conn(|conn| conn.query_row(
            "SELECT COUNT(*) FROM worker_registry WHERE workspace_id='w' AND runtime_id='r' AND worker_id=?1",
            [worker_id().to_string()],
            |row| row.get(0),
        ).map_err(StoreError::from)).unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn execution_fence_blocks_stale_upsert_and_new_assignment() {
        let store = setup();
        let plan = store.plan_worker_removal(&req(), &inv()).unwrap();
        store
            .begin_worker_removal("w", &plan.plan_id, &plan.input_fingerprint)
            .unwrap();
        let stale = WorkerRegistryRecord {
            workspace_id: "w".into(),
            worker: RuntimeWorkerRef {
                runtime_id: "r".into(),
                worker_id: worker_id().to_string(),
            },
            display_name: "stale".into(),
            profile: None,
            retention_state: "normal".into(),
            transcript_ref: None,
            session_ref: None,
            summary_ref: None,
            diagnostics_ref: None,
            created_at: "created".into(),
            updated_at: "rev2".into(),
        };
        store.upsert_worker_registry(&stale).unwrap();
        let revision: String = store.with_conn(|conn| conn.query_row(
            "SELECT updated_at FROM worker_registry WHERE workspace_id='w' AND runtime_id='r' AND worker_id=?1",
            [worker_id().to_string()],
            |row| row.get(0),
        ).map_err(StoreError::from)).unwrap();
        assert_eq!(revision, "rev1");

        let assignment = TicketCoderAssignmentRecord {
            workspace_id: "w".into(),
            ticket_id: "new-ticket".into(),
            assignment_id: "new-assignment".into(),
            worker: RuntimeWorkerRef {
                runtime_id: "r".into(),
                worker_id: worker_id().to_string(),
            },
            assigned_by: "test".into(),
            assigned_at: "t".into(),
        };
        assert!(
            store
                .set_current_ticket_coder_assignment(
                    &assignment,
                    None,
                    "event",
                    "assignment-operation",
                    false,
                )
                .is_err()
        );
    }

    #[test]
    fn runtime_result_must_match_prepared_worker_revision() {
        let s = setup();
        let plan = s.plan_worker_removal(&req(), &inv()).unwrap();
        let prepared = s
            .prepare_worker_removal_execution("w", &plan.plan_id, &plan.input_fingerprint)
            .unwrap();
        let mut runtime_result = WorkerRetentionExecutionResult {
            operation_id: prepared.plan.operation_id.clone(),
            input_fingerprint: prepared.plan.input_fingerprint.clone(),
            expected_worker_revision: prepared.plan.worker_revision.clone(),
            worker_id: worker_id(),
            session_disposition: prepared.plan.session_disposition,
            diagnostics_disposition: prepared.plan.diagnostics_disposition,
            archive: None,
            source_removed: true,
            diagnostics_retained: false,
        };
        runtime_result.expected_worker_revision = "stale-revision".to_string();
        let error = s
            .commit_worker_removal(
                "w",
                &plan.operation_id,
                &plan.input_fingerprint,
                &runtime_result,
            )
            .unwrap_err();
        assert!(
            !error.to_string().is_empty(),
            "mismatched Runtime revision must be rejected"
        );
    }

    #[test]
    fn worker_removal_recovery_is_target_keyed_and_preserves_original_reason() {
        let store = setup();
        let request = req();
        let plan = store.plan_worker_removal(&request, &inv()).unwrap();
        let recovered_planned = store
            .recover_worker_removal_execution("w", &request.worker)
            .unwrap()
            .unwrap();
        assert_eq!(
            recovered_planned.plan.state,
            WorkerRemovalPlanState::Planned
        );
        assert_eq!(recovered_planned.plan.plan_id, plan.plan_id);
        store
            .prepare_worker_removal_execution("w", &plan.plan_id, &plan.input_fingerprint)
            .unwrap();
        store
            .fail_worker_removal(
                "w",
                &plan.operation_id,
                &plan.input_fingerprint,
                "runtime_remove_failed",
            )
            .unwrap();

        let recovered = store
            .recover_worker_removal_execution("w", &request.worker)
            .unwrap()
            .unwrap();
        assert_eq!(recovered.plan.plan_id, plan.plan_id);
        assert_eq!(recovered.plan.reason, request.reason);
    }

    #[test]
    fn stale_failed_removal_is_not_recovered_after_worker_authority_changes() {
        let store = setup();
        let request = req();
        let plan = store.plan_worker_removal(&request, &inv()).unwrap();
        store
            .prepare_worker_removal_execution("w", &plan.plan_id, &plan.input_fingerprint)
            .unwrap();
        store
            .fail_worker_removal(
                "w",
                &plan.operation_id,
                &plan.input_fingerprint,
                "runtime_remove_failed",
            )
            .unwrap();
        store
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE worker_registry SET updated_at='rev2' WHERE workspace_id='w' AND runtime_id='r' AND worker_id=?1",
                    [worker_id().to_string()],
                )?;
                Ok(())
            })
            .unwrap();

        assert!(
            store
                .recover_worker_removal_execution("w", &request.worker)
                .unwrap()
                .is_none()
        );
        let replacement = store.plan_worker_removal(&request, &inv()).unwrap();
        assert_eq!(replacement.worker_revision, "rev2");
        assert_ne!(replacement.plan_id, plan.plan_id);
    }

    #[test]
    fn succeeded_worker_removal_recovers_after_registry_purge() {
        let s = setup();
        let request = req();
        let plan = s.plan_worker_removal(&request, &inv()).unwrap();
        let prepared = s
            .prepare_worker_removal_execution("w", &plan.plan_id, &plan.input_fingerprint)
            .unwrap();
        let runtime_result = WorkerRetentionExecutionResult {
            operation_id: prepared.plan.operation_id.clone(),
            input_fingerprint: prepared.plan.input_fingerprint.clone(),
            expected_worker_revision: prepared.plan.worker_revision.clone(),
            worker_id: worker_id(),
            session_disposition: prepared.plan.session_disposition,
            diagnostics_disposition: prepared.plan.diagnostics_disposition,
            archive: Some(worker_runtime::retention::WorkerSessionArchiveManifest {
                schema_version: 1,
                archive_id: prepared.plan.archive_id.clone().unwrap(),
                workspace_id: "w".into(),
                source_runtime_id: "r".into(),
                source_worker_id: worker_id(),
                source_session_id: "s".into(),
                segment_ids: vec!["a".into()],
                source_created_at: "created".into(),
                removed_at: "removed".into(),
                archived_at_unix_seconds: 1,
                effective_profile: None,
                retention_class: None,
                content_checksum_sha256: "sum".into(),
                content_bytes: 1,
                content_file_count: 1,
                policy_id: prepared.plan.policy_id.clone(),
                policy_revision: prepared.plan.policy_revision,
                operation_id: prepared.plan.operation_id.clone(),
                input_fingerprint: prepared.plan.input_fingerprint.clone(),
            }),
            source_removed: true,
            diagnostics_retained: false,
        };
        s.commit_worker_removal(
            "w",
            &plan.operation_id,
            &plan.input_fingerprint,
            &runtime_result,
        )
        .unwrap();
        assert!(
            s.get_worker_registry("w", &request.worker)
                .unwrap()
                .is_none()
        );
        let recovered = s
            .recover_worker_removal_execution("w", &request.worker)
            .unwrap()
            .unwrap();
        assert_eq!(recovered.plan.state, WorkerRemovalPlanState::Succeeded);
        assert_eq!(
            recovered.runtime_request.expected_worker_revision,
            plan.worker_revision
        );
    }

    #[test]
    fn recovery_preserves_attachment_failure_stage_for_retry_ordering() {
        let s = setup();
        let request = req();
        let plan = s.plan_worker_removal(&request, &inv()).unwrap();
        s.prepare_worker_removal_execution("w", &plan.plan_id, &plan.input_fingerprint)
            .unwrap();
        s.fail_worker_removal(
            "w",
            &plan.operation_id,
            &plan.input_fingerprint,
            "workdir_attachment_release_failed",
        )
        .unwrap();
        let recovered = s
            .recover_worker_removal_execution("w", &request.worker)
            .unwrap()
            .unwrap();
        assert_eq!(
            recovered.prior_failure_category.as_deref(),
            Some("workdir_attachment_release_failed")
        );
        assert_eq!(recovered.plan.state, WorkerRemovalPlanState::Failed);
    }

    #[test]
    fn old_schema_upgrade_seeds_existing_workspace() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("server.db");
        {
            let connection = rusqlite::Connection::open(&path).unwrap();
            crate::store::configure_sqlite(&connection).unwrap();
            crate::store::apply_migrations_through(&connection, 27).unwrap();
            connection
                .execute(
                    "INSERT INTO accounts(
                        account_id, kind, handle, display_name, created_at, updated_at
                     ) VALUES ('owner-account', 'user', 'owner-account', 'Owner Account', 'old', 'old')",
                    [],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO workspaces(
                        workspace_id, display_name, state, created_at, updated_at, owner_account_id
                     ) VALUES ('legacy', 'Legacy', 'active', 'old', 'old', 'owner-account')",
                    [],
                )
                .unwrap();
        }
        let reopened = SqliteWorkspaceStore::open(&path).unwrap();
        let p = reopened.worker_retention_policy("legacy").unwrap().unwrap();
        assert_eq!(p.policy_id, CONSERVATIVE_POLICY_ID);
        assert_eq!(p.session_disposition, SessionDisposition::Archive);
        assert_eq!(p.metadata_disposition, MetadataDisposition::Tombstone);
    }
}

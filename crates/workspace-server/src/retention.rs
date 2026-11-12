//! Workspace DB authority for deterministic Worker retention planning.
//! Runtime receives only resolved dispositions and stable ids; policy authority
//! never comes from prompts, profiles, or model input.

use crate::{
    Error as StoreError,
    store::{SqliteWorkspaceStore, WorkerRemovalCommit, commit_worker_catalog_removal},
};
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
    pub policy_digest: String,
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
    /// Internal evidence cannot be supplied through serialized deletion requests.
    #[serde(skip)]
    pub(crate) journal_erasure: Option<ErasedJournalSession>,
}

const ERASED_JOURNAL_POLICY_ID: &str = "backend-journal-erased-session-v1";

/// Monotonic domain erasure proof, bound to the exact source Worker and Session.
/// The only constructor rereads the Journal tombstone; ordinary Purge grants no erase authority.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ErasedJournalSession {
    workspace_id: String,
    worker: RuntimeWorkerRef,
    session_id: String,
}
impl ErasedJournalSession {
    pub(crate) fn validated(
        workspace_id: &str,
        store: &crate::subjektiv::SubjektivStore,
        attr: &crate::subjektiv::SubjectSessionAttribution,
    ) -> crate::Result<Option<Self>> {
        let host = subjektiv::journal::JournalHostContext::validated_session(store, attr.clone())
            .map_err(|e| StoreError::Store(e.to_string()))?;
        Ok(store
            .journal_session_erased(&host)
            .map_err(|e| StoreError::Store(e.to_string()))?
            .then(|| Self {
                workspace_id: workspace_id.into(),
                worker: RuntimeWorkerRef::new(&attr.runtime_id, &attr.worker_id),
                session_id: attr.session_id.clone(),
            }))
    }

    fn matches(
        &self,
        req: &WorkerRemovalPlanRequest,
        inventory: &WorkerRetentionInventory,
    ) -> bool {
        self.workspace_id == req.workspace_id
            && self.worker == req.worker
            && inventory
                .session_id
                .as_ref()
                .is_none_or(|id| *id == self.session_id)
    }
}

/// Physical Runtime erasure acknowledgement, not merely a domain tombstone.
/// Only the scoped erase request and a matching Session response construct it.
/// The Runtime acknowledgement includes associated archived diagnostic originals;
/// historical retention receipts retain their original values.
#[derive(Clone, Debug)]
pub struct RuntimeJournalErasureAck {
    domain: ErasedJournalSession,
    source: runtime_api::SessionPublicSource,
}

impl ErasedJournalSession {
    pub(crate) fn erase_runtime_source(
        self,
        runtime: &crate::hosts::RuntimeRegistry,
        source: runtime_api::SessionPublicSource,
    ) -> crate::Result<RuntimeJournalErasureAck> {
        let response = runtime
            .session_material_erase(
                &self.worker.runtime_id,
                runtime_api::SessionMaterialEraseRequest {
                    workspace_id: self.workspace_id.clone(),
                    source: source.clone(),
                    expected_session_id: self.session_id.clone(),
                },
            )
            .map_err(|e| e.into_error())?;
        if response.session_id != self.session_id {
            return Err(StoreError::RepositoryConflict(
                "Session erase receipt differs from the requested identity".into(),
            ));
        }
        Ok(RuntimeJournalErasureAck {
            domain: self,
            source,
        })
    }
}

impl RuntimeJournalErasureAck {
    pub(crate) fn domain_proof(&self) -> &ErasedJournalSession {
        &self.domain
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkerRemovalBlocker {
    Hold,
    PendingTicketClaim {
        operation_id: String,
        ticket_id: String,
    },
    UnfinishedWork {
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
    pub worker_updated_at: String,
    pub policy_id: String,
    pub policy_digest: String,
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
    pub policy_digest: String,
    pub operation_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerSessionArchiveRecord {
    pub archive_id: String,
    pub workspace_id: String,
    pub worker: RuntimeWorkerRef,
    pub session_id: String,
    pub checksum_sha256: String,
    pub content_bytes: u64,
    pub policy_id: String,
    pub policy_digest: String,
    pub operation_id: String,
    pub committed_at: String,
    pub expires_at: Option<String>,
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
    #[error("Worker retention policy digest conflict: expected {expected}, current {actual}")]
    PolicyDigestConflict { expected: String, actual: String },
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
        expected: &str,
        update: &WorkerRetentionPolicyUpdate,
    ) -> Result<WorkerRetentionPolicy, WorkerRetentionError> {
        validate_policy(update)?;
        self.with_conn_mut(|conn| {
            let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let current=load_policy(&tx,workspace_id)?.ok_or_else(|| StoreError::InvalidInput(format!("policy-missing:{workspace_id}")))?;
            if current.policy_digest != expected {
                return Err(StoreError::InvalidInput(format!("policy-conflict:{}", serde_json::to_string(&(expected, &current.policy_digest)).unwrap())));
            }
            let digest = worker_retention_policy_digest(update);
            let now = Utc::now().to_rfc3339();
            tx.execute("INSERT OR IGNORE INTO workspace_worker_retention_policy_snapshots
              (workspace_id,policy_id,policy_digest,session_disposition,metadata_disposition,archive_retention_kind,archive_retention_seconds,diagnostics_disposition,diagnostics_retention_seconds,created_at)
              VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",params![workspace_id,update.policy_id,digest,sess(update.session_disposition),meta(update.metadata_disposition),archive_kind(update.archive_retention),archive_seconds(update.archive_retention),diag(update.diagnostics_disposition),update.diagnostics_retention_seconds,now])?;
            let changed=tx.execute("UPDATE workspace_worker_retention_policies SET policy_id=?1,policy_digest=?2,updated_at=?3 WHERE workspace_id=?4 AND policy_digest=?5",
              params![update.policy_id,digest,now,workspace_id,expected])?;
            if changed!=1 { return Err(StoreError::InvalidInput(format!("policy-conflict:{}", serde_json::to_string(&(expected, &current.policy_digest)).unwrap()))); }
            tx.commit()?; load_policy(conn,workspace_id)?.ok_or_else(|| StoreError::InvalidInput("updated policy missing".into()))
        }).map_err(map_error)
    }

    pub fn plan_worker_removal(
        &self,
        req: &WorkerRemovalPlanRequest,
        inv: &WorkerRetentionInventory,
    ) -> Result<WorkerRemovalPlan, WorkerRetentionError> {
        self.plan_worker_removal_inner(req, inv, false)
    }

    /// An automatic Job cleanup may create its first canonical child, but cannot
    /// replace one hidden by authority/staleness filters after an uncertain effect.
    /// Blocked plans never pass begin/admit execution and remain diagnostic plans.
    /// The predecessor check and first-plan save share the canonical writer Tx.
    pub(crate) fn plan_job_cleanup_worker_removal(
        &self,
        req: &WorkerRemovalPlanRequest,
        inv: &WorkerRetentionInventory,
    ) -> Result<WorkerRemovalPlan, WorkerRetentionError> {
        self.plan_worker_removal_inner(req, inv, true)
    }

    fn plan_worker_removal_inner(
        &self,
        req: &WorkerRemovalPlanRequest,
        inv: &WorkerRetentionInventory,
        first_job_cleanup_child: bool,
    ) -> Result<WorkerRemovalPlan, WorkerRetentionError> {
        validate_plan(req, inv)?;
        if req
            .journal_erasure
            .as_ref()
            .is_some_and(|proof| !proof.matches(req, inv))
        {
            return Err(WorkerRetentionError::Invalid(
                "Journal erasure proof does not match removal source".into(),
            ));
        }
        let erased_journal = req.journal_erasure.is_some();
        let now = Utc::now().to_rfc3339();
        self.with_conn_mut(|conn| {
            let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if first_job_cleanup_child {
                let prior: Option<String> = tx.query_row(
                    "SELECT plan_id FROM worker_removal_operations WHERE workspace_id=?1 AND runtime_id=?2 AND worker_id=?3 AND state!='blocked' ORDER BY created_at,operation_id LIMIT 1",
                    params![req.workspace_id,req.worker.runtime_id,req.worker.worker_id], |row| row.get(0),
                ).optional()?;
                if let Some(plan_id) = prior {
                    return Err(StoreError::InvalidInput(format!("stale:{plan_id}:Job cleanup must reconcile its existing canonical removal; a filtered child is not absence")));
                }
            }
            // A Journal intent owns the original removal operation even if registry
            // observations move. Beginning it still rechecks policy, holds and work.
            let journal_plan: Option<String> = tx.query_row(
                "SELECT plan_id FROM worker_removal_operations WHERE workspace_id=?1 AND runtime_id=?2 AND worker_id=?3 AND journal_removal_json IS NOT NULL AND state IN ('planned','executing','failed')",
                params![req.workspace_id,req.worker.runtime_id,req.worker.worker_id], |row| row.get(0),
            ).optional()?;
            if let Some(plan_id) = journal_plan {
                let plan = load_plan(&tx, &plan_id)?.ok_or_else(||StoreError::Store("Journal removal plan disappeared".into()))?;
                require_current_subject_deletion_plan(&tx, &plan)?;
                tx.commit()?;
                return Ok(plan);
            }
            let policy=load_removal_policy(&tx,&req.workspace_id,&req.worker,erased_journal,true)?.ok_or_else(|| StoreError::InvalidInput(format!("policy-missing:{}",req.workspace_id)))?;
            let worker=match load_worker(&tx,&req.workspace_id,&req.worker)? {
                Some(v)=>v,
                None=>{
                    let other:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM worker_registry WHERE runtime_id=?1 AND worker_id=?2 AND workspace_id!=?3)",params![req.worker.runtime_id,req.worker.worker_id,req.workspace_id],|r|r.get(0))?;
                    return Err(StoreError::InvalidInput(if other{"cross-workspace".into()}else{"worker-missing".into()}));
                }
            };
            let pending:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM subject_deletion_removals WHERE workspace_id=?1 AND runtime_id=?2 AND worker_id=?3 AND completed=0)",params![req.workspace_id,req.worker.runtime_id,req.worker.worker_id],|row|row.get(0))?;
            if pending {return Err(StoreError::InvalidInput("Pinned pre-deletion removal must reconcile before another retention plan".into()));}
            if crate::store::subject_deletion_worker_kind(&tx, &req.workspace_id, &req.worker)?.is_some() {
                let prior = tx.prepare("SELECT plan_id FROM worker_removal_operations WHERE workspace_id=?1 AND runtime_id=?2 AND worker_id=?3 AND state IN ('executing','failed')")?
                    .query_map(params![req.workspace_id,req.worker.runtime_id,req.worker.worker_id],|r|r.get::<_,String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                for id in prior {
                    let prior=load_plan(&tx,&id)?.ok_or_else(||StoreError::Store("removal plan disappeared".into()))?;
                    require_current_subject_deletion_plan(&tx,&prior)?;
                }
            }
            let mut blockers=Vec::new();
            if worker.retention_state=="pinned" { blockers.push(WorkerRemovalBlocker::Hold); }
            if let Some((assignment_id,ticket_id))=tx.query_row("SELECT a.assignment_id,a.ticket_id FROM ticket_active_worker_assignments c JOIN ticket_worker_assignments a ON a.workspace_id=c.workspace_id AND a.ticket_id=c.ticket_id AND a.assignment_id=c.assignment_id WHERE a.workspace_id=?1 AND a.runtime_id=?2 AND a.worker_id=?3",params![req.workspace_id,req.worker.runtime_id,req.worker.worker_id],|r|Ok((r.get(0)?,r.get(1)?))).optional()? {
                blockers.push(WorkerRemovalBlocker::UnfinishedWork{assignment_id,ticket_id});
            }
            if let Some((operation_id,ticket_id))=tx.query_row("SELECT operation_id,ticket_id FROM ticket_assignment_operations WHERE workspace_id=?1 AND runtime_id=?2 AND worker_id=?3 AND claim_state='pending' AND action IN ('assign','reassign')",params![req.workspace_id,req.worker.runtime_id,req.worker.worker_id],|r|Ok((r.get(0)?,r.get(1)?))).optional()? {
                blockers.push(WorkerRemovalBlocker::PendingTicketClaim{operation_id,ticket_id});
            }
            // Cleanup may bind its first canonical child only after admission.
            // A hold/unfinished responsibility is not a child to execute or replace.
            // Once a row exists the earlier canonical-child check still fences it.
            if first_job_cleanup_child && !blockers.is_empty() {
                return Err(StoreError::InvalidInput(format!("blocked:{}", serde_json::to_string(&blockers).map_err(|error| StoreError::Store(error.to_string()))?)));
            }
            let fp=fingerprint(req,&worker.updated_at,inv,&policy,&blockers)?;
            let plan_id=stable("wrp",&fp); let operation_id=stable("wro",&fp);
            let archive_id=(policy.session_disposition==SessionDisposition::Archive).then(||stable("wra",&fp));
            let state=if blockers.is_empty(){WorkerRemovalPlanState::Planned}else{WorkerRemovalPlanState::Blocked};
            tx.execute("INSERT OR IGNORE INTO worker_removal_operations(operation_id,plan_id,input_fingerprint,workspace_id,runtime_id,worker_id,worker_updated_at,policy_id,policy_digest,session_disposition,metadata_disposition,archive_retention_kind,archive_retention_seconds,diagnostics_disposition,diagnostics_retention_seconds,archive_id,blockers_json,state,reason,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?20)",params![operation_id,plan_id,fp,req.workspace_id,req.worker.runtime_id,req.worker.worker_id,worker.updated_at,policy.policy_id,policy.policy_digest,sess(policy.session_disposition),meta(policy.metadata_disposition),archive_kind(policy.archive_retention),archive_seconds(policy.archive_retention),diag(policy.diagnostics_disposition),policy.diagnostics_retention_seconds,archive_id,serde_json::to_string(&blockers).map_err(|e|StoreError::InvalidInput(e.to_string()))?,state_s(state),req.reason,now])?;
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
            require_current_subject_deletion_plan(&tx, &plan)?;
            let journal_pinned = journal_removal_is_pinned(&tx, &plan.operation_id)?;
            if plan.state==WorkerRemovalPlanState::Succeeded{tx.commit()?;return Ok(plan);}
            if plan.state==WorkerRemovalPlanState::Blocked {
                return Err(StoreError::InvalidInput(format!("blocked:{}",serde_json::to_string(&plan.blockers).unwrap())));
            }
            if !matches!(plan.state, WorkerRemovalPlanState::Planned | WorkerRemovalPlanState::Failed | WorkerRemovalPlanState::Executing) {
                return Err(StoreError::InvalidInput(format!("stale:{plan_id}:plan state {} is not executable", state_s(plan.state))));
            }
            if !plan.blockers.is_empty(){return Err(StoreError::InvalidInput(format!("blocked:{}",serde_json::to_string(&plan.blockers).unwrap())));}
            let reconciliation=crate::store::subject_deletion_removal_matches(&tx,workspace_id,&plan.worker,&plan.operation_id,&plan.input_fingerprint)?;
            if !reconciliation {
                let policy=load_removal_policy(&tx,workspace_id,&plan.worker,plan.policy_id==ERASED_JOURNAL_POLICY_ID,plan.policy_id=="backend-subject-removal-archive-v1")?.ok_or_else(||StoreError::InvalidInput(format!("policy-missing:{workspace_id}")))?;
                if policy.policy_id!=plan.policy_id||policy.policy_digest!=plan.policy_digest{
                    if !journal_pinned {mark_stale(&tx,&plan,"policy content changed")?;}tx.commit()?;
                    return Err(stale_error(&plan,"policy content changed"));
                }
            }
            let worker=load_worker(&tx,workspace_id,&plan.worker)?.ok_or_else(||StoreError::InvalidInput(format!("stale:{plan_id}:Worker missing")))?;
            if !reconciliation && !journal_pinned && worker.updated_at!=plan.worker_updated_at{
                mark_stale(&tx,&plan,"Worker updated_at changed")?;tx.commit()?;
                return Err(stale_error(&plan,"Worker updated_at changed"));
            }
            if worker.retention_state=="pinned"{
                if !reconciliation && !journal_pinned {mark_stale(&tx,&plan,"hold added")?;}tx.commit()?;
                return Err(stale_error(&plan,"hold added"));
            }
            let assigned:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM ticket_active_worker_assignments c JOIN ticket_worker_assignments a ON a.workspace_id=c.workspace_id AND a.ticket_id=c.ticket_id AND a.assignment_id=c.assignment_id WHERE a.workspace_id=?1 AND a.runtime_id=?2 AND a.worker_id=?3 UNION ALL SELECT 1 FROM ticket_assignment_operations WHERE workspace_id=?1 AND runtime_id=?2 AND worker_id=?3 AND claim_state='pending' AND action IN ('assign','reassign'))",params![workspace_id,plan.worker.runtime_id,plan.worker.worker_id],|r|r.get(0))?;
            if assigned{
                if !reconciliation && !journal_pinned {mark_stale(&tx,&plan,"unfinished work added")?;}tx.commit()?;
                return Err(stale_error(&plan,"unfinished work added"));
            }
            let now=Utc::now().to_rfc3339();
            tx.execute("UPDATE worker_removal_operations SET state='executing',updated_at=?1 WHERE operation_id=?2",params![now,plan.operation_id])?;
            plan.state=WorkerRemovalPlanState::Executing;plan.updated_at=now;tx.commit()?;Ok(plan)
        }).map_err(map_error)
    }

    /// Revalidates Backend authority and derives the complete Runtime request
    /// from the immutable plan. Callers cannot substitute Worker timestamps or
    /// dispositions without causing a fingerprint/manifest mismatch.
    pub fn prepare_worker_removal_execution(
        &self,
        workspace_id: &str,
        plan_id: &str,
        input_fingerprint: &str,
    ) -> Result<PreparedWorkerRemoval, WorkerRetentionError> {
        let plan = self.begin_worker_removal(workspace_id, plan_id, input_fingerprint)?;
        if let Some(reconciliation) = self.subject_deletion_removal(workspace_id, &plan.worker)?
            && reconciliation.request.operation_id == plan.operation_id
        {
            return Ok(PreparedWorkerRemoval {
                runtime_request: reconciliation.request,
                prior_failure_category: plan.failure_category.clone(),
                plan,
            });
        }
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
        self.with_conn(|conn| require_current_subject_deletion_plan(conn, &plan))
            .map_err(map_error)?;
        let prior_failure_category = plan.failure_category.clone();
        Ok(PreparedWorkerRemoval {
            runtime_request: WorkerRetentionExecutionRequest {
                operation_id: plan.operation_id.clone(),
                input_fingerprint: plan.input_fingerprint.clone(),
                archive_id: plan.archive_id.clone(),
                workspace_id: plan.workspace_id.clone(),
                source_runtime_id: plan.worker.runtime_id.clone(),
                worker_id: worker_id,
                source_created_at: worker.created_at,
                removed_at,
                effective_profile: worker.profile,
                retention_class: None,
                policy_id: plan.policy_id.clone(),
                session_disposition: plan.session_disposition,
                diagnostics_disposition: plan.diagnostics_disposition,
            },
            plan,
            prior_failure_category,
        })
    }

    /// Observe the selected canonical plan without reconstructing a Runtime request.
    /// Completed metadata Purge receipts remain observable without a tombstone;
    /// observing a plan does not authorize execution or invent source identity.
    pub fn recover_worker_removal_plan(
        &self,
        workspace_id: &str,
        worker: &RuntimeWorkerRef,
    ) -> Result<Option<WorkerRemovalPlan>, WorkerRetentionError> {
        bounded("workspace", workspace_id, 160)?;
        let plan = self.with_conn(|conn| {
            conn.query_row(
                "SELECT plan_id FROM worker_removal_operations
                 WHERE workspace_id=?1 AND runtime_id=?2 AND worker_id=?3
                   AND state IN ('planned','executing','failed','succeeded')
                   AND (
                     state='succeeded' OR journal_removal_json IS NOT NULL OR EXISTS(
                       SELECT 1 FROM subject_deletion_removals d
                       WHERE d.workspace_id=worker_removal_operations.workspace_id
                         AND d.runtime_id=worker_removal_operations.runtime_id
                         AND d.worker_id=worker_removal_operations.worker_id
                         AND d.operation_id=worker_removal_operations.operation_id
                     ) OR worker_updated_at=(
                       SELECT updated_at FROM worker_registry
                       WHERE workspace_id=?1 AND runtime_id=?2 AND worker_id=?3
                     )
                   )
                 ORDER BY CASE WHEN EXISTS(SELECT 1 FROM subject_deletion_workers d WHERE d.workspace_id=worker_removal_operations.workspace_id AND d.runtime_id=worker_removal_operations.runtime_id AND d.worker_id=worker_removal_operations.worker_id AND d.prior_removal_operation_id=worker_removal_operations.operation_id) THEN 0 WHEN EXISTS(SELECT 1 FROM subject_deletion_removals d WHERE d.workspace_id=worker_removal_operations.workspace_id AND d.runtime_id=worker_removal_operations.runtime_id AND d.worker_id=worker_removal_operations.worker_id AND d.operation_id=worker_removal_operations.operation_id) THEN 1 WHEN journal_removal_json IS NOT NULL AND state IN ('planned','executing','failed') THEN 2 WHEN state='succeeded' THEN 3 ELSE 4 END,
                          created_at DESC,operation_id DESC LIMIT 1",
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
        self.with_conn(|conn| require_current_subject_deletion_plan(conn, &plan))
            .map_err(map_error)?;
        Ok(Some(plan))
    }

    pub fn recover_worker_removal_execution(
        &self,
        workspace_id: &str,
        worker: &RuntimeWorkerRef,
    ) -> Result<Option<PreparedWorkerRemoval>, WorkerRetentionError> {
        let Some(plan) = self.recover_worker_removal_plan(workspace_id, worker)? else {
            return Ok(None);
        };
        let prior_failure_category = plan.failure_category.clone();
        if let Some(reconciliation) = self.subject_deletion_removal(workspace_id, &plan.worker)?
            && reconciliation.request.operation_id == plan.operation_id
        {
            return Ok(Some(PreparedWorkerRemoval {
                runtime_request: reconciliation.request,
                prior_failure_category,
                plan,
            }));
        }
        let worker_id = plan
            .worker
            .worker_id
            .parse::<worker_runtime::identity::WorkerId>()
            .map_err(|_| {
                WorkerRetentionError::Invalid("Worker id must be a canonical UUIDv7".to_string())
            })?;
        let (source_created_at, effective_profile) =
            if plan.state == WorkerRemovalPlanState::Succeeded {
                // The registry row is gone, but an exact replay still needs the
                // original source identity used by the Runtime removal freeze.
                let tombstone = self
                    .worker_tombstone(workspace_id, &plan.worker)?
                    .filter(|tombstone| tombstone.operation_id == plan.operation_id)
                    .ok_or_else(|| WorkerRetentionError::StalePlan {
                        plan_id: plan.plan_id.clone(),
                        reason: "completed removal has no matching Worker tombstone".into(),
                    })?;
                (tombstone.created_at, tombstone.profile)
            } else {
                let worker = self
                    .with_conn(|conn| load_worker(conn, workspace_id, &plan.worker))?
                    .ok_or(WorkerRetentionError::WorkerNotFound)?;
                (worker.created_at, worker.profile)
            };
        Ok(Some(PreparedWorkerRemoval {
            runtime_request: WorkerRetentionExecutionRequest {
                operation_id: plan.operation_id.clone(),
                input_fingerprint: plan.input_fingerprint.clone(),
                archive_id: plan.archive_id.clone(),
                workspace_id: plan.workspace_id.clone(),
                source_runtime_id: plan.worker.runtime_id.clone(),
                worker_id: worker_id,
                source_created_at,
                removed_at: plan.created_at.clone(),
                effective_profile,
                retention_class: None,
                policy_id: plan.policy_id.clone(),
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
            crate::store::record_journal_removal_physical_failure(&tx, operation_id)?;
            tx.commit()?;Ok(())
        })?;
        Ok(())
    }

    /// An unknown acceptance is not failure. Supersede only after physical erasure
    /// fenced Archive effects and Runtime attested the same remaining aggregate.
    pub(crate) fn supersede_erased_worker_removal(
        &self,
        prepared: &PreparedWorkerRemoval,
        ack: &RuntimeJournalErasureAck,
        inventory: &WorkerRetentionInventory,
    ) -> Result<(), WorkerRetentionError> {
        let proof = ack.domain_proof();
        if proof.workspace_id != prepared.plan.workspace_id
            || proof.worker != prepared.plan.worker
            || prepared.plan.session_disposition != SessionDisposition::Archive
            || inventory.workspace_id != proof.workspace_id
            || inventory.runtime_id != proof.worker.runtime_id
            || inventory.worker_id.to_string() != proof.worker.worker_id
            || inventory
                .session_id
                .as_ref()
                .is_some_and(|id| id != &proof.session_id)
        {
            return Err(WorkerRetentionError::Invalid(
                "Erasure reconciliation source mismatch".into(),
            ));
        }
        self.with_conn_mut(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let plan = load_plan_op(&tx, &prepared.plan.operation_id)?
                .ok_or_else(|| StoreError::InvalidInput("operation missing".into()))?;
            if plan != prepared.plan
                || !matches!(
                    plan.state,
                    WorkerRemovalPlanState::Planned
                        | WorkerRemovalPlanState::Executing
                        | WorkerRemovalPlanState::Failed
                )
            {
                return Err(StoreError::InvalidInput("active operation mismatch".into()));
            }
            insert_erasure_reconciliation_audit(&tx, &plan, ack, None)?;
            mark_stale(&tx, &plan, "journal_session_erased_archive_fenced")?;
            tx.commit()?;
            Ok(())
        })?;
        Ok(())
    }

    pub(crate) fn audit_erased_worker_removal_receipt(
        &self,
        prepared: &PreparedWorkerRemoval,
        ack: &RuntimeJournalErasureAck,
        result: &WorkerRetentionExecutionResult,
    ) -> Result<(), WorkerRetentionError> {
        let proof = ack.domain_proof();
        if proof.workspace_id != prepared.plan.workspace_id
            || proof.worker != prepared.plan.worker
            || result.operation_id != prepared.plan.operation_id
            || result.input_fingerprint != prepared.plan.input_fingerprint
            || result.worker_id.to_string() != proof.worker.worker_id
            || !result.source_removed
            || result.session_disposition != prepared.plan.session_disposition
            || result.diagnostics_disposition != prepared.plan.diagnostics_disposition
            || result.archive.as_ref().is_none_or(|a| {
                a.source_session_id != proof.session_id
                    || Some(&a.archive_id) != prepared.plan.archive_id.as_ref()
                    || a.workspace_id != proof.workspace_id
                    || a.source_runtime_id != proof.worker.runtime_id
                    || a.source_worker_id.to_string() != proof.worker.worker_id
                    || a.operation_id != result.operation_id
                    || a.input_fingerprint != result.input_fingerprint
            })
        {
            return Err(WorkerRetentionError::Invalid(
                "Erasure receipt mismatch".into(),
            ));
        }
        self.with_conn_mut(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            insert_erasure_reconciliation_audit(&tx, &prepared.plan, ack, Some(result))?;
            tx.commit()?;
            Ok(())
        })?;
        Ok(())
    }

    pub fn commit_worker_removal(
        &self,
        workspace_id: &str,
        operation_id: &str,
        fp: &str,
        result: &WorkerRetentionExecutionResult,
    ) -> Result<WorkerRemovalCommit, WorkerRetentionError> {
        self.commit_worker_removal_inner(workspace_id, operation_id, fp, result, None)
    }

    /// Receipt-confirmed removal after erasure must not republish the erased Archive.
    /// The original operation, fingerprint and Runtime result remain unchanged.
    pub(crate) fn commit_erased_worker_removal(
        &self,
        workspace_id: &str,
        prepared: &PreparedWorkerRemoval,
        result: &WorkerRetentionExecutionResult,
        ack: &RuntimeJournalErasureAck,
    ) -> Result<WorkerRemovalCommit, WorkerRetentionError> {
        self.commit_worker_removal_inner(
            workspace_id,
            &prepared.plan.operation_id,
            &prepared.plan.input_fingerprint,
            result,
            Some(ack),
        )
    }

    fn commit_worker_removal_inner(
        &self,
        workspace_id: &str,
        operation_id: &str,
        fp: &str,
        result: &WorkerRetentionExecutionResult,
        erasure: Option<&RuntimeJournalErasureAck>,
    ) -> Result<WorkerRemovalCommit, WorkerRetentionError> {
        self.with_conn_mut(|conn|{
            let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let mut plan=load_plan_op(&tx,operation_id)?.ok_or_else(||StoreError::InvalidInput("operation missing".into()))?;
            if plan.workspace_id!=workspace_id{return Err(StoreError::InvalidInput("cross-workspace".into()));}
            if plan.input_fingerprint!=fp||result.input_fingerprint!=fp||result.operation_id!=operation_id{return Err(StoreError::InvalidInput(format!("fingerprint:{operation_id}")));}
            require_current_subject_deletion_plan(&tx, &plan)?;
            if plan.state==WorkerRemovalPlanState::Succeeded{
                // Repair projection fences left by the pre-atomic implementation and
                // return a replayable change for a broadcast missed after commit.
                let catalog=commit_worker_catalog_removal(&tx,workspace_id,&plan.worker)?;
                tx.commit()?;
                return Ok(WorkerRemovalCommit { plan, catalog });
            }
            if plan.state != WorkerRemovalPlanState::Executing {
                return Err(StoreError::InvalidInput(format!("stale:{}:plan state {} is not committable", plan.plan_id, state_s(plan.state))));
            }
            let journal_pinned = journal_removal_is_pinned(&tx, &plan.operation_id)?;
            if result.worker_id.to_string() != plan.worker.worker_id
                || result.session_disposition != plan.session_disposition
                || result.diagnostics_disposition != plan.diagnostics_disposition
            {
                return Err(StoreError::InvalidInput("Runtime retention result does not match removal plan".into()));
            }
            if !result.source_removed{return Err(StoreError::InvalidInput("Runtime source was not removed".into()));}
            if let Some(ack) = erasure {
                let proof = ack.domain_proof();
                if proof.workspace_id != workspace_id || proof.worker != plan.worker
                    || result.archive.as_ref().is_none_or(|a| a.source_session_id != proof.session_id) {
                    return Err(StoreError::InvalidInput("Erased removal receipt requires matching source and physical Runtime erasure acknowledgement".into()));
                }
            }
            let reconciliation=crate::store::subject_deletion_removal_matches(&tx,workspace_id,&plan.worker,&plan.operation_id,&plan.input_fingerprint)?;
            let worker=load_worker(&tx,workspace_id,&plan.worker)?.ok_or_else(||StoreError::InvalidInput("Worker missing before commit".into()))?;
            // A pinned pre-deletion request is reconciled unchanged under the permanent
            // Subject/Runtime exclusion. Its Worker identity cannot be reused. Metadata
            // version movement must not discard an already-applied canonical result.
            if !reconciliation && !journal_pinned && worker.updated_at!=plan.worker_updated_at{return Err(StoreError::InvalidInput(format!("stale:{}:Worker updated_at changed",plan.plan_id)));}
            if worker.retention_state=="pinned" { return Err(StoreError::InvalidInput(format!("stale:{}:hold added",plan.plan_id))); }
            let assigned:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM ticket_active_worker_assignments c JOIN ticket_worker_assignments a ON a.workspace_id=c.workspace_id AND a.ticket_id=c.ticket_id AND a.assignment_id=c.assignment_id WHERE a.workspace_id=?1 AND a.runtime_id=?2 AND a.worker_id=?3 UNION ALL SELECT 1 FROM ticket_assignment_operations WHERE workspace_id=?1 AND runtime_id=?2 AND worker_id=?3 AND claim_state='pending' AND action IN ('assign','reassign'))",params![workspace_id,plan.worker.runtime_id,plan.worker.worker_id],|row|row.get(0))?;
            if assigned { return Err(StoreError::InvalidInput(format!("stale:{}:unfinished work added",plan.plan_id))); }
            let now=Utc::now().to_rfc3339();
            if let Some(a)=&result.archive{
                if plan.archive_id.as_deref()!=Some(&a.archive_id)||a.workspace_id!=workspace_id||a.source_runtime_id!=plan.worker.runtime_id||a.source_worker_id.to_string()!=plan.worker.worker_id||a.policy_id!=plan.policy_id||a.operation_id!=operation_id||a.input_fingerprint!=fp{return Err(StoreError::InvalidInput("archive manifest mismatch".into()));}
                if erasure.is_none() {
                let expires_at=match plan.archive_retention { ArchiveRetention::Forever=>None, ArchiveRetention::ForSeconds{seconds}=>{let seconds=i64::try_from(seconds).map_err(|_|StoreError::InvalidInput("archive retention deadline overflow".into()))?;Some((Utc::now()+chrono::Duration::seconds(seconds)).to_rfc3339())} };
                tx.execute("INSERT OR IGNORE INTO worker_session_archives(archive_id,workspace_id,runtime_id,worker_id,session_id,checksum_sha256,content_bytes,policy_id,policy_digest,operation_id,committed_at,expires_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",params![a.archive_id,workspace_id,plan.worker.runtime_id,plan.worker.worker_id,a.source_session_id,a.content_checksum_sha256,a.content_bytes,plan.policy_id,plan.policy_digest,operation_id,now,expires_at])?;
                let observe_grants = {
                    let mut statement = tx.prepare(
                        "SELECT grant_id, controller_runtime_id, controller_worker_id,
                                permissions_json, created_at
                         FROM worker_control_grants
                         WHERE workspace_id = ?1 AND subject_runtime_id = ?2
                           AND subject_worker_id = ?3 AND revoked_at IS NULL",
                    )?;
                    statement
                        .query_map(
                            params![workspace_id, plan.worker.runtime_id, plan.worker.worker_id],
                            |row| {
                                Ok((
                                    row.get::<_, String>(0)?,
                                    row.get::<_, String>(1)?,
                                    row.get::<_, String>(2)?,
                                    row.get::<_, String>(3)?,
                                    row.get::<_, String>(4)?,
                                ))
                            },
                        )?
                        .collect::<std::result::Result<Vec<_>, _>>()?
                };
                for (grant_id, controller_runtime_id, controller_worker_id, permissions, created_at) in observe_grants {
                    let permissions: Vec<String> = serde_json::from_str(&permissions)
                        .map_err(|error| StoreError::Store(format!("invalid Worker control grant permissions: {error}")))?;
                    if permissions.iter().any(|permission| permission == "observe") {
                        tx.execute(
                            "INSERT OR IGNORE INTO worker_session_archive_observe_grants
                             (workspace_id,archive_id,controller_runtime_id,controller_worker_id,
                              subject_runtime_id,subject_worker_id,source_grant_id,granted_at)
                             VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                            params![
                                workspace_id,
                                a.archive_id,
                                controller_runtime_id,
                                controller_worker_id,
                                plan.worker.runtime_id,
                                plan.worker.worker_id,
                                grant_id,
                                created_at
                            ],
                        )?;
                    }
                }
                }
            }else if plan.session_disposition==SessionDisposition::Archive{return Err(StoreError::InvalidInput("archive manifest missing".into()));}
            match plan.diagnostics_disposition {
                DiagnosticsDisposition::Purge if result.diagnostics_retained => return Err(StoreError::InvalidInput("Runtime retained diagnostics for purge disposition".into())),
                DiagnosticsDisposition::Retain if !result.diagnostics_retained => return Err(StoreError::InvalidInput("Runtime did not retain diagnostics".into())),
                DiagnosticsDisposition::Retain if erasure.is_none() => {
                    let seconds=plan.diagnostics_retention_seconds.ok_or_else(||StoreError::InvalidInput("diagnostics retention deadline missing".into()))?;
                    let seconds=i64::try_from(seconds).map_err(|_|StoreError::InvalidInput("diagnostics retention deadline overflow".into()))?;
                    let expires_at=(Utc::now()+chrono::Duration::seconds(seconds)).to_rfc3339();
                    tx.execute("INSERT OR IGNORE INTO worker_diagnostics_archives(operation_id,workspace_id,runtime_id,worker_id,policy_id,policy_digest,committed_at,expires_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![operation_id,workspace_id,plan.worker.runtime_id,plan.worker.worker_id,plan.policy_id,plan.policy_digest,now,expires_at])?;
                }
                DiagnosticsDisposition::Purge | DiagnosticsDisposition::Retain => {}
            }
            if plan.metadata_disposition==MetadataDisposition::Tombstone{
                tx.execute("INSERT OR IGNORE INTO worker_tombstones(workspace_id,runtime_id,worker_id,display_name,profile,worker_created_at,removed_at,archive_id,policy_id,policy_digest,operation_id) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",params![workspace_id,plan.worker.runtime_id,plan.worker.worker_id,worker.display_name,worker.profile,worker.created_at,now,if erasure.is_some() { None } else { plan.archive_id.clone() },plan.policy_id,plan.policy_digest,operation_id])?;
            }
            tx.execute("UPDATE worker_workdir_links SET unlinked_at=?4 WHERE workspace_id=?1 AND runtime_id=?2 AND worker_id=?3 AND unlinked_at IS NULL",params![workspace_id,plan.worker.runtime_id,plan.worker.worker_id,now])?;
            let deleted=tx.execute("DELETE FROM worker_registry WHERE workspace_id=?1 AND runtime_id=?2 AND worker_id=?3 AND (updated_at=?4 OR ?5)",params![workspace_id,plan.worker.runtime_id,plan.worker.worker_id,plan.worker_updated_at,reconciliation || journal_pinned])?;
            tx.execute("UPDATE worker_create_reservations SET state='removed',updated_at=?4 WHERE workspace_id=?1 AND runtime_id=?2 AND worker_id=?3 AND state IN ('reserved','created')",params![workspace_id,plan.worker.runtime_id,plan.worker.worker_id,now])?;
            if deleted!=1{return Err(StoreError::InvalidInput(format!("stale:{}:removal fence changed",plan.plan_id)));}
            let catalog=commit_worker_catalog_removal(&tx,workspace_id,&plan.worker)?;
            // Explicit erasure retains any incomplete Journal observation; the
            // physical erasure acknowledgement is not Journal completion proof.
            if erasure.is_none() {crate::store::complete_journal_removal(&tx, operation_id)?;}
            tx.execute("UPDATE worker_removal_operations SET state='succeeded',failure_category=NULL,updated_at=?1 WHERE operation_id=?2",params![now,operation_id])?;
            if let Some(ack) = erasure {
                insert_erasure_reconciliation_audit(&tx, &plan, ack, Some(result))?;
            }
            tx.execute("INSERT OR IGNORE INTO worker_retention_audit_events(event_id,operation_id,workspace_id,event_kind,detail,created_at) VALUES(?1,?2,?3,'worker_removed',?4,?5)",params![stable("wre",operation_id),operation_id,workspace_id,format!("runtime_id={} worker_id={} session={} metadata={} diagnostics={}",plan.worker.runtime_id,plan.worker.worker_id,sess(plan.session_disposition),meta(plan.metadata_disposition),diag(plan.diagnostics_disposition)),now])?;
            tx.commit()?;plan.state=WorkerRemovalPlanState::Succeeded;plan.updated_at=now;Ok(WorkerRemovalCommit { plan, catalog })
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
        self.with_conn(|conn|conn.query_row("SELECT display_name,profile,worker_created_at,removed_at,archive_id,policy_id,policy_digest,operation_id FROM worker_tombstones WHERE workspace_id=?1 AND runtime_id=?2 AND worker_id=?3",params![workspace_id,worker.runtime_id,worker.worker_id],|r|Ok(WorkerTombstone{workspace_id:workspace_id.into(),worker:worker.clone(),display_name:r.get(0)?,profile:r.get(1)?,created_at:r.get(2)?,removed_at:r.get(3)?,archive_id:r.get(4)?,policy_id:r.get(5)?,policy_digest:r.get(6)?,operation_id:r.get(7)?})).optional().map_err(StoreError::from))
    }

    pub fn worker_session_archive(
        &self,
        workspace_id: &str,
        worker: &RuntimeWorkerRef,
        session_id: &str,
    ) -> crate::Result<Option<WorkerSessionArchiveRecord>> {
        self.with_conn(|connection| {
            connection
                .query_row(
                    "SELECT archive_id, checksum_sha256, content_bytes, policy_id,
                            policy_digest, operation_id, committed_at, expires_at
                     FROM worker_session_archives
                     WHERE workspace_id = ?1 AND runtime_id = ?2 AND worker_id = ?3
                       AND session_id = ?4",
                    params![
                        workspace_id,
                        worker.runtime_id,
                        worker.worker_id,
                        session_id
                    ],
                    |row| {
                        Ok(WorkerSessionArchiveRecord {
                            archive_id: row.get(0)?,
                            workspace_id: workspace_id.to_string(),
                            worker: worker.clone(),
                            session_id: session_id.to_string(),
                            checksum_sha256: row.get(1)?,
                            content_bytes: row.get::<_, i64>(2)? as u64,
                            policy_id: row.get(3)?,
                            policy_digest: row.get(4)?,
                            operation_id: row.get(5)?,
                            committed_at: row.get(6)?,
                            expires_at: row.get(7)?,
                        })
                    },
                )
                .optional()
                .map_err(StoreError::from)
        })
    }

    pub fn worker_session_archive_observable_by(
        &self,
        workspace_id: &str,
        archive_id: &str,
        controller: &RuntimeWorkerRef,
        subject: &RuntimeWorkerRef,
    ) -> crate::Result<bool> {
        self.with_conn(|connection| {
            connection
                .query_row(
                    "SELECT EXISTS(
                         SELECT 1 FROM worker_session_archive_observe_grants
                         WHERE workspace_id = ?1 AND archive_id = ?2
                           AND controller_runtime_id = ?3 AND controller_worker_id = ?4
                           AND subject_runtime_id = ?5 AND subject_worker_id = ?6
                           AND revoked_at IS NULL
                     )",
                    params![
                        workspace_id,
                        archive_id,
                        controller.runtime_id,
                        controller.worker_id,
                        subject.runtime_id,
                        subject.worker_id
                    ],
                    |row| row.get(0),
                )
                .map_err(StoreError::from)
        })
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
/// Recording Sessions may contain original text in tool results. A terminal,
/// Host-bound Journal attempt or a validated source-erasure proof grants this
/// exception; profile names and serialized deletion input grant nothing.
/// A saved erasure plan retains the monotonic decision across restart, but creates
/// no Runtime erase authority: the domain/physical tombstones must already exist.
/// This plan-local policy never changes the Workspace conversation policy.
fn load_removal_policy(
    c: &Connection,
    w: &str,
    worker: &RuntimeWorkerRef,
    erased_journal: bool,
    preserve_subject_history: bool,
) -> crate::Result<Option<WorkerRetentionPolicy>> {
    let Some(mut policy) = load_policy(c, w)? else {
        return Ok(None);
    };
    // Snapshot authority outranks normal policy and Journal cleanup policy. Body
    // and delegated Sessions must never inherit the ephemeral Job exception.
    if let Some(is_job) = crate::store::subject_deletion_worker_kind(c, w, worker)? {
        let update = WorkerRetentionPolicyUpdate {
            policy_id: if is_job {
                "subject-deletion-job-v1"
            } else {
                "subject-deletion-conversation-v1"
            }
            .into(),
            session_disposition: if is_job {
                SessionDisposition::Purge
            } else {
                SessionDisposition::Archive
            },
            metadata_disposition: MetadataDisposition::Tombstone,
            archive_retention: ArchiveRetention::Forever,
            // Runtime calls diagnostic archiving Retain, not Archive. Preserve
            // the Workspace's duration, or thirty days when it normally purges.
            diagnostics_disposition: if is_job {
                DiagnosticsDisposition::Purge
            } else {
                DiagnosticsDisposition::Retain
            },
            diagnostics_retention_seconds: if is_job {
                None
            } else {
                Some(
                    policy
                        .diagnostics_retention_seconds
                        .unwrap_or(30 * 24 * 60 * 60),
                )
            },
        };
        policy.policy_id = update.policy_id.clone();
        policy.policy_digest = worker_retention_policy_digest(&update);
        policy.session_disposition = update.session_disposition;
        policy.metadata_disposition = update.metadata_disposition;
        policy.archive_retention = update.archive_retention;
        policy.diagnostics_disposition = update.diagnostics_disposition;
        policy.diagnostics_retention_seconds = update.diagnostics_retention_seconds;
        return Ok(Some(policy));
    }
    if erased_journal || crate::store::terminal_journal_job_worker(c, w, worker)? {
        let update = WorkerRetentionPolicyUpdate {
            policy_id: if erased_journal {
                ERASED_JOURNAL_POLICY_ID.into()
            } else {
                "backend-journal-ephemeral-session-v1".into()
            },
            session_disposition: SessionDisposition::Purge,
            metadata_disposition: MetadataDisposition::Tombstone,
            archive_retention: ArchiveRetention::Forever,
            diagnostics_disposition: DiagnosticsDisposition::Purge,
            diagnostics_retention_seconds: None,
        };
        policy.policy_id = update.policy_id.clone();
        policy.policy_digest = worker_retention_policy_digest(&update);
        policy.session_disposition = update.session_disposition;
        policy.metadata_disposition = update.metadata_disposition;
        policy.archive_retention = update.archive_retention;
        policy.diagnostics_disposition = update.diagnostics_disposition;
        policy.diagnostics_retention_seconds = None;
    } else if preserve_subject_history && policy.session_disposition == SessionDisposition::Purge && c.query_row(
        "SELECT EXISTS(SELECT 1 FROM worker_create_reservations WHERE workspace_id=?1 AND runtime_id=?2 AND worker_id=?3 AND substr(singleton_key,1,10)='subjektiv:')",
        params![w, worker.runtime_id, worker.worker_id],
        |r| r.get::<_, bool>(0),
    )? {
        // Removing a Subject body releases occupancy, not its original history.
        // A configured Purge must not silently erase an unfinished Journal. Explicit
        // Session erasure uses the separate, proof-bound branch above.
        let update = WorkerRetentionPolicyUpdate {
            policy_id: "backend-subject-removal-archive-v1".into(),
            session_disposition: SessionDisposition::Archive,
            metadata_disposition: policy.metadata_disposition,
            archive_retention: policy.archive_retention,
            diagnostics_disposition: policy.diagnostics_disposition,
            diagnostics_retention_seconds: policy.diagnostics_retention_seconds,
        };
        policy.policy_id = update.policy_id.clone();
        policy.policy_digest = worker_retention_policy_digest(&update);
        policy.session_disposition = update.session_disposition;
    }
    Ok(Some(policy))
}
fn require_current_subject_deletion_plan(
    c: &Connection,
    plan: &WorkerRemovalPlan,
) -> crate::Result<()> {
    if crate::store::subject_deletion_removal_matches(
        c,
        &plan.workspace_id,
        &plan.worker,
        &plan.operation_id,
        &plan.input_fingerprint,
    )? {
        // An unfinished canonical request is pinned separately from successes. Only
        // its exact original input may reconcile; any required Job copy erasure is
        // a subsequent physical operation and never changes this historical result.
        return Ok(());
    }
    if plan.state == WorkerRemovalPlanState::Succeeded
        && crate::store::subject_deletion_prior_removal_matches(
            c,
            &plan.workspace_id,
            &plan.worker,
            &plan.operation_id,
        )?
    {
        // An exact successful receipt pinned before Subject deletion owns its
        // independent original/archive lifecycle. Never retarget its policy.
        return Ok(());
    }
    if crate::store::subject_deletion_worker_kind(c, &plan.workspace_id, &plan.worker)?.is_some() {
        let current = load_removal_policy(c, &plan.workspace_id, &plan.worker, false, true)?
            .ok_or_else(|| StoreError::InvalidInput("Subject deletion policy missing".into()))?;
        if plan.session_disposition != current.session_disposition
            || plan.metadata_disposition != current.metadata_disposition
            || plan.archive_retention != current.archive_retention
            || plan.diagnostics_disposition != current.diagnostics_disposition
        {
            return Err(stale_error(
                plan,
                "Subject deletion retention differs from saved plan; reconcile canonical Runtime removal before retry",
            ));
        }
    }
    Ok(())
}

fn load_policy(c: &Connection, w: &str) -> crate::Result<Option<WorkerRetentionPolicy>> {
    c.query_row(
        "SELECT p.policy_id,p.policy_digest,r.session_disposition,r.metadata_disposition,
                r.archive_retention_kind,r.archive_retention_seconds,
                r.diagnostics_disposition,r.diagnostics_retention_seconds,r.created_at,p.updated_at
         FROM workspace_worker_retention_policies p
         JOIN workspace_worker_retention_policy_snapshots r
           ON r.workspace_id=p.workspace_id AND r.policy_id=p.policy_id AND r.policy_digest=p.policy_digest
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
                policy_digest: row.get(1)?,
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
                worker_updated_at,policy_id,policy_digest,session_disposition,
                metadata_disposition,archive_retention_kind,archive_retention_seconds,
                diagnostics_disposition,diagnostics_retention_seconds,archive_id,blockers_json,
                state,reason,created_at,updated_at,failure_category
         FROM worker_removal_operations WHERE {key}=?1"
    );
    c.query_row(&query, params![id], |row| {
        let session: String = row.get(9)?;
        let metadata: String = row.get(10)?;
        let archive_kind: String = row.get(11)?;
        let archive_seconds: Option<i64> = row.get(12)?;
        let diagnostics: String = row.get(13)?;
        let blockers: String = row.get(16)?;
        let state: String = row.get(17)?;
        Ok(WorkerRemovalPlan {
            plan_id: row.get(0)?,
            operation_id: row.get(1)?,
            input_fingerprint: row.get(2)?,
            workspace_id: row.get(3)?,
            worker: RuntimeWorkerRef {
                runtime_id: row.get(4)?,
                worker_id: row.get(5)?,
            },
            worker_updated_at: row.get(6)?,
            policy_id: row.get(7)?,
            policy_digest: row.get(8)?,
            session_disposition: parse_s(&session)?,
            metadata_disposition: parse_m(&metadata)?,
            archive_retention: parse_archive(&archive_kind, archive_seconds)?,
            diagnostics_disposition: parse_d(&diagnostics)?,
            diagnostics_retention_seconds: row.get::<_, Option<i64>>(14)?.map(|v| v as u64),
            archive_id: row.get(15)?,
            blockers: serde_json::from_str(&blockers).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    16,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?,
            state: parse_state(&state)?,
            reason: row.get(18)?,
            created_at: row.get(19)?,
            updated_at: row.get(20)?,
            failure_category: row.get(21)?,
        })
    })
    .optional()
    .map_err(StoreError::from)
}
fn insert_erasure_reconciliation_audit(
    tx: &rusqlite::Transaction<'_>,
    plan: &WorkerRemovalPlan,
    ack: &RuntimeJournalErasureAck,
    result: Option<&WorkerRetentionExecutionResult>,
) -> crate::Result<()> {
    let proof = ack.domain_proof();
    let kind = if result.is_some() {
        "erased_removal_receipt"
    } else {
        "erased_archive_superseded"
    };
    let detail = serde_json::json!({
        "input_fingerprint": plan.input_fingerprint,
        "session_id": proof.session_id,
        "archive_id": plan.archive_id,
        "source_removed": result.map(|r| r.source_removed),
        "old_diagnostics_retained": result.map(|r| r.diagnostics_retained),
        "runtime_erasure_ack": { "session_id": proof.session_id, "source": ack.source },
        "old_acceptance": if result.is_some() { "receipt_confirmed" } else { "unknown_archive_fenced" },
    });
    tx.execute("INSERT OR IGNORE INTO worker_retention_audit_events(event_id,operation_id,workspace_id,event_kind,detail,created_at) VALUES(?1,?2,?3,?4,?5,?6)",
        params![stable("wre", &format!("{}:{kind}", plan.operation_id)), plan.operation_id, plan.workspace_id, kind, detail.to_string(), Utc::now().to_rfc3339()])?;
    Ok(())
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
    worker_updated_at: &str,
    i: &WorkerRetentionInventory,
    p: &WorkerRetentionPolicy,
    b: &[WorkerRemovalBlocker],
) -> crate::Result<String> {
    serde_json::to_vec(&serde_json::json!([
        r.workspace_id,
        r.worker.runtime_id,
        r.worker.worker_id,
        worker_updated_at,
        i.session_id,
        i.segment_ids,
        p.policy_id,
        p.policy_digest,
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
/// Content identity of the complete resolved policy, excluding observation timestamps.
/// The ordered tuple is shared with schema seeding and migration; no update counter is used.
pub fn worker_retention_policy_digest(policy: &WorkerRetentionPolicyUpdate) -> String {
    let bytes = serde_json::to_vec(&serde_json::json!([
        policy.policy_id,
        policy.session_disposition,
        policy.metadata_disposition,
        policy.archive_retention,
        policy.diagnostics_disposition,
        policy.diagnostics_retention_seconds,
    ]))
    .expect("retention policy content is serializable");
    format!("sha256:{}", hash(&bytes))
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
        let (expected, actual) = serde_json::from_str::<(String, String)>(x).unwrap_or_default();
        return WorkerRetentionError::PolicyDigestConflict { expected, actual };
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
fn journal_removal_is_pinned(conn: &Connection, operation_id: &str) -> Result<bool, StoreError> {
    Ok(conn.query_row(
        "SELECT journal_removal_json IS NOT NULL FROM worker_removal_operations WHERE operation_id=?1",
        [operation_id], |row| row.get(0),
    )?)
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
    use crate::store::{
        ControlPlaneStore, TicketWorkerAssignmentRecord, WorkerCatalogChange, WorkerRegistryRecord,
    };
    use worker_runtime::identity::WorkerId;
    fn worker_id() -> WorkerId {
        WorkerId::from_legacy_u64(1)
    }
    fn setup() -> SqliteWorkspaceStore {
        let s = SqliteWorkspaceStore::in_memory_fixture().unwrap();
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
                 ) VALUES('w',?1,'r','one','builtin:coder','normal','created','2026-08-24T00:00:00Z')",
                [worker_id().to_string()],
            )?;
            c.execute(
                "INSERT INTO typed_tickets (workspace_id, ticket_id, slug, title, status, kind, priority, workflow_state, workflow_state_explicit) \
                 VALUES ('w', 'ticket', 'ticket', 'Ticket', 'open', 'task', 'normal', 'open', 1)",
                [],
            )?;
            c.execute("INSERT INTO typed_ticket_events(workspace_id,ticket_id,event_index,event_ref,kind,body) VALUES('w','ticket',-1,'ticket:-1','comment','')", [])?;
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
            journal_erasure: None,
        }
    }
    fn journal_binding(root: &std::path::Path) -> crate::store::JournalRemovalBinding {
        let manager = feature_storage::FeatureStorage::new(root);
        let registration = manager
            .register(feature_storage::FeatureRegistration::new(
                "journal-test",
                &[],
            ))
            .unwrap();
        let database = manager.scope("w").unwrap().open(&registration).unwrap();
        crate::store::JournalRemovalBinding {
            subject_id: "subject".into(),
            writer_session_id: "session".into(),
            writer_binding_id: "binding".into(),
            database: database.identity().clone(),
        }
    }

    fn frozen_journal_member(root: &std::path::Path) -> crate::store::FrozenJournalRemovalMember {
        use session_store::public_material::SessionPublicMaterialReader;
        use session_store::{FsStore, LogEntry, Store};
        let session_id = session_store::new_session_id();
        let segment = session_store::new_segment_id();
        let store = FsStore::new(root).unwrap();
        store
            .create_segment(
                session_id,
                segment,
                &[LogEntry::AnnotatedSegmentStart {
                    ts: 1,
                    session_id,
                    system_prompt: None,
                    config: Default::default(),
                    history: vec![session_store::LoggedHistoryEntry {
                        item: session_store::LoggedItem::Message {
                            role: session_store::LoggedRole::User,
                            content: vec![session_store::LoggedContentPart::Text {
                                text: "retained original body".into(),
                            }],
                        },
                        metadata: session_store::LoggedSessionHistoryMetadata {
                            input_metadata: None,
                            entry_id: session_store::LoggedSessionHistoryEntryId(
                                "original-input".into(),
                            ),
                            origin: session_store::LoggedSessionHistoryOrigin::LocalHumanInput,
                            derivation: None,
                        },
                    }],
                    forked_from: None,
                    compacted_from: None,
                }],
            )
            .unwrap();
        let reader =
            SessionPublicMaterialReader::open_fs(root, session_id, segment, Default::default())
                .unwrap();
        let descriptors: Vec<_> = reader.descriptors().cloned().collect();
        assert_eq!(descriptors.len(), 1);
        crate::store::FrozenJournalRemovalMember {
            session_id: session_id.to_string(),
            active_segment_id: Some(segment),
            materialized: true,
            end: session_store::capture::RecordingPosition {
                canonical_order: descriptors.len() as u64,
                byte_offset: 0,
            },
            descriptors,
        }
    }

    // Journal inputs were exported from the published owner, not reconstructed by
    // the current registry. The unrelated permission cutover is a fixture precondition.
    #[test]
    fn published_journal_histories_preserve_frozen_intents_through_owner_preflight_and_reopen() {
        use crate::store::{JournalRemovalCause as Cause, JournalRemovalPhase as Phase};
        use rusqlite::types::Value;
        type SavedTable = (String, Vec<String>, Vec<Vec<Value>>);
        fn quoted(name: &str) -> String {
            format!("\"{}\"", name.replace('"', "\"\""))
        }
        fn rows(
            conn: &Connection,
            table: &str,
            columns: &[String],
        ) -> crate::Result<Vec<Vec<Value>>> {
            let fields = columns
                .iter()
                .map(|name| quoted(name))
                .collect::<Vec<_>>()
                .join(",");
            let mut statement = conn.prepare(&format!(
                "SELECT {fields} FROM {} ORDER BY {fields}",
                quoted(table)
            ))?;
            Ok(statement
                .query_map([], |row| (0..columns.len()).map(|i| row.get(i)).collect())?
                .collect::<rusqlite::Result<Vec<_>>>()?)
        }
        fn snapshot(conn: &Connection) -> crate::Result<Vec<SavedTable>> {
            let tables = conn.prepare("SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")?
                .query_map([], |row| row.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
            tables
                .into_iter()
                .map(|table| {
                    let columns = conn
                        .prepare(&format!("PRAGMA table_info({})", quoted(&table)))?
                        .query_map([], |row| row.get::<_, String>(1))?
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    let data = rows(conn, &table, &columns)?;
                    Ok((table, columns, data))
                })
                .collect()
        }
        fn assert_preserved(
            conn: &Connection,
            saved: &[SavedTable],
            migrated: bool,
        ) -> crate::Result<()> {
            for (table, columns, before) in saved {
                // Schema130 removes obsolete execution history, not Journal intents.
                // Preflight must still leave every original table and row untouched.
                if migrated
                    && matches!(
                        table.as_str(),
                        "backend_job_deliveries"
                            | "notification_job_legacy_receipts"
                            | "backend_job_delivery_pre_content_history"
                            | "backend_job_attempt_pre_content_history"
                            | "backend_job_pre_content_history"
                            | "backend_job_attempt_upgrade_history"
                            | "backend_job_upgrade_history"
                            | "backend_job_attempt_legacy_state_history"
                            | "backend_job_legacy_state_history"
                    )
                {
                    assert!(!conn.query_row(
                        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name=?1)",
                        [table],
                        |row| row.get::<_, bool>(0),
                    )?, "obsolete Job table remains: {table}");
                    continue;
                }
                if table == "typed_ticket_orchestration_plans"
                    && !conn.query_row(
                        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name=?1)",
                        [table],
                        |row| row.get::<_, bool>(0),
                    )?
                {
                    assert!(
                        before.is_empty(),
                        "this Journal fixture has no Plan memos to retire"
                    );
                    ticket::verify_sqlite_ticket_schema(conn)?;
                    continue;
                }
                let after = rows(conn, table, columns)?;
                if table == "__yoi_schema_migrations" || table == "ticket_schema_migrations" {
                    assert!(
                        after.starts_with(before),
                        "the complete old {table} ledger, including applied_at, must remain a prefix"
                    );
                    if table == "ticket_schema_migrations" && after.len() != before.len() {
                        assert_eq!(after.len(), before.len() + 1);
                        assert_eq!(after.last().unwrap()[0], Value::Integer(17));
                        ticket::verify_sqlite_ticket_schema(conn)?;
                    }
                } else if migrated
                    && matches!(table.as_str(), "backend_jobs" | "backend_job_attempts")
                {
                    let job_id = columns
                        .iter()
                        .position(|column| column == "job_id")
                        .unwrap();
                    let obsolete = Value::Text("obsolete-journal-job".into());
                    assert_eq!(
                        before.iter().filter(|row| row[job_id] == obsolete).count(),
                        1
                    );
                    let retained: Vec<_> = before
                        .iter()
                        .filter(|row| row[job_id] != obsolete)
                        .cloned()
                        .collect();
                    assert_eq!(
                        after, retained,
                        "only obsolete Job execution may be removed from {table}"
                    );
                } else {
                    assert_eq!(&after, before, "old columns and values changed in {table}");
                }
            }
            Ok(())
        }
        fn journal_ddl(conn: &Connection) -> crate::Result<Vec<(String, String)>> {
            Ok(conn.prepare("SELECT name,sql FROM sqlite_schema WHERE tbl_name='worker_removal_operations' AND sql IS NOT NULL ORDER BY name")?
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?)
        }
        for frozen in [
            include_str!("fixtures/published_journal124_ordered.sql"),
            include_str!("fixtures/published_journal124_fresh.sql"),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("server.sqlite");
            let conn = Connection::open(&path).unwrap();
            crate::published_journal_test_support::install(&conn, frozen);
            // Keep the published current Pending Job, and add a synthetic obsolete
            // Journal Job whose grant predates binding_id. Removing its execution
            // rows must neither complete nor discard the independent removal intent.
            conn.execute_batch(r#"
                INSERT INTO backend_jobs (
                    workspace_id,job_id,purpose,input_digest,input_ref,request_json,
                    intent_fingerprint,state,current_attempt,created_at,updated_at
                ) SELECT workspace_id,'obsolete-journal-job',purpose,input_digest,input_ref,
                    json_set(request_json,'$.job_id','obsolete-journal-job',
                        '$.grants.subjektiv_journal',json('{"subject_id":"subject","session_id":"session","journal_id":"journal","selected_ranges":[]}')),
                    intent_fingerprint,state,current_attempt,created_at,updated_at
                FROM backend_jobs WHERE job_id='after-retirement';
                INSERT INTO backend_job_attempts (
                    workspace_id,job_id,attempt_id,attempt,input_digest,state,
                    deadline_at,created_at,updated_at
                ) SELECT workspace_id,'obsolete-journal-job','obsolete-journal-job:attempt:1',
                    attempt,input_digest,state,deadline_at,created_at,updated_at
                FROM backend_job_attempts WHERE job_id='after-retirement';
            "#).unwrap();
            let (operation_id, metadata): (String, String) = conn
                .query_row(
                    "SELECT operation_id,journal_removal_json FROM worker_removal_operations",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            let expected: crate::store::JournalRemovalMetadata =
                serde_json::from_str(&metadata).unwrap();
            assert_eq!(expected.phase, Phase::AwaitingEffectsEnd);
            assert_eq!(expected.cause, Some(Cause::ExecutionUnconfirmed));
            let before = snapshot(&conn).unwrap();
            let ddl = journal_ddl(&conn).unwrap();
            drop(conn);
            // Dry-run executes the owning migration path on its private copy.
            SqliteWorkspaceStore::migration_plan(&path).unwrap();
            let conn = Connection::open(&path).unwrap();
            assert_preserved(&conn, &before, false).unwrap();
            assert_eq!(journal_ddl(&conn).unwrap(), ddl);
            drop(conn);
            SqliteWorkspaceStore::migrate_database(&path).unwrap();
            for _ in 0..2 {
                let reopened = SqliteWorkspaceStore::open_for_test(&path).unwrap();
                reopened.with_conn(|conn| {
                    assert_preserved(conn, &before, true)?;
                    assert_eq!(journal_ddl(conn)?, ddl);
                    assert!(conn.execute("UPDATE worker_removal_operations SET journal_removal_json=json_set(journal_removal_json,'$.binding.writer_binding_id','replacement') WHERE operation_id=?1", [&operation_id]).is_err());
                    assert!(conn.execute("UPDATE worker_removal_operations SET journal_removal_json=json_set(journal_removal_json,'$.phase','invalid') WHERE operation_id=?1", [&operation_id]).is_err());
                    assert_eq!(conn.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |row| row.get::<_,u64>(0))?, 0);
                    Ok(())
                }).unwrap();
                let prepared = reopened
                    .recover_worker_removal_execution("w", &req().worker)
                    .unwrap()
                    .unwrap();
                let progress = reopened.get_journal_removal(&prepared).unwrap().unwrap();
                assert_eq!(progress.operation_id, operation_id);
                assert_eq!(progress.metadata, expected);
                assert!(
                    reopened
                        .get_backend_job("w", "obsolete-journal-job")
                        .unwrap()
                        .is_none()
                );
                assert!(
                    reopened
                        .freeze_journal_removal(&prepared, &progress, &[])
                        .is_err()
                );
                assert_eq!(
                    reopened
                        .get_backend_job("w", "after-retirement")
                        .unwrap()
                        .unwrap()
                        .state,
                    crate::backend_job::BackendJobState::Pending
                );
            }
        }
    }

    #[test]
    fn published_journal_histories_reject_missing_identity_guard_without_repairing_input() {
        for frozen in [
            include_str!("fixtures/published_journal124_ordered.sql"),
            include_str!("fixtures/published_journal124_fresh.sql"),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("server.sqlite");
            let conn = Connection::open(&path).unwrap();
            crate::published_journal_test_support::install(&conn, frozen);
            // Corrupt only this private negative fixture, never an adopted database.
            conn.execute_batch("DROP TRIGGER worker_removal_journal_immutable;")
                .unwrap();
            let metadata: String = conn
                .query_row(
                    "SELECT journal_removal_json FROM worker_removal_operations",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            let history = |conn: &Connection| {
                conn.prepare(
                    "SELECT version,name,applied_at FROM __yoi_schema_migrations ORDER BY version",
                )
                .unwrap()
                .query_map([], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
            };
            let before = history(&conn);
            assert!(SqliteWorkspaceStore::migration_plan(&path).is_err());
            assert!(SqliteWorkspaceStore::open_for_test(&path).is_err());
            assert_eq!(history(&conn), before);
            assert_eq!(
                conn.query_row(
                    "SELECT journal_removal_json FROM worker_removal_operations",
                    [],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
                metadata
            );
            assert_eq!(conn.query_row("SELECT count(*) FROM sqlite_schema WHERE name='worker_removal_journal_immutable'", [], |row| row.get::<_,u64>(0)).unwrap(), 0);
        }
    }

    #[test]
    fn journal_removal_metadata_reopens_and_cas_fences_same_operation() {
        use crate::store::{JournalRemovalCause as Cause, JournalRemovalPhase as Phase};
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("server.db");
        let store = setup();
        let plan = store.plan_worker_removal(&req(), &inv()).unwrap();
        let prepared = store
            .prepare_worker_removal_execution("w", &plan.plan_id, &plan.input_fingerprint)
            .unwrap();
        let binding = journal_binding(&temp.path().join("features"));
        let authority: &dyn ControlPlaneStore = &store;
        let initial = authority.init_journal_removal(&prepared, &binding).unwrap();
        assert_eq!(initial.metadata.phase, Phase::AwaitingQuiescence);
        assert_eq!(initial.metadata.members, None);
        assert_eq!(
            authority.init_journal_removal(&prepared, &binding).unwrap(),
            initial
        );
        let mut replacement = binding.clone();
        replacement.writer_session_id = "successor".into();
        assert!(
            authority
                .init_journal_removal(&prepared, &replacement)
                .is_err()
        );
        replacement = binding.clone();
        replacement.writer_binding_id = "successor-binding".into();
        assert!(
            authority
                .init_journal_removal(&prepared, &replacement)
                .is_err()
        );
        replacement = journal_binding(&temp.path().join("replacement-features"));
        assert_ne!(replacement.database, binding.database);
        assert!(
            authority
                .init_journal_removal(&prepared, &replacement)
                .is_err()
        );
        let mut wrong = prepared.clone();
        wrong.plan.input_fingerprint = "changed".into();
        assert!(authority.get_journal_removal(&wrong).is_err());
        wrong = prepared.clone();
        wrong.plan.worker.runtime_id = "another-runtime".into();
        assert!(authority.get_journal_removal(&wrong).is_err());
        wrong = prepared.clone();
        wrong.plan.workspace_id = "another-workspace".into();
        assert!(authority.init_journal_removal(&wrong, &binding).is_err());
        let members = vec![frozen_journal_member(&temp.path().join("history"))];
        let frozen = authority
            .freeze_journal_removal(&prepared, &initial, &members)
            .unwrap();
        assert_eq!(frozen.operation_id, plan.operation_id);
        assert_eq!(frozen.input_fingerprint, plan.input_fingerprint);
        assert_eq!(frozen.metadata.phase, Phase::Recording);
        assert!(
            authority
                .update_journal_removal_progress(
                    &prepared,
                    &initial,
                    Phase::AwaitingQuiescence,
                    Some(Cause::SourceUnavailable)
                )
                .is_err()
        );
        let held = authority
            .update_journal_removal_progress(
                &prepared,
                &frozen,
                Phase::Recording,
                Some(Cause::JobPending),
            )
            .unwrap();
        assert!(
            authority
                .update_journal_removal_progress(&prepared, &held, Phase::Completed, None)
                .is_err()
        );
        assert!(
            authority
                .update_journal_removal_progress(&prepared, &held, Phase::Released, None)
                .is_err()
        );
        store
            .with_conn(|conn| {
                let mut disk = Connection::open(&path)?;
                rusqlite::backup::Backup::new(conn, &mut disk)?.run_to_completion(
                    128,
                    std::time::Duration::from_millis(1),
                    None,
                )?;
                Ok(())
            })
            .unwrap();
        drop(store);
        let reopened = SqliteWorkspaceStore::open_for_test(&path).unwrap();
        let concurrent = SqliteWorkspaceStore::open_for_test(&path).unwrap();
        assert_eq!(
            reopened.get_journal_removal(&prepared).unwrap(),
            Some(held.clone())
        );
        assert_eq!(
            reopened
                .list_pending_journal_removals("w", None, 1)
                .unwrap(),
            vec![held.clone()]
        );
        assert!(
            reopened
                .list_pending_journal_removals("other", None, 1)
                .unwrap()
                .is_empty()
        );
        assert!(
            reopened
                .list_pending_journal_removals("w", Some(&held.operation_id), 1)
                .unwrap()
                .is_empty()
        );
        assert!(
            reopened
                .list_pending_journal_removals("w", None, 101)
                .is_err()
        );
        let next = concurrent
            .update_journal_removal_progress(
                &prepared,
                &held,
                Phase::AwaitingEffectsEnd,
                Some(Cause::ExecutionUnconfirmed),
            )
            .unwrap();
        assert!(
            reopened
                .update_journal_removal_progress(
                    &prepared,
                    &held,
                    Phase::Recording,
                    Some(Cause::JobFailed)
                )
                .is_err()
        );
        let mut changed = members.clone();
        changed[0].end.byte_offset += 1;
        assert!(
            reopened
                .freeze_journal_removal(&prepared, &next, &changed)
                .is_err()
        );
        for mutate in [0, 1, 2, 3, 4] {
            let mut changed = members.clone();
            match mutate {
                0 => changed[0].active_segment_id = Some(session_store::new_segment_id()),
                1 => changed[0].materialized = false,
                2 => changed[0].descriptors[0]
                    .identity
                    .entry_ref
                    .push_str("-changed"),
                3 => changed[0].descriptors[0].sha256 = "f".repeat(64),
                _ => changed[0].descriptors[0].total_bytes += 1,
            }
            assert!(
                reopened
                    .freeze_journal_removal(&prepared, &next, &changed)
                    .is_err()
            );
        }
        assert_eq!(
            reopened
                .freeze_journal_removal(&prepared, &next, &members)
                .unwrap(),
            next
        );
        let json = serde_json::to_value(&next).unwrap();
        assert_eq!(
            serde_json::from_value::<crate::store::JournalRemovalProgress>(json).unwrap(),
            next
        );
        let mut invalid_metadata = serde_json::to_value(&next.metadata).unwrap();
        invalid_metadata["body"] = serde_json::json!("not admitted metadata");
        assert!(
            serde_json::from_value::<crate::store::JournalRemovalMetadata>(invalid_metadata)
                .is_err()
        );
    }

    #[test]
    fn journal_removal_physical_failure_preserves_frozen_metadata_and_cas_atomically() {
        use crate::store::{JournalRemovalCause as Cause, JournalRemovalPhase as Phase};
        let temp = tempfile::tempdir().unwrap();
        let store = setup();
        let plan = store.plan_worker_removal(&req(), &inv()).unwrap();
        let prepared = store
            .prepare_worker_removal_execution("w", &plan.plan_id, &plan.input_fingerprint)
            .unwrap();
        let initial = store
            .init_journal_removal(&prepared, &journal_binding(&temp.path().join("features")))
            .unwrap();
        let frozen = store
            .freeze_journal_removal(
                &prepared,
                &initial,
                &[frozen_journal_member(&temp.path().join("history"))],
            )
            .unwrap();
        let effects = store
            .update_journal_removal_progress(&prepared, &frozen, Phase::AwaitingEffectsEnd, None)
            .unwrap();
        // These are store observations only, not a Runtime archive publication proof.
        let retained = store
            .retain_journal_recording(
                &prepared,
                &effects,
                prepared.runtime_request.archive_id.as_deref().unwrap(),
            )
            .unwrap();
        let released = store
            .update_journal_removal_progress(&prepared, &retained, Phase::Released, None)
            .unwrap();
        store.with_conn(|conn| {
            conn.execute_batch("CREATE TRIGGER reject_failure_cause BEFORE UPDATE OF journal_removal_json ON worker_removal_operations WHEN json_extract(NEW.journal_removal_json,'$.cause')='execution_unconfirmed' BEGIN SELECT RAISE(ABORT,'injected cause save failure'); END;")?;
            Ok(())
        }).unwrap();
        assert!(
            store
                .fail_worker_removal(
                    "w",
                    &plan.operation_id,
                    &plan.input_fingerprint,
                    "runtime_remove"
                )
                .is_err()
        );
        assert_eq!(
            store.get_journal_removal(&prepared).unwrap(),
            Some(released.clone())
        );
        assert_eq!(
            store
                .recover_worker_removal_execution("w", &plan.worker)
                .unwrap()
                .unwrap()
                .plan
                .state,
            WorkerRemovalPlanState::Executing
        );
        store
            .with_conn(|conn| {
                conn.execute_batch("DROP TRIGGER reject_failure_cause;")?;
                Ok(())
            })
            .unwrap();
        assert!(
            store
                .fail_worker_removal(
                    "w",
                    &plan.operation_id,
                    "wrong-fingerprint",
                    "runtime_remove"
                )
                .is_err()
        );
        assert_eq!(
            store.get_journal_removal(&prepared).unwrap(),
            Some(released.clone())
        );
        store
            .fail_worker_removal(
                "w",
                &plan.operation_id,
                &plan.input_fingerprint,
                "runtime_remove",
            )
            .unwrap();
        let held = store.get_journal_removal(&prepared).unwrap().unwrap();
        assert_eq!(held.metadata.phase, Phase::Released);
        assert_eq!(held.metadata.cause, Some(Cause::ExecutionUnconfirmed));
        assert_eq!(held.metadata.binding, released.metadata.binding);
        assert_eq!(held.metadata.members, released.metadata.members);
        assert_eq!(
            store
                .recover_worker_removal_execution("w", &plan.worker)
                .unwrap()
                .unwrap()
                .plan
                .state,
            WorkerRemovalPlanState::Failed
        );
        assert!(
            store
                .update_journal_removal_progress(&prepared, &released, Phase::Released, None)
                .is_err(),
            "failure annotation must fence the old full-progress CAS"
        );
        assert!(
            store
                .update_journal_removal_progress(&prepared, &held, Phase::Completed, None)
                .is_err()
        );
    }

    #[test]
    fn journal_removal_pins_intent_without_weakening_policy_hold_or_ticket_blockers() {
        let temp = tempfile::tempdir().unwrap();
        let store = setup();
        let plan = store.plan_worker_removal(&req(), &inv()).unwrap();
        let prepared = store
            .prepare_worker_removal_execution("w", &plan.plan_id, &plan.input_fingerprint)
            .unwrap();
        store
            .init_journal_removal(&prepared, &journal_binding(temp.path()))
            .unwrap();
        store
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE worker_registry SET updated_at='later' WHERE workspace_id='w'",
                    [],
                )?;
                Ok(())
            })
            .unwrap();
        let recovered = store
            .recover_worker_removal_execution("w", &plan.worker)
            .unwrap()
            .unwrap();
        assert_eq!(recovered.plan.operation_id, plan.operation_id);
        assert_eq!(
            store
                .plan_worker_removal(&req(), &inv())
                .unwrap()
                .operation_id,
            plan.operation_id
        );
        assert_eq!(
            store
                .prepare_worker_removal_execution("w", &plan.plan_id, &plan.input_fingerprint)
                .unwrap()
                .plan
                .input_fingerprint,
            plan.input_fingerprint
        );
        let policy = store.worker_retention_policy("w").unwrap().unwrap();
        let original = WorkerRetentionPolicyUpdate {
            policy_id: policy.policy_id,
            session_disposition: policy.session_disposition,
            metadata_disposition: policy.metadata_disposition,
            archive_retention: policy.archive_retention,
            diagnostics_disposition: policy.diagnostics_disposition,
            diagnostics_retention_seconds: policy.diagnostics_retention_seconds,
        };
        let mut changed_policy = original.clone();
        changed_policy.metadata_disposition = MetadataDisposition::Purge;
        let changed_policy = store
            .update_worker_retention_policy("w", &policy.policy_digest, &changed_policy)
            .unwrap();
        assert!(
            matches!(store.prepare_worker_removal_execution("w", &plan.plan_id, &plan.input_fingerprint), Err(WorkerRetentionError::StalePlan { reason, .. }) if reason == "policy content changed")
        );
        assert_eq!(
            store
                .plan_worker_removal(&req(), &inv())
                .unwrap()
                .operation_id,
            plan.operation_id
        );
        store
            .update_worker_retention_policy("w", &changed_policy.policy_digest, &original)
            .unwrap();
        store
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE worker_registry SET retention_state='pinned' WHERE workspace_id='w'",
                    [],
                )?;
                Ok(())
            })
            .unwrap();
        assert!(
            matches!(store.prepare_worker_removal_execution("w", &plan.plan_id, &plan.input_fingerprint), Err(WorkerRetentionError::StalePlan { reason, .. }) if reason == "hold added")
        );
        store
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE worker_registry SET retention_state='normal' WHERE workspace_id='w'",
                    [],
                )?;
                Ok(())
            })
            .unwrap();
        assert!(
            install_current_assignment(&store, "journal-blocker").is_err(),
            "the existing removal admission fence must reject new Ticket responsibility"
        );
        assert!(
            store
                .prepare_worker_removal_execution("w", &plan.plan_id, &plan.input_fingerprint)
                .is_ok()
        );
        let pinned = store.plan_worker_removal(&req(), &inv()).unwrap();
        assert_eq!(pinned.state, WorkerRemovalPlanState::Executing);
        assert_eq!(pinned.operation_id, plan.operation_id);
        assert_eq!(
            store
                .list_pending_journal_removals("w", None, 100)
                .unwrap()
                .len(),
            1
        );
    }

    fn subject_removal_source(
        store: &SqliteWorkspaceStore,
        allocation: &str,
        singleton: Option<&str>,
    ) -> (WorkerRemovalPlanRequest, WorkerRetentionInventory) {
        store.with_conn(|conn| {
            conn.execute("INSERT OR IGNORE INTO workspace_memory_settings(workspace_id,language,created_at,updated_at) VALUES('w','English','created','observed')", [])?;
            Ok(())
        }).unwrap();
        let memory = store.get_workspace_memory_settings("w").unwrap();
        let reservation = store
            .reserve_worker_create("w", "r", allocation, allocation, singleton, &memory)
            .unwrap();
        store
            .complete_worker_create_reservation("w", reservation.worker_id)
            .unwrap();
        let worker = RuntimeWorkerRef::new("r", reservation.worker_id.to_string());
        store
            .upsert_worker_registry(&WorkerRegistryRecord {
                workspace_id: "w".into(),
                worker: worker.clone(),
                display_name: allocation.into(),
                profile: Some("builtin:coder".into()),
                retention_state: "normal".into(),
                transcript_ref: None,
                session_ref: None,
                summary_ref: None,
                diagnostics_ref: None,
                created_at: "created".into(),
                updated_at: "observed".into(),
            })
            .unwrap();
        let mut inventory = inv();
        inventory.worker_id = reservation.worker_id;
        (
            WorkerRemovalPlanRequest {
                workspace_id: "w".into(),
                worker,
                reason: "cleanup".into(),
                journal_erasure: None,
            },
            inventory,
        )
    }

    #[test]
    fn journal_removal_pending_intents_find_pre_binding_save_failures_with_bounded_subject_scope() {
        let store = setup();
        let temp = tempfile::tempdir().unwrap();
        let mut expected = Vec::new();
        for (allocation, singleton) in [
            ("subject-a", Some("subjektiv:a")),
            ("subject-b", Some("subjektiv:b")),
            ("subject-c", Some("subjektiv:c")),
            ("ordinary", None),
            ("consolidation", Some("subjektiv-consolidation:a")),
        ] {
            let (request, inventory) = subject_removal_source(&store, allocation, singleton);
            let plan = store.plan_worker_removal(&request, &inventory).unwrap();
            let prepared = store
                .prepare_worker_removal_execution("w", &plan.plan_id, &plan.input_fingerprint)
                .unwrap();
            if singleton.is_some_and(|key| key.starts_with("subjektiv:")) {
                expected.push(prepared);
            }
        }
        expected.sort_by(|a, b| a.plan.operation_id.cmp(&b.plan.operation_id));
        assert_eq!(expected.len(), 3);
        store.with_conn(|conn| {
            conn.execute_batch("CREATE TRIGGER reject_journal_init BEFORE UPDATE OF journal_removal_json ON worker_removal_operations BEGIN SELECT RAISE(ABORT,'injected Journal init save failure'); END;")?;
            Ok(())
        }).unwrap();
        assert!(
            store
                .init_journal_removal(&expected[0], &journal_binding(temp.path()))
                .is_err()
        );
        assert!(store.get_journal_removal(&expected[0]).unwrap().is_none());
        let authority: &dyn ControlPlaneStore = &store;
        let first = authority
            .list_pending_worker_removal_intents("w", None, 2)
            .unwrap()
            .into_iter()
            .map(|intent| intent.preparation.unwrap())
            .collect::<Vec<_>>();
        assert_eq!(first, expected[..2]);
        let second = authority
            .list_pending_worker_removal_intents(
                "w",
                Some(&first.last().unwrap().plan.operation_id),
                2,
            )
            .unwrap()
            .into_iter()
            .map(|intent| intent.preparation.unwrap())
            .collect::<Vec<_>>();
        assert_eq!(second, expected[2..]);
        assert!(
            authority
                .list_pending_worker_removal_intents(
                    "w",
                    Some(&second.last().unwrap().plan.operation_id),
                    2
                )
                .unwrap()
                .is_empty()
        );
        assert!(
            authority
                .list_pending_worker_removal_intents("other", None, 100)
                .unwrap()
                .is_empty()
        );
        assert!(
            authority
                .list_pending_worker_removal_intents("w", None, 0)
                .is_err()
        );
        assert!(
            authority
                .list_pending_worker_removal_intents("w", None, 101)
                .is_err()
        );
        assert!(
            authority
                .list_pending_journal_removals("w", None, 100)
                .unwrap()
                .is_empty()
        );
        let path = temp.path().join("server.db");
        store
            .with_conn(|conn| {
                conn.execute_batch("DROP TRIGGER reject_journal_init;")?;
                let mut disk = Connection::open(&path)?;
                rusqlite::backup::Backup::new(conn, &mut disk)?.run_to_completion(
                    128,
                    std::time::Duration::from_millis(1),
                    None,
                )?;
                Ok(())
            })
            .unwrap();
        drop(store);
        let reopened = SqliteWorkspaceStore::open_for_test(&path).unwrap();
        assert_eq!(
            reopened
                .list_pending_worker_removal_intents("w", None, 100)
                .unwrap()
                .into_iter()
                .map(|intent| intent.preparation.unwrap())
                .collect::<Vec<_>>(),
            expected
        );
        let row = &expected[0];
        reopened.with_conn(|conn| {
            conn.execute("UPDATE worker_registry SET retention_state='pinned' WHERE workspace_id=?1 AND runtime_id=?2 AND worker_id=?3", params![row.plan.workspace_id,row.plan.worker.runtime_id,row.plan.worker.worker_id])?;
            Ok(())
        }).unwrap();
        assert!(
            reopened
                .prepare_worker_removal_execution(
                    "w",
                    &row.plan.plan_id,
                    &row.plan.input_fingerprint
                )
                .is_err(),
            "discovery cannot waive current hold"
        );
    }

    #[test]
    fn journal_removal_pending_intent_cursor_advances_past_held_rows_to_independent_plans() {
        let store = setup();
        let mut plans = ["first", "second", "third"].map(|name| {
            let (request, inventory) =
                subject_removal_source(&store, name, Some(&format!("subjektiv:{name}")));
            let plan = store.plan_worker_removal(&request, &inventory).unwrap();
            (request, inventory, plan)
        });
        plans.sort_by(|a, b| a.2.operation_id.cmp(&b.2.operation_id));
        for (request, _, _) in &plans[..2] {
            let mut registry = store
                .get_worker_registry("w", &request.worker)
                .unwrap()
                .unwrap();
            registry.updated_at = "changed".into();
            store.upsert_worker_registry(&registry).unwrap();
        }
        let replacement = store.plan_worker_removal(&plans[1].0, &plans[1].1).unwrap();
        let mut expected = plans
            .iter()
            .map(|(_, _, plan)| plan.operation_id.clone())
            .collect::<Vec<_>>();
        expected.push(replacement.operation_id);
        expected.sort();
        let mut seen = Vec::new();
        let mut valid = Vec::new();
        let mut after = None;
        loop {
            let page = store
                .list_pending_worker_removal_intents("w", after.as_deref(), 1)
                .unwrap();
            if page.is_empty() {
                break;
            }
            assert_eq!(page.len(), 1);
            for intent in &page {
                seen.push(intent.operation_id.clone());
                match &intent.preparation {
                    Ok(prepared) => valid.push(prepared.plan.operation_id.clone()),
                    Err(_) => assert_ne!(intent.worker, plans[2].0.worker),
                }
            }
            after = page.last().map(|intent| intent.operation_id.clone());
        }
        assert_eq!(
            seen, expected,
            "held rows occupy pages and must retain the discovery cursor"
        );
        assert_eq!(valid, [plans[2].2.operation_id.clone()]);
        assert!(
            store
                .list_pending_worker_removal_intents("w", None, 1)
                .unwrap()[0]
                .preparation
                .is_err()
        );
    }

    #[test]
    fn journal_removal_pending_intents_reject_stale_or_ambiguous_pre_init_plans() {
        let store = setup();
        let (request, inventory) =
            subject_removal_source(&store, "subject", Some("subjektiv:subject"));
        let first = store.plan_worker_removal(&request, &inventory).unwrap();
        assert_eq!(
            store
                .list_pending_worker_removal_intents("w", None, 100)
                .unwrap()[0]
                .preparation
                .as_ref()
                .unwrap()
                .plan
                .operation_id,
            first.operation_id
        );
        let mut registry = store
            .get_worker_registry("w", &request.worker)
            .unwrap()
            .unwrap();
        registry.updated_at = "changed".into();
        store.upsert_worker_registry(&registry).unwrap();
        assert!(
            store
                .list_pending_worker_removal_intents("w", None, 100)
                .unwrap()
                .remove(0)
                .preparation
                .unwrap_err()
                .to_string()
                .contains("stale")
        );
        let replacement = store.plan_worker_removal(&request, &inventory).unwrap();
        assert_ne!(replacement.operation_id, first.operation_id);
        assert!(
            store
                .list_pending_worker_removal_intents("w", None, 100)
                .unwrap()
                .remove(0)
                .preparation
                .unwrap_err()
                .to_string()
                .contains("Ambiguous")
        );
        assert_eq!(
            store
                .get_journal_removal(
                    &store
                        .recover_worker_removal_execution("w", &request.worker)
                        .unwrap()
                        .unwrap()
                )
                .unwrap(),
            None
        );
        let count: u64 = store
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM worker_removal_operations WHERE workspace_id='w'",
                    [],
                    |row| row.get(0),
                )
                .map_err(StoreError::from)
            })
            .unwrap();
        assert_eq!(
            count, 2,
            "discovery must not create or replace a canonical plan"
        );
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
        let updated = s
            .update_worker_retention_policy("w", &p.policy_digest, &u)
            .unwrap();
        assert_eq!(updated.policy_digest, worker_retention_policy_digest(&u));
        assert_eq!(
            updated.archive_retention,
            ArchiveRetention::ForSeconds { seconds: 3_600 }
        );
        assert!(matches!(
            s.update_worker_retention_policy("w", &p.policy_digest, &u),
            Err(WorkerRetentionError::PolicyDigestConflict { .. })
        ));
    }
    #[test]
    fn identical_policy_content_keeps_digest_and_removal_plan() {
        let store = setup();
        let policy = store.worker_retention_policy("w").unwrap().unwrap();
        let update = WorkerRetentionPolicyUpdate {
            policy_id: policy.policy_id.clone(),
            session_disposition: policy.session_disposition,
            metadata_disposition: policy.metadata_disposition,
            archive_retention: policy.archive_retention,
            diagnostics_disposition: policy.diagnostics_disposition,
            diagnostics_retention_seconds: policy.diagnostics_retention_seconds,
        };
        assert_eq!(
            policy.policy_digest,
            worker_retention_policy_digest(&update)
        );
        let before = store.plan_worker_removal(&req(), &inv()).unwrap();
        let after = store
            .update_worker_retention_policy("w", &policy.policy_digest, &update)
            .unwrap();
        assert_eq!(policy.policy_digest, after.policy_digest);
        assert_eq!(before, store.plan_worker_removal(&req(), &inv()).unwrap());
        store
            .begin_worker_removal("w", &before.plan_id, &before.input_fingerprint)
            .unwrap();
        let mut changed = update.clone();
        changed.metadata_disposition = MetadataDisposition::Purge;
        assert_ne!(
            worker_retention_policy_digest(&update),
            worker_retention_policy_digest(&changed)
        );
        changed = update.clone();
        changed.archive_retention = ArchiveRetention::ForSeconds { seconds: 3600 };
        assert_ne!(
            worker_retention_policy_digest(&update),
            worker_retention_policy_digest(&changed)
        );
    }

    #[test]
    fn deterministic_plan_hold_and_cross_workspace() {
        let s = setup();
        let a = s.plan_worker_removal(&req(), &inv()).unwrap();
        let b = s.plan_worker_removal(&req(), &inv()).unwrap();
        assert_eq!(a.plan_id, b.plan_id);
        assert_eq!(a.worker_updated_at, "2026-08-24T00:00:00Z");
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
        s.update_worker_retention_policy(
            "w",
            &s.worker_retention_policy("w")
                .unwrap()
                .unwrap()
                .policy_digest,
            &u,
        )
        .unwrap();
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
    fn prepared_execution_is_derived_from_pinned_plan() {
        let s = setup();
        let plan = s.plan_worker_removal(&req(), &inv()).unwrap();
        let prepared = s
            .prepare_worker_removal_execution("w", &plan.plan_id, &plan.input_fingerprint)
            .unwrap();
        assert_eq!(
            prepared.runtime_request.session_disposition,
            SessionDisposition::Archive
        );
        assert_eq!(
            prepared.runtime_request.input_fingerprint,
            prepared.plan.input_fingerprint
        );
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
        let archived_grant_id = "archive-observe-grant";
        let archived_controller =
            RuntimeWorkerRef::new("r", WorkerId::from_legacy_u64(2).to_string());
        s.with_conn(|conn| {
            conn.execute(
                "INSERT INTO worker_registry(\
                    workspace_id,worker_id,runtime_id,display_name,profile,retention_state,created_at,updated_at\
                 ) VALUES('w',?1,'r','controller','builtin:companion','normal','created','controller-rev1')",
                [&archived_controller.worker_id],
            )?;
            Ok(())
        })
        .unwrap();
        s.create_worker_control_grant(&crate::store::WorkerControlGrantRecord {
            workspace_id: "w".into(),
            grant_id: archived_grant_id.into(),
            controller: archived_controller.clone(),
            subject: req().worker,
            relation: "subject_session".into(),
            origin: "test".into(),
            permissions: vec!["observe".into()],
            operation_id: archived_grant_id.into(),
            created_at: "2026-10-02T00:00:00Z".into(),
            revoked_at: None,
        })
        .unwrap();
        s.with_conn(|conn| {
            conn.execute("INSERT INTO typed_tickets(workspace_id,ticket_id,slug,title,status,kind,priority,workflow_state,workflow_state_explicit) VALUES('w','ticket-old','ticket-old','Old Ticket','open','task','normal','open',1)", [])?;
            conn.execute("INSERT INTO worker_registry(workspace_id,worker_id,runtime_id,display_name,profile,retention_state,created_at,updated_at) VALUES('w','1','r','old worker','builtin:coder','normal','created','2026-08-24T00:00:00Z')", [])?;
            conn.execute("INSERT INTO worker_create_reservations(workspace_id,allocation_key,worker_id,runtime_id,create_fingerprint,state,created_at,updated_at) VALUES('w','allocation-old',?1,'r','fingerprint','created','created','created')", [worker_id().to_string()])?;
            conn.execute("INSERT INTO ticket_worker_assignments(workspace_id,ticket_id,assignment_id,runtime_id,worker_id,assigned_by,assigned_at) VALUES('w','ticket-old','assignment-old','r','1','test','t')", [])?;
            conn.execute("DELETE FROM worker_registry WHERE workspace_id='w' AND runtime_id='r' AND worker_id='1'", [])?;
            conn.execute("DELETE FROM typed_tickets WHERE workspace_id='w' AND ticket_id='ticket-old'", [])?;
            Ok(())
        }).unwrap();
        let p = s.plan_worker_removal(&req(), &inv()).unwrap();
        let original = s
            .prepare_worker_removal_execution("w", &p.plan_id, &p.input_fingerprint)
            .unwrap();
        let result = WorkerRetentionExecutionResult {
            operation_id: p.operation_id.clone(),
            input_fingerprint: p.input_fingerprint.clone(),

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
                source_active_segment_id: None,
                segment_ids: vec!["a".into()],
                retained_sessions: Vec::new(),
                source_created_at: "created".into(),
                removed_at: "removed".into(),
                archived_at_unix_seconds: 1,
                effective_profile: None,
                retention_class: None,
                content_checksum_sha256: "sum".into(),
                content_bytes: 1,
                content_file_count: 1,
                policy_id: p.policy_id.clone(),

                operation_id: p.operation_id.clone(),
                input_fingerprint: p.input_fingerprint.clone(),
            }),
            source_removed: true,
            diagnostics_retained: false,
        };
        assert_eq!(
            s.commit_worker_removal("w", &p.operation_id, &p.input_fingerprint, &result)
                .unwrap()
                .plan
                .state,
            WorkerRemovalPlanState::Succeeded
        );
        assert!(
            s.worker_session_archive_observable_by(
                "w",
                p.archive_id.as_deref().unwrap(),
                &archived_controller,
                &p.worker,
            )
            .unwrap(),
            "the explicit observe grant survives Worker removal for the committed archive"
        );
        assert!(
            s.revoke_worker_control_grant("w", archived_grant_id, "2026-10-02T00:01:00Z")
                .unwrap(),
            "the existing grant revocation authority must also revoke its archive projection"
        );
        assert!(
            !s.worker_session_archive_observable_by(
                "w",
                p.archive_id.as_deref().unwrap(),
                &archived_controller,
                &p.worker,
            )
            .unwrap(),
            "every later cursor page and direct Read must observe archive grant revocation"
        );
        assert!(s.worker_tombstone("w", &p.worker).unwrap().is_some());
        let recovered = s
            .recover_worker_removal_execution("w", &p.worker)
            .unwrap()
            .unwrap();
        assert_eq!(
            recovered.runtime_request, original.runtime_request,
            "registry removal must not change the frozen Runtime request"
        );
        let reservation_state: String = s.with_conn(|conn| {
            conn.query_row(
                "SELECT state FROM worker_create_reservations WHERE workspace_id='w' AND allocation_key='allocation-old'",
                [],
                |row| row.get(0),
            )
            .map_err(StoreError::from)
        }).unwrap();
        assert_eq!(reservation_state, "removed");
        assert_eq!(
            s.commit_worker_removal("w", &p.operation_id, &p.input_fingerprint, &result)
                .unwrap()
                .plan
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
    fn install_current_assignment(
        s: &SqliteWorkspaceStore,
        assignment_id: &str,
    ) -> crate::Result<()> {
        let assignment = TicketWorkerAssignmentRecord {
            workspace_id: "w".into(),
            ticket_id: "ticket".into(),
            assignment_id: assignment_id.into(),
            worker: req().worker,
            assigned_by: "test".into(),
            assigned_at: "t".into(),
        };
        s.set_current_ticket_worker_assignment(
            &assignment,
            None,
            &format!("event-{assignment_id}"),
            &format!("operation-{assignment_id}"),
            false,
        )
        .map(|_| ())
    }

    fn seed_current_assignment(s: &SqliteWorkspaceStore) {
        install_current_assignment(s, "assignment").unwrap();
    }

    fn reopened_terminal_assignment_store() -> SqliteWorkspaceStore {
        let store = setup();
        seed_current_assignment(&store);
        store
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE typed_tickets SET status='closed', workflow_state='closed'
                     WHERE workspace_id='w' AND ticket_id='ticket'",
                    [],
                )?;
                conn.execute(
                    "UPDATE typed_tickets SET status='open', workflow_state='open'
                     WHERE workspace_id='w' AND ticket_id='ticket'",
                    [],
                )?;
                Ok(())
            })
            .unwrap();
        // Narrow lifecycle-schema evidence: closing durably releases work,
        // while reopening retains routing without reviving that assignment.
        let released: i64 = store
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM ticket_assignment_work_releases
                WHERE workspace_id='w' AND assignment_id='assignment'",
                    [],
                    |row| row.get(0),
                )
                .map_err(StoreError::from)
            })
            .unwrap();
        assert_eq!(released, 1);
        store
    }

    #[test]
    fn pending_ticket_claim_blocks_removal_and_is_rechecked_after_planning() {
        let s = setup();
        let old = s.plan_worker_removal(&req(), &inv()).unwrap();
        s.reserve_ticket_assignment_operation(
            "w",
            "claim",
            "ticket",
            "r",
            Some(&worker_id().to_string()),
            "intent",
            "t",
        )
        .unwrap();
        let blocked = s.plan_worker_removal(&req(), &inv()).unwrap();
        assert!(
            matches!(&blocked.blockers[..], [WorkerRemovalBlocker::PendingTicketClaim{operation_id,ticket_id}] if operation_id=="claim" && ticket_id=="ticket")
        );
        assert!(
            s.begin_worker_removal("w", &old.plan_id, &old.input_fingerprint)
                .is_err()
        );
        s.fail_ticket_assignment_operation("w", "claim", "No Runtime effects dispatched")
            .unwrap();
        assert!(
            s.plan_worker_removal(&req(), &inv())
                .unwrap()
                .blockers
                .is_empty()
        );
    }

    #[test]
    fn assignment_and_orphan_are_authoritative() {
        let s = setup();
        seed_current_assignment(&s);
        let p = s.plan_worker_removal(&req(), &inv()).unwrap();
        assert!(
            matches!(&p.blockers[..],[WorkerRemovalBlocker::UnfinishedWork{assignment_id,ticket_id}] if assignment_id=="assignment"&&ticket_id=="ticket")
        );
        let runtime_only = WorkerRetentionInventory {
            workspace_id: "w".into(),
            runtime_id: "r".into(),
            worker_id: WorkerId::from_legacy_u64(2),
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
    fn unfinished_work_blocker_uses_explicit_wire_kind() {
        let wire = serde_json::json!({
            "kind": "unfinished_work",
            "assignment_id": "assignment",
            "ticket_id": "ticket",
        });
        let blocker: WorkerRemovalBlocker = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(serde_json::to_value(blocker).unwrap(), wire);
    }

    #[test]
    fn terminal_retained_assignment_allows_worker_removal_even_after_reopen() {
        for reopen in [false, true] {
            let store = setup();
            seed_current_assignment(&store);
            store
                .with_conn(|conn| {
                    conn.execute(
                        "UPDATE typed_tickets SET status='closed', workflow_state='closed'
                             WHERE workspace_id='w' AND ticket_id='ticket'",
                        [],
                    )?;
                    if reopen {
                        conn.execute(
                            "UPDATE typed_tickets SET status='open', workflow_state='open'
                                 WHERE workspace_id='w' AND ticket_id='ticket'",
                            [],
                        )?;
                    }
                    Ok(())
                })
                .unwrap();
            store
                .update_worker_retention_policy(
                    "w",
                    &store
                        .worker_retention_policy("w")
                        .unwrap()
                        .unwrap()
                        .policy_digest,
                    &WorkerRetentionPolicyUpdate {
                        policy_id: "purge".into(),
                        session_disposition: SessionDisposition::Purge,
                        metadata_disposition: MetadataDisposition::Tombstone,
                        archive_retention: ArchiveRetention::Forever,
                        diagnostics_disposition: DiagnosticsDisposition::Purge,
                        diagnostics_retention_seconds: None,
                    },
                )
                .unwrap();
            let plan = store.plan_worker_removal(&req(), &inv()).unwrap();
            assert!(plan.blockers.is_empty(), "closed case, reopened={reopen}");
            store
                .prepare_worker_removal_execution("w", &plan.plan_id, &plan.input_fingerprint)
                .unwrap();
            let result = WorkerRetentionExecutionResult {
                operation_id: plan.operation_id.clone(),
                input_fingerprint: plan.input_fingerprint.clone(),
                worker_id: worker_id(),
                session_disposition: plan.session_disposition,
                diagnostics_disposition: plan.diagnostics_disposition,
                archive: None,
                source_removed: true,
                diagnostics_retained: false,
            };
            let committed = store
                .commit_worker_removal("w", &plan.operation_id, &plan.input_fingerprint, &result)
                .unwrap();
            assert_eq!(committed.plan.state, WorkerRemovalPlanState::Succeeded);
            assert!(
                store
                    .get_worker_registry("w", &plan.worker)
                    .unwrap()
                    .is_none()
            );
            assert!(store.worker_tombstone("w", &plan.worker).unwrap().is_some());
            let (retained, historical): (i64, i64) = store
                .with_conn(|conn| {
                    Ok((
                        conn.query_row(
                            "SELECT COUNT(*) FROM ticket_current_worker_assignments
                                 WHERE workspace_id='w' AND assignment_id='assignment'",
                            [],
                            |row| row.get(0),
                        )?,
                        conn.query_row(
                            "SELECT COUNT(*) FROM ticket_worker_assignments
                                 WHERE workspace_id='w' AND assignment_id='assignment'",
                            [],
                            |row| row.get(0),
                        )?,
                    ))
                })
                .unwrap();
            assert_eq!(
                (retained, historical),
                (1, 1),
                "assignment history must survive removal"
            );
        }
    }

    #[test]
    fn unfinished_work_added_after_planning_blocks_preparation() {
        let store = setup();
        let plan = store.plan_worker_removal(&req(), &inv()).unwrap();
        seed_current_assignment(&store);
        let error = store
            .prepare_worker_removal_execution("w", &plan.plan_id, &plan.input_fingerprint)
            .unwrap_err();
        assert!(
            matches!(error, WorkerRetentionError::StalePlan { reason, .. }
            if reason == "unfinished work added")
        );
        assert!(
            store
                .get_worker_registry("w", &plan.worker)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn distinct_assignment_after_terminal_end_blocks_preparation() {
        let store = reopened_terminal_assignment_store();
        let plan = store.plan_worker_removal(&req(), &inv()).unwrap();
        assert!(plan.blockers.is_empty());
        install_current_assignment(&store, "new-assignment").unwrap();
        let blocked = store.plan_worker_removal(&req(), &inv()).unwrap();
        assert_eq!(
            blocked.blockers,
            vec![WorkerRemovalBlocker::UnfinishedWork {
                assignment_id: "new-assignment".into(),
                ticket_id: "ticket".into(),
            }]
        );
        let replay = store.plan_worker_removal(&req(), &inv()).unwrap();
        assert_eq!(replay.plan_id, blocked.plan_id);
        assert_eq!(replay.input_fingerprint, blocked.input_fingerprint);
        assert_ne!(blocked.plan_id, plan.plan_id);
        let error = store
            .prepare_worker_removal_execution("w", &plan.plan_id, &plan.input_fingerprint)
            .unwrap_err();
        assert!(
            matches!(error, WorkerRetentionError::StalePlan { reason, .. }
                if reason == "unfinished work added")
        );
        assert!(
            store
                .get_worker_registry("w", &plan.worker)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn prepared_removal_fences_distinct_assignment_after_terminal_end() {
        let store = reopened_terminal_assignment_store();
        let plan = store.plan_worker_removal(&req(), &inv()).unwrap();
        store
            .prepare_worker_removal_execution("w", &plan.plan_id, &plan.input_fingerprint)
            .unwrap();
        let error = install_current_assignment(&store, "new-assignment").unwrap_err();
        assert!(
            matches!(error, StoreError::TicketAssignmentConflict(message)
            if message.contains("being retained or has been removed"))
        );
        let (current, history): (i64, i64) = store
            .with_conn(|conn| {
                Ok((
                    conn.query_row(
                        "SELECT COUNT(*) FROM ticket_current_worker_assignments
                         WHERE workspace_id='w' AND ticket_id='ticket'",
                        [],
                        |row| row.get(0),
                    )?,
                    conn.query_row(
                        "SELECT COUNT(*) FROM ticket_worker_assignments
                         WHERE workspace_id='w' AND assignment_id='new-assignment'",
                        [],
                        |row| row.get(0),
                    )?,
                ))
            })
            .unwrap();
        assert_eq!(current, 1, "the ended responsibility must remain retained");
        assert_eq!(history, 0, "the rejected new identity must roll back");
    }

    #[test]
    fn commit_rechecks_distinct_assignment_independently_of_admission_fence() {
        let store = reopened_terminal_assignment_store();
        let plan = store.plan_worker_removal(&req(), &inv()).unwrap();
        store
            .prepare_worker_removal_execution("w", &plan.plan_id, &plan.input_fingerprint)
            .unwrap();
        // Fault injection is confined to this in-memory authority fixture: use
        // direct SQLite writes to bypass the public API removal fence, and drop
        // both principal admission triggers to prove commit's independent guard.
        store
            .with_conn_mut(|conn| {
                let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
                tx.execute_batch(
                    "DROP TRIGGER ticket_active_worker_principal_insert;
                     DROP TRIGGER ticket_active_worker_principal_update;",
                )?;
                tx.execute(
                    "INSERT INTO ticket_worker_assignments (
                        workspace_id, ticket_id, assignment_id, runtime_id, worker_id,
                        assigned_by, assigned_at
                     ) VALUES ('w', 'ticket', 'new-assignment', 'r', ?1, 'test', 'new')",
                    [worker_id().to_string()],
                )?;
                let replaced = tx.execute(
                    "UPDATE ticket_current_worker_assignments
                     SET assignment_id='new-assignment', updated_at='new'
                     WHERE workspace_id='w' AND ticket_id='ticket' AND role='worker'
                       AND assignment_id='assignment'
                       AND EXISTS (
                           SELECT 1 FROM ticket_assignment_work_releases
                           WHERE workspace_id='w' AND assignment_id='assignment'
                       )",
                    [],
                )?;
                if replaced != 1 {
                    return Err(StoreError::InvalidInput(
                        "fault injection requires one ended current assignment".into(),
                    ));
                }
                tx.commit()?;
                Ok(())
            })
            .unwrap();
        let result = WorkerRetentionExecutionResult {
            operation_id: plan.operation_id.clone(),
            input_fingerprint: plan.input_fingerprint.clone(),

            worker_id: worker_id(),
            session_disposition: plan.session_disposition,
            diagnostics_disposition: plan.diagnostics_disposition,
            archive: None,
            source_removed: true,
            diagnostics_retained: false,
        };
        let error = store
            .commit_worker_removal("w", &plan.operation_id, &plan.input_fingerprint, &result)
            .unwrap_err();
        assert!(
            matches!(error, WorkerRetentionError::StalePlan { reason, .. }
            if reason == "unfinished work added")
        );
        assert!(
            store
                .get_worker_registry("w", &plan.worker)
                .unwrap()
                .is_some()
        );
        assert!(store.worker_tombstone("w", &plan.worker).unwrap().is_none());
        let (state, active): (String, String) = store
            .with_conn(|conn| {
                Ok((
                    conn.query_row(
                        "SELECT state FROM worker_removal_operations WHERE operation_id=?1",
                        [&plan.operation_id],
                        |row| row.get(0),
                    )?,
                    conn.query_row(
                        "SELECT assignment_id FROM ticket_active_worker_assignments
                         WHERE workspace_id='w' AND ticket_id='ticket'",
                        [],
                        |row| row.get(0),
                    )?,
                ))
            })
            .unwrap();
        assert_eq!(
            state, "executing",
            "rejected commit must remain recoverable"
        );
        assert_eq!(
            active, "new-assignment",
            "the ended identity must not reactivate"
        );
    }

    #[test]
    fn terminal_assignment_does_not_bypass_hold_added_before_commit() {
        let store = setup();
        seed_current_assignment(&store);
        store
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE typed_tickets SET status='closed', workflow_state='closed'
                     WHERE workspace_id='w' AND ticket_id='ticket'",
                    [],
                )?;
                Ok(())
            })
            .unwrap();
        let plan = store.plan_worker_removal(&req(), &inv()).unwrap();
        store
            .prepare_worker_removal_execution("w", &plan.plan_id, &plan.input_fingerprint)
            .unwrap();
        store
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE worker_registry SET retention_state='pinned'
                     WHERE workspace_id='w' AND runtime_id='r' AND worker_id=?1",
                    [worker_id().to_string()],
                )?;
                Ok(())
            })
            .unwrap();
        let result = WorkerRetentionExecutionResult {
            operation_id: plan.operation_id.clone(),
            input_fingerprint: plan.input_fingerprint.clone(),

            worker_id: worker_id(),
            session_disposition: plan.session_disposition,
            diagnostics_disposition: plan.diagnostics_disposition,
            archive: None,
            source_removed: true,
            diagnostics_retained: false,
        };
        let error = store
            .commit_worker_removal("w", &plan.operation_id, &plan.input_fingerprint, &result)
            .unwrap_err();
        assert!(
            matches!(error, WorkerRetentionError::StalePlan { reason, .. }
            if reason == "hold added")
        );
        assert!(
            store
                .get_worker_registry("w", &plan.worker)
                .unwrap()
                .is_some()
        );
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
        s.update_worker_retention_policy(
            "w",
            &s.worker_retention_policy("w")
                .unwrap()
                .unwrap()
                .policy_digest,
            &u,
        )
        .unwrap();
        let p = s.plan_worker_removal(&req(), &inv()).unwrap();
        s.begin_worker_removal("w", &p.plan_id, &p.input_fingerprint)
            .unwrap();
        let r = WorkerRetentionExecutionResult {
            operation_id: p.operation_id.clone(),
            input_fingerprint: p.input_fingerprint.clone(),

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
    fn metadata_purge_plan_observation_preserves_pinned_receipt_and_repairs_catalog() {
        let store = setup();
        let request = req();
        let policy = store.worker_retention_policy("w").unwrap().unwrap();
        store
            .update_worker_retention_policy(
                "w",
                &policy.policy_digest,
                &WorkerRetentionPolicyUpdate {
                    policy_id: "purge".into(),
                    session_disposition: SessionDisposition::Purge,
                    metadata_disposition: MetadataDisposition::Purge,
                    archive_retention: ArchiveRetention::Forever,
                    diagnostics_disposition: DiagnosticsDisposition::Purge,
                    diagnostics_retention_seconds: None,
                },
            )
            .unwrap();
        let plan = store.plan_worker_removal(&request, &inv()).unwrap();
        let mut alternative_request = request.clone();
        alternative_request.reason = "different removal intent".into();
        let alternative = store
            .plan_worker_removal(&alternative_request, &inv())
            .unwrap();
        assert_ne!(alternative.operation_id, plan.operation_id);
        let authority: &dyn ControlPlaneStore = &store;
        authority
            .prepare_worker_removal_execution("w", &plan.plan_id, &plan.input_fingerprint)
            .unwrap();
        let result = WorkerRetentionExecutionResult {
            operation_id: plan.operation_id.clone(),
            input_fingerprint: plan.input_fingerprint.clone(),
            worker_id: worker_id(),
            session_disposition: SessionDisposition::Purge,
            diagnostics_disposition: DiagnosticsDisposition::Purge,
            archive: None,
            source_removed: true,
            diagnostics_retained: false,
        };
        let committed = authority
            .commit_worker_removal("w", &plan.operation_id, &plan.input_fingerprint, &result)
            .unwrap();
        assert_eq!(committed.plan.state, WorkerRemovalPlanState::Succeeded);
        assert_eq!(
            committed.plan.metadata_disposition,
            MetadataDisposition::Purge
        );
        assert!(
            store
                .get_worker_registry("w", &request.worker)
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .worker_tombstone("w", &request.worker)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            authority
                .recover_worker_removal_plan("w", &request.worker)
                .unwrap(),
            Some(committed.plan.clone())
        );
        assert!(matches!(
            authority.recover_worker_removal_execution("w", &request.worker),
            Err(WorkerRetentionError::StalePlan { plan_id, reason })
                if plan_id == plan.plan_id
                    && reason == "completed removal has no matching Worker tombstone"
        ));
        assert!(
            authority
                .recover_worker_removal_plan("other", &request.worker)
                .unwrap()
                .is_none()
        );
        assert!(
            authority
                .recover_worker_removal_plan(
                    "w",
                    &RuntimeWorkerRef::new("r", WorkerId::from_legacy_u64(2).to_string()),
                )
                .unwrap()
                .is_none()
        );

        // Subject deletion pins this exact successful receipt, even though its
        // Purge policy differs from a newly planned Subject body removal.
        store
            .prepare_subject_deletion(
                "w",
                "subject",
                "delete",
                std::slice::from_ref(&request.worker),
            )
            .unwrap();
        store
            .with_conn(|conn| {
                assert!(crate::store::subject_deletion_prior_removal_matches(
                    conn, "w", &request.worker, &plan.operation_id,
                )?);
                // Simulate a later receipt and a pre-atomic catalog projection gap.
                // Neither may replace the receipt pinned at Subject deletion.
                conn.execute(
                    "UPDATE worker_removal_operations SET state='succeeded',created_at='9999-01-01T00:00:00Z' WHERE operation_id=?1",
                    [&alternative.operation_id],
                )?;
                conn.execute(
                    "DELETE FROM worker_registry_projection_removals WHERE workspace_id='w' AND runtime_id='r' AND worker_id=?1",
                    [&request.worker.worker_id],
                )?;
                Ok(())
            })
            .unwrap();
        assert_eq!(
            authority
                .recover_worker_removal_plan("w", &request.worker)
                .unwrap(),
            Some(committed.plan.clone())
        );
        assert!(matches!(
            authority.recover_worker_removal_execution("w", &request.worker),
            Err(WorkerRetentionError::StalePlan { plan_id, reason })
                if plan_id == plan.plan_id
                    && reason == "completed removal has no matching Worker tombstone"
        ));
        let repaired = authority
            .recover_succeeded_worker_removal_catalog("w", &request.worker)
            .unwrap();
        assert!(matches!(
            repaired.changes.as_slice(),
            [WorkerCatalogChange::Removed(worker)] if worker == &request.worker
        ));
        store
            .with_conn(|conn| {
                let fences: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM worker_registry_projection_removals WHERE workspace_id='w' AND runtime_id='r' AND worker_id=?1",
                    [&request.worker.worker_id],
                    |row| row.get(0),
                )?;
                assert_eq!(fences, 1);
                Ok(())
            })
            .unwrap();
        assert_eq!(
            authority
                .recover_worker_removal_plan("w", &request.worker)
                .unwrap(),
            Some(committed.plan)
        );
        assert!(
            store
                .get_worker_registry("w", &request.worker)
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .worker_tombstone("w", &request.worker)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn terminal_catalog_failure_rolls_back_removal_projection_and_operation() {
        let store = setup();
        let purge = WorkerRetentionPolicyUpdate {
            policy_id: "purge".into(),
            session_disposition: SessionDisposition::Purge,
            metadata_disposition: MetadataDisposition::Tombstone,
            archive_retention: ArchiveRetention::Forever,
            diagnostics_disposition: DiagnosticsDisposition::Purge,
            diagnostics_retention_seconds: None,
        };
        store
            .update_worker_retention_policy(
                "w",
                &store
                    .worker_retention_policy("w")
                    .unwrap()
                    .unwrap()
                    .policy_digest,
                &purge,
            )
            .unwrap();
        let plan = store.plan_worker_removal(&req(), &inv()).unwrap();
        store
            .prepare_worker_removal_execution("w", &plan.plan_id, &plan.input_fingerprint)
            .unwrap();
        let result = WorkerRetentionExecutionResult {
            operation_id: plan.operation_id.clone(),
            input_fingerprint: plan.input_fingerprint.clone(),

            worker_id: worker_id(),
            session_disposition: SessionDisposition::Purge,
            diagnostics_disposition: DiagnosticsDisposition::Purge,
            archive: None,
            source_removed: true,
            diagnostics_retained: false,
        };
        store
            .with_conn(|conn| {
                conn.execute_batch(
                    "CREATE TRIGGER fail_worker_projection_removal
                     BEFORE INSERT ON worker_registry_projection_removals
                     BEGIN SELECT RAISE(ABORT, 'injected projection failure'); END;",
                )?;
                Ok(())
            })
            .unwrap();

        assert!(
            store
                .commit_worker_removal("w", &plan.operation_id, &plan.input_fingerprint, &result,)
                .is_err()
        );
        let (registry, removal, state, tombstone): (i64, i64, String, i64) = store
            .with_conn(|conn| {
                Ok((
                    conn.query_row(
                        "SELECT COUNT(*) FROM worker_registry WHERE workspace_id='w' AND runtime_id='r' AND worker_id=?1",
                        [worker_id().to_string()],
                        |row| row.get(0),
                    )?,
                    conn.query_row(
                        "SELECT COUNT(*) FROM worker_registry_projection_removals WHERE workspace_id='w' AND runtime_id='r' AND worker_id=?1",
                        [worker_id().to_string()],
                        |row| row.get(0),
                    )?,
                    conn.query_row(
                        "SELECT state FROM worker_removal_operations WHERE operation_id=?1",
                        [&plan.operation_id],
                        |row| row.get(0),
                    )?,
                    conn.query_row(
                        "SELECT COUNT(*) FROM worker_tombstones WHERE workspace_id='w' AND runtime_id='r' AND worker_id=?1",
                        [worker_id().to_string()],
                        |row| row.get(0),
                    )?,
                ))
            })
            .unwrap();
        assert_eq!(registry, 1, "catalog deletion must roll back");
        assert_eq!(removal, 0, "removal fence must not partially commit");
        assert_eq!(state, "executing", "operation must remain recoverable");
        assert_eq!(tombstone, 0, "retention metadata must roll back");

        store
            .with_conn(|conn| {
                conn.execute_batch("DROP TRIGGER fail_worker_projection_removal;")?;
                Ok(())
            })
            .unwrap();
        let committed = store
            .commit_worker_removal("w", &plan.operation_id, &plan.input_fingerprint, &result)
            .unwrap();
        assert!(matches!(
            committed.catalog.changes.as_slice(),
            [WorkerCatalogChange::Removed(worker)] if worker == &plan.worker
        ));
        assert_eq!(committed.plan.state, WorkerRemovalPlanState::Succeeded);
    }

    #[test]
    fn commit_requires_executing_state_and_exact_runtime_result() {
        let store = setup();
        let plan = store.plan_worker_removal(&req(), &inv()).unwrap();
        let mut result = WorkerRetentionExecutionResult {
            operation_id: plan.operation_id.clone(),
            input_fingerprint: plan.input_fingerprint.clone(),

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
            updated_at: "2026-08-24T00:00:01Z".into(),
        };
        store.upsert_worker_registry(&stale).unwrap();
        let updated_at: String = store.with_conn(|conn| conn.query_row(
            "SELECT updated_at FROM worker_registry WHERE workspace_id='w' AND runtime_id='r' AND worker_id=?1",
            [worker_id().to_string()],
            |row| row.get(0),
        ).map_err(StoreError::from)).unwrap();
        assert_eq!(updated_at, "2026-08-24T00:00:00Z");

        let assignment = TicketWorkerAssignmentRecord {
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
                .set_current_ticket_worker_assignment(
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
    fn runtime_result_must_match_prepared_input_fingerprint() {
        let s = setup();
        let plan = s.plan_worker_removal(&req(), &inv()).unwrap();
        let prepared = s
            .prepare_worker_removal_execution("w", &plan.plan_id, &plan.input_fingerprint)
            .unwrap();
        let mut runtime_result = WorkerRetentionExecutionResult {
            operation_id: prepared.plan.operation_id.clone(),
            input_fingerprint: prepared.plan.input_fingerprint.clone(),

            worker_id: worker_id(),
            session_disposition: prepared.plan.session_disposition,
            diagnostics_disposition: prepared.plan.diagnostics_disposition,
            archive: None,
            source_removed: true,
            diagnostics_retained: false,
        };
        runtime_result.input_fingerprint = "stale-input-fingerprint".to_string();
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
            "mismatched Runtime input fingerprint must be rejected"
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
                    "UPDATE worker_registry SET updated_at='2026-08-24T00:00:01Z' WHERE workspace_id='w' AND runtime_id='r' AND worker_id=?1",
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
        assert_eq!(replacement.worker_updated_at, "2026-08-24T00:00:01Z");
        assert_ne!(replacement.plan_id, plan.plan_id);
    }

    #[test]
    fn automatic_job_cleanup_admits_first_execution_only_after_a_blocked_plan_clears() {
        let store = setup();
        let request = req();
        store
            .update_worker_retention("w", &request.worker, "pinned", "2026-08-24T00:00:01Z")
            .unwrap();
        assert!(matches!(
            store.plan_job_cleanup_worker_removal(&request, &inv()),
            Err(WorkerRetentionError::Blocked(blockers)) if blockers == vec![WorkerRemovalBlocker::Hold]
        ));
        store.with_conn(|connection| {
            assert_eq!(connection.query_row(
                "SELECT count(*) FROM worker_removal_operations WHERE workspace_id='w' AND runtime_id='r' AND worker_id=?1",
                [request.worker.worker_id.as_str()], |row| row.get::<_, i64>(0),
            )?, 0, "automatic admission refusal must not create a child");
            Ok(())
        }).unwrap();
        // Explicit planning may retain a diagnostic Blocked row. It never
        // authorizes execution or prevents cleanup's first admitted child.
        let blocked = store.plan_worker_removal(&request, &inv()).unwrap();
        assert_eq!(blocked.state, WorkerRemovalPlanState::Blocked);
        assert!(
            store
                .prepare_worker_removal_execution("w", &blocked.plan_id, &blocked.input_fingerprint)
                .is_err()
        );
        assert!(
            store
                .recover_worker_removal_execution("w", &request.worker)
                .unwrap()
                .is_none()
        );
        store
            .update_worker_retention("w", &request.worker, "normal", "2026-08-24T00:00:02Z")
            .unwrap();
        let first = store
            .plan_job_cleanup_worker_removal(&request, &inv())
            .unwrap();
        assert_ne!(first.operation_id, blocked.operation_id);
        let execution = store
            .prepare_worker_removal_execution("w", &first.plan_id, &first.input_fingerprint)
            .unwrap();
        assert_eq!(execution.runtime_request.operation_id, first.operation_id);
        assert!(
            store
                .plan_job_cleanup_worker_removal(&request, &inv())
                .is_err()
        );
        store
            .with_conn(|connection| {
                assert_eq!(
                    load_plan(connection, &blocked.plan_id)?.unwrap().state,
                    WorkerRemovalPlanState::Blocked
                );
                assert_eq!(
                    load_plan(connection, &first.plan_id)?.unwrap().state,
                    WorkerRemovalPlanState::Executing
                );
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn automatic_job_cleanup_cannot_replace_a_hidden_or_stale_canonical_child() {
        for changed in ["policy", "worker"] {
            let store = setup();
            let request = req();
            let original = store
                .plan_job_cleanup_worker_removal(&request, &inv())
                .unwrap();
            store
                .prepare_worker_removal_execution(
                    "w",
                    &original.plan_id,
                    &original.input_fingerprint,
                )
                .unwrap();
            store
                .fail_worker_removal(
                    "w",
                    &original.operation_id,
                    &original.input_fingerprint,
                    "runtime_outcome_unconfirmed",
                )
                .unwrap();
            if changed == "policy" {
                let policy = store.worker_retention_policy("w").unwrap().unwrap();
                store
                    .update_worker_retention_policy(
                        "w",
                        &policy.policy_digest,
                        &WorkerRetentionPolicyUpdate {
                            policy_id: "changed-after-uncertain-removal".into(),
                            session_disposition: policy.session_disposition,
                            metadata_disposition: policy.metadata_disposition,
                            archive_retention: policy.archive_retention,
                            diagnostics_disposition: policy.diagnostics_disposition,
                            diagnostics_retention_seconds: policy.diagnostics_retention_seconds,
                        },
                    )
                    .unwrap();
                assert!(
                    store
                        .prepare_worker_removal_execution(
                            "w",
                            &original.plan_id,
                            &original.input_fingerprint
                        )
                        .is_err()
                );
            } else {
                store.with_conn(|connection| {
                    connection.execute("UPDATE worker_registry SET updated_at='2026-08-24T00:00:01Z' WHERE workspace_id='w' AND runtime_id='r' AND worker_id=?1", [worker_id().to_string()])?;
                    Ok(())
                }).unwrap();
            }
            assert!(
                store
                    .recover_worker_removal_execution("w", &request.worker)
                    .unwrap()
                    .is_none()
            );
            assert!(
                store
                    .plan_job_cleanup_worker_removal(&request, &inv())
                    .is_err(),
                "{changed}"
            );
            store
                .with_conn(|connection| {
                    let count: i64 = connection.query_row(
                        "SELECT COUNT(*) FROM worker_removal_operations WHERE workspace_id='w'",
                        [],
                        |row| row.get(0),
                    )?;
                    assert_eq!(
                        count, 1,
                        "filtered canonical ownership must not become a new operation"
                    );
                    let saved = load_plan(connection, &original.plan_id)?.unwrap();
                    assert_eq!(saved.operation_id, original.operation_id);
                    assert_eq!(saved.input_fingerprint, original.input_fingerprint);
                    assert_ne!(saved.state, WorkerRemovalPlanState::Succeeded);
                    Ok(())
                })
                .unwrap();
        }
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
                source_active_segment_id: None,
                segment_ids: vec!["a".into()],
                retained_sessions: Vec::new(),
                source_created_at: "created".into(),
                removed_at: "removed".into(),
                archived_at_unix_seconds: 1,
                effective_profile: None,
                retention_class: None,
                content_checksum_sha256: "sum".into(),
                content_bytes: 1,
                content_file_count: 1,
                policy_id: prepared.plan.policy_id.clone(),

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
        s.with_conn(|conn| {
            conn.execute(
                "DELETE FROM worker_registry_projection_removals WHERE workspace_id='w' AND runtime_id='r' AND worker_id=?1",
                [worker_id().to_string()],
            )?;
            Ok(())
        })
        .unwrap();
        let repaired = s
            .recover_succeeded_worker_removal_catalog("w", &request.worker)
            .unwrap();
        assert!(matches!(
            repaired.changes.as_slice(),
            [WorkerCatalogChange::Removed(worker)] if worker == &request.worker
        ));
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
            recovered.runtime_request.input_fingerprint,
            plan.input_fingerprint
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
    fn subject_deletion_snapshot_overrides_policy_without_changing_normal_removal() {
        let store = setup();
        let ordinary = setup().plan_worker_removal(&req(), &inv()).unwrap();
        store
            .prepare_subject_deletion("w", "subject", "delete", &[req().worker.clone()])
            .unwrap();
        let body = store.plan_worker_removal(&req(), &inv()).unwrap();
        assert_eq!(body.session_disposition, SessionDisposition::Archive);
        assert_eq!(body.archive_retention, ArchiveRetention::Forever);
        assert_eq!(body.metadata_disposition, MetadataDisposition::Tombstone);
        assert_eq!(body.diagnostics_disposition, DiagnosticsDisposition::Retain);
        assert!(body.diagnostics_retention_seconds.is_some());
        assert_ne!(body.policy_id, ordinary.policy_id);
        // A new deletion-owned plan uses the archival policy; independently saved
        // old requests are reconciled separately, without borrowing this policy.
        let ordinary_store = setup();
        assert_eq!(
            ordinary_store
                .plan_worker_removal(&req(), &inv())
                .unwrap()
                .policy_id,
            ordinary.policy_id
        );
    }

    #[test]
    fn subject_deletion_pins_old_archive_input_without_counting_it_as_job_purge() {
        let store = setup();
        let old = store.plan_worker_removal(&req(), &inv()).unwrap();
        let original = store
            .prepare_worker_removal_execution("w", &old.plan_id, &old.input_fingerprint)
            .unwrap();
        store
            .prepare_subject_deletion("w", "subject", "delete", &[req().worker.clone()])
            .unwrap();
        store
            .with_conn(|c| {
                c.execute(
                    "UPDATE subject_deletion_workers SET is_job=1 WHERE workspace_id='w'",
                    [],
                )?;
                Ok(())
            })
            .unwrap();
        let recovered = store
            .recover_worker_removal_execution("w", &req().worker)
            .unwrap()
            .unwrap();
        assert_eq!(recovered.runtime_request, original.runtime_request);
        let prepared = store
            .prepare_worker_removal_execution("w", &old.plan_id, &old.input_fingerprint)
            .unwrap();
        assert_eq!(prepared.runtime_request, original.runtime_request);
        assert!(store.plan_worker_removal(&req(), &inv()).is_err());
        assert!(
            store
                .erase_subject_deleted_jobs("w", "subject", "delete")
                .is_err()
        );
        let fresh = store
            .with_conn(|c| load_removal_policy(c, "w", &req().worker, false, true))
            .unwrap()
            .unwrap();
        assert_eq!(fresh.session_disposition, SessionDisposition::Purge);
        assert_eq!(fresh.diagnostics_disposition, DiagnosticsDisposition::Purge);
        assert_eq!(fresh.metadata_disposition, MetadataDisposition::Tombstone);
        assert_eq!(
            store
                .with_conn(|c| load_plan(c, &old.plan_id))
                .unwrap()
                .unwrap()
                .state,
            WorkerRemovalPlanState::Executing
        );
    }
}

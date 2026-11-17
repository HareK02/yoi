//! Launch policy persistence uses the Store's existing connection and transaction
//! fence. Schema installation belongs to ordered Workspace migrations, never here.

use super::*;
use server_api::authorization::CoreWorkerExecutionGrant;
use server_api::launch_policy::{
    WorkerLaunchPolicy, WorkerLaunchPolicyMutation, WorkerLaunchPolicyReceipt,
};

impl SqliteWorkspaceStore {
    pub fn worker_launch_policy(
        &self,
        workspace: &str,
        policy_id: &str,
    ) -> Result<Option<WorkerLaunchPolicy>> {
        self.with_conn(|conn| crate::launch_policy::policy(conn, workspace, policy_id))
    }

    pub fn worker_launch_policies(&self, workspace: &str) -> Result<Vec<WorkerLaunchPolicy>> {
        self.with_conn(|conn| crate::launch_policy::policies(conn, workspace))
    }

    /// `account` must come from authenticated owner context, never request data.
    pub fn mutate_worker_launch_policy(
        &self,
        workspace: &str,
        account: &str,
        request: &WorkerLaunchPolicyMutation,
    ) -> Result<WorkerLaunchPolicyReceipt> {
        self.with_immediate_transaction(|tx| {
            crate::launch_policy::mutate(tx, workspace, account, request)
        })
    }

    pub fn core_worker_execution_grant(
        &self,
        workspace: &str,
        runtime: &str,
        worker: &str,
    ) -> Result<Option<CoreWorkerExecutionGrant>> {
        self.with_conn(|conn| crate::launch_policy::core_grant(conn, workspace, runtime, worker))
    }

    /// Host-only binding after reservation and issuance authorization. Call the
    /// connection-level helper instead when binding other reservation data in the
    /// same transaction; never dispatch until all those writes are committed.
    #[cfg(test)]
    pub(crate) fn bind_core_worker_execution_grant(
        &self,
        grant: &CoreWorkerExecutionGrant,
    ) -> Result<CoreWorkerExecutionGrant> {
        self.with_immediate_transaction(|tx| crate::launch_policy::bind_core_grant(tx, grant))
    }

    pub fn revoke_core_worker_execution_grant(
        &self,
        workspace: &str,
        account: &str,
        grant_id: &str,
        operation_key: &str,
    ) -> Result<()> {
        self.with_immediate_transaction(|tx| {
            crate::launch_policy::revoke_core_grant(tx, workspace, account, grant_id, operation_key)
        })
    }
}

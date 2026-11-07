//! Workspace-owned inventory omissions retain public resource references. These
//! facts never come from provider diagnostics, saved text or diagnostic code.
use error_contract::{
    Browser, ContractError, Disclosure, FailureCause, FailureFacts, FailureIdentity,
    InternalTransfer, OperationContext, ProjectFor, Storage, Telemetry, Tool,
};
use workdir::management_failure::{
    WorkdirManagementFailure, WorkdirManagementOperation, WorkdirManagementReason,
    WorkdirManagementVisibility,
};

pub(crate) struct LegacyExternalGrantPermissions {
    pub workdir_id: String,
    pub grant_id: String,
}
impl LegacyExternalGrantPermissions {
    fn details(&self) -> WorkdirManagementFailure {
        WorkdirManagementFailure {
            operation: WorkdirManagementOperation::List,
            reason: WorkdirManagementReason::LegacyExternalGrantPermissions,
        }
    }
}
impl FailureCause for LegacyExternalGrantPermissions {
    fn identity(&self) -> FailureIdentity {
        self.details().identity()
    }
    fn validate(&self) -> Result<(), ContractError> {
        for id in [&self.workdir_id, &self.grant_id] {
            if id.is_empty()
                || id.len() > 128
                || !id
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
            {
                return Err(ContractError::InvalidDetails);
            }
        }
        Ok(())
    }
}
macro_rules! project {
    ($($audience:ty),+) => {$ (
        impl ProjectFor<$audience> for LegacyExternalGrantPermissions {
            type Authorization = WorkdirManagementVisibility;
            type Details = WorkdirManagementFailure;
            fn disclose(&self, _: &FailureFacts<Self>, visibility: &Self::Authorization) -> Result<Disclosure<Self::Details>, ContractError> {
                if !visibility.can_view_cause { return Err(ContractError::ProjectionUnavailable); }
                let mut facts = FailureFacts::new(self.details())?;
                for (operation, reference) in [("workdir.inventory", &self.workdir_id), ("workdir.grant", &self.grant_id)] {
                    let mut context = OperationContext::new(operation, "legacy_permissions")?;
                    context.operation_ref = Some(reference.clone());
                    facts.with_context(context)?;
                }
                <WorkdirManagementFailure as ProjectFor<$audience>>::disclose(&self.details(), &facts, visibility)
            }
        }
    )+};
}
project!(Browser, Tool, InternalTransfer, Storage, Telemetry);

use std::sync::Arc;

use agen::tool::ToolError;
use workdir::{
    ResolvedWorkdirSession, WorkdirAttachmentAlias, WorkdirSessionCapability, WorkdirSessionHandle,
    WorkdirSessionRouter,
};

pub(crate) fn singleton_router(session: WorkdirSessionHandle) -> Arc<WorkdirSessionRouter> {
    let router = Arc::new(WorkdirSessionRouter::new());
    router
        .attach(
            WorkdirAttachmentAlias::new("workdir").expect("static attachment alias is valid"),
            session,
        )
        .expect("fresh singleton router accepts its session");
    router
}

pub(crate) fn resolve_session(
    router: &WorkdirSessionRouter,
    target_workdir: Option<&str>,
    required: WorkdirSessionCapability,
) -> Result<ResolvedWorkdirSession, ToolError> {
    let selected = router
        .resolve(target_workdir)
        .map_err(|error| ToolError::InvalidArgument(error.to_string()))?;
    if !selected.session.capabilities().supports(required) {
        return Err(crate::ToolsError::from(workdir::WorkdirError::Unsupported(required)).into());
    }
    Ok(selected)
}

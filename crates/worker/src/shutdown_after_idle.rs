use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use async_trait::async_trait;
use protocol::WorkerStatus;
use ticket::config::TicketRole;

use crate::hook::{Hook, HookPostToolAction, PostToolCall, ToolResultSummary};

const TICKET_INTAKE_READY_TOOL_NAME: &str = "TicketIntakeReady";

#[derive(Clone, Default)]
pub(crate) struct ShutdownAfterIdleRequest {
    requested: Arc<AtomicBool>,
}

impl ShutdownAfterIdleRequest {
    pub(crate) fn request(&self) {
        self.requested.store(true, Ordering::Release);
    }

    pub(crate) fn take(&self) -> bool {
        self.requested.swap(false, Ordering::AcqRel)
    }

    #[cfg(test)]
    pub(crate) fn is_requested(&self) -> bool {
        self.requested.load(Ordering::Acquire)
    }
}

pub(crate) fn is_ticket_intake_role(role: Option<&str>) -> bool {
    matches!(role.and_then(TicketRole::parse), Some(TicketRole::Intake))
}

pub(crate) fn take_shutdown_request_after_status(
    shutdown_after_idle: &ShutdownAfterIdleRequest,
    status: WorkerStatus,
) -> bool {
    status == WorkerStatus::Idle && shutdown_after_idle.take()
}

pub(crate) struct TicketIntakeReadyShutdownHook {
    shutdown_after_idle: ShutdownAfterIdleRequest,
    eligible_ticket_intake_role: bool,
}

impl TicketIntakeReadyShutdownHook {
    pub(crate) fn new(
        shutdown_after_idle: ShutdownAfterIdleRequest,
        eligible_ticket_intake_role: bool,
    ) -> Self {
        Self {
            shutdown_after_idle,
            eligible_ticket_intake_role,
        }
    }

    fn observe_tool_result(&self, info: &ToolResultSummary) {
        if self.eligible_ticket_intake_role
            && info.tool_name == TICKET_INTAKE_READY_TOOL_NAME
            && !info.is_error
        {
            self.shutdown_after_idle.request();
        }
    }
}

#[async_trait]
impl Hook<PostToolCall> for TicketIntakeReadyShutdownHook {
    async fn call(&self, info: &ToolResultSummary) -> HookPostToolAction {
        self.observe_tool_result(info);
        HookPostToolAction::Continue
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use llm_engine::tool::ToolOutput;

    fn tool_result(name: &str, is_error: bool) -> ToolResultSummary {
        ToolResultSummary {
            call_id: "tool-1".to_string(),
            tool_name: name.to_string(),
            is_error,
            output: ToolOutput {
                summary: "result".to_string(),
                content: None,
            },
        }
    }

    #[test]
    fn successful_ticket_intake_ready_schedules_shutdown_after_idle_for_intake_role() {
        let request = ShutdownAfterIdleRequest::default();
        let hook = TicketIntakeReadyShutdownHook::new(request.clone(), true);

        hook.observe_tool_result(&tool_result(TICKET_INTAKE_READY_TOOL_NAME, false));

        assert!(request.is_requested());
        assert!(request.take());
        assert!(!request.is_requested());
    }

    #[test]
    fn failed_ticket_intake_ready_does_not_schedule_shutdown_after_idle() {
        let request = ShutdownAfterIdleRequest::default();
        let hook = TicketIntakeReadyShutdownHook::new(request.clone(), true);

        hook.observe_tool_result(&tool_result(TICKET_INTAKE_READY_TOOL_NAME, true));

        assert!(!request.is_requested());
    }

    #[test]
    fn non_intake_role_does_not_schedule_shutdown_after_idle() {
        let request = ShutdownAfterIdleRequest::default();
        let hook = TicketIntakeReadyShutdownHook::new(request.clone(), false);

        hook.observe_tool_result(&tool_result(TICKET_INTAKE_READY_TOOL_NAME, false));

        assert!(!request.is_requested());
    }

    #[test]
    fn other_successful_tools_do_not_schedule_shutdown_after_idle() {
        let request = ShutdownAfterIdleRequest::default();
        let hook = TicketIntakeReadyShutdownHook::new(request.clone(), true);

        hook.observe_tool_result(&tool_result("TicketShow", false));

        assert!(!request.is_requested());
    }

    #[test]
    fn only_ticket_intake_runtime_role_is_eligible() {
        assert!(is_ticket_intake_role(Some("intake")));
        assert!(!is_ticket_intake_role(Some("orchestrator")));
        assert!(!is_ticket_intake_role(Some("coder")));
        assert!(!is_ticket_intake_role(Some("reviewer")));
        assert!(!is_ticket_intake_role(Some("unknown")));
        assert!(!is_ticket_intake_role(None));
    }

    #[test]
    fn shutdown_after_idle_is_taken_only_after_idle_status() {
        let request = ShutdownAfterIdleRequest::default();
        request.request();

        assert!(!take_shutdown_request_after_status(
            &request,
            WorkerStatus::Running
        ));
        assert!(request.is_requested());

        assert!(take_shutdown_request_after_status(
            &request,
            WorkerStatus::Idle
        ));
        assert!(!request.is_requested());
    }
}

import type {
  CleanupTargetKind,
  CleanupWorkdirCandidate,
  CleanupWorkerCandidate,
  RuntimeCleanupExecutionResponse,
  RuntimeCleanupPlanResponse,
} from "$lib/generated/runtime-api";
import type {
  BrowserCreateWorkerResponse as SharedBrowserCreateWorkerResponse,
  BrowserWorkerWorkingDirectorySelection
    as SharedBrowserWorkerWorkingDirectorySelection,
  Diagnostic as SharedDiagnostic,
  WorkerCapabilitySummary as SharedWorkerCapabilitySummary,
  WorkerLaunchOptionsResponse as SharedWorkerLaunchOptionsResponse,
  WorkerLaunchProfileCandidate as SharedWorkerLaunchProfileCandidate,
  WorkerLaunchRuntimeOption as SharedWorkerLaunchRuntimeOption,
  WorkerLaunchWorkerSummary,
  WorkerSummary as SharedWorkerSummary,
  WorkerWorkdirAttachmentSummary as SharedWorkerWorkdirAttachmentSummary,
  WorkingDirectoryRepositoryOption as SharedWorkingDirectoryRepositoryOption,
} from "$lib/generated/worker-launch-api";
import type {
  WorkingDirectoryCreateRequest,
  WorkingDirectoryCreateResponse,
  WorkingDirectoryDetailResponse,
  WorkingDirectoryListResponse,
  WorkingDirectoryOccupancy,
  WorkingDirectorySummary,
} from "$lib/generated/workdir-api";
import type {
  Event as PodProtocolEvent,
  Method as PodProtocolMethod,
  Segment as PodProtocolSegment,
  WorkerStateSnapshot,
} from "$lib/generated/protocol";
import type {
  GitCommitSummary as SharedGitCommitSummary,
  GitRemoteSummary as SharedGitRemoteSummary,
  GitRepositorySummary as SharedGitRepositorySummary,
  RepositoryDetailResponse as SharedRepositoryDetailResponse,
  RepositoryListResponse as SharedRepositoryListResponse,
  RepositoryLogResponse as SharedRepositoryLogResponse,
  RepositorySummary as SharedRepositorySummary,
  WorkspaceResponse as SharedWorkspaceResponse,
} from "$lib/workspace/api/workspace-model";

export type {
  PodProtocolEvent,
  PodProtocolMethod,
  PodProtocolSegment,
  WorkingDirectoryCreateRequest,
  WorkingDirectoryCreateResponse,
  WorkingDirectoryDetailResponse,
  WorkingDirectoryListResponse,
  WorkingDirectoryOccupancy,
  WorkingDirectorySummary,
};
export type WorkspaceResponse = SharedWorkspaceResponse;

export type Diagnostic = SharedDiagnostic;

export type Runtime = {
  runtime_id: string;
  label: string;
  kind: string;
  status: string;
  host_ids: string[];
  worker_creation_available: boolean;
  os: string;
  arch: string;
  diagnostics: Diagnostic[];
  management?: {
    built_in: boolean;
    config_managed: boolean;
    removable: boolean;
    endpoint_configured: boolean;
    token_ref_configured: boolean;
  };
};

export type WorkerCapabilities = SharedWorkerCapabilitySummary;

export type WorkerWorkdirAttachment = SharedWorkerWorkdirAttachmentSummary;

export type Worker =
  & Omit<
    SharedWorkerSummary,
    "display_name" | "tags" | "worker_state" | "diagnostics"
  >
  & {
    display_name: string;
    tags: string[];
    worker_state?: WorkerStateSnapshot | null;
    diagnostics: Diagnostic[];
  };

export type WorkerOperationState = "accepted" | "unsupported" | "rejected";

/** Typed result of restore across Runtime, Workspace, Web, and TUI clients. */
export type WorkerRestoreState =
  | "accepted"
  | "rejected"
  | "rolled_back"
  | "reconciliation_required";

export type WorkerRestoreResult = {
  state: WorkerRestoreState;
  worker?: WorkerLaunchWorkerSummary | null;
  diagnostics: Diagnostic[];
};

export type WorkerLaunchRuntimeOption = SharedWorkerLaunchRuntimeOption;
export type WorkerLaunchProfileCandidate = SharedWorkerLaunchProfileCandidate;
export type WorkingDirectoryRepositoryOption =
  SharedWorkingDirectoryRepositoryOption;

export type {
  CleanupTargetKind,
  CleanupWorkdirCandidate,
  CleanupWorkerCandidate,
  RuntimeCleanupExecutionResponse,
  RuntimeCleanupPlanResponse,
};

export type BrowserWorkerWorkingDirectorySelection =
  SharedBrowserWorkerWorkingDirectorySelection;
export type WorkerLaunchOptionsResponse = SharedWorkerLaunchOptionsResponse;
export type BrowserCreateWorkerResponse = SharedBrowserCreateWorkerResponse;

export type WorkerInputResult = {
  state: WorkerOperationState;
  runtime_id: string;
  worker_id: string;
  diagnostics: Diagnostic[];
};

export type ListResponse<T> = {
  workspace_id: string;
  limit: number;
  items: T[];
  source: string;
  diagnostics: Diagnostic[];
};

export type RepositorySummary = SharedRepositorySummary;
export type GitRepositorySummary = SharedGitRepositorySummary;
export type GitRemoteSummary = SharedGitRemoteSummary;
export type GitCommitSummary = SharedGitCommitSummary;
export type RepositoryListResponse = SharedRepositoryListResponse;
export type RepositoryDetailResponse = SharedRepositoryDetailResponse;
export type RepositoryLogResponse = SharedRepositoryLogResponse;

export type {
  ObjectiveDetail,
  ObjectiveLinkedTicketSummary,
  ObjectiveListResponse,
  ObjectiveSummary,
  TicketDetail,
  TicketEventDetail,
  TicketListResponse,
} from "$lib/generated/ticket-api";
export type {
  TicketDetailDerivedRelation as DerivedTicketRelation,
  TicketDetailRelation as TicketRelation,
  TicketDetailRelationBlocker as TicketRelationBlocker,
  TicketDetailRelationNotice as TicketRelationNotice,
  TicketDetailRelationView as TicketRelationView,
  TicketListItemSummary as TicketSummary,
} from "$lib/generated/ticket-api";

export type {
  CompanionCancelRequest,
  CompanionLifecycleState,
  CompanionMessageDisposition,
  CompanionMessageRequest,
  CompanionMessageResponse,
  CompanionStatusResponse,
  CompanionTranscriptItem,
  CompanionTranscriptProjection,
  CompanionTranscriptRole,
  CompanionTransportSummary,
} from "$lib/generated/companion-api";

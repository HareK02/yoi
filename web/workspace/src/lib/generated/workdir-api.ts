// Generated from workspace-api. Do not edit by hand.
// Regenerate: cargo run -q -p workspace-api --features typescript --example generate_workdir_api_types > web/workspace/src/lib/generated/workdir-api.ts

export type DiagnosticSeverity = "info" | "warning" | "error";

export type Diagnostic = { code: string, severity: DiagnosticSeverity, message: string, };

export type WorkingDirectoryMaterializerKind = "runtime_git_cache" | "local_git_worktree";

export type WorkingDirectoryStatusKind = "active" | "cleanup_pending" | "corrupted" | "not_found" | "unknown";

export type WorkingDirectoryCleanupTarget = { kind: string, working_directory_id: string, repository_id: string, };

export type WorkingDirectoryOccupancy = { runtime_id: string, worker_id: string, display_name: string, linked_at: string, };

export type WorkingDirectorySummary = { working_directory_id: string, repository_id: string, creation_selector?: string | null, creation_ref?: string | null, creation_tree?: string | null, current_selector?: string | null, current_ref?: string | null, current_tree?: string | null, observed_at_epoch_seconds?: number | null, materializer_kind: WorkingDirectoryMaterializerKind, cleanup_target?: WorkingDirectoryCleanupTarget | null, status: WorkingDirectoryStatusKind, cleanliness?: string | null, primary_worker_id?: string | null, occupied_by?: WorkingDirectoryOccupancy | null, };

export type WorkingDirectoryCreateRequest = { runtime_id?: string | null, repository_id: string, selector?: string | null, operation_id?: string | null, };

export type WorkingDirectoryListResponse = { workspace_id: string, items: Array<WorkingDirectorySummary>, diagnostics: Array<Diagnostic>, };

export type WorkingDirectoryDetailResponse = { workspace_id: string, runtime_id: string, item: WorkingDirectorySummary, diagnostics: Array<Diagnostic>, };

export type WorkingDirectoryCreateResponse = { workspace_id: string, runtime_id: string, item: WorkingDirectorySummary, diagnostics: Array<Diagnostic>, };

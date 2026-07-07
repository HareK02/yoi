import type { BrowserWorkerExecutionWorkspaceSelection, WorkerLaunchOptionsResponse } from './types';

export type WorkerLaunchFormState = {
  runtime_id: string;
  display_name: string;
  profile: string;
  initial_text: string;
  execution_workspace_allocation_id: string;
  execution_workspace_repository_id: string;
  execution_workspace_selector: string;
  relative_cwd: string;
};

export type BrowserCreateWorkerRequest = {
  runtime_id: string;
  display_name: string;
  profile: string;
  initial_text: string;
  execution_workspace?: BrowserWorkerExecutionWorkspaceSelection;
};

export function defaultWorkerLaunchForm(
  options: WorkerLaunchOptionsResponse | null,
  current: WorkerLaunchFormState,
): WorkerLaunchFormState {
  const preferredRuntime = options?.runtimes.find((runtime) => runtime.can_spawn_worker && runtime.status === 'active')
    ?? options?.runtimes.find((runtime) => runtime.can_spawn_worker)
    ?? options?.runtimes[0];
  const preferredProfile = options?.profiles.find((candidate) => candidate.id === 'builtin:coder')
    ?? options?.profiles[0];
  const preferredExecutionWorkspace = options?.execution_workspaces.find((workspace) => workspace.status === 'active')
    ?? options?.execution_workspaces[0];
  const preferredRepository = options?.repositories.find((repository) => repository.id === current.execution_workspace_repository_id)
    ?? options?.repositories[0];

  return {
    runtime_id: current.runtime_id || preferredRuntime?.runtime_id || '',
    display_name: current.display_name || 'Coding Worker',
    profile: options?.profiles.some((candidate) => candidate.id === current.profile)
      ? current.profile
      : preferredProfile?.id || '',
    initial_text: current.initial_text,
    execution_workspace_allocation_id: options?.execution_workspaces.some(
      (workspace) => workspace.allocation_id === current.execution_workspace_allocation_id,
    )
      ? current.execution_workspace_allocation_id
      : preferredExecutionWorkspace?.allocation_id || '',
    execution_workspace_repository_id: current.execution_workspace_repository_id || preferredRepository?.id || '',
    execution_workspace_selector: current.execution_workspace_selector || preferredRepository?.default_selector || 'HEAD',
    relative_cwd: current.relative_cwd,
  };
}

export function buildBrowserCreateWorkerRequest(
  form: WorkerLaunchFormState,
): BrowserCreateWorkerRequest {
  const request: BrowserCreateWorkerRequest = {
    runtime_id: form.runtime_id,
    display_name: form.display_name,
    profile: form.profile,
    initial_text: form.initial_text,
  };
  if (form.execution_workspace_allocation_id) {
    request.execution_workspace = {
      allocation_id: form.execution_workspace_allocation_id,
    };
    const relativeCwd = form.relative_cwd.trim();
    if (relativeCwd) {
      request.execution_workspace.relative_cwd = relativeCwd;
    }
  }
  return request;
}

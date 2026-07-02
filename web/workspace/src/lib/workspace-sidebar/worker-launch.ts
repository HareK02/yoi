import type { WorkerLaunchOptionsResponse } from './types';

export type WorkerLaunchFormState = {
  runtime_id: string;
  display_name: string;
  profile: string;
  initial_text: string;
};

export type BrowserCreateWorkerRequest = WorkerLaunchFormState;

export function defaultWorkerLaunchForm(
  options: WorkerLaunchOptionsResponse | null,
  current: WorkerLaunchFormState,
): WorkerLaunchFormState {
  const preferredRuntime = options?.runtimes.find((runtime) => runtime.can_spawn_worker && runtime.status === 'active')
    ?? options?.runtimes.find((runtime) => runtime.can_spawn_worker)
    ?? options?.runtimes[0];
  const preferredProfile = options?.profiles.find((candidate) => candidate.id === 'builtin:coder')
    ?? options?.profiles[0];

  return {
    runtime_id: current.runtime_id || preferredRuntime?.runtime_id || '',
    display_name: current.display_name || 'Coding Worker',
    profile: options?.profiles.some((candidate) => candidate.id === current.profile)
      ? current.profile
      : preferredProfile?.id || '',
    initial_text: current.initial_text,
  };
}

export function buildBrowserCreateWorkerRequest(
  form: WorkerLaunchFormState,
): BrowserCreateWorkerRequest {
  return {
    runtime_id: form.runtime_id,
    display_name: form.display_name,
    profile: form.profile,
    initial_text: form.initial_text,
  };
}

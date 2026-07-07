import type {
  BrowserWorkerWorkingDirectorySelection,
  WorkerLaunchOptionsResponse,
} from "./types";

export type WorkerLaunchFormState = {
  runtime_id: string;
  display_name: string;
  profile: string;
  initial_text: string;
  working_directory_allocation_id: string;
  working_directory_repository_id: string;
  working_directory_selector: string;
  relative_cwd: string;
};

export type BrowserCreateWorkerRequest = {
  runtime_id: string;
  display_name: string;
  profile: string;
  initial_text: string;
  working_directory?: BrowserWorkerWorkingDirectorySelection;
};

export function defaultWorkerLaunchForm(
  options: WorkerLaunchOptionsResponse | null,
  current: WorkerLaunchFormState,
): WorkerLaunchFormState {
  const preferredRuntime =
    options?.runtimes.find((runtime) =>
      runtime.can_spawn_worker && runtime.status === "active"
    ) ??
      options?.runtimes.find((runtime) => runtime.can_spawn_worker) ??
      options?.runtimes[0];
  const preferredProfile =
    options?.profiles.find((candidate) => candidate.id === "builtin:coder") ??
      options?.profiles[0];
  const preferredWorkingDirectory =
    options?.working_directories.find((directory) =>
      directory.status === "active"
    ) ??
      options?.working_directories[0];
  const preferredRepository =
    options?.repositories.find((repository) =>
      repository.id === current.working_directory_repository_id
    ) ??
      options?.repositories[0];

  return {
    runtime_id: current.runtime_id || preferredRuntime?.runtime_id || "",
    display_name: current.display_name || "Coding Worker",
    profile:
      options?.profiles.some((candidate) => candidate.id === current.profile)
        ? current.profile
        : preferredProfile?.id || "",
    initial_text: current.initial_text,
    working_directory_allocation_id: options?.working_directories.some(
        (directory) =>
          directory.allocation_id === current.working_directory_allocation_id,
      )
      ? current.working_directory_allocation_id
      : preferredWorkingDirectory?.allocation_id || "",
    working_directory_repository_id: current.working_directory_repository_id ||
      preferredRepository?.id || "",
    working_directory_selector: current.working_directory_selector ||
      preferredRepository?.default_selector || "HEAD",
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
  if (form.working_directory_allocation_id) {
    request.working_directory = {
      allocation_id: form.working_directory_allocation_id,
    };
    const relativeCwd = form.relative_cwd.trim();
    if (relativeCwd) {
      request.working_directory.relative_cwd = relativeCwd;
    }
  }
  return request;
}

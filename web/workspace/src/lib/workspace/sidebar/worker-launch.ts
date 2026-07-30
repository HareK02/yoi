import type {
  BrowserWorkerWorkingDirectorySelection,
  WorkerLaunchOptionsResponse,
} from "./types";

export type WorkerLaunchFormState = {
  runtime_id: string;
  display_name: string;
  profile: string;
  initial_text: string;
  working_directory_id: string;
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
  const preferredProfile = options?.profiles.find((candidate) =>
    candidate.id === options.default_profile
  );
  const availableWorkingDirectories =
    options?.working_directories.filter((directory) =>
      directory.status === "active" &&
      directory.cleanliness === "clean" &&
      directory.primary_worker_id == null &&
      directory.occupied_by == null
    ) ?? [];
  const selectedRuntime = current.runtime_id
    ? options?.runtimes.find((runtime) =>
      runtime.runtime_id === current.runtime_id
    )
    : preferredRuntime;
  const workdirlessRuntime =
    selectedRuntime?.working_directory_required === false;
  const preferredWorkingDirectory = workdirlessRuntime
    ? undefined
    : availableWorkingDirectories.find((directory) =>
      Boolean(current.working_directory_repository_id) &&
      directory.repository_id === current.working_directory_repository_id &&
      (!current.working_directory_selector ||
        directory.requested_selector === current.working_directory_selector)
    ) ?? availableWorkingDirectories.find((directory) =>
      Boolean(current.working_directory_repository_id) &&
      directory.repository_id === current.working_directory_repository_id
    ) ?? (current.working_directory_repository_id
      ? undefined
      : availableWorkingDirectories[0]);
  const preferredRepository =
    options?.repositories.find((repository) =>
      repository.id === current.working_directory_repository_id
    ) ??
      options?.repositories[0];

  return {
    runtime_id: current.runtime_id || preferredRuntime?.runtime_id || "",
    display_name: current.display_name || "Worker",
    profile:
      options?.profiles.some((candidate) => candidate.id === current.profile)
        ? current.profile
        : preferredProfile?.id || "",
    initial_text: current.initial_text,
    working_directory_id:
      !workdirlessRuntime && availableWorkingDirectories.some(
          (directory) =>
            directory.working_directory_id === current.working_directory_id,
        )
        ? current.working_directory_id
        : preferredWorkingDirectory?.working_directory_id || "",
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
  if (form.working_directory_id) {
    request.working_directory = {
      working_directory_id: form.working_directory_id,
    };
    const relativeCwd = form.relative_cwd.trim();
    if (relativeCwd) {
      request.working_directory.relative_cwd = relativeCwd;
    }
  }
  return request;
}

import type { CreateWorkspaceWorkerRequest } from "$lib/generated/worker-launch-api";
import { parseCreateWorkspaceWorkerRequest } from "$lib/workspace/api/workers";

import type { WorkerLaunchOptionsResponse } from "./types";

export type WorkerLaunchFormState = {
  runtime_id: string;
  display_name: string;
  profile: string;
  initial_text: string;
  working_directory_id: string;
  working_directory_repository_key: string;
  working_directory_selector: string;
  relative_cwd: string;
};

export function defaultWorkerLaunchForm(
  options: WorkerLaunchOptionsResponse | null,
  current: WorkerLaunchFormState,
): WorkerLaunchFormState {
  const preferredRuntime =
    options?.runtimes.find((runtime) =>
      runtime.worker_creation_available && runtime.status === "active"
    ) ??
      options?.runtimes.find((runtime) => runtime.worker_creation_available) ??
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
      Boolean(current.working_directory_repository_key) &&
      directory.repository_key === current.working_directory_repository_key &&
      (!current.working_directory_selector ||
        (directory.current_selector ?? directory.creation_selector) ===
          current.working_directory_selector)
    ) ?? availableWorkingDirectories.find((directory) =>
      Boolean(current.working_directory_repository_key) &&
      directory.repository_key === current.working_directory_repository_key
    ) ?? (current.working_directory_repository_key
      ? undefined
      : availableWorkingDirectories[0]);
  const preferredRepository =
    options?.repositories.find((repository) =>
      repository.repository_key === current.working_directory_repository_key
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
    working_directory_repository_key:
      current.working_directory_repository_key ||
      preferredRepository?.repository_key || "",
    working_directory_selector: current.working_directory_selector ||
      preferredRepository?.default_selector || "HEAD",
    relative_cwd: current.relative_cwd,
  };
}

export function buildCreateWorkspaceWorkerRequest(
  form: WorkerLaunchFormState,
): CreateWorkspaceWorkerRequest {
  const initialMessage = form.initial_text.trim();
  return parseCreateWorkspaceWorkerRequest({
    runtime_id: form.runtime_id.trim(),
    display_name: form.display_name.trim(),
    profile: form.profile.trim() || null,
    ticket_assignment: null,
    initial_submit: initialMessage
      ? [{ kind: "text", content: form.initial_text }]
      : [],
    working_directory: form.working_directory_id
      ? {
        working_directory_id: form.working_directory_id,
        relative_cwd: form.relative_cwd.trim() || null,
      }
      : null,
    control_operation_id: null,
  });
}

import type {
  BrowserWorkerWorkingDirectorySelection,
  CreateWorkspaceWorkerRequest,
} from "$lib/generated/worker-launch-api";
import { parseCreateWorkspaceWorkerRequest } from "$lib/workspace/api/workers";

import type { WorkerLaunchOptionsResponse } from "./types";

export type WorkerLaunchAttachmentFormState = {
  alias: string;
  working_directory_id: string;
  relative_cwd: string;
};

export type WorkerLaunchFormState = {
  runtime_id: string;
  display_name: string;
  profile: string;
  initial_text: string;
  workdir_attachments: WorkerLaunchAttachmentFormState[];
  working_directory_repository_key: string;
  working_directory_selector: string;
};

const WORKDIR_ALIAS_PATTERN = /^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/;

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
      (directory.source.kind === "external_grant" ||
        directory.cleanliness === "clean") &&
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
      directory.source.kind === "repository" &&
      directory.source.repository_key ===
        current.working_directory_repository_key &&
      (!current.working_directory_selector ||
        (directory.current_selector ?? directory.creation_selector) ===
          current.working_directory_selector)
    ) ?? availableWorkingDirectories.find((directory) =>
      Boolean(current.working_directory_repository_key) &&
      directory.source.kind === "repository" &&
      directory.source.repository_key ===
        current.working_directory_repository_key
    ) ?? (current.working_directory_repository_key
      ? undefined
      : availableWorkingDirectories[0]);
  const preferredRepository =
    options?.repositories.find((repository) =>
      repository.repository_key === current.working_directory_repository_key
    ) ??
      options?.repositories[0];
  const availableWorkdirIds = new Set(
    availableWorkingDirectories.map((directory) =>
      directory.working_directory_id
    ),
  );
  const currentAttachments = workdirlessRuntime
    ? []
    : current.workdir_attachments.filter((attachment) =>
      availableWorkdirIds.has(attachment.working_directory_id)
    );
  const workdirAttachments = currentAttachments.length > 0
    ? currentAttachments
    : preferredWorkingDirectory
    ? [{
      alias: "workdir",
      working_directory_id: preferredWorkingDirectory.working_directory_id,
      relative_cwd: "",
    }]
    : selectedRuntime?.working_directory_required === true
    ? [{ alias: "workdir", working_directory_id: "", relative_cwd: "" }]
    : [];

  return {
    runtime_id: current.runtime_id || preferredRuntime?.runtime_id || "",
    display_name: current.display_name || "Worker",
    profile:
      options?.profiles.some((candidate) => candidate.id === current.profile)
        ? current.profile
        : preferredProfile?.id || "",
    initial_text: current.initial_text,
    workdir_attachments: workdirAttachments,
    working_directory_repository_key:
      current.working_directory_repository_key ||
      preferredRepository?.repository_key || "",
    working_directory_selector: current.working_directory_selector ||
      preferredRepository?.default_selector || "HEAD",
  };
}

export function workerLaunchAttachmentError(
  attachments: WorkerLaunchAttachmentFormState[],
): string | null {
  const aliases = new Set<string>();
  const workdirIds = new Set<string>();

  for (const [index, attachment] of attachments.entries()) {
    const position = index + 1;
    const alias = attachment.alias.trim();
    const workdirId = attachment.working_directory_id.trim();
    const relativeCwd = attachment.relative_cwd.trim();

    if (!WORKDIR_ALIAS_PATTERN.test(alias)) {
      return `Attachment ${position} alias must be 1–64 ASCII letters, digits, dots, underscores, or hyphens, and start with a letter or digit.`;
    }
    if (aliases.has(alias)) {
      return `Attachment alias “${alias}” is used more than once.`;
    }
    aliases.add(alias);

    if (!workdirId) {
      return `Attachment ${position} must select a Workdir.`;
    }
    if (workdirIds.has(workdirId)) {
      return "A Workdir cannot be attached more than once to one Worker.";
    }
    workdirIds.add(workdirId);

    if (
      relativeCwd.startsWith("/") ||
      relativeCwd.split("/").some((component) => component === "..")
    ) {
      return `Attachment ${position} relative cwd must stay inside the selected Workdir.`;
    }
  }

  return null;
}

function validatedAttachments(
  attachments: WorkerLaunchAttachmentFormState[],
): BrowserWorkerWorkingDirectorySelection[] {
  const error = workerLaunchAttachmentError(attachments);
  if (error) {
    throw new Error(error);
  }
  return attachments.map((attachment) => ({
    alias: attachment.alias.trim(),
    working_directory_id: attachment.working_directory_id.trim(),
    relative_cwd: attachment.relative_cwd.trim() || null,
  }));
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
    workdir_attachments: validatedAttachments(form.workdir_attachments),
    control_operation_id: null,
  });
}

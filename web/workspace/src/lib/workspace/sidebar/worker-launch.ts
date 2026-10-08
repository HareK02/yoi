import type {
  BrowserWorkerWorkingDirectorySelection,
  CreateWorkspaceWorkerRequest,
  CreateWorkspaceWorkerTicketAssignmentRequest,
  RepositoryApiError,
  WorkerLaunchProfileCandidate,
} from "#lib/generated/worker-launch-api.ts";
import { parseCreateWorkspaceWorkerRequest } from "#lib/workspace/api/workers.ts";

import type { WorkerLaunchOptionsResponse } from "./types";

export type WorkerLaunchAttachmentFormState = {
  alias: string;
  working_directory_id: string;
  relative_cwd: string;
};

export type WorkerLaunchFormState = {
  runtime_id: string;
  flow?: string;
  ticket_assignment?: CreateWorkspaceWorkerTicketAssignmentRequest | null;
  display_name: string;
  profile: string;
  subjektiv_subject_id: string;
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
  const runtime =
    options?.runtimes.find((candidate) => candidate.runtime_id === current.runtime_id) ??
      preferredRuntime;
  const availableWorkdirIds = new Set(
    (options?.working_directories ?? []).filter((directory) =>
      directory.status === "active" && directory.source.kind !== "workspace_config" &&
      (directory.source.kind === "external_grant" || directory.cleanliness === "clean") &&
      directory.occupied_by == null
    ).map((directory) => directory.working_directory_id),
  );
  // Defaults never claim resource attachments or infer an execution recipe from a Ticket.
  return {
    ...current,
    runtime_id: current.runtime_id || preferredRuntime?.runtime_id || "",
    display_name: current.display_name || "Worker",
    profile:
      genericLaunchProfiles(options?.profiles ?? []).some((candidate) =>
          candidate.id === current.profile
        )
        ? current.profile
        : "",
    workdir_attachments: runtime?.supports_workdir_attachments === false
      ? []
      : current.workdir_attachments.filter((attachment) =>
        availableWorkdirIds.has(attachment.working_directory_id)
      ),
    working_directory_selector: current.working_directory_selector || "HEAD",
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
  if (["builtin:reviewer", "reviewer"].includes(form.profile.trim())) {
    throw new Error("Trusted Reviewer launch requires a bound review request.");
  }
  const initialMessage = form.initial_text.trim();
  return parseCreateWorkspaceWorkerRequest({
    runtime_id: form.runtime_id.trim(),
    display_name: form.display_name.trim(),
    profile: form.profile.trim() || null,
    ticket_assignment: form.ticket_assignment ?? null,
    initial_submit: [
      ...(initialMessage ? [{ kind: "text", content: form.initial_text }] : []),
      ...(form.flow?.trim() ? [{ kind: "flow", selector: form.flow.trim() }] : []),
    ],
    workdir_attachments: validatedAttachments(form.workdir_attachments),
    feature_connections: form.subjektiv_subject_id.trim()
      ? { subjektiv: { subject_id: form.subjektiv_subject_id.trim() } }
      : {},
    control_operation_id: null,
  });
}

// A trusted Reviewer is launched through the bound review path, not a Profile selection.
export function genericLaunchProfiles(
  profiles: WorkerLaunchProfileCandidate[],
): WorkerLaunchProfileCandidate[] {
  return profiles.filter((candidate) =>
    candidate.id !== "builtin:reviewer" && candidate.id !== "reviewer"
  );
}

export function workerLaunchOutcomeUnknown(error: RepositoryApiError): boolean {
  return [
    error.error,
    error.message,
    ...(error.diagnostics ?? []).flatMap((item) => [item.code, item.message]),
  ]
    .some((value) => /outcome[_\s-]?unknown|OutcomeUnknown/i.test(value));
}

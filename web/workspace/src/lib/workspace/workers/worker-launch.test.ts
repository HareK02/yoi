import {
  buildCreateWorkspaceWorkerRequest,
  defaultWorkerLaunchForm,
  genericLaunchProfiles,
  workerLaunchOutcomeUnknown,
} from "../sidebar/worker-launch.ts";
import type { WorkerLaunchFormState } from "../sidebar/worker-launch.ts";
import { emptyLaunchOptions } from "#lib/workspace/sidebar/worker-launch.test-fixtures.ts";
import { load } from "../../../routes/w/[workspaceId]/workers/new/+page.ts";

declare const Deno: { test(name: string, fn: () => void): void };
const form: WorkerLaunchFormState = {
  runtime_id: "embedded",
  display_name: "Worker",
  profile: "",
  subjektiv_subject_id: "",
  initial_text: "",
  workdir_attachments: [],
  working_directory_repository_key: "",
  working_directory_selector: "HEAD",
};
function equal(actual: unknown, expected: unknown) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      `Expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`,
    );
  }
}

Deno.test("generic Ticket request keeps conclusion and review instructions as natural text with no mandatory Flow or profile", () => {
  for (
    const text of [
      "Review is unnecessary.",
      "Review, then merge if approved.",
      "Return without a conclusion.",
    ]
  ) {
    const request = buildCreateWorkspaceWorkerRequest({
      ...form,
      initial_text: text,
      ticket_assignment: { ticket_id: "T-720", operation_id: "launch-720" },
    });
    equal(request.ticket_assignment, {
      ticket_id: "T-720",
      operation_id: "launch-720",
    });
    equal(request.profile, null);
    equal(request.workdir_attachments, []);
    equal(request.initial_submit, [{ kind: "text", content: text }]);
  }
});

Deno.test("explicit Flow and resource selections remain separate from the Ticket request", () => {
  const request = buildCreateWorkspaceWorkerRequest({
    ...form,
    runtime_id: "remote",
    profile: "project:analysis",
    initial_text: "  Review is unnecessary.\nReturn without conclusion.  ",
    flow: " project:investigate ",
    subjektiv_subject_id: "subject-1",
    ticket_assignment: { ticket_id: "T-720", operation_id: "launch-720" },
    workdir_attachments: [{
      alias: "sessions",
      working_directory_id: "read-only-grant",
      relative_cwd: "logs",
    }],
  });
  equal(request.initial_submit, [{
    kind: "text",
    content: "  Review is unnecessary.\nReturn without conclusion.  ",
  }, { kind: "flow", selector: "project:investigate" }]);
  equal(request.workdir_attachments, [{
    alias: "sessions",
    working_directory_id: "read-only-grant",
    relative_cwd: "logs",
  }]);
  equal(request.feature_connections, {
    subjektiv: { subject_id: "subject-1" },
  });
});

Deno.test("launch defaults never claim repository or Workdir resources", () => {
  const options = emptyLaunchOptions("workspace");
  options.repositories = [{ repository_key: "repo", default_selector: "main" }];
  options.working_directories = [{
    working_directory_id: "wd-1",
    source: { kind: "repository", repository_key: "repo" },
    materializer_kind: "runtime_git_clone",
    status: "active",
    cleanliness: "clean",
  }];
  const result = defaultWorkerLaunchForm(options, {
    ...form,
    runtime_id: "remote",
  });
  equal(result.profile, "");
  equal(result.workdir_attachments, []);
  equal(result.working_directory_repository_key, "");
});

Deno.test("launch defaults preserve explicit attachments only on a supporting Runtime", () => {
  const options = emptyLaunchOptions("workspace");
  options.working_directories = [{
    working_directory_id: "read-only-grant",
    source: {
      kind: "external_grant",
      grant_id: "grant-1",
      grant_permissions: { read: true, write: false, command: false },
    },
    materializer_kind: "client_hosted_external",
    status: "active",
    cleanliness: "unknown",
  }];
  const attachments = [{
    alias: "sessions",
    working_directory_id: "read-only-grant",
    relative_cwd: "logs",
  }];
  for (const runtime of options.runtimes) {
    const result = defaultWorkerLaunchForm(options, {
      ...form,
      runtime_id: runtime.runtime_id,
      workdir_attachments: attachments,
    });
    equal(
      result.workdir_attachments,
      runtime.supports_workdir_attachments ? attachments : [],
    );
  }
});

Deno.test("generic launch cannot select a trusted Reviewer profile or infer it from URL roles", () => {
  const options = emptyLaunchOptions("workspace");
  options.profiles.push({
    ...options.profiles[0],
    id: "builtin:reviewer",
    label: "Reviewer",
  });
  equal(genericLaunchProfiles(options.profiles).map((item) => item.id), [
    "builtin:companion",
  ]);
  equal(
    defaultWorkerLaunchForm(options, { ...form, profile: "builtin:reviewer" })
      .profile,
    "",
  );
  const data = load({
    params: { workspaceId: "workspace" },
    url: new URL(
      "https://example.test/workers/new?ticketId=T-720&ticketRole=reviewer&profile=builtin:reviewer&repositoryKey=repo",
    ),
  });
  equal(data.ticketContext, {
    ticketId: "T-720",
    ticketTitle: "T-720",
    initialInput: "Work on Ticket T-720.",
  });
  let rejected = false;
  try {
    buildCreateWorkspaceWorkerRequest({ ...form, profile: "builtin:reviewer" });
  } catch {
    rejected = true;
  }
  equal(rejected, true);
});

Deno.test("OutcomeUnknown is distinguished from a definite permission rejection", () => {
  for (const code of ["OutcomeUnknown", "worker_spawn_outcome_unknown"]) {
    equal(
      workerLaunchOutcomeUnknown({
        error: "runtime_operation_failed",
        message: "Uncertain launch",
        diagnostics: [{ code, message: "Reconcile", severity: "error" }],
      }),
      true,
    );
  }
  equal(
    workerLaunchOutcomeUnknown({
      error: "permission_denied",
      message: "Read only",
      diagnostics: [],
    }),
    false,
  );
});

import {
  buildCreateWorkspaceWorkerRequest,
  defaultWorkerLaunchForm,
  workerLaunchAttachmentError,
} from "./worker-launch.ts";
import type { WorkerLaunchOptionsResponse } from "./types.ts";

declare const Deno: {
  test(name: string, fn: () => Promise<void> | void): void;
};

Deno.test("defaultWorkerLaunchForm uses the Backend-published defaults and initial Workdir", () => {
  const form = defaultWorkerLaunchForm(
    options,
    emptyForm({ initial_text: "hello" }),
  );

  assertEquals(form.runtime_id, "remote");
  assertEquals(form.display_name, "Worker");
  assertEquals(form.profile, "builtin:coder");
  assertEquals(form.initial_text, "hello");
  assertEquals(form.workdir_attachments, [{
    alias: "workdir",
    working_directory_id: "wd-1-repo",
    relative_cwd: "",
  }]);
  assertEquals(form.working_directory_repository_key, "repo");
  assertEquals(form.working_directory_selector, "HEAD");
});

Deno.test("defaultWorkerLaunchForm preserves an available read-only External Workdir", () => {
  const external = {
    working_directory_id: "external-1",
    display_name: "Session analysis",
    source: { kind: "external_grant" as const, grant_id: "grant-1" },
    materializer_kind: "client_hosted_external" as const,
    status: "active" as const,
    cleanliness: "unknown",
  };
  const form = defaultWorkerLaunchForm(
    { ...options, working_directories: [external] },
    emptyForm({
      workdir_attachments: [{
        alias: "sessions",
        working_directory_id: "external-1",
        relative_cwd: "",
      }],
    }),
  );

  assertEquals(form.workdir_attachments, [{
    alias: "sessions",
    working_directory_id: "external-1",
    relative_cwd: "",
  }]);
});

Deno.test("defaultWorkerLaunchForm preserves an available Ticket role profile", () => {
  const reviewerOptions = {
    ...options,
    profiles: [
      ...options.profiles,
      { id: "builtin:reviewer", label: "Reviewer", description: "review" },
    ],
  };
  const form = defaultWorkerLaunchForm(
    reviewerOptions,
    emptyForm({
      display_name: "Review worker",
      profile: "builtin:reviewer",
      initial_text: "Review the ticket.",
      working_directory_repository_key: "repo",
      working_directory_selector: "HEAD",
    }),
  );

  assertEquals(form.profile, "builtin:reviewer");
});

Deno.test("defaultWorkerLaunchForm skips occupied Workdirs and leaves an editable attachment", () => {
  const form = defaultWorkerLaunchForm(
    {
      ...options,
      working_directories: [{
        ...options.working_directories[0],
        occupied_by: {
          runtime_id: "embedded",
          worker_id: "0198f82e-6d90-7f15-a121-174a02e10e77",
          display_name: "Worker 12",
          linked_at: "2026-07-24T00:00:00Z",
        },
      }],
    },
    emptyForm({ initial_text: "hello" }),
  );

  assertEquals(form.workdir_attachments, [{
    alias: "workdir",
    working_directory_id: "",
    relative_cwd: "",
  }]);
});

Deno.test("defaultWorkerLaunchForm preserves a Ticket repository target", () => {
  const form = defaultWorkerLaunchForm(
    {
      ...options,
      repositories: [
        ...options.repositories,
        { repository_key: "ticket-repo", default_selector: "main" },
      ],
      working_directories: [
        options.working_directories[0],
        {
          ...options.working_directories[0],
          working_directory_id: "ticket-workdir",
          source: { kind: "repository", repository_key: "ticket-repo" },
          creation_selector: "work/ticket",
        },
      ],
    },
    emptyForm({
      display_name: "Ticket worker",
      profile: "builtin:coder",
      initial_text: "Work on a ticket.",
      working_directory_repository_key: "ticket-repo",
      working_directory_selector: "work/ticket",
    }),
  );

  assertEquals(form.workdir_attachments, [{
    alias: "workdir",
    working_directory_id: "ticket-workdir",
    relative_cwd: "",
  }]);
  assertEquals(form.working_directory_repository_key, "ticket-repo");
  assertEquals(form.working_directory_selector, "work/ticket");
});

Deno.test("defaultWorkerLaunchForm clears attachments for a Workdir-less Runtime", () => {
  const form = defaultWorkerLaunchForm(
    options,
    emptyForm({
      runtime_id: "embedded",
      workdir_attachments: [{
        alias: "checkout",
        working_directory_id: "wd-1-repo",
        relative_cwd: "src",
      }],
    }),
  );

  assertEquals(form.workdir_attachments, []);
});

Deno.test("buildCreateWorkspaceWorkerRequest emits multiple aliased attachments", () => {
  const request = buildCreateWorkspaceWorkerRequest(emptyForm({
    runtime_id: "remote",
    display_name: "Worker",
    profile: "builtin:coder",
    initial_text: "go",
    workdir_attachments: [
      {
        alias: " checkout ",
        working_directory_id: "wd-1-repo",
        relative_cwd: "crates/yoi",
      },
      {
        alias: "docs",
        working_directory_id: "wd-2-docs",
        relative_cwd: "  ",
      },
    ],
  }));

  assertEquals(request, {
    runtime_id: "remote",
    display_name: "Worker",
    profile: "builtin:coder",
    ticket_assignment: null,
    initial_submit: [{ kind: "text", content: "go" }],
    workdir_attachments: [
      {
        alias: "checkout",
        working_directory_id: "wd-1-repo",
        relative_cwd: "crates/yoi",
      },
      {
        alias: "docs",
        working_directory_id: "wd-2-docs",
        relative_cwd: null,
      },
    ],
    control_operation_id: null,
  });
});

Deno.test("buildCreateWorkspaceWorkerRequest validates attachment aliases and selections", () => {
  assertEquals(
    workerLaunchAttachmentError([attachment("invalid/alias", "wd-1-repo")]),
    "Attachment 1 alias must be 1–64 ASCII letters, digits, dots, underscores, or hyphens, and start with a letter or digit.",
  );
  assertThrows(
    () =>
      buildCreateWorkspaceWorkerRequest(emptyForm({
        workdir_attachments: [
          attachment("checkout", "wd-1-repo"),
          attachment("checkout", "wd-2-docs"),
        ],
      })),
    "Attachment alias “checkout” is used more than once.",
  );
  assertThrows(
    () =>
      buildCreateWorkspaceWorkerRequest(emptyForm({
        workdir_attachments: [
          attachment("checkout", "wd-1-repo"),
          attachment("docs", "wd-1-repo"),
        ],
      })),
    "A Workdir cannot be attached more than once to one Worker.",
  );
  assertThrows(
    () =>
      buildCreateWorkspaceWorkerRequest(emptyForm({
        workdir_attachments: [attachment("checkout", "")],
      })),
    "Attachment 1 must select a Workdir.",
  );
  assertThrows(
    () =>
      buildCreateWorkspaceWorkerRequest(emptyForm({
        workdir_attachments: [{
          ...attachment("checkout", "wd-1-repo"),
          relative_cwd: "../outside",
        }],
      })),
    "Attachment 1 relative cwd must stay inside the selected Workdir.",
  );
});

Deno.test("buildCreateWorkspaceWorkerRequest sends no initial segments for an empty draft", () => {
  const request = buildCreateWorkspaceWorkerRequest(emptyForm({
    runtime_id: "embedded",
    profile: "builtin:companion",
    initial_text: "   ",
  }));

  assertEquals(request.initial_submit, []);
});

Deno.test("buildCreateWorkspaceWorkerRequest emits an empty attachment list for Workdir-less launches", () => {
  const request = buildCreateWorkspaceWorkerRequest(emptyForm({
    runtime_id: "embedded",
    display_name: "Worker",
    profile: "builtin:companion",
    initial_text: "chat",
  }));

  assertEquals(request, {
    runtime_id: "embedded",
    display_name: "Worker",
    profile: "builtin:companion",
    ticket_assignment: null,
    initial_submit: [{ kind: "text", content: "chat" }],
    workdir_attachments: [],
    control_operation_id: null,
  });
});

function emptyForm(
  overrides: Partial<Parameters<typeof buildCreateWorkspaceWorkerRequest>[0]> =
    {},
): Parameters<typeof buildCreateWorkspaceWorkerRequest>[0] {
  return {
    runtime_id: "",
    display_name: "",
    profile: "",
    initial_text: "",
    workdir_attachments: [],
    working_directory_repository_key: "",
    working_directory_selector: "",
    ...overrides,
  };
}

function attachment(alias: string, workingDirectoryId: string) {
  return {
    alias,
    working_directory_id: workingDirectoryId,
    relative_cwd: "",
  };
}

function assertEquals<T>(actual: T, expected: T): void {
  const actualJson = JSON.stringify(actual);
  const expectedJson = JSON.stringify(expected);
  if (actualJson !== expectedJson) {
    throw new Error(`Expected ${expectedJson}, got ${actualJson}`);
  }
}

function assertThrows(fn: () => unknown, expectedMessage: string): void {
  try {
    fn();
  } catch (error) {
    if (error instanceof Error && error.message === expectedMessage) return;
    throw new Error(
      `Expected error ${JSON.stringify(expectedMessage)}, got ${String(error)}`,
    );
  }
  throw new Error(`Expected error ${JSON.stringify(expectedMessage)}`);
}

const options: WorkerLaunchOptionsResponse = {
  workspace_id: "workspace",
  runtimes: [
    {
      runtime_id: "remote",
      display_name: "Remote",
      status: "active",
      worker_creation_available: true,
      built_in: false,
      working_directory_required: true,
      diagnostics: [],
    },
    {
      runtime_id: "embedded",
      display_name: "Embedded",
      status: "active",
      worker_creation_available: true,
      built_in: true,
      working_directory_required: false,
      diagnostics: [],
    },
  ],
  default_profile: "builtin:coder",
  profiles: [
    { id: "builtin:companion", label: "Companion", description: "chat" },
    { id: "builtin:coder", label: "Coder", description: "code" },
  ],
  repositories: [
    { repository_key: "repo", default_selector: "HEAD" },
  ],
  working_directories: [
    {
      working_directory_id: "wd-1-repo",
      source: { kind: "repository", repository_key: "repo" },
      creation_selector: "HEAD",
      creation_ref: "0123456789abcdef",
      current_selector: null,
      current_ref: "0123456789abcdef",
      materializer_kind: "runtime_git_clone",
      status: "active",
      cleanliness: "clean",
      cleanup_target: {
        kind: "runtime_git_clone",
        working_directory_id: "wd-1-repo",
        repository_key: "repo",
      },
    },
  ],
  diagnostics: [],
};

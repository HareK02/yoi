import {
  buildCreateWorkspaceWorkerRequest,
  defaultWorkerLaunchForm,
} from "./worker-launch.ts";
import type { WorkerLaunchOptionsResponse } from "./types.ts";

declare const Deno: {
  test(name: string, fn: () => Promise<void> | void): void;
};

function assertEquals<T>(actual: T, expected: T): void {
  const actualJson = JSON.stringify(actual);
  const expectedJson = JSON.stringify(expected);
  if (actualJson !== expectedJson) {
    throw new Error(`Expected ${expectedJson}, got ${actualJson}`);
  }
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
      repository_key: "repo",
      creation_selector: "HEAD",
      creation_ref: "0123456789abcdef",
      current_selector: null,
      current_ref: "0123456789abcdef",
      materializer_kind: "local_git_worktree",
      status: "active",
      cleanliness: "clean",
      primary_worker_id: null,
      cleanup_target: {
        kind: "git_worktree",
        working_directory_id: "wd-1-repo",
        repository_key: "repo",
      },
    },
  ],
  diagnostics: [],
};

Deno.test("defaultWorkerLaunchForm uses the Backend-published Workspace default profile", () => {
  const form = defaultWorkerLaunchForm(options, {
    runtime_id: "",
    display_name: "",
    profile: "",
    initial_text: "hello",
    working_directory_id: "",
    working_directory_repository_key: "",
    working_directory_selector: "",
    relative_cwd: "",
  });

  assertEquals(form.runtime_id, "remote");
  assertEquals(form.display_name, "Worker");
  assertEquals(form.profile, "builtin:coder");
  assertEquals(form.initial_text, "hello");
  assertEquals(form.working_directory_id, "wd-1-repo");
  assertEquals(form.working_directory_repository_key, "repo");
  assertEquals(form.working_directory_selector, "HEAD");
});

Deno.test("defaultWorkerLaunchForm preserves an available Ticket role profile", () => {
  const reviewerOptions = {
    ...options,
    profiles: [
      ...options.profiles,
      { id: "builtin:reviewer", label: "Reviewer", description: "review" },
    ],
  };
  const form = defaultWorkerLaunchForm(reviewerOptions, {
    runtime_id: "",
    display_name: "Review worker",
    profile: "builtin:reviewer",
    initial_text: "Review the ticket.",
    working_directory_id: "",
    working_directory_repository_key: "repo",
    working_directory_selector: "HEAD",
    relative_cwd: "",
  });

  assertEquals(form.profile, "builtin:reviewer");
});

Deno.test("defaultWorkerLaunchForm skips occupied working directories", () => {
  const form = defaultWorkerLaunchForm(
    {
      ...options,
      working_directories: [
        {
          ...options.working_directories[0],
          occupied_by: {
            runtime_id: "embedded",
            worker_id: "0198f82e-6d90-7f15-a121-174a02e10e77",
            display_name: "Worker 12",
            linked_at: "2026-07-24T00:00:00Z",
          },
        },
      ],
    },
    {
      runtime_id: "",
      display_name: "",
      profile: "",
      initial_text: "hello",
      working_directory_id: "",
      working_directory_repository_key: "",
      working_directory_selector: "",
      relative_cwd: "",
    },
  );

  assertEquals(form.working_directory_id, "");
});

Deno.test("defaultWorkerLaunchForm preserves a Ticket repository target", () => {
  const form = defaultWorkerLaunchForm(
    {
      ...options,
      repositories: [
        ...options.repositories,
        {
          repository_key: "ticket-repo",
          default_selector: "main",
        },
      ],
      working_directories: [
        options.working_directories[0],
        {
          ...options.working_directories[0],
          working_directory_id: "ticket-workdir",
          repository_key: "ticket-repo",
          creation_selector: "work/ticket",
        },
      ],
    },
    {
      runtime_id: "",
      display_name: "Ticket worker",
      profile: "builtin:coder",
      initial_text: "Work on a ticket.",
      working_directory_id: "",
      working_directory_repository_key: "ticket-repo",
      working_directory_selector: "work/ticket",
      relative_cwd: "",
    },
  );

  assertEquals(form.working_directory_id, "ticket-workdir");
  assertEquals(form.working_directory_repository_key, "ticket-repo");
  assertEquals(form.working_directory_selector, "work/ticket");
});

Deno.test("buildCreateWorkspaceWorkerRequest sends working_directory id and relative cwd only", () => {
  const request = buildCreateWorkspaceWorkerRequest({
    runtime_id: "embedded",
    display_name: "Worker",
    profile: "builtin:coder",
    initial_text: "go",
    working_directory_id: "wd-1-repo",
    working_directory_repository_key: "repo",
    working_directory_selector: "main",
    relative_cwd: "crates/yoi",
  });

  assertEquals(request, {
    runtime_id: "embedded",
    display_name: "Worker",
    profile: "builtin:coder",
    initial_submit: [{ kind: "text", content: "go" }],
    working_directory: {
      working_directory_id: "wd-1-repo",
      relative_cwd: "crates/yoi",
    },
  });
});

Deno.test("buildCreateWorkspaceWorkerRequest sends no initial segments for an empty draft", () => {
  const request = buildCreateWorkspaceWorkerRequest({
    runtime_id: "embedded",
    display_name: "Worker",
    profile: "builtin:companion",
    initial_text: "   ",
    working_directory_id: "",
    working_directory_repository_key: "",
    working_directory_selector: "",
    relative_cwd: "",
  });

  assertEquals(request.initial_submit, []);
});

Deno.test("buildCreateWorkspaceWorkerRequest omits working_directory for embedded no-workdir launches", () => {
  const request = buildCreateWorkspaceWorkerRequest({
    runtime_id: "embedded",
    display_name: "Worker",
    profile: "builtin:companion",
    initial_text: "chat",
    working_directory_id: "",
    working_directory_repository_key: "",
    working_directory_selector: "",
    relative_cwd: "",
  });

  assertEquals(request, {
    runtime_id: "embedded",
    display_name: "Worker",
    profile: "builtin:companion",
    initial_submit: [{ kind: "text", content: "chat" }],
  });
});

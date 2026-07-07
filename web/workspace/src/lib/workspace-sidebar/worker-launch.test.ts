import {
  buildBrowserCreateWorkerRequest,
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
      can_spawn_worker: true,
      built_in: false,
      diagnostics: [],
    },
    {
      runtime_id: "embedded",
      display_name: "Embedded",
      status: "active",
      can_spawn_worker: true,
      built_in: false,
      diagnostics: [],
    },
  ],
  profiles: [
    { id: "builtin:companion", label: "Companion", description: "chat" },
    { id: "builtin:coder", label: "Coder", description: "code" },
  ],
  repositories: [
    { id: "repo", display_name: "Repo", default_selector: "HEAD" },
  ],
  working_directories: [
    {
      allocation_id: "alloc-1-repo",
      repository_id: "repo",
      requested_selector: "HEAD",
      materializer_kind: "local_git_worktree",
      dirty_state_policy: "clean_point_only",
      resolved_commit: "0123456789abcdef",
      status: "active",
      cleanup_policy: "manual_or_worker_stop",
      cleanup_target: {
        kind: "git_worktree",
        allocation_id: "alloc-1-repo",
        repository_id: "repo",
      },
    },
  ],
  diagnostics: [],
};

Deno.test("defaultWorkerLaunchForm chooses active runtime, coder profile, repository, and working directory", () => {
  const form = defaultWorkerLaunchForm(options, {
    runtime_id: "",
    display_name: "",
    profile: "",
    initial_text: "hello",
    working_directory_allocation_id: "",
    working_directory_repository_id: "",
    working_directory_selector: "",
    relative_cwd: "",
  });

  assertEquals(form.runtime_id, "remote");
  assertEquals(form.display_name, "Coding Worker");
  assertEquals(form.profile, "builtin:coder");
  assertEquals(form.initial_text, "hello");
  assertEquals(form.working_directory_allocation_id, "alloc-1-repo");
  assertEquals(form.working_directory_repository_id, "repo");
  assertEquals(form.working_directory_selector, "HEAD");
});

Deno.test("buildBrowserCreateWorkerRequest sends allocation id and relative cwd only", () => {
  const request = buildBrowserCreateWorkerRequest({
    runtime_id: "embedded",
    display_name: "Worker",
    profile: "builtin:coder",
    initial_text: "go",
    working_directory_allocation_id: "alloc-1-repo",
    working_directory_repository_id: "repo",
    working_directory_selector: "main",
    relative_cwd: "crates/yoi",
  });

  assertEquals(request, {
    runtime_id: "embedded",
    display_name: "Worker",
    profile: "builtin:coder",
    initial_text: "go",
    working_directory: {
      allocation_id: "alloc-1-repo",
      relative_cwd: "crates/yoi",
    },
  });
});

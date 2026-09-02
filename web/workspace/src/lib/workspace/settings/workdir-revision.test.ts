import type { WorkingDirectorySummary } from "../sidebar/types.ts";
import { formatCurrentWorkdirRevision } from "./workdir-revision.ts";

declare const Deno: {
  test(name: string, fn: () => Promise<void> | void): void;
};

function assertEquals<T>(actual: T, expected: T): void {
  if (actual !== expected) {
    throw new Error(`Expected ${String(expected)}, got ${String(actual)}`);
  }
}

function workdir(
  current_selector: string | null,
  current_ref: string | null,
): WorkingDirectorySummary {
  return {
    working_directory_id: "workdir-1",
    repository_key: "repository-1",
    current_selector,
    current_ref,
    materializer_kind: "local_git_worktree",
    status: "active",
    cleanup_target: {
      kind: "local_git_worktree",
      working_directory_id: "workdir-1",
      repository_key: "repository-1",
    },
  };
}

Deno.test("Git detached Workdir shows only its current ref", () => {
  assertEquals(
    formatCurrentWorkdirRevision(
      workdir(null, "0123456789abcdef0123456789abcdef01234567"),
      "git",
    ),
    "0123456789ab",
  );
});

Deno.test("Git Workdir with a selector shows selector at current ref", () => {
  assertEquals(
    formatCurrentWorkdirRevision(
      workdir("feature/current", "fedcba9876543210fedcba9876543210fedcba98"),
      "git",
    ),
    "feature/current@fedcba987654",
  );
});

Deno.test("non-Git Workdir does not receive Git hash formatting", () => {
  assertEquals(
    formatCurrentWorkdirRevision(
      workdir("snapshot", "revision-value"),
      "archive",
    ),
    "snapshot · revision-value",
  );
});

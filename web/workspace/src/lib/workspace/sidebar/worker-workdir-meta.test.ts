import type { WorkingDirectorySummary } from "$lib/generated/workdir-api";
import {
  type SidebarWorkdirAttachment,
  sidebarWorkdirMeta,
} from "./worker-workdir-meta.ts";

function assertEquals(actual: unknown, expected: unknown): void {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      `expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`,
    );
  }
}

function assert(condition: boolean, message: string): void {
  if (!condition) throw new Error(message);
}

declare const Deno: {
  test(name: string, fn: () => void | Promise<void>): void;
};

function workdir(
  overrides: Partial<WorkingDirectorySummary> = {},
): WorkingDirectorySummary {
  return {
    working_directory_id: "workdir-1234567890abcdef",
    repository_key: "main",
    materializer_kind: "runtime_git_clone",
    status: "active",
    ...overrides,
  };
}

function attachment(
  alias: string,
  overrides: Partial<WorkingDirectorySummary> = {},
): SidebarWorkdirAttachment {
  const workingDirectory = workdir(overrides);
  return {
    alias,
    repository_key: workingDirectory.repository_key,
    working_directory_id: workingDirectory.working_directory_id,
    working_directory: workingDirectory,
  };
}

Deno.test("Worker sidebar Workdir label prefers the current branch selector", () => {
  const meta = sidebarWorkdirMeta([
    attachment("checkout", {
      creation_selector: "develop",
      current_selector: "refs/heads/work/T-642-sidebar",
      current_ref: "0123456789abcdef0123456789abcdef01234567",
    }),
  ]);

  assertEquals(meta.text, "main:work/T-642-sidebar");
  assert(
    meta.details.includes("checkout — main:work/T-642-sidebar"),
    "attachment alias should remain available in details",
  );
  assert(
    meta.details.includes("Workdir workdir-1234567890abcdef"),
    "opaque Workdir id should remain available in details",
  );
});

Deno.test("Worker sidebar Workdir label uses the creation branch before observation", () => {
  const meta = sidebarWorkdirMeta([
    attachment("checkout", { creation_selector: "develop" }),
  ]);

  assertEquals(meta.text, "main:develop");
});

Deno.test("Worker sidebar Workdir label marks detached HEAD instead of reusing creation branch", () => {
  const meta = sidebarWorkdirMeta([
    attachment("checkout", {
      creation_selector: "develop",
      current_ref: "0123456789abcdef0123456789abcdef01234567",
    }),
  ]);

  assertEquals(meta.text, "main:detached@0123456789ab");
});

Deno.test("Worker sidebar Workdir label marks an untrusted selector as a detached hash", () => {
  const meta = sidebarWorkdirMeta([
    attachment("checkout", {
      creation_selector: "develop",
      current_selector: "fedcba9876543210fedcba9876543210fedcba98",
    }),
  ]);

  assertEquals(meta.text, "main:detached@fedcba987654");
});

Deno.test("Worker sidebar Workdir label has an explicit unavailable-projection fallback", () => {
  const meta = sidebarWorkdirMeta([{
    alias: "checkout",
    repository_key: "main",
    working_directory_id: "workdir-1234567890abcdef",
  }]);

  assertEquals(meta.text, "main:unknown@workdir-1234");
});

Deno.test("Worker sidebar Workdir metadata preserves long names for truncated display details", () => {
  const repository = "repository-with-a-name-that-exceeds-the-sidebar-width";
  const branch =
    "feature/workdir-label-with-a-name-that-also-exceeds-the-sidebar-width";
  const meta = sidebarWorkdirMeta([
    attachment("long-checkout", {
      repository_key: repository,
      current_selector: branch,
    }),
  ]);

  assertEquals(meta.text, `${repository}:${branch}`);
  assert(
    meta.details.includes(`long-checkout — ${repository}:${branch}`),
    "full repository and branch should remain available in details when the primary line is truncated",
  );
});

Deno.test("Worker sidebar Workdir metadata keeps multiple attachments distinguishable", () => {
  const meta = sidebarWorkdirMeta([
    attachment("frontend", { current_selector: "develop" }),
    attachment("backend", {
      working_directory_id: "workdir-fedcba0987654321",
      repository_key: "server",
      current_selector: "release",
    }),
  ]);

  assertEquals(meta.text, "main:develop · server:release");
  assert(
    meta.details.includes("frontend — main:develop"),
    "first alias should be in details",
  );
  assert(
    meta.details.includes("backend — server:release"),
    "second alias should be in details",
  );
});

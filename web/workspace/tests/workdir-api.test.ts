declare const Deno: {
  test(name: string, fn: () => void | Promise<void>): void;
};

import {
  parseWorkingDirectoryCreateResponse,
  parseWorkingDirectoryListResponse,
  validateWorkingDirectoryCreateRequest,
} from "../src/lib/workspace/api/workdirs.ts";

const summary = {
  working_directory_id: "workdir-1",
  repository_key: "main",
  materializer_kind: "runtime_git_cache",
  status: "active",
  occupied_by: {
    runtime_id: "arcadia",
    worker_id: "worker-1",
    display_name: "Coder",
    linked_at: "2026-01-01T00:00:00Z",
  },
};

Deno.test("Workdir REST validation accepts the generated list and create contracts", () => {
  const list = parseWorkingDirectoryListResponse({
    workspace_id: "workspace-a",
    items: [summary],
    diagnostics: [],
  });
  if (list.items[0]?.occupied_by?.runtime_id !== "arcadia") {
    throw new Error("occupancy subject was not preserved");
  }

  const created = parseWorkingDirectoryCreateResponse({
    workspace_id: "workspace-a",
    runtime_id: "arcadia",
    item: summary,
    diagnostics: [],
  });
  if (created.runtime_id !== "arcadia") {
    throw new Error("create Runtime was not preserved");
  }
});

Deno.test("Workdir REST validation rejects stale response JSON", () => {
  let rejected = false;
  try {
    parseWorkingDirectoryListResponse({
      workspace_id: "workspace-a",
      items: [summary],
      diagnostics: [],
      source: "legacy-runtime",
    });
  } catch {
    rejected = true;
  }
  if (!rejected) throw new Error("stale response field was accepted");
});

Deno.test("Workdir REST validation enforces create operation fields", () => {
  const request = validateWorkingDirectoryCreateRequest({
    runtime_id: "arcadia",
    repository_key: "main",
    selector: "develop",
    operation_id: "operation-1",
  });
  if (request.operation_id !== "operation-1") {
    throw new Error("operation id was not preserved");
  }

  for (
    const invalid of [
      { runtime_id: "arcadia", operation_id: "operation-1" },
      { repository_key: "main", operation_key: "operation-1" },
    ]
  ) {
    let rejected = false;
    try {
      validateWorkingDirectoryCreateRequest(invalid);
    } catch {
      rejected = true;
    }
    if (!rejected) {
      throw new Error(
        `invalid request was accepted: ${JSON.stringify(invalid)}`,
      );
    }
  }
});

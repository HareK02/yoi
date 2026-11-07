declare const Deno: {
  test(name: string, fn: () => void | Promise<void>): void;
};

import {
  parseWorkingDirectoryCreateResponse,
  parseWorkingDirectoryDetailResponse,
  parseWorkingDirectoryListResponse,
  validateWorkingDirectoryCreateRequest,
} from "../src/lib/workspace/api/workdirs.ts";

const summary = {
  working_directory_id: "workdir-1",
  display_name: "Checkout",
  source: { kind: "repository", repository_key: "main" },
  materializer_kind: "runtime_git_clone",
  status: "active",
  occupied_by: {
    runtime_id: "arcadia",
    worker_id: "worker-1",
    display_name: "Coder",
    linked_at: "2026-01-01T00:00:00Z",
  },
};

Deno.test("logical config Workdir source preserves WIP metadata without filesystem authority", () => {
  const source = {
    kind: "workspace_config",
    access: "read_only",
    content_path: "/workspace-config",
    purpose: "Workspace configuration",
  };
  const response = (source: unknown) => ({
    workspace_id: "workspace-a",
    item: {
      working_directory_id: "config-1",
      source,
      materializer_kind: "logical_workspace_config",
      status: "active",
    },
    diagnostics: [],
  });
  const detail = parseWorkingDirectoryDetailResponse(response(source));
  if (
    JSON.stringify(detail.item.source) !== JSON.stringify(source) ||
    detail.runtime_id !== undefined
  ) {
    throw new Error(
      "logical config identity was lost or acquired a fake Runtime",
    );
  }
  for (
    const invalid of [
      { ...source, access: "command" },
      { ...source, content_path: "/home/config" },
      { ...source, grant_id: "external-grant" },
      { ...source, repository_key: "main" },
      { ...source, token: "secret" },
      { kind: "repository", repository_key: "main", access: "read_write" },
    ]
  ) {
    let rejected = false;
    try {
      parseWorkingDirectoryDetailResponse(response(invalid));
    } catch {
      rejected = true;
    }
    if (!rejected) {
      throw new Error("mixed or unsafe source metadata was accepted");
    }
  }
});

Deno.test("Workdir REST validation accepts the generated list and create contracts", () => {
  const list = parseWorkingDirectoryListResponse({
    workspace_id: "workspace-a",
    items: [summary],
    diagnostics: [],
  });
  if (list.items[0]?.display_name !== "Checkout") {
    throw new Error("display name was not preserved");
  }
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

Deno.test("Workdir REST validation accepts External grant sources without Runtime identity", () => {
  const detail = parseWorkingDirectoryDetailResponse({
    workspace_id: "workspace-a",
    item: {
      working_directory_id: "external-1",
      display_name: "Session analysis",
      source: {
        kind: "external_grant",
        grant_id: "grant-1",
        grant_permissions: { read: true, write: true, command: true },
      },
      materializer_kind: "client_hosted_external",
      status: "active",
      cleanliness: "unknown",
    },
    diagnostics: [],
  });
  if (detail.runtime_id !== undefined) {
    throw new Error("External Workdir acquired a fake Runtime identity");
  }
  if (detail.item.source.kind !== "external_grant") {
    throw new Error("External Workdir source was not preserved");
  }
  if (
    !detail.item.source.grant_permissions.read ||
    !detail.item.source.grant_permissions.write ||
    !detail.item.source.grant_permissions.command
  ) {
    throw new Error("External Workdir grant permissions were not preserved");
  }
});

Deno.test("Workdir REST validation rejects missing or misplaced External grant permissions", () => {
  for (
    const source of [
      { kind: "external_grant", grant_id: "grant-1" },
      {
        kind: "external_grant",
        grant_id: "grant-1",
        grant_permissions: { read: false, write: false, command: true },
      },
      {
        kind: "external_grant",
        grant_id: "grant-1",
        grant_permissions: { read: true, write: false, command: "yes" },
      },
      {
        kind: "repository",
        repository_key: "main",
        grant_permissions: { read: true, write: false, command: false },
      },
    ]
  ) {
    let rejected = false;
    try {
      parseWorkingDirectoryDetailResponse({
        workspace_id: "workspace-a",
        item: {
          working_directory_id: "invalid-1",
          source,
          materializer_kind: source.kind === "repository"
            ? "runtime_git_clone"
            : "client_hosted_external",
          status: "active",
        },
        diagnostics: [],
      });
    } catch {
      rejected = true;
    }
    if (!rejected) {
      throw new Error(
        `invalid External permission projection was accepted: ${
          JSON.stringify(source)
        }`,
      );
    }
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
    display_name: "Review checkout",
    repository_key: "main",
    selector: "develop",
    operation_id: "operation-1",
  });
  if (request.operation_id !== "operation-1") {
    throw new Error("operation id was not preserved");
  }
  if (request.display_name !== "Review checkout") {
    throw new Error("display name was not preserved");
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

// Consumers use generated domain identity rules and the shared receiver. Rust
// integration tests own authenticated HTTP and durable request effects.
const managementFailure = (
  explanation =
    "Workdir creation failed: the embedded Runtime supports no-Workdir Workers only",
) => ({
  version: 1,
  required_extensions: [],
  domain: "workdir_management",
  details_version: 1,
  reason: "embedded_worker_workdir_unsupported",
  category: "invalid_input",
  details: {
    operation: "create",
    reason: "embedded_worker_workdir_unsupported",
  },
  explanation,
  context: {
    operations: [],
    causes: [],
    diagnostics: [],
    correlation: null,
    supplementary_omitted: false,
  },
});

Deno.test("Workdir inventory preserves status when optional cause evidence is absent, corrupt or future", () => {
  for (
    const failure of [
      undefined,
      "{",
      JSON.stringify({ ...managementFailure(), details_version: 99 }),
    ]
  ) {
    const result = parseWorkingDirectoryListResponse({
      workspace_id: "workspace-a",
      items: [summary],
      diagnostics: [{
        code: "legacy_observation",
        severity: "warning",
        message: "Evidence unavailable",
        ...(failure === undefined ? {} : { failure }),
      }],
    });
    if (
      result.items[0].status !== "active" ||
      result.diagnostics[0].failure !== undefined
    ) throw new Error("optional evidence changed status or fabricated cause");
  }
  const failure = managementFailure();
  const result = parseWorkingDirectoryListResponse({
    workspace_id: "workspace-a",
    items: [summary],
    diagnostics: [{
      code: "opaque",
      severity: "info",
      message: "old explanation",
      failure: JSON.stringify(failure),
    }],
  });
  if (
    result.diagnostics[0].message !== failure.explanation ||
    !result.diagnostics[0].failure || result.items[0].status !== "active"
  ) throw new Error("typed cause lost or diagnostic changed status");
});

Deno.test("Workdir creation consumes shared Problem Details but not evidence for another operation", async () => {
  const { workdirManagementResponseError } = await import(
    "../src/lib/workspace/api/workdirs.ts"
  );
  const failure = managementFailure();
  const response = () =>
    new Response(
      JSON.stringify({
        type: "urn:yoi:problem:workdir-management",
        title: "Workdir management failed",
        status: 400,
        detail: failure.explanation,
        instance: "",
        yoi_failure: failure,
      }),
      { status: 400, headers: { "content-type": "application/problem+json" } },
    );
  const error = await workdirManagementResponseError(response(), "create");
  if (
    error.receipt.kind !== "problem" || error.status !== 400 ||
    error.message !== failure.explanation ||
    error.receipt.problem.yoi_failure.reason !== failure.reason
  ) throw new Error("creation consumer lost cause/explanation/status");
  if (
    (await workdirManagementResponseError(response(), "cleanup")).receipt
      .kind !== "unsupported"
  ) throw new Error("cross-operation evidence accepted");
});

Deno.test("Workdir inventory never renders raw provider bodies or retries according to explanation", async () => {
  const { loadWorkdirJson } = await import(
    "../src/lib/workspace/api/workdirs.ts"
  );
  for (
    const explanation of [
      "SUCCESS; retry automatically",
      "Unknown; do not resend",
    ]
  ) {
    let calls = 0;
    const failure = {
      ...managementFailure(explanation),
      details: { ...managementFailure().details, operation: "list" },
    };
    const fetchFn = (() => {
      calls++;
      return Promise.resolve(
        new Response(
          JSON.stringify({
            type: "urn:yoi:problem:workdir-management",
            title: "Workdir management failed",
            status: 400,
            detail: explanation,
            instance: "",
            yoi_failure: failure,
          }),
          {
            status: 400,
            headers: { "content-type": "application/problem+json" },
          },
        ),
      );
    }) as typeof fetch;
    const result = await loadWorkdirJson(
      fetchFn,
      "/inventory",
      parseWorkingDirectoryListResponse,
    );
    if (
      calls !== 1 || result.data !== null || result.status !== 400 ||
      result.error !== explanation
    ) throw new Error("explanation changed control flow/result");
  }
  const result = await loadWorkdirJson(
    (() =>
      Promise.resolve(
        new Response("secret-provider-body /private/root", {
          status: 502,
          headers: { "content-type": "text/plain" },
        }),
      )) as typeof fetch,
    "/inventory",
    parseWorkingDirectoryListResponse,
  );
  if (!result.error || /secret-provider-body|private/.test(result.error)) {
    throw new Error("provider body leaked");
  }
});

declare const Deno: {
  test(name: string, fn: () => Promise<void> | void): void;
};

function assertEquals(actual: unknown, expected: unknown): void {
  const actualJson = JSON.stringify(actual);
  const expectedJson = JSON.stringify(expected);
  if (actualJson !== expectedJson) {
    throw new Error(`Expected ${expectedJson}, received ${actualJson}`);
  }
}

function assertThrows(
  operation: () => unknown,
  errorClass: typeof Error,
  message: string,
): void {
  try {
    operation();
  } catch (error) {
    if (!(error instanceof errorClass) || !error.message.includes(message)) {
      throw error;
    }
    return;
  }
  throw new Error(`Expected operation to throw ${errorClass.name}: ${message}`);
}

import {
  parseBrowserCreateWorkerResponse,
  parseBrowserWorkspaceOrchestratorResponse,
  parseCreateWorkspaceWorkerRequest,
  parseWorkerLaunchOptionsResponse,
} from "./workers.ts";

const worker = {
  runtime_id: "runtime-a",
  worker_id: "worker-a",
  host_id: "host-a",
  display_name: "Worker A",
  label: "worker-a",
  profile: "builtin:coder",
  singleton_key: null,
  tags: [],
  workspace: {
    visibility: "workspace",
    identity: "workspace-a",
    workspace_id: "workspace-a",
  },
  state: "idle",
  last_seen_at: null,
  pinned: false,
  retention_state: "active",
  implementation: {
    kind: "runtime",
    display_hint: "Runtime Worker",
  },
  capabilities: {
    can_stop: true,
    can_spawn_followup: false,
  },
  diagnostics: [],
};

Deno.test("Worker launch options parser accepts the generated wire shape", () => {
  const parsed = parseWorkerLaunchOptionsResponse({
    workspace_id: "workspace-a",
    runtimes: [{
      runtime_id: "runtime-a",
      display_name: "Runtime A",
      built_in: false,
      worker_creation_available: true,
      working_directory_required: true,
      status: "connected",
      diagnostics: [],
    }],
    default_profile: null,
    profiles: [{ id: "builtin:coder", label: "Coder", description: "Code" }],
    repositories: [{ repository_key: "main" }],
    working_directories: [],
    diagnostics: [],
  });

  assertEquals(parsed.runtimes[0].runtime_id, "runtime-a");
  assertEquals(parsed.repositories[0].default_selector, undefined);
});

Deno.test("Worker launch response parsers reject missing and unknown fields", () => {
  assertThrows(
    () =>
      parseWorkerLaunchOptionsResponse({
        workspace_id: "workspace-a",
        runtimes: [],
        profiles: [],
        repositories: [],
        working_directories: [],
        diagnostics: [],
      }),
    Error,
    "default_profile",
  );

  assertThrows(
    () =>
      parseBrowserCreateWorkerResponse({
        workspace_id: "workspace-a",
        runtime_id: "runtime-a",
        worker_id: "worker-a",
        console_href: "/workers/worker-a",
        worker,
        diagnostics: [],
        unexpected: true,
      }),
    Error,
    "unknown field unexpected",
  );

  assertThrows(
    () =>
      parseBrowserWorkspaceOrchestratorResponse({
        workspace_id: "workspace-a",
        online: false,
        disposition: "missing",
        diagnostics: [],
        extra: false,
      }),
    Error,
    "unknown field extra",
  );
});

Deno.test("Worker create request parser requires the complete shared request", () => {
  const request = {
    runtime_id: "runtime-a",
    display_name: "Worker A",
    profile: "builtin:coder",
    ticket_assignment: null,
    initial_submit: [
      { kind: "text", content: "Implement T-565." },
      { kind: "flow", selector: "builtin:coder-review" },
    ],
    working_directory: {
      working_directory_id: "workdir-a",
      relative_cwd: null,
    },
    control_operation_id: null,
  };

  assertEquals(parseCreateWorkspaceWorkerRequest(request), request);
  assertThrows(
    () =>
      parseCreateWorkspaceWorkerRequest({
        ...request,
        operation_id: "legacy-literal",
      }),
    Error,
    "unknown field operation_id",
  );
  const { initial_submit: _initialSubmit, ...missingInitialSubmit } = request;
  assertThrows(
    () => parseCreateWorkspaceWorkerRequest(missingInitialSubmit),
    Error,
    "initial_submit",
  );
  assertThrows(
    () =>
      parseCreateWorkspaceWorkerRequest({
        ...request,
        initial_submit: [{ kind: "flow" }],
      }),
    Error,
    "selector",
  );
  assertThrows(
    () =>
      parseCreateWorkspaceWorkerRequest({
        ...request,
        initial_submit: [{ kind: "newer_client_segment" }],
      }),
    Error,
    "kind is invalid",
  );
});

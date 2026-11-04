// @vitest-environment happy-dom

import { cleanup, fireEvent, render, waitFor } from "@testing-library/svelte";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import ConfigSourceEditor from "./ConfigSourceEditor.svelte";
import type { WorkspaceConfigTreeResponse } from "./types.ts";

const toolchain = vi.hoisted(() => ({
  construct: vi.fn(),
  setSnapshot: vi.fn(),
  analyze: vi.fn(),
  complete: vi.fn(),
  close: vi.fn(),
  applyChanges: vi.fn(),
  changesBetween: vi.fn(),
  format: vi.fn(),
}));
vi.mock("./toolchain.ts", () => ({
  ConfigSourceToolchain: class {
    constructor() {
      toolchain.construct();
    }
    setSnapshot = toolchain.setSnapshot;
    analyze = toolchain.analyze;
    complete = toolchain.complete;
    close = toolchain.close;
    applyChanges = toolchain.applyChanges;
    changesBetween = toolchain.changesBetween;
    format = toolchain.format;
  },
}));

const schemaSource = "schema body\n".repeat(6000);
const tree: WorkspaceConfigTreeResponse = {
  snapshot: {
    digest: "sha256:tree",
    entries: {
      "main.dcdl": {
        path: "main.dcdl",
        content_type: "decodal",
        content: "WorkspaceConfigSchema({})\n",
        content_digest: "sha256:main",
      },
    },
  },
  contract: {
    contract_version: 1,
    decodal_version: "0.4.0",
    schema_version: 1,
    entrypoints: ["main.dcdl"],
    import_policy_version: 1,
    schema_bundle: {
      contributions: [{
        provider_id: "builtin",
        namespace: "workspace",
        version: "1",
        source: schemaSource,
        source_digest: "sha256:contribution",
      }],
      source: schemaSource,
      fingerprint: "sha256:schema",
    },
    fingerprint: "sha256:toolchain",
  },
  projection_digest: "sha256:projection",
};
const fetcher = vi.fn();
const sourceResponse = (state = tree) => ({
  state,
  history_head: "head:observed",
  source_stored: true,
  catalog: { workspace: "w", namespaces: [] },
  catalog_digest: "sha256:catalog",
  expected_legacy_export: "sha256:legacy",
  policies: [],
  migration_diagnostics: [],
});
const response = () => new Response(JSON.stringify(sourceResponse()));
const jsonResponse = (body: unknown) => new Response(JSON.stringify(body));
const previewResponse = (state = tree, expected_head: string | null = "head:observed") => jsonResponse({
  expected_head, state, changed_paths: ["module.dcdl"], changed_subjects: [],
});
function receipt(operation_id: string, state = tree, history_head = "head:adopted") {
  return {
    workspace_id: "w", operation_id, history_head, state,
    source_stored: true, policy_committed: true, runtime_application: "next_current_policy_check",
  };
}

beforeEach(() => {
  vi.resetAllMocks();
  vi.stubGlobal("fetch", fetcher);
  fetcher.mockImplementation(async () => response());
  toolchain.setSnapshot.mockResolvedValue(undefined);
  toolchain.analyze.mockResolvedValue([]);
  toolchain.complete.mockResolvedValue(null);
});
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

function renderEditor(workspaceId = "w") {
  return render(ConfigSourceEditor, { workspaceId });
}

function expectNoEditor(view: ReturnType<typeof renderEditor>) {
  expect(view.queryByText("Select or create a source")).toBeNull();
  expect(view.queryByRole("button", { name: "Create local source" }))
    .toBeNull();
  expect(view.queryByRole("button", { name: "Commit" })).toBeNull();
  expect(view.container.querySelector(".cm-editor")).toBeNull();
}

async function expectReady(view: ReturnType<typeof renderEditor>) {
  await waitFor(() =>
    expect(view.getByRole("button", { name: "main.dcdl entrypoint" })).not
      .toBeNull()
  );
  expect(view.container.querySelector('.cm-content[contenteditable="true"]'))
    .not.toBeNull();
  expect(view.container.textContent).toContain("WorkspaceConfigSchema({})");
  expect(view.queryByRole("alert")).toBeNull();
}

test("loading is not an empty source tree; editing waits for schema installation", async () => {
  let finishFetch!: (response: Response) => void;
  let finishSetup!: () => void;
  fetcher.mockReturnValueOnce(
    new Promise<Response>((resolve) => {
      finishFetch = resolve;
    }),
  );
  toolchain.setSnapshot.mockReturnValueOnce(
    new Promise<void>((resolve) => {
      finishSetup = resolve;
    }),
  );
  const view = render(ConfigSourceEditor, { workspaceId: "w/one" });
  expect(view.getByRole("status").textContent).toContain("Loading source tree");
  expectNoEditor(view);
  finishFetch(response());
  await waitFor(() =>
    expect(toolchain.setSnapshot).toHaveBeenCalledWith(
      tree.snapshot,
      tree.contract.schema_bundle,
    )
  );
  expectNoEditor(view);
  finishSetup();
  await expectReady(view);
  expect(fetcher).toHaveBeenCalledWith(
    "/api/w/w%2Fone/config/adoption",
    expect.anything(),
  );
});

test.each(["http", "invalid"])(
  "%s failure has a manual reload, not creation controls or automatic retries",
  async (failure) => {
    fetcher.mockResolvedValueOnce(
      failure === "http"
        ? new Response("Backend unavailable", { status: 503 })
        : new Response(JSON.stringify({ ...sourceResponse(), unexpected: true })),
    );
    const view = renderEditor();
    await waitFor(() =>
      expect(view.getByRole("alert").textContent).toContain(
        "Unable to load workspace configuration",
      )
    );
    expect(view.getByRole("alert").textContent).toContain(
      failure === "http" ? "Backend unavailable" : "invalid response",
    );
    expectNoEditor(view);
    expect(toolchain.setSnapshot).not.toHaveBeenCalled();
    await new Promise((resolve) => setTimeout(resolve, 300));
    expect(fetcher).toHaveBeenCalledOnce();
    await fireEvent.click(
      view.getByRole("button", { name: "Reload source tree" }),
    );
    await expectReady(view);
    expect(fetcher).toHaveBeenCalledTimes(2);
  },
);

test.each(["construct", "setSnapshot"] as const)(
  "toolchain %s failure is caught and manual reload initializes a fresh toolchain",
  async (phase) => {
    if (phase === "construct") {
      toolchain.construct.mockImplementationOnce(() => {
        throw new Error("Worker unavailable");
      });
    } else {toolchain.setSnapshot.mockRejectedValueOnce(
        new Error("Schema setup failed"),
      );}
    const view = renderEditor();
    await waitFor(() =>
      expect(view.getByRole("alert").textContent).toContain(
        phase === "construct" ? "Worker unavailable" : "Schema setup failed",
      )
    );
    expectNoEditor(view);
    await fireEvent.click(
      view.getByRole("button", { name: "Reload source tree" }),
    );
    await expectReady(view);
    expect(toolchain.construct).toHaveBeenCalledTimes(2);
  },
);

test("a successfully loaded empty tree still offers source creation", async () => {
  fetcher.mockResolvedValueOnce(
    new Response(JSON.stringify(sourceResponse({
      ...tree,
      snapshot: { ...tree.snapshot, entries: {} },
    }))),
  );
  const view = renderEditor();
  await waitFor(() =>
    expect(view.getByText("Select or create a source")).not.toBeNull()
  );
  expect(view.getByRole("button", { name: "Create local source" })).not
    .toBeNull();
  expect(view.queryByRole("alert")).toBeNull();
});

test("unmount during schema setup closes the toolchain", async () => {
  let failSetup!: (error: Error) => void;
  toolchain.setSnapshot.mockReturnValueOnce(
    new Promise<void>((_, reject) => {
      failSetup = reject;
    }),
  );
  toolchain.close.mockImplementationOnce(() =>
    failSetup(new Error("toolchain closed"))
  );
  const view = renderEditor();
  await waitFor(() => expect(toolchain.setSnapshot).toHaveBeenCalledOnce());
  view.unmount();
  await new Promise((resolve) => setTimeout(resolve, 0));
  expect(toolchain.close).toHaveBeenCalledOnce();
});

test("builtin diagnostics show read-only paths without adding editable workspace files", async () => {
  toolchain.analyze.mockResolvedValue([{
    path: "$builtin/profiles/companion.dcdl",
    tree_digest: "sha256:tree",
    kind: "constraint_violation",
    span: { start_byte: 1, end_byte: 4 },
    message: "Imported value does not match the requested schema",
    labels: [],
    notes: [],
  }]);
  const view = renderEditor();
  await expectReady(view);
  await waitFor(() => {
    const diagnostic = view.container.querySelector(".config-source-diagnostics");
    expect(diagnostic?.textContent).toContain("$builtin/profiles/companion.dcdl (read-only builtin source)");
    expect(diagnostic?.textContent).toContain("bytes 1–4");
  });
  const paths = view.getByRole("navigation", { name: "Virtual configuration paths" });
  expect(paths.textContent).not.toContain("$builtin/");
  expect(paths.querySelectorAll("button")).toHaveLength(1);
  expect(view.container.querySelector('.cm-content[contenteditable="true"]')).not.toBeNull();
});

test("unknown builtin diagnostics retain the workspace importer path", async () => {
  toolchain.analyze.mockResolvedValue([{
    path: "main.dcdl",
    tree_digest: "sha256:tree",
    kind: "import",
    span: { start_byte: 8, end_byte: 40 },
    message: "unknown or non-public read-only builtin source: $builtin/profiles/missing.dcdl",
    labels: [],
    notes: [],
  }]);
  const view = renderEditor();
  await expectReady(view);
  await waitFor(() => {
    const diagnostic = view.container.querySelector(".config-source-diagnostics");
    expect(diagnostic?.textContent).toContain("main.dcdl · bytes 8–40");
    expect(diagnostic?.textContent).toContain("unknown or non-public read-only builtin source");
  });
  expect(view.getByRole("navigation", { name: "Virtual configuration paths" }).textContent).not.toContain("$builtin/");
});

test.each(["resolve", "reject"] as const)(
  "superseded analysis %s cannot replace the current diagnostics or status",
  async (settlement) => {
    let resolveOld!: (value: unknown[]) => void;
    let rejectOld!: (error: Error) => void;
    const oldAnalysis = new Promise<unknown[]>((resolve, reject) => {
      resolveOld = resolve;
      rejectOld = reject;
    });
    toolchain.analyze.mockReturnValueOnce(oldAnalysis);
    const view = renderEditor();
    await expectReady(view);
    await waitFor(() => expect(toolchain.analyze).toHaveBeenCalledOnce());
    await fireEvent.click(view.getByRole("button", { name: "Create local source" }));
    await waitFor(() => expect(toolchain.analyze).toHaveBeenCalledTimes(2));
    if (settlement === "resolve") {
      resolveOld([{
        path: "main.dcdl", tree_digest: tree.snapshot.digest, kind: "import", span: { start_byte: 0, end_byte: 1 },
        message: "Stale diagnostic", labels: [], notes: [],
      }]);
    } else rejectOld(new Error("Stale analysis failure"));
    await oldAnalysis.catch(() => {});
    await waitFor(() => {
      expect(view.container.textContent).toContain("Creating local source module.dcdl");
      expect(view.container.textContent).not.toContain("Stale diagnostic");
      expect(view.container.textContent).not.toContain("Stale analysis failure");
    });
  },
);

test("unmount during fetch does not start a toolchain", async () => {
  let finishFetch!: (response: Response) => void;
  fetcher.mockReturnValueOnce(
    new Promise<Response>((resolve) => {
      finishFetch = resolve;
    }),
  );
  const view = renderEditor();
  view.unmount();
  finishFetch(response());
  await new Promise((resolve) => setTimeout(resolve, 0));
  expect(toolchain.construct).not.toHaveBeenCalled();
});


test("diagnostics from another tree digest are not displayed", async () => {
  toolchain.analyze.mockResolvedValue([
    {
      path: "main.dcdl", tree_digest: "sha256:old-tree", kind: "syntax",
      span: { start_byte: 0, end_byte: 1 }, message: "Wrong tree diagnostic",
      labels: [], notes: [],
    },
    {
      path: "main.dcdl", tree_digest: tree.snapshot.digest, kind: "syntax",
      span: { start_byte: 0, end_byte: 1 }, message: "Current tree diagnostic",
      labels: [], notes: [],
    },
  ]);
  const view = renderEditor();
  await expectReady(view);
  await waitFor(() => expect(view.container.textContent).toContain("Current tree diagnostic"));
  expect(view.container.textContent).not.toContain("Wrong tree diagnostic");
});

const change = { kind: "create", path: "module.dcdl", content_type: "decodal", content: "{}\n" } as const;
const candidate = {
  digest: "sha256:local-draft",
  entries: {
    ...tree.snapshot.entries,
    "module.dcdl": {
      path: change.path, content_type: change.content_type, content: change.content,
      content_digest: "sha256:module",
    },
  },
};
const candidateState = { ...tree, snapshot: candidate };

async function prepareDraft() {
  toolchain.applyChanges.mockResolvedValue(candidate);
  toolchain.changesBetween.mockResolvedValue([change]);
  toolchain.format.mockImplementation(async (source: string) => source);
  const view = renderEditor();
  await expectReady(view);
  await fireEvent.click(view.getByRole("button", { name: "Create local source" }));
  return view;
}

const requestAt = (index: number) => JSON.parse(fetcher.mock.calls[index][1].body);

function commitReceiptResponse(state = candidateState, historyHead = "head:adopted") {
  return async (_input: string, init: RequestInit) =>
    jsonResponse(receipt(JSON.parse(String(init.body)).operation_id, state, historyHead));
}

test.each([409, 422])("preview rejection %s preserves the draft and observed editing metadata", async (status) => {
  const view = await prepareDraft();
  const commit = view.getByRole("button", { name: "Commit" });
  fetcher.mockResolvedValueOnce(new Response(
    status === 409 ? "editing head mismatch" : "candidate evaluation failed", { status },
  ));
  await fireEvent.click(commit);
  await waitFor(() => expect(view.container.textContent).toContain(
    status === 409 ? "editing head mismatch" : "candidate evaluation failed",
  ));
  const first = requestAt(1);
  expect(first).toEqual({
    operation_id: expect.any(String), expected_head: "head:observed",
    expected_catalog_digest: "sha256:catalog", expected_legacy_export: "sha256:legacy",
    source: { base_digest: tree.snapshot.digest, changes: [change], entrypoints: ["main.dcdl"] },
  });
  expect(fetcher.mock.calls[1][0]).toBe("/api/w/w/config/adoption/preview");
  expect(view.container.querySelector(".cm-content")?.textContent).toContain("{}");
  expect(commit).toHaveProperty("disabled", false);
  expect(toolchain.setSnapshot).toHaveBeenCalledOnce();
  if (status === 409) expect(view.getByRole("alert").textContent).toContain("Discard local candidate");

  fetcher.mockResolvedValueOnce(previewResponse(candidateState));
  fetcher.mockImplementationOnce(commitReceiptResponse());
  await fireEvent.click(commit);
  await waitFor(() => expect(view.container.textContent).toContain("Committed formatted source tree"));
  const second = requestAt(2);
  expect(second).toEqual({ ...first, operation_id: expect.any(String) });
  expect(second.operation_id).not.toBe(first.operation_id);
  expect(requestAt(3)).toEqual(second);
  expect(toolchain.setSnapshot).toHaveBeenLastCalledWith(candidate, tree.contract.schema_bundle);
  expect(commit).toHaveProperty("disabled", true);
});

test("initial editable migration draft can be adopted without manually changing a file", async () => {
  fetcher.mockResolvedValueOnce(jsonResponse({
    ...sourceResponse(), history_head: null, source_stored: false,
  }));
  toolchain.changesBetween.mockResolvedValue([]);
  const view = renderEditor();
  await expectReady(view);
  const commit = view.getByRole("button", { name: "Commit" });
  expect(commit).toHaveProperty("disabled", false);
  fetcher.mockResolvedValueOnce(previewResponse(tree, null));
  fetcher.mockImplementationOnce(commitReceiptResponse(tree));
  await fireEvent.click(commit);
  await waitFor(() => expect(view.container.textContent).toContain("Committed formatted source tree"));
  expect(requestAt(1).expected_head).toBeNull();
  expect(requestAt(1).source.changes).toEqual([]);
  expect(requestAt(2)).toEqual(requestAt(1));
  expect(commit).toHaveProperty("disabled", true);
});

test("commit rejection does not refresh the head or discard local source", async () => {
  const view = await prepareDraft();
  fetcher.mockResolvedValueOnce(previewResponse(candidateState));
  fetcher.mockResolvedValueOnce(new Response("editing head mismatch", { status: 409 }));
  await fireEvent.click(view.getByRole("button", { name: "Commit" }));
  await waitFor(() => expect(view.getByRole("alert").textContent).toContain("Discard local candidate"));
  expect(fetcher).toHaveBeenCalledTimes(3);
  expect(view.queryByRole("button", { name: "Reconcile adoption" })).toBeNull();
  expect(view.container.querySelector(".cm-content")?.textContent).toContain("{}");
  fetcher.mockResolvedValueOnce(previewResponse(candidateState));
  fetcher.mockImplementationOnce(commitReceiptResponse());
  await fireEvent.click(view.getByRole("button", { name: "Commit" }));
  await waitFor(() => expect(view.container.textContent).toContain("Committed formatted source tree"));
  expect(requestAt(3).expected_head).toBe(requestAt(1).expected_head);
  expect(requestAt(3).operation_id).not.toBe(requestAt(1).operation_id);
});

test.each(["not_observed", "pending_transfer"])("lost commit followed by %s retains the exact request for explicit resume", async (status) => {
  const view = await prepareDraft();
  const unknown = status === "not_observed" ? { status } : {
    status, transfer_identity: "transfer:one", export_digest: "sha256:legacy",
  };
  fetcher.mockResolvedValueOnce(previewResponse(candidateState));
  fetcher.mockRejectedValueOnce(new TypeError("response lost"));
  fetcher.mockResolvedValueOnce(jsonResponse(unknown));
  await fireEvent.click(view.getByRole("button", { name: "Commit" }));
  await waitFor(() => expect(view.container.textContent).toContain(
    status === "not_observed" ? "not observed does not mean failed" : "transfer pending",
  ));
  expect(fetcher).toHaveBeenCalledTimes(4);
  const request = requestAt(1);
  expect(requestAt(2)).toEqual(request);
  expect(requestAt(3)).toEqual(request);
  expect(fetcher.mock.calls[3][0]).toBe("/api/w/w/config/adoption/reconcile");
  expect(toolchain.setSnapshot).toHaveBeenCalledOnce();
  expect(view.container.querySelector(".cm-content")?.textContent).toContain("{}");

  fetcher.mockResolvedValueOnce(jsonResponse(unknown));
  fetcher.mockImplementationOnce(commitReceiptResponse());
  await fireEvent.click(view.getByRole("button", { name: "Commit" }));
  await waitFor(() => expect(view.container.textContent).toContain("Committed formatted source tree"));
  expect(fetcher).toHaveBeenCalledTimes(6);
  expect(requestAt(4)).toEqual(request);
  expect(requestAt(5)).toEqual(request);
  expect(view.queryByRole("button", { name: "Reconcile adoption" })).toBeNull();
});

test("adopted reconciliation updates head and clears the legacy export for the next explicit adoption", async () => {
  const view = await prepareDraft();
  fetcher.mockResolvedValueOnce(previewResponse(candidateState));
  fetcher.mockResolvedValueOnce(new Response("invalid receipt"));
  fetcher.mockImplementationOnce(async (_input: string, init: RequestInit) => jsonResponse({
    status: "adopted", receipt: receipt(JSON.parse(String(init.body)).operation_id, candidateState),
  }));
  await fireEvent.click(view.getByRole("button", { name: "Commit" }));
  await waitFor(() => expect(view.container.textContent).toContain("Committed formatted source tree"));
  const first = requestAt(2);
  expect(requestAt(3)).toEqual(first);
  // A local deletion is another explicit adoption, not a retry of the lost reply.
  const deletion = { kind: "delete", path: "module.dcdl", expected_digest: "sha256:module" };
  toolchain.applyChanges.mockResolvedValue(tree.snapshot);
  toolchain.changesBetween.mockResolvedValue([deletion]);
  await fireEvent.click(view.getByRole("button", { name: "Delete" }));
  fetcher.mockResolvedValueOnce(previewResponse(tree, "head:adopted"));
  fetcher.mockImplementationOnce(commitReceiptResponse(tree, "head:second"));
  await fireEvent.click(view.getByRole("button", { name: "Commit" }));
  await waitFor(() => expect(fetcher).toHaveBeenCalledTimes(6));
  const second = requestAt(4);
  expect(second.expected_head).toBe("head:adopted");
  expect(second.expected_legacy_export).toBeNull();
  expect(second.expected_catalog_digest).toBe(first.expected_catalog_digest);
  expect(second.operation_id).not.toBe(first.operation_id);
  expect(second.source.base_digest).toBe(candidate.digest);
});

test("edited draft requires reconciliation and cannot replace or resume an unknown request", async () => {
  const view = await prepareDraft();
  fetcher.mockResolvedValueOnce(previewResponse(candidateState));
  fetcher.mockRejectedValueOnce(new TypeError("response lost"));
  fetcher.mockResolvedValueOnce(jsonResponse({ status: "not_observed" }));
  await fireEvent.click(view.getByRole("button", { name: "Commit" }));
  await waitFor(() => expect(view.container.textContent).toContain("not observed does not mean failed"));
  const original = requestAt(2);
  toolchain.format.mockResolvedValue("{ changed: true }\n");
  await fireEvent.click(view.getByRole("button", { name: "Format" }));
  await waitFor(() => expect(view.container.querySelector(".cm-content")?.textContent).toContain("changed"));
  fetcher.mockResolvedValueOnce(jsonResponse({ status: "not_observed" }));
  await fireEvent.click(view.getByRole("button", { name: "Commit" }));
  await waitFor(() => expect(view.container.textContent).toContain("Draft changed; reconcile the original request"));
  expect(fetcher).toHaveBeenCalledTimes(5);
  expect(requestAt(4)).toEqual(original);
  expect(toolchain.setSnapshot).toHaveBeenCalledOnce();

  fetcher.mockResolvedValueOnce(jsonResponse({ status: "adopted", receipt: receipt(original.operation_id, candidateState) }));
  await fireEvent.click(view.getByRole("button", { name: "Reconcile adoption" }));
  await waitFor(() => expect(view.container.textContent).toContain("Later local edits remain unsaved"));
  expect(view.container.querySelector(".cm-content")?.textContent).toContain("changed");
  expect(requestAt(5)).toEqual(original);
  expect(view.queryByRole("button", { name: "Reconcile adoption" })).toBeNull();
});

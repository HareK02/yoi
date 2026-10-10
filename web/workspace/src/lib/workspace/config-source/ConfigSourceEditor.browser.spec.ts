// @vitest-environment happy-dom

import { cleanup, fireEvent, render, waitFor } from "@testing-library/svelte";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import ConfigSourceEditor from "./ConfigSourceEditor.svelte";

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
const tree = {
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
const response = () => new Response(JSON.stringify(tree));

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
    "/api/w/w%2Fone/config/source-tree",
    expect.anything(),
  );
});

test.each(["http", "invalid"])(
  "%s failure has a manual reload, not creation controls or automatic retries",
  async (failure) => {
    fetcher.mockResolvedValueOnce(
      failure === "http"
        ? new Response("Backend unavailable", { status: 503 })
        : new Response(JSON.stringify({ ...tree, unexpected: true })),
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
    new Response(JSON.stringify({
      ...tree,
      snapshot: { ...tree.snapshot, entries: {} },
    })),
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

test.each([409, 422])("failed commit %s preserves the local draft and authoritative base digest for retry", async (status) => {
  const change = { kind: "create", path: "module.dcdl", content_type: "decodal", content: "{}\n" };
  const candidate = {
    digest: "sha256:local-draft",
    entries: {
      ...tree.snapshot.entries,
      "module.dcdl": {
        path: change.path,
        content_type: change.content_type,
        content: change.content,
        content_digest: "sha256:module",
      },
    },
  };
  toolchain.applyChanges.mockResolvedValue(candidate);
  toolchain.changesBetween.mockResolvedValue([change]);
  toolchain.format.mockImplementation(async (source: string) => source);
  const view = renderEditor();
  await expectReady(view);
  await fireEvent.click(view.getByRole("button", { name: "Create local source" }));
  const commit = view.getByRole("button", { name: "Commit" });
  expect(commit).toBeInstanceOf(HTMLButtonElement);
  fetcher.mockResolvedValueOnce(new Response(
    status === 409 ? "base content digest mismatch" : "candidate evaluation failed",
    { status },
  ));
  await fireEvent.click(commit);
  await waitFor(() => expect(view.container.textContent).toContain(
    status === 409 ? "base content digest mismatch" : "candidate evaluation failed",
  ));
  const expectedRequest = {
    base_digest: tree.snapshot.digest, changes: [change], entrypoints: ["main.dcdl"],
  };
  expect(JSON.parse(fetcher.mock.calls[1][1].body)).toEqual(expectedRequest);
  expect(view.container.querySelector(".cm-content")?.textContent).toContain("{}");
  expect(commit).toHaveProperty("disabled", false);
  expect(toolchain.setSnapshot).toHaveBeenCalledOnce();
  if (status === 409) expect(view.getByRole("alert").textContent).toContain("Discard local candidate");

  fetcher.mockResolvedValueOnce(new Response(JSON.stringify({ ...tree, snapshot: candidate })));
  await fireEvent.click(commit);
  await waitFor(() => expect(view.container.textContent).toContain("Committed formatted source tree"));
  expect(JSON.parse(fetcher.mock.calls[2][1].body)).toEqual(expectedRequest);
  expect(toolchain.setSnapshot).toHaveBeenLastCalledWith(candidate, tree.contract.schema_bundle);
  expect(commit).toHaveProperty("disabled", true);
});

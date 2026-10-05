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
  },
}));

const schemaSource = "schema body\n".repeat(6000);
const tree = {
  snapshot: {
    revision: 7,
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

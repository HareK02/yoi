declare const Deno: {
  test(name: string, fn: () => void | Promise<void>): void;
};

import { createWorkspaceProfileApi } from "../src/lib/workspace/settings/profile-api.ts";

Deno.test("workspace profile API delegates metadata calls to current route contract", async () => {
  const originalFetch = globalThis.fetch;
  const requests: Array<{ input: string; init?: RequestInit }> = [];
  globalThis.fetch = ((input: RequestInfo | URL, init?: RequestInit) => {
    requests.push({ input: String(input), init });
    return Promise.resolve(Response.json({
      workspace_id: "workspace-a",
      display_name: "Alpha",
      revision: "revision-2",
    }));
  }) as typeof fetch;

  try {
    const api = createWorkspaceProfileApi();
    await api.getMetadata("workspace-a");
    await api.updateMetadata("workspace-a", "Alpha updated", "revision-1");
  } finally {
    globalThis.fetch = originalFetch;
  }

  if (requests.length !== 2) throw new Error("expected two metadata requests");
  if (
    requests.some((request) => request.input.includes("/settings/metadata"))
  ) {
    throw new Error("obsolete metadata endpoint was used");
  }
  if (
    requests.some((request) => !request.input.endsWith("/settings/workspace"))
  ) {
    throw new Error("current Workspace settings endpoint was not used");
  }
  const updateBody = JSON.parse(String(requests[1].init?.body));
  if (
    updateBody.display_name !== "Alpha updated" ||
    updateBody.revision !== "revision-1" ||
    "expected_revision" in updateBody
  ) {
    throw new Error(`unexpected update payload: ${JSON.stringify(updateBody)}`);
  }
});

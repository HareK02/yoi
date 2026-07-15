import { workspaceApiPath, workspaceRoute } from "./http.ts";

declare const Deno: {
  test(name: string, fn: () => Promise<void> | void): void;
  readTextFile(path: string | URL): Promise<string>;
};

function assertEquals<T>(actual: T, expected: T): void {
  const actualJson = JSON.stringify(actual);
  const expectedJson = JSON.stringify(expected);
  if (actualJson !== expectedJson) {
    throw new Error(`Expected ${expectedJson}, got ${actualJson}`);
  }
}

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) {
    throw new Error(message);
  }
}

Deno.test("workspace route helpers scope browser routes and API by immutable workspace id", () => {
  assertEquals(workspaceRoute("workspace 1"), "/w/workspace%201");
  assertEquals(workspaceRoute("workspace 1", "/objectives"), "/w/workspace%201/objectives");
  assertEquals(
    workspaceApiPath("workspace 1", "/repositories/repo-a"),
    "/api/w/workspace%201/repositories/repo-a",
  );
});

Deno.test("unscoped layout bootstraps then redirects instead of loading unscoped workspace data", async () => {
  const layout = await Deno.readTextFile(new URL("./../../../routes/+layout.ts", import.meta.url));
  assert(
    layout.includes('loadJson<WorkspaceResponse>(fetch, "/api/workspace")'),
    "unscoped layout may use only the workspace-id bootstrap endpoint",
  );
  assert(
    layout.includes("throw redirect(307") && layout.includes("workspaceRoute("),
    "unscoped layout should redirect to the scoped workspace route",
  );
  assert(
    !layout.includes('`/api${path}`') && !layout.includes('"/api/repositories"'),
    "layout must not fall back to unscoped workspace-scoped API calls",
  );

  const unscopedSettings = await Deno.readTextFile(
    new URL("./../../../routes/settings/+page.svelte", import.meta.url),
  );
  assert(
    unscopedSettings.includes("Redirecting to scoped settings") &&
      !unscopedSettings.includes("fetch(") &&
      !unscopedSettings.includes("settingsApiPath"),
    "unscoped settings route should remain a thin redirect shim, not a data/control surface",
  );
});

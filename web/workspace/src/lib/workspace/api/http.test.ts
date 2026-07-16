import {
  loadWorkspaceSkillCatalog,
  workspaceApiPath,
  workspaceRoute,
  workspaceSkillActivationPath,
  workspaceSkillCatalogPath,
  workspaceSkillDetailPath,
} from "./http.ts";

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
  assertEquals(
    workspaceRoute("workspace 1", "/objectives"),
    "/w/workspace%201/objectives",
  );
  assertEquals(
    workspaceApiPath("workspace 1", "/repositories/repo-a"),
    "/api/w/workspace%201/repositories/repo-a",
  );
});

Deno.test("unscoped layout bootstraps then redirects instead of loading unscoped workspace data", async () => {
  const layout = await Deno.readTextFile(
    new URL("./../../../routes/+layout.ts", import.meta.url),
  );
  assert(
    layout.includes('loadJson<WorkspaceResponse>(fetch, "/api/workspace")'),
    "unscoped layout may use only the workspace-id bootstrap endpoint",
  );
  assert(
    layout.includes("throw redirect(307") && layout.includes("workspaceRoute("),
    "unscoped layout should redirect to the scoped workspace route",
  );
  assert(
    !layout.includes("`/api${path}`") &&
      !layout.includes('"/api/repositories"'),
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

Deno.test("Skill API paths use workspace backend scoped endpoints", () => {
  assertEquals(workspaceSkillCatalogPath("ws 1"), "/api/w/ws%201/skills");
  assertEquals(
    workspaceSkillDetailPath("ws 1", "triage-errors"),
    "/api/w/ws%201/skills/triage-errors",
  );
  assertEquals(
    workspaceSkillActivationPath("ws 1", "triage-errors"),
    "/api/w/ws%201/skills/triage-errors/activate",
  );
});

Deno.test("loadWorkspaceSkillCatalog fetches lightweight catalog", async () => {
  const result = await loadWorkspaceSkillCatalog(
    ((input: RequestInfo | URL) => {
      assertEquals(String(input), "/api/w/ws-1/skills");
      return Promise.resolve(
        new Response(
          JSON.stringify({
            authority: "workspace-backend-skills-v0",
            entries: [{
              name: "triage-errors",
              description: "Use when triaging errors.",
              provenance: { kind: "workspace", id: "workspace:triage-errors" },
              overrides: [],
              diagnostics: [],
            }],
            diagnostics: [],
          }),
          { status: 200 },
        ),
      );
    }) as typeof fetch,
    "ws-1",
  );

  assertEquals(result.error, null);
  assertEquals(result.data?.entries[0].name, "triage-errors");
  assertEquals(JSON.stringify(result.data).includes("SKILL.md body"), false);
});

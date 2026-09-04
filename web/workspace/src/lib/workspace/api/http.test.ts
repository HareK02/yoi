import {
  loadWorkspaceSkillCatalog,
  loadWorkspaceSkillDetail,
  workspaceApiPath,
  workspaceRoute,
  workspaceSkillActivationPath,
  workspaceSkillCatalogPath,
  workspaceSkillDetailPath,
} from "./http.ts";
import { SKILL_API_LIMITS } from "$lib/generated/skill-api.ts";

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

Deno.test("root layout leaves Workspace selection explicit", async () => {
  const layout = await Deno.readTextFile(
    new URL("./../../../routes/+layout.ts", import.meta.url),
  );
  assert(
    !layout.includes('"/api/workspace"') &&
      !layout.includes("redirect(") &&
      layout.includes("listWorkspaces(fetch)") &&
      layout.includes("accessibleWorkspaces"),
    "root layout may list accessible Workspaces but must not infer or redirect to a singleton Workspace",
  );
});

Deno.test("Workspace route changes dispose old multiplexed subscription state", async () => {
  const [layout, multiplexer] = await Promise.all([
    Deno.readTextFile(
      new URL(
        "./../../../routes/w/[workspaceId]/+layout.svelte",
        import.meta.url,
      ),
    ),
    Deno.readTextFile(new URL("./../multiplexer.ts", import.meta.url)),
  ]);
  assert(
    layout.includes("disposeWorkspaceMultiplexer(workspaceId)") &&
      multiplexer.includes("multiplexers.delete(workspaceId)") &&
      multiplexer.includes("this.#subscriptions.clear()") &&
      multiplexer.includes("this.#socket?.close()"),
    "changing Workspace must dispose old subscriptions and transport state",
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
            authority: "workspace-config-skills-v1",
            projection: {
              config_revision: 7,
              tree_digest: "tree-digest",
            },
            entries: [{
              name: "triage-errors",
              description: "Use when triaging errors.",
              activation_status: "active",
              projection_status: "valid",
              provenance: {
                kind: "workspace",
                id: "workspace:triage-errors",
                virtual_path: "skills/triage-errors/SKILL.md",
                revision: 7,
                source_digest: "source-digest",
                tree_digest: "tree-digest",
              },
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

Deno.test("Skill loaders redact and bound non-success response diagnostics", async () => {
  const secret = "SENSITIVE-SKILL-BODY-CONTENT".repeat(300);
  const result = await loadWorkspaceSkillCatalog(
    (() =>
      Promise.resolve(new Response(secret, { status: 500 }))) as typeof fetch,
    "ws-1",
  );

  assertEquals(result.data, null);
  assertEquals(result.error, "Skill API request failed with HTTP 500");
  assert(
    !result.error?.includes(secret.slice(0, 64)),
    "Skill API diagnostic must not expose response body content",
  );
  assert(
    (result.error?.length ?? 0) <= 256,
    "Skill API diagnostic must remain bounded",
  );
});

Deno.test("Skill loaders stop reading success responses above the wire byte limit", async () => {
  const oversized = `{"body":"${
    "x".repeat(SKILL_API_LIMITS.maxResponseBytes + 1)
  }"}`;
  const result = await loadWorkspaceSkillDetail(
    (() =>
      Promise.resolve(
        new Response(oversized, { status: 200 }),
      )) as typeof fetch,
    "ws-1",
    "release",
  );

  assertEquals(result.data, null);
  assertEquals(result.error, "Skill API response exceeds its byte limit");
  assert(
    !result.error?.includes(oversized.slice(0, 64)),
    "Skill API diagnostic must not expose oversized response content",
  );
});

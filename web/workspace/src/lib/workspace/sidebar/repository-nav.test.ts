import { projectRepositoryNav } from "./repository-nav.ts";
import type { RepositoryListResponse } from "./types.ts";

declare const Deno: {
  test(name: string, fn: () => void): void;
};

function assertEquals<T>(actual: T, expected: T): void {
  const actualJson = JSON.stringify(actual);
  const expectedJson = JSON.stringify(expected);
  if (actualJson !== expectedJson) {
    throw new Error(`Expected ${expectedJson}, got ${actualJson}`);
  }
}

function repositories(
  items: RepositoryListResponse["items"],
): RepositoryListResponse {
  return {
    workspace_id: "workspace-1",
    items,
    source: "workspace_backend_config",
    diagnostics: [],
  };
}

Deno.test("repository nav does not invent main for an empty registry", () => {
  const projection = projectRepositoryNav(
    {
      workspace_id: "workspace-1",
      items: [],
      source: "workspace_backend_config",
      diagnostics: [
        {
          code: "repository_config_empty",
          severity: "warning",
          message: "No repositories configured",
        },
      ],
    },
    "/w/workspace-1",
    "workspace-1",
  );

  assertEquals(projection.count, 0);
  assertEquals(projection.items, []);
  assertEquals(projection.diagnostics[0].code, "repository_config_empty");
});

Deno.test("repository nav links configured non-main repository ids", () => {
  const projection = projectRepositoryNav(
    repositories([
      {
        id: "infra",
        display_name: "Infrastructure",
        kind: "git",
        provider: "git",
        record_authority: "workspace-backend-config",
        git: null,
        diagnostics: [],
      },
    ]),
    "/w/workspace-1/repositories/infra",
    "workspace-1",
  );

  assertEquals(projection.count, 1);
  assertEquals(projection.items[0], {
    id: "infra",
    title: "Infrastructure",
    href: "/w/workspace-1/repositories/infra",
    meta: "git repository · read-only",
    active: true,
  });
});

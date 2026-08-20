declare const Deno: {
  test(name: string, fn: () => void | Promise<void>): void;
};

function assertEquals(actual: unknown, expected: unknown): void {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      `expected ${JSON.stringify(expected)}, received ${
        JSON.stringify(actual)
      }`,
    );
  }
}

async function assertRejects(
  operation: () => Promise<unknown>,
  errorType: typeof WorkspaceCatalogError,
): Promise<void> {
  try {
    await operation();
  } catch (error) {
    if (error instanceof errorType) return;
    throw error;
  }
  throw new Error("expected operation to reject");
}

import {
  createWorkspace,
  loadWorkspaceCatalog,
  WorkspaceCatalogError,
} from "../src/lib/workspace/api/workspace-catalog.ts";

Deno.test("workspace catalog enriches each visible workspace without dropping siblings", async () => {
  const fetcher = (input: string | URL | Request) => {
    const url = String(input);
    if (url.startsWith("/api/workspaces")) {
      return Promise.resolve(Response.json([
        {
          workspace_id: "w-a",
          owner_account_id: null,
          display_name: "Alpha",
          state: "active",
          created_at: "1",
          updated_at: "2",
        },
        {
          workspace_id: "w-b",
          owner_account_id: null,
          display_name: "Beta",
          state: "active",
          created_at: "1",
          updated_at: "3",
        },
      ]));
    }
    if (url.includes("w-a")) {
      return Promise.resolve(Response.json([{
        workspace_id: "w-a",
        repository_id: "main",
        name: "Main",
        kind: "local_path",
        uri: "/srv/alpha",
        default_ref: "develop",
      }]));
    }
    return Promise.resolve(new Response("unavailable", { status: 503 }));
  };

  const items = await loadWorkspaceCatalog(fetcher as typeof fetch);
  assertEquals(items.length, 2);
  assertEquals(items[0].repositories[0].repository_id, "main");
  assertEquals(items[1].repositories, []);
  assertEquals(typeof items[1].repository_error, "string");
});

Deno.test("workspace creation preserves caller-owned operation key across retry", async () => {
  const bodies: unknown[] = [];
  const request = {
    operation_key: "web-create-1",
    display_name: "Alpha",
    repository: {
      uri: "/srv/alpha",
      display_name: "Main",
      default_ref: "develop",
    },
  };
  const fetcher = (_input: string | URL | Request, init?: RequestInit) => {
    bodies.push(JSON.parse(String(init?.body)));
    return Promise.resolve(
      new Response(JSON.stringify({ message: "retry" }), {
        status: 503,
        headers: { "content-type": "application/json" },
      }),
    );
  };

  await assertRejects(
    () => createWorkspace(fetcher as typeof fetch, request),
    WorkspaceCatalogError,
  );
  await assertRejects(
    () => createWorkspace(fetcher as typeof fetch, request),
    WorkspaceCatalogError,
  );
  assertEquals(bodies, [request, request]);
});

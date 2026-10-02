declare const Deno: {
  test(name: string, fn: () => void | Promise<void>): void;
  readTextFile(path: URL): Promise<string>;
};

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

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
          owner_account_id: "account-1",
          display_name: "Alpha",
          state: "active",
          created_at: "1",
          updated_at: "2",
        },
        {
          workspace_id: "w-b",
          owner_account_id: "account-1",
          display_name: "Beta",
          state: "active",
          created_at: "1",
          updated_at: "3",
        },
      ]));
    }
    if (url.includes("w-a")) {
      return Promise.resolve(Response.json({
        workspace_id: "w-a",
        items: [{
          repository_key: "main",
          kind: "git",
          provider: "git",
          source: { kind: "local_path", uri: "/srv/alpha" },
          source_revision: 1,
          source_fingerprint: "sha256:alpha",
          observed_status: "ready",
          default_selector: "develop",
          record_authority: "workspace-control-plane",
        }],
        source: "workspace-control-plane",
        diagnostics: [],
      }));
    }
    return Promise.resolve(new Response("unavailable", { status: 503 }));
  };

  const items = await loadWorkspaceCatalog(fetcher as typeof fetch);
  assertEquals(items.length, 2);
  assertEquals(items[0].repositories[0].repository_key, "main");
  assertEquals(items[1].repositories, []);
  assertEquals(typeof items[1].repository_error, "string");
});

Deno.test("workspace creation preserves caller-owned operation key across retry", async () => {
  const bodies: unknown[] = [];
  const request = {
    operation_key: "web-create-1",
    display_name: "Alpha",
    repository: {
      repository_key: "platform",
      uri: "/srv/alpha",
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

Deno.test("workspace creation form starts with an explicit required Repository key", async () => {
  const source = await Deno.readTextFile(
    new URL("../src/routes/+page.svelte", import.meta.url),
  );

  assert(
    source.includes('let repositoryKey = $state("");'),
    "the form must start without an implicit Repository key",
  );
  assert(
    source.includes("<input bind:value={repositoryKey} required"),
    "the Browser must require creator input for the Repository key",
  );
  assert(
    source.includes("repository_key: normalized.repositoryKey"),
    "the Browser must submit the creator-provided Repository key",
  );
  assert(
    !source.includes('repositoryKey || "main"') &&
      !source.includes("repositoryKey || 'main'"),
    "the Browser must not fall back to main",
  );
});

import { error, redirect } from "@sveltejs/kit";
import { deferPage, resolvePageData, settle } from "./deferred.ts";

declare const Deno: {
  test(name: string, fn: () => void | Promise<void>): void;
};
function assert(condition: unknown): asserts condition {
  if (!condition) throw new Error("Assertion failed");
}
function gate<T>() {
  let release!: (value: T) => void;
  return {
    promise: new Promise<T>((resolve) => {
      release = resolve;
    }),
    release: (value: T) => release(value),
  };
}

Deno.test("deferred load returns before API completion and snapshots navigation dependencies", async () => {
  const api = gate<string>();
  const event = {
    params: { id: "first" },
    parent: async () => ({}),
    url: new URL("https://example.test/first?q=old"),
  };
  const load = deferPage(async ({ params, url }: typeof event) => ({
    value: await api.promise,
    id: params.id,
    query: url.searchParams.get("q"),
  }));
  const response = load(event);
  assert(!(response instanceof Promise));
  event.params.id = "second";
  event.url.searchParams.set("q", "new");
  api.release("complete");
  const result = await response.deferredPage;
  assert(
    result.ok && result.data.id === "first" && result.data.query === "old" &&
      result.data.value === "complete",
  );
});

Deno.test("deferred redirects and HTTP failures are handled results, not unhandled rejections", async () => {
  for (
    const [run, status, location] of [
      [() => redirect(308, "/canonical"), 308, "/canonical"],
      [() => error(403, "Permission denied"), 403, null],
      [
        () => {
          throw new Error("Failed read");
        },
        null,
        null,
      ],
    ] as const
  ) {
    const load = deferPage((
      _event: { params: {}; url: URL; parent: () => Promise<unknown> },
    ) => run());
    const result = await load({
      params: {},
      parent: async () => ({}),
      url: new URL("https://example.test/"),
    }).deferredPage;
    assert(
      !result.ok && result.failure.status === status &&
        result.failure.redirect === location,
    );
  }
});

Deno.test("page content cannot override a failed Workspace permission lookup", async () => {
  const result = await resolvePageData({
    deferredWorkspace: settle(Promise.reject(new Error("Workspace denied"))),
    deferredPage: settle({ title: "Sensitive content" }),
  });
  assert(!result.ok && result.failure.message === "Workspace denied");
});

Deno.test("resolved data preserves page overrides without waiting for the global catalog", async () => {
  const catalog = gate<never>();
  const result = await resolvePageData({
    deferredWorkspace: settle({
      workspace: { name: "team" },
      repositories: { items: [] },
    }),
    deferredCatalog: catalog.promise,
    workspaceId: "team",
    deferredPage: settle({
      repositories: { data: null, error: "Unavailable" },
      ticket: "T-1",
    }),
  });
  assert(
    result.ok && result.data.repositories.error === "Unavailable" &&
      result.data.workspace.name === "team" && result.data.ticket === "T-1" &&
      result.data.workspaceId === "team",
  );
});

Deno.test("private deferred loader starts only after parent admission and never on parent failure", async () => {
  for (const outcome of ["admit", "anonymous", "expired", "error"] as const) {
    const admission = gate<void>();
    let calls = 0;
    const load = deferPage(
      async (
        _event: { params: {}; url: URL; parent: () => Promise<unknown> },
      ) => {
        ++calls;
        return {};
      },
    );
    const result = load({
      params: {},
      url: new URL("https://app.invalid/w/a/tickets"),
      parent: async () => {
        await admission.promise;
        if (outcome === "anonymous") redirect(307, "/");
        if (outcome === "expired") {
          redirect(307, "/login?returnTo=%2Fw%2Fa%2Ftickets");
        }
        if (outcome === "error") {
          error(503, "Unknown session");
        }
        return {};
      },
    });
    await Promise.resolve();
    assert(calls === 0);
    admission.release();
    const settled = await result.deferredPage;
    assert(calls === (outcome === "admit" ? 1 : 0));
    assert(settled.ok === (outcome === "admit"));
  }
});

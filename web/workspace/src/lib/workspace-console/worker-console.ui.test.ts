declare const Deno: {
  test(name: string, fn: () => Promise<void> | void): void;
  readTextFile(path: string | URL): Promise<string>;
};

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) {
    throw new Error(message);
  }
}

Deno.test("workspace Worker list and sidebar attach through Worker Console hrefs", async () => {
  const workspacePage = await Deno.readTextFile(
    new URL("./../../routes/+page.svelte", import.meta.url),
  );
  const workersNav = await Deno.readTextFile(
    new URL("../workspace-sidebar/WorkersNavSection.svelte", import.meta.url),
  );
  const sidebar = await Deno.readTextFile(
    new URL("../workspace-sidebar/WorkspaceSidebar.svelte", import.meta.url),
  );

  assert(
    workspacePage.includes("workerConsoleHref(worker, workspaceId)") &&
      workspacePage.includes("Open Console"),
    "top Worker list should expose an attach action per Worker",
  );
  assert(
    workersNav.includes("workerConsoleHref(worker, workspaceId)") &&
      workersNav.includes("aria-current"),
    "Workers sidebar rows should link to the Worker target Console route",
  );
  assert(
    !sidebar.includes("CompanionNavSection") &&
      sidebar.includes("WorkersNavSection"),
    "standalone Companion/Console navigation should not remain canonical",
  );
});

Deno.test("Worker Console page is routed by runtime_id and worker_id through backend APIs", async () => {
  const consolePage = await Deno.readTextFile(
    new URL(
      "./../../routes/w/[workspaceId]/runtimes/[runtimeId]/workers/[workerId]/console/+page.svelte",
      import.meta.url,
    ),
  );
  const routeLoad = await Deno.readTextFile(
    new URL(
      "./../../routes/w/[workspaceId]/runtimes/[runtimeId]/workers/[workerId]/console/+page.ts",
      import.meta.url,
    ),
  );

  assert(
    routeLoad.includes("workspaceId") &&
      routeLoad.includes("runtimeId") && routeLoad.includes("workerId"),
    "route load should expose workspace and target ids",
  );
  assert(
    consolePage.includes("workspaceApiPath(workspaceId, path)") &&
      consolePage.includes(
        "workerApiPath(`/runtimes/${encodeURIComponent(target.runtimeId)}/workers/${encodeURIComponent(target.workerId)}`)",
      ),
    "Worker detail should use the scoped backend Worker detail API",
  );
  assert(
    consolePage.includes("/transcript?limit=200") &&
      consolePage.includes("/events/ws") && consolePage.includes("/input"),
    "Console should use bounded transcript, observation WS, and input APIs",
  );
  assert(
    !consolePage.includes("/api/companion"),
    "Console page must not use Companion-specific APIs",
  );
  assert(
    consolePage.includes("function advanceReloadToken()") &&
      consolePage.includes("nextReloadToken += 1") &&
      !consolePage.includes("reloadToken += 1"),
    "reload token advancement should not synchronously read and write the rune state",
  );
  assert(
    consolePage.includes(
      "advanceReloadToken();\n    void loadConsoleData(target);",
    ) &&
      !consolePage.includes("void refreshConsole();\n  });\n\n  $effect"),
    "target-change effect should load data without depending on manual refresh state reads",
  );
});

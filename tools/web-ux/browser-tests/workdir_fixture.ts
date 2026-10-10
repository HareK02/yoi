// Synthetic API boundary only; the production route and shell own all rendering.
export function workdirFixture() {
  let mountReleased = false;
  const deleted = new Set<string>();
  const requests: {
    workspace: string;
    method: string;
    path: string;
    body?: unknown;
  }[] = [];
  const summary = (
    id: string,
    status = "active",
    cleanliness = "clean",
    occupied = false,
  ) => ({
    working_directory_id: id,
    display_name: id === "retry-dir" ? "Cleanup retry — distributed development checkout" : id,
    source: { kind: "repository", repository_key: "main" },
    materializer_kind: "runtime_git_clone",
    status,
    cleanliness,
    current_ref: "a".repeat(40),
    current_selector: "develop",
    occupied_by: occupied
      ? {
        runtime_id: "fixture-runtime",
        worker_id: "worker-1",
        display_name: "Active Worker",
        linked_at: "2026-01-01T00:00:00Z",
      }
      : null,
  });
  const items = (workspace: string) =>
    workspace === "home-empty" ? [] : [
      ...(deleted.has(workspace) ? [] : [summary("retry-dir", "cleanup_pending", "unknown")]),
      summary("dirty-dir", "active", "dirty"),
      summary("occupied-dir", "active", "clean", true),
    ];
  const plan = (workspace: string) => ({
    workspace_id: workspace,
    runtime_id: "fixture-runtime",
    generated_at: "2026-01-01T00:00:00Z",
    digest: "digest-1",
    workers: [],
    diagnostics: [],
    workdirs: items(workspace).map((item) => ({
      target_id: `workdir:${item.working_directory_id}`,
      action: item.cleanliness === "dirty" ? "workdir_dirty_discard" : "workdir_clean_cleanup",
      workdir_id: item.working_directory_id,
      runtime_id: "fixture-runtime",
      repository_key: "main",
      reason: "Ordinary cleanup rechecks current conditions",
      linked_worker_ids: item.occupied_by ? ["worker-1"] : [],
      linked_running_worker_ids: item.occupied_by ? ["worker-1"] : [],
      running_linked: Boolean(item.occupied_by),
      pinned_linked: false,
      file_status: item.status === "cleanup_pending" ? "pending" : "present",
      cleanliness: item.cleanliness,
      blocking_reason: item.occupied_by ? "Workdir is linked to a running Worker" : null,
    })),
  });
  return async (request: Request): Promise<Response | null> => {
    const url = new URL(request.url);
    if (
      url.pathname === "/fixture-workdirs-control" && request.method === "POST"
    ) {
      const control = await request.json();
      mountReleased = control.mountReleased === true;
      if (control.reset) {
        deleted.clear();
        requests.length = 0;
      }
      return Response.json({ ok: true });
    }
    if (url.pathname === "/fixture-workdirs") {
      return Response.json({ requests });
    }
    const match = url.pathname.match(/^\/api\/w\/([^/]+)(\/runtimes.*)$/);
    if (!match) return null;
    const [, workspace, path] = match;
    if (path === "/runtimes") {
      return Response.json({
        workspace_id: workspace,
        limit: 100,
        source: "fixture",
        diagnostics: [],
        items: [],
      });
    }
    if (!path.startsWith("/runtimes/fixture-runtime/")) return null;
    const body = request.method !== "GET" ? await request.json() : undefined;
    requests.push({ workspace, method: request.method, path, body });
    if (path.endsWith("/working-directories")) {
      return Response.json({
        workspace_id: workspace,
        items: items(workspace),
        diagnostics: [],
      });
    }
    if (path.endsWith("/cleanup-plan")) return Response.json(plan(workspace));
    // Baseline route uses cleanup-executions; same retained outcome for before captures.
    if (path.endsWith("/cleanup-executions")) {
      return Response.json({
        workspace_id: workspace,
        runtime_id: "fixture-runtime",
        executed_at: "2026-01-01T00:00:00Z",
        diagnostics: [],
        plan_after: plan(workspace),
        results: [{
          target_id: "workdir:retry-dir",
          action: "workdir_clean_cleanup",
          status: "retained",
          message: "Provider cleanup failed",
        }],
      });
    }
    if (
      path.endsWith("/working-directories/retry-dir") &&
      request.method === "DELETE"
    ) {
      if (workspace === "home-error") {
        return Response.json({
          message: "Synthetic private host path must not render",
        }, { status: 503 });
      }
      if (mountReleased) deleted.add(workspace);
      return Response.json({
        working_directory_id: "retry-dir",
        disposition: mountReleased ? "removed" : "attention_required",
        retryable: !mountReleased,
        failure_category: mountReleased ? null : "mount_present",
      });
    }
    return Response.json({ error: "Unexpected fixture Workdir request" }, {
      status: 404,
    });
  };
}

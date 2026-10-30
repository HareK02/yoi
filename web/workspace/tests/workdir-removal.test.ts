declare const Deno: {
  test(name: string, fn: () => void | Promise<void>): void;
};
import { parseWorkingDirectoryRemovalResponse } from "../src/lib/workspace/api/workdirs.ts";
import {
  canRemoveWorkdir,
  removalCause,
  removeWorkdir,
  WorkdirRemovalRequests,
} from "../src/lib/workspace/settings/workdir-removal.ts";
import type {
  CleanupWorkdirCandidate,
  WorkingDirectorySummary,
} from "../src/lib/workspace/sidebar/types.ts";

function assert(value: unknown, message = "assertion failed"): asserts value {
  if (!value) throw new Error(message);
}

Deno.test("removal wire boundary rejects malformed results and mismatched identities", async () => {
  for (
    const extra of [
      { retryable: "true" },
      { disposition: "forgotten" },
      { failure_category: {} },
      { failure_category: "x".repeat(129) },
      { working_directory_id: "x".repeat(129) },
      { unexpected: true },
    ]
  ) {
    let rejected = false;
    try {
      parseWorkingDirectoryRemovalResponse({
        working_directory_id: "dir",
        disposition: "retained",
        retryable: true,
        ...extra,
      });
    } catch {
      rejected = true;
    }
    assert(
      rejected,
      `accepted malformed removal result: ${JSON.stringify(extra)}`,
    );
  }
  let rejected = false;
  try {
    await removeWorkdir(
      (() =>
        Promise.resolve(
          Response.json({
            working_directory_id: "other",
            disposition: "removed",
            retryable: false,
          }),
        )) as typeof fetch,
      "workspace",
      "runtime",
      "dir",
      "request-1",
    );
  } catch {
    rejected = true;
  }
  assert(rejected, "accepted removal evidence for a different Workdir");
});

Deno.test("removal response size is bounded before decoding provider content", async () => {
  let rejected = false;
  try {
    await removeWorkdir(
      (() => Promise.resolve(new Response("x".repeat(4097)))) as typeof fetch,
      "workspace",
      "runtime",
      "dir",
      "request-1",
    );
  } catch {
    rejected = true;
  }
  assert(rejected, "accepted an oversized removal response");
});

Deno.test("removal causes give bounded actions without echoing untrusted provider details", () => {
  const categories = [
    "mount_present",
    "mount_check_unavailable",
    "permission_denied",
    "resource_busy",
    "storage_unavailable",
    "ownership_unknown",
    "changes_present",
    "changes_unknown",
    "provider_cleanup_failed",
    "provider_observation_unknown",
    "provider_cleanup_outcome_unknown",
    "authority_invalid",
    "provider_identity_changed",
    "runtime_unavailable",
    "provider_unavailable",
    "unsupported_target",
    "blocked_by_live_authority",
    "dirty_or_unknown",
    "authority_changed",
  ];
  for (const category of categories) {
    const text = removalCause(category);
    assert(text !== removalCause(null), `missing category ${category}`);
    assert(
      text.length < 400 && !text.includes("/"),
      `unbounded or pathful category ${category}`,
    );
  }
  for (
    const untrusted of [
      "/private/host/path",
      "token=synthetic-secret",
      "toString",
      "__proto__",
      "future_category",
      "x".repeat(10000),
    ]
  ) {
    const text = removalCause(untrusted);
    assert(
      text === removalCause(null) && !text.includes(untrusted),
      "echoed unknown category",
    );
  }
});

Deno.test("pending unknown cleanup permits ordinary removal while current dirty and live guards remain protected", () => {
  const workdir = {
    working_directory_id: "dir",
    status: "cleanup_pending",
    cleanliness: "unknown",
    source: { kind: "repository", repository_key: "main" },
    materializer_kind: "runtime_git_clone",
  } as WorkingDirectorySummary;
  const candidate = {
    action: "workdir_clean_cleanup",
    blocking_reason: null,
    running_linked: false,
    pinned_linked: false,
  } as CleanupWorkdirCandidate;
  assert(canRemoveWorkdir(workdir, candidate));
  for (
    const guard of [
      { action: "workdir_dirty_discard" },
      { blocking_reason: "protected" },
      { running_linked: true },
      { pinned_linked: true },
    ]
  ) {
    assert(
      !canRemoveWorkdir(
        workdir,
        { ...candidate, ...guard } as CleanupWorkdirCandidate,
      ),
    );
  }
  assert(!canRemoveWorkdir({ ...workdir, cleanliness: "dirty" }, candidate));
  assert(
    canRemoveWorkdir(workdir, {
      ...candidate,
      action: "workdir_record_delete",
    }),
  );
  assert(!canRemoveWorkdir(workdir, { ...candidate, action: "worker_delete" }));
  assert(!canRemoveWorkdir(workdir, undefined));
  assert(
    !canRemoveWorkdir({
      ...workdir,
      occupied_by: {
        worker_id: "worker",
        runtime_id: "runtime",
        display_name: "Worker",
        linked_at: "now",
      },
    }, candidate),
  );
});

Deno.test("new requests after confirmed failure get distinct IDs and expose only allowlisted HTTP causes", async () => {
  const requests: { path: string; body: string; method: string }[] = [];
  const fetchFn = (async (path: RequestInfo | URL, init?: RequestInit) => {
    requests.push({
      path: String(path),
      body: String(init?.body),
      method: String(init?.method),
    });
    return Response.json({
      working_directory_id: "dir/x",
      disposition: "attention_required",
      retryable: true,
      failure_category: "mount_present",
    });
  }) as typeof fetch;
  const removalRequests = new WorkdirRemovalRequests();
  for (let i = 0; i < 2; i++) {
    const result = await removalRequests.remove(
      fetchFn,
      "workspace/x",
      "runtime/x",
      "dir/x",
    );
    assert(result.retryable && result.failure_category === "mount_present");
  }
  assert(
    JSON.parse(requests[0].body).operation_id !==
      JSON.parse(requests[1].body).operation_id,
    "new explicit request reused a failed request identity",
  );
  assert(
    requests[0].path ===
      "/api/w/workspace%2Fx/runtimes/runtime%2Fx/working-directories/dir%2Fx",
  );
  assert(
    requests[0].method === "DELETE" &&
      JSON.parse(requests[0].body).reason ===
        "User requested Workdir deletion" &&
      typeof JSON.parse(requests[0].body).operation_id === "string",
  );
  for (
    const [status, category] of [
      [403, "permission_denied"],
      [409, "authority_changed"],
      [503, "provider_unavailable"],
      [500, null],
    ] as const
  ) {
    let message = "";
    try {
      await removeWorkdir(
        (() =>
          Promise.resolve(
            Response.json({ message: "/private/path token=synthetic-secret" }, {
              status,
            }),
          )) as typeof fetch,
        "workspace",
        "runtime",
        "dir",
        "request-1",
      );
    } catch (error) {
      message = error instanceof Error ? error.message : String(error);
    }
    assert(
      message === removalCause(category),
      `HTTP ${status} leaked provider text`,
    );
  }
});

Deno.test("unknown removal outcomes keep the same identity and body until a terminal result", async () => {
  for (const transportLoss of [false, true]) {
    const calls: string[] = [];
    const requester = new WorkdirRemovalRequests();
    const fetchFn = (async (_path: RequestInfo | URL, init?: RequestInit) => {
      calls.push(String(init?.body));
      if (calls.length === 1 && transportLoss) {
        throw new Error("connection lost");
      }
      return Response.json({
        working_directory_id: "dir",
        disposition: "attention_required",
        retryable: calls.length !== 1,
        failure_category: calls.length === 1
          ? "provider_cleanup_outcome_unknown"
          : "resource_busy",
      });
    }) as typeof fetch;
    await requester.remove(fetchFn, "workspace", "runtime", "dir").catch(() =>
      undefined
    );
    await requester.remove(fetchFn, "workspace", "runtime", "dir");
    await requester.remove(fetchFn, "workspace", "runtime", "dir");
    assert(calls[0] === calls[1], "unknown outcome changed request identity");
    assert(
      calls[1] !== calls[2],
      "confirmed failure kept old request identity",
    );
  }
});

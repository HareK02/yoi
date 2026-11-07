// 実loopback Serverへの限定接続。fixture資格情報はstdinのみ、応答をmockしない。
import {
  createWorkspaceRepository,
  loadWorkspaceRepositoryDetail,
  loadWorkspaceRepositoryLog,
} from "../../src/lib/workspace/api/repositories.ts";
import type { RepositoryApiResult } from "../../src/lib/workspace/api/repositories.ts";
import { receiveProblem } from "../../src/lib/workspace/failure/receive.ts";
import { decodeDomainFailureDetails } from "../../src/lib/workspace/failure/domains.ts";
function assert(value: unknown, message: string): asserts value {
  if (!value) throw Error(message);
}
function equal(actual: unknown, expected: unknown) {
  assert(
    JSON.stringify(actual) === JSON.stringify(expected),
    "typed Repository cause differs",
  );
}
const fixture: { origin: string; workspace_id: string; token: string } = JSON
  .parse(await new Response(Deno.stdin.readable).text());
const origin = new URL(fixture.origin);
assert(
  origin.protocol === "http:" && origin.hostname === "127.0.0.1",
  "loopback fixture required",
);
let posts = 0;
const fetchOwner: typeof fetch = (
  input: RequestInfo | URL,
  init?: RequestInit,
) => {
  const url = new URL(
    input instanceof Request ? input.url : String(input),
    origin,
  );
  assert(
    url.origin === origin.origin && url.pathname.startsWith("/api/"),
    "off-origin request refused",
  );
  if (init?.method === "POST") posts++;
  const headers = new Headers(init?.headers);
  headers.set("authorization", `Bearer ${fixture.token}`);
  return fetch(url, { ...init, headers, redirect: "error" });
};
const ws = fixture.workspace_id;
const unknown = await loadWorkspaceRepositoryDetail(
  fetchOwner,
  ws,
  "missing-canary",
);
function cause(
  result: RepositoryApiResult<unknown>,
  status: number,
  operation: string,
  reason: string,
) {
  assert(
    result.data === null && result.status === status &&
      result.receipt?.kind === "problem",
    "Repository API lost status or typed receipt",
  );
  const failure = result.receipt.problem.yoi_failure;
  equal(failure.details, { operation, reason });
  equal(failure.domain, "repository");
  assert(
    result.error !== null && !result.error.includes("canary") &&
      result.error.includes(failure.explanation),
    "safe concrete explanation lost",
  );
}
cause(unknown, 404, "summary_by_key", "unknown_repository");
for (
  const [key, reason, status] of [
    ["missing-canary", "unknown_repository", 404],
    ["files", "unsupported_provider", 400],
  ] as const
) {
  cause(
    await loadWorkspaceRepositoryLog(fetchOwner, ws, key),
    status,
    "recent_log",
    reason,
  );
  for (const prefix of ["/api", `/api/w/${encodeURIComponent(ws)}`]) {
    const response = await fetchOwner(`${prefix}/repositories/${key}/log`);
    equal(response.status, status);
    const receipt = await receiveProblem(response, decodeDomainFailureDetails);
    assert(
      receipt.kind === "problem",
      "real log response not accepted by shared Web receiver",
    );
    equal(receipt.problem.yoi_failure.details, {
      operation: "recent_log",
      reason,
    });
    assert(!receipt.problem.detail.includes("canary"), "private source leaked");
  }
}
const request = {
  repository_key: "web-registration",
  source: "/private/original-canary",
  default_ref: "main",
};
const created = await createWorkspaceRepository(fetchOwner, ws, request);
assert(
  created.data?.replayed === false && created.error === null,
  "registration did not commit",
);
const replay = await createWorkspaceRepository(fetchOwner, ws, request);
assert(replay.data?.replayed === true, "exact intent no longer replays");
const conflict = await createWorkspaceRepository(fetchOwner, ws, {
  ...request,
  source: "/private/different-canary",
});
cause(conflict, 409, "register", "registration_intent_conflict");
equal(posts, 3);
console.log(
  "Repository real HTTP/Web receiver preserves typed reasons, status and registration intent.",
);

import {
  decodeWorkerStartupProblem,
  submitWorkerLaunch,
  workerLaunchNeedsReconciliation,
  workerStartupNotice,
} from "./startup-failure.ts";
import { startupFixture } from "./startup-failure.test-fixtures.ts";
import { buildCreateWorkspaceWorkerRequest } from "../sidebar/worker-launch.ts";

declare const Deno: {
  test(name: string, run: () => void | Promise<void>): void;
};
function equal(actual: unknown, expected: unknown) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      `Expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`,
    );
  }
}
const decode = (body: unknown, status?: number) =>
  decodeWorkerStartupProblem(
    JSON.stringify(body),
    status ?? (body as { status: number }).status,
  );

Deno.test("exact T812 HTTP fixtures preserve cause and independent operation facts including backend persistence", () => {
  for (const name of ["denial", "lost"] as const) {
    const body = startupFixture(name);
    const receipt = decode(body);
    if (receipt.kind !== "problem") throw new Error(receipt.kind);
    equal(receipt.problem.yoi_failure.details, body.yoi_failure.details);
    equal(receipt.extension, body.yoi_startup);
    equal(workerStartupNotice(receipt).explanation, body.detail);
    // 実409でもServerがunknownと観測したacceptanceを拒否/未実行へ補完しない。
    equal(workerLaunchNeedsReconciliation(receipt), true);
  }
});
Deno.test("definite rejection, unknown and compensation remain independent of wording, status, severity and diagnostic order", () => {
  for (const name of ["denial", "lost", "rejected", "compensation"] as const) {
    for (
      const text of [
        "文面が変わりました。",
        "OutcomeUnknown",
        "No effects; retry immediately!",
      ]
    ) {
      const body = JSON.parse(JSON.stringify(startupFixture(name)));
      body.title = "変更されたtitle";
      body.detail = body.yoi_failure.explanation = text;
      body.future_extension = { raw_body: "secret /internal/path token" };
      body.yoi_failure.context.diagnostics = [{
        code: "safe",
        explanation: "Supplement",
      }, { code: "other", explanation: "Other supplement" }];
      const received = decode(body);
      equal(received.kind, "problem");
      equal(workerLaunchNeedsReconciliation(received), name !== "rejected");
      body.yoi_failure.context.diagnostics.reverse();
      equal(workerLaunchNeedsReconciliation(decode(body)), name !== "rejected");
      equal(
        JSON.stringify(workerStartupNotice(received)).includes("secret"),
        false,
      );
    }
  }
});
Deno.test("unknown type permits validated public explanation but absent/malformed/unknown mandatory results never unlock launch", () => {
  const body = JSON.parse(JSON.stringify(startupFixture("rejected")));
  body.type = "urn:future:problem";
  equal(workerLaunchNeedsReconciliation(decode(body)), false);
  for (
    const mutate of [
      (v: typeof body) => delete v.yoi_startup,
      (v: typeof body) => delete v.yoi_startup.operation.execution,
      (v: typeof body) => {
        v.yoi_startup.operation.acceptance = "future";
      },
      (v: typeof body) => {
        v.yoi_startup.version = 2;
      },
      (v: typeof body) => {
        v.yoi_failure.required_extensions = ["future"];
      },
      (v: typeof body) => {
        v.yoi_failure.details.principal = "future";
      },
      (v: typeof body) => {
        v.yoi_startup.secondary = Array(9).fill({
          kind: "cleanup",
          phase: "cleanup",
          failure: "feature",
        });
      },
      (v: typeof body) => {
        v.yoi_startup.operation.correlation = "/internal/secret";
      },
      (v: typeof body) => {
        v.yoi_failure.category = "internal";
      },
      (v: typeof body) => {
        v.status = 502;
      },
      (v: typeof body) => {
        v.yoi_failure.details.raw_body = "secret";
      },
    ]
  ) {
    const changed = structuredClone(body);
    mutate(changed);
    const receipt = decode(changed, 409);
    if (receipt.kind === "problem") {
      throw new Error("invalid/unsupported contract accepted");
    }
    equal(workerLaunchNeedsReconciliation(receipt), true);
    equal(
      JSON.stringify(workerStartupNotice(receipt)).includes("secret"),
      false,
    );
  }
});
Deno.test("Runtime's envelope reason is failed, not its closed detail reason; optional backend persistence stays independent", () => {
  const body = JSON.parse(JSON.stringify(startupFixture("rejected")));
  body.yoi_failure.domain = "worker_startup";
  body.yoi_failure.reason = "failed";
  body.yoi_failure.category = "internal";
  body.yoi_failure.details = { reason: "metadata_pending", stage: "metadata" };
  delete body.yoi_startup.backend_result_persistence;
  const receipt = decode(body);
  equal(receipt.kind, "problem");
  if (receipt.kind === "problem") {
    equal(receipt.extension?.backend_result_persistence, null);
  }
  body.yoi_failure.reason = "metadata_pending";
  equal(decode(body).kind, "invalid");
});
Deno.test("actual launch caller handles transport/parse/oversize/legacy failure safely and sends one POST only", async () => {
  const request = buildCreateWorkspaceWorkerRequest({
    runtime_id: "embedded",
    display_name: "Kept draft",
    profile: "",
    subjektiv_subject_id: "",
    initial_text: "Do not change",
    workdir_attachments: [],
    working_directory_repository_key: "",
    working_directory_selector: "HEAD",
  });
  for (
    const response of [
      () =>
        new Response("secret raw body", {
          status: 500,
          headers: { "content-type": "application/problem+json" },
        }),
      () =>
        new Response("x".repeat(32769), {
          status: 502,
          headers: { "content-type": "application/problem+json" },
        }),
      () =>
        Response.json({ error: "permission_denied", message: "raw secret" }, {
          status: 403,
        }),
      () => Response.json({ invalid: "success body" }),
      () => {
        throw new TypeError("secret transport");
      },
      () =>
        new Response(JSON.stringify(startupFixture("denial")), {
          status: 409,
          headers: { "content-type": "application/problem+json" },
        }),
    ]
  ) {
    let calls = 0;
    const result = await submitWorkerLaunch(
      ((_path: unknown, init: RequestInit) => {
        calls++;
        equal(init.method, "POST");
        equal(JSON.parse(String(init.body)), request);
        return Promise.resolve(response());
      }) as typeof fetch,
      "/api/w/test/workers",
      request,
    );
    equal(calls, 1);
    if (result.kind !== "failed") throw new Error("unexpected creation");
    equal(workerLaunchNeedsReconciliation(result.receipt), true);
    equal(
      JSON.stringify(workerStartupNotice(result.receipt)).includes("secret"),
      false,
    );
  }
});

Deno.test("persistence uncertainty does not become retry permission even alongside a rejected execution", () => {
  for (const field of ["persistence", "backend_result_persistence"]) {
    for (const outcome of ["not_saved", "conflict", "outcome_unknown"]) {
      const body = JSON.parse(JSON.stringify(startupFixture("rejected")));
      body.yoi_startup[field] = outcome;
      equal(workerLaunchNeedsReconciliation(decode(body)), true);
    }
  }
});

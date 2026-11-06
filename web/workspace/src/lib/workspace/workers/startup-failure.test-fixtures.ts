import denial from "../../../../../../crates/runtime-api/tests/fixtures/startup-browser-session-record-denied.problem.json" with {
  type: "json",
};
import responseLost from "../../../../../../crates/runtime-api/tests/fixtures/startup-browser-registration-response-lost.problem.json" with {
  type: "json",
};

// 最初の二件だけがT812の実HTTP捕捉。派生ケースはWeb受信/操作/描画試験用で、Serverの保証ではない。
export function startupFixture(
  name: "denial" | "lost" | "rejected" | "compensation",
) {
  const body = structuredClone(name === "lost" ? responseLost : denial);
  if (name === "rejected") {
    body.yoi_startup.operation.acceptance = "rejected";
    body.yoi_startup.operation.execution = "rejected";
    body.yoi_startup.operation.backend_compensation = "completed";
    body.yoi_startup.secondary = [];
    body.yoi_startup.resources = {
      resources: "none",
      cleanup: "not_needed",
      durable_effects_possible: false,
    };
  }
  if (name === "compensation") {
    return {
      ...body,
      status: 500,
      type: "urn:yoi:problem:startup-failed",
      detail:
        "Worker creation finalization failed; acceptance, execution and compensation are reported separately.",
      yoi_failure: {
        ...body.yoi_failure,
        domain: "startup_boundary",
        reason: "finalize_failed",
        category: "internal",
        details: { reason: "finalize_failed" },
        explanation:
          "Worker creation finalization failed; acceptance, execution and compensation are reported separately.",
      },
      yoi_startup: {
        ...body.yoi_startup,
        operation: {
          ...body.yoi_startup.operation,
          acceptance: "accepted",
          execution: "errored",
          backend_compensation: "residual",
          finalize: { stage: "publication", compensation: "residual" },
        },
        secondary: [{ kind: "cleanup", failure: "feature" }],
        resources: {
          resources: "worker_startup",
          cleanup: { failed: "feature" },
          durable_effects_possible: true,
        },
      },
    };
  }
  return body;
}

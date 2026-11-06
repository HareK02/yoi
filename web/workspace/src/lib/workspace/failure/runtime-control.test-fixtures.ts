import type { RestoreStartupEvidence } from "../api/runtime-workers.ts";

export function startupObservations(): RestoreStartupEvidence {
  return {
    primary: JSON.stringify({
      ...controlProblem().yoi_failure,
      domain: "session_registration",
      reason: "permission_denied",
      category: "authorization",
      details: {
        reason: "permission_denied",
        principal: "subject",
        required_operation: "session.record",
      },
      explanation: "Session registration permission denied.",
    }),
    secondary: [
      { kind: "projection_unavailable", phase: "metadata_save" },
      { kind: "cleanup", failure: "workdir" },
      {
        kind: "persistence",
        phase: "runtime_snapshot_save",
        outcome: "outcome_unknown",
      },
    ],
    resources: {
      resources: "worker_startup",
      cleanup: { failed: "workdir" },
      durable_effects_possible: true,
    },
    persistence: "outcome_unknown",
    io_kind: "permission_denied",
  };
}

export function controlProblem(
  reason = "execution_rejected",
  explanation = "The execution owner rejected the operation.",
  status = 409,
) {
  return {
    type: "urn:yoi:problem:runtime-control",
    instance: "",
    title: "Runtime control failed",
    status,
    detail: explanation,
    yoi_failure: {
      version: 1,
      required_extensions: [],
      domain: "runtime_control",
      reason,
      category: "conflict",
      details_version: 1,
      details: reason,
      explanation,
      context: {
        operations: [],
        causes: [],
        diagnostics: [],
        correlation: null,
        supplementary_omitted: false,
      },
    },
  };
}
export function controlProblemResponse(problem = controlProblem()) {
  return new Response(JSON.stringify(problem), {
    status: problem.status,
    headers: { "content-type": "application/problem+json" },
  });
}

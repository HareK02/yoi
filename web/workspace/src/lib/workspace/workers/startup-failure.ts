import type {
  Category,
  CompensationOutcome,
  CreationAcceptance,
  ExecutionOutcome,
  FinalizeStage,
  RuntimeStartupReason,
  SessionPrincipal,
  SessionRegistrationReason,
  StartupBoundaryReason,
  StartupCleanupFailure,
  StartupFailurePhase,
  StartupIoKind,
  StartupPersistenceOutcome,
  StartupProblemDetails,
  StartupProblemFacts,
  StartupStage,
} from "#lib/generated/worker-launch-api.ts";
import { readBoundedJson } from "#lib/workspace/api/http.ts";
import { parseBrowserCreateWorkerResponse } from "#lib/workspace/api/workers.ts";
import type { FailureEnvelope } from "#lib/generated/error-contract.ts";
import { failureNotice } from "#lib/workspace/failure/model.ts";
import type { FailureNoticeModel } from "#lib/workspace/failure/model.ts";
import {
  decodeProblem,
  failureIdentifier,
  failureRecord,
  receiveProblem,
} from "#lib/workspace/failure/receive.ts";
import type {
  DetailsDecode,
  FailureReceipt,
} from "#lib/workspace/failure/receive.ts";

type StartupCause = StartupProblemDetails["yoi_failure"]["details"];
export type StartupReceipt = FailureReceipt<StartupCause, StartupProblemFacts>;
class UnsupportedContract extends Error {}
const unsupported = (): never => {
  throw new UnsupportedContract();
};
const invalid = (): never => {
  throw new Error("Invalid startup contract");
};
function exact(value: Record<string, unknown>, keys: string[]) {
  if (Object.keys(value).some((key) => !keys.includes(key))) invalid();
}
function enumValue<T extends string>(value: unknown, values: readonly T[]): T {
  if (typeof value !== "string") return invalid();
  if (!values.includes(value as T)) return unsupported();
  return value as T;
}
function optional<T>(value: unknown, decode: (v: unknown) => T): T | null {
  return value == null ? null : decode(value);
}
const acceptance = [
  "rejected",
  "accepted",
  "unknown",
] as const satisfies readonly CreationAcceptance[];
const execution = [
  "accepted",
  "busy",
  "rejected",
  "errored",
  "unsupported",
  "unknown",
] as const satisfies readonly ExecutionOutcome[];
const compensation = [
  "completed",
  "residual",
  "unknown",
] as const satisfies readonly CompensationOutcome[];
const persistence = [
  "persisted",
  "not_saved",
  "conflict",
  "outcome_unknown",
] as const satisfies readonly StartupPersistenceOutcome[];
const cleanupFailure = [
  "ownership",
  "workdir",
  "artifacts",
  "allocation",
  "feature",
  "unsupported",
] as const satisfies readonly StartupCleanupFailure[];
const phase = [
  "cleanup",
  "runtime_record_delete",
  "runtime_snapshot_save",
  "runtime_removal_publish",
  "metadata_save",
  "runtime_reconciliation_record_save",
  "runtime_reconciliation_publish",
] as const satisfies readonly StartupFailurePhase[];
const finalizeStage = [
  "worker_identity",
  "workspace_api_binding",
  "create_reservation",
  "worker_registry",
  "workdir_attachment",
  "workdir_registry",
  "ticket_assignment_bind",
  "ticket_assignment_current",
  "ticket_state_accept",
  "session_registration",
  "initial_input",
  "publication",
] as const satisfies readonly FinalizeStage[];
const stage = [
  "preflight",
  "metadata",
  "reconstruction",
  "recovery",
  "binding",
  "feature",
  "activation_effects",
  "controller",
] as const satisfies readonly StartupStage[];
const ioKind = [
  "not_found",
  "permission_denied",
  "already_exists",
  "invalid_input",
  "invalid_data",
  "interrupted",
  "timed_out",
  "would_block",
  "unexpected_eof",
  "other",
] as const satisfies readonly StartupIoKind[];

// 公開schemaのclosed discriminator/fieldを検証するだけで、reasonの説明・秘匿・HTTP分類は生成しない。
export function decodeStartupCause(
  envelope: FailureEnvelope<unknown>,
): DetailsDecode<StartupCause> {
  try {
    if (envelope.details_version !== 1) return { kind: "unsupported" };
    const value = failureRecord(envelope.details);
    let category: Category;
    let expectedReason = value.reason;
    if (envelope.domain === "session_registration") {
      exact(value, ["reason", "principal", "required_operation"]);
      enumValue(
        value.reason,
        [
          "permission_denied",
          "attribution_missing",
          "binding_mismatch",
          "transport",
          "outcome_unknown",
          "invalid_response",
        ] as const satisfies readonly SessionRegistrationReason[],
      );
      enumValue(
        value.principal,
        [
          "operator",
          "subject",
          "unknown",
        ] as const satisfies readonly SessionPrincipal[],
      );
      if (value.required_operation !== "session.record") {
        return { kind: "unsupported" };
      }
      category = value.reason === "transport"
        ? "dependency"
        : value.reason === "outcome_unknown"
        ? "unknown"
        : value.reason === "invalid_response"
        ? "internal"
        : "authorization";
    } else if (envelope.domain === "worker_startup") {
      exact(value, ["reason", "stage"]);
      enumValue(
        value.reason,
        [
          "startup_failed",
          "segment_empty",
          "system_prompt_missing",
          "metadata_pending",
          "metadata_missing",
          "metadata_inactive",
          "manifest_missing",
          "allocation_failed",
          "session_read_failed",
          "metadata_read_failed",
          "provider_failed",
          "attribution_failed",
          "feature_failed",
        ] as const satisfies readonly RuntimeStartupReason[],
      );
      enumValue(value.stage, stage);
      expectedReason = "failed";
      category = "internal";
    } else if (envelope.domain === "startup_boundary") {
      exact(value, ["reason"]);
      enumValue(
        value.reason,
        [
          "timeout",
          "transport",
          "decode",
          "invalid_response",
          "too_large",
          "unsupported_version",
          "unsupported_reason",
          "finalize_failed",
        ] as const satisfies readonly StartupBoundaryReason[],
      );
      category = value.reason === "finalize_failed" ? "internal" : "dependency";
    } else return { kind: "unsupported" };
    if (expectedReason !== envelope.reason || category !== envelope.category) {
      return { kind: "invalid" };
    }
    return { kind: "known", details: value as StartupCause };
  } catch (error) {
    return {
      kind: error instanceof UnsupportedContract ? "unsupported" : "invalid",
    };
  }
}

export function decodeStartupFacts(
  root: Readonly<Record<string, unknown>>,
): DetailsDecode<StartupProblemFacts> {
  try {
    const facts = failureRecord(root.yoi_startup);
    exact(facts, [
      "version",
      "operation",
      "secondary",
      "resources",
      "persistence",
      "backend_result_persistence",
      "io_kind",
    ]);
    if (
      typeof facts.version !== "number" || !Number.isInteger(facts.version) ||
      facts.version < 1 || facts.version > 65535
    ) return invalid();
    if (facts.version !== 1) return { kind: "unsupported" };
    const operation = failureRecord(facts.operation);
    exact(operation, [
      "acceptance",
      "execution",
      "finalize",
      "backend_compensation",
      "correlation",
    ]);
    const parsedOperation = {
      acceptance: enumValue(operation.acceptance, acceptance),
      execution: enumValue(operation.execution, execution),
      finalize: optional(operation.finalize, (v) => {
        const finalize = failureRecord(v);
        exact(finalize, ["stage", "compensation"]);
        return {
          stage: enumValue(finalize.stage, finalizeStage),
          compensation: enumValue(finalize.compensation, compensation),
        };
      }),
      backend_compensation: optional(
        operation.backend_compensation,
        (v) => enumValue(v, compensation),
      ),
      correlation: optional(operation.correlation, (v) => {
        const reference = failureIdentifier(v);
        if (!/^[A-Za-z0-9._-]+$/u.test(reference)) return invalid();
        return reference;
      }),
    };
    if (!Array.isArray(facts.secondary) || facts.secondary.length > 8) {
      return invalid();
    }
    const secondary = facts.secondary.map((v) => {
      const item = failureRecord(v);
      if (item.kind !== "cleanup") enumValue(item.phase, phase);
      if (item.kind === "projection_unavailable") {
        exact(item, ["kind", "phase"]);
      } else if (item.kind === "cleanup") {
        exact(item, ["kind", "failure"]);
        enumValue(item.failure, cleanupFailure);
      } else if (item.kind === "persistence") {
        exact(item, ["kind", "phase", "outcome"]);
        enumValue(item.outcome, persistence);
      } else return unsupported();
      return item;
    });
    const resources = optional(facts.resources, (v) => {
      const item = failureRecord(v);
      exact(item, ["resources", "cleanup", "durable_effects_possible"]);
      const resources = enumValue(item.resources, [
        "none",
        "worker_startup",
        "unproven",
      ]);
      if (typeof item.durable_effects_possible !== "boolean") return invalid();
      if (typeof item.cleanup === "string") {
        enumValue(item.cleanup, ["not_needed", "required", "confirmed"]);
      } else {
        const failed = failureRecord(item.cleanup);
        exact(failed, ["failed"]);
        enumValue(failed.failed, cleanupFailure);
      }
      return { ...item, resources };
    });
    // castsは正本生成型のnominal bounds markerを満たす検証済み値に限定する。
    return {
      kind: "known",
      details: {
        version: 1,
        operation: parsedOperation,
        secondary,
        resources,
        persistence: optional(
          facts.persistence,
          (v) => enumValue(v, persistence),
        ),
        backend_result_persistence: optional(
          facts.backend_result_persistence,
          (v) => enumValue(v, persistence),
        ),
        io_kind: optional(facts.io_kind, (v) => enumValue(v, ioKind)),
      } as StartupProblemFacts,
    };
  } catch (error) {
    return {
      kind: error instanceof UnsupportedContract ? "unsupported" : "invalid",
    };
  }
}

export function decodeWorkerStartupProblem(
  text: string,
  status: number,
  contentType = "application/problem+json",
): StartupReceipt {
  return decodeProblem(
    text,
    status,
    contentType,
    decodeStartupCause,
    decodeStartupFacts,
  );
}
export function receiveWorkerStartupProblem(
  response: Response,
): Promise<StartupReceipt> {
  return receiveProblem(response, decodeStartupCause, decodeStartupFacts);
}

// UIの新規送信抑止。原因/HTTP/診断/cleanup完了から未実行を推測しない。
// accepted/unknown/受理後errorや未確認の補償・副作用は既存Workers/Ticketの正本で照合する。
export function workerLaunchNeedsReconciliation(
  receipt: StartupReceipt,
): boolean {
  if (receipt.kind !== "problem" || !receipt.extension) return true;
  const {
    operation,
    resources,
    secondary,
    persistence,
    backend_result_persistence,
  } = receipt.extension;
  return (persistence != null && persistence !== "persisted") ||
    (backend_result_persistence != null &&
      backend_result_persistence !== "persisted") ||
    operation.acceptance !== "rejected" ||
    !["rejected", "unsupported"].includes(operation.execution) ||
    (operation.backend_compensation != null &&
      operation.backend_compensation !== "completed") ||
    (operation.finalize != null &&
      operation.finalize.compensation !== "completed") ||
    secondary.some((item) =>
      item.kind !== "persistence" || item.outcome !== "persisted"
    ) ||
    (resources != null &&
      (resources.durable_effects_possible || resources.resources !== "none" ||
        !["not_needed", "confirmed"].includes(String(resources.cleanup))));
}

export function workerStartupNotice(
  receipt: StartupReceipt,
): FailureNoticeModel {
  const notice = failureNotice(receipt);
  if (receipt.kind !== "problem" || !receipt.extension) return notice;
  const facts = receipt.extension;
  notice.supplements.push(
    `Creation acceptance: ${facts.operation.acceptance}. Execution outcome: ${facts.operation.execution}.`,
  );
  if (facts.operation.correlation) {
    notice.details.push({
      label: "Support correlation",
      value: facts.operation.correlation,
    });
  }
  if (facts.operation.backend_compensation) {
    const text = `Backend cleanup: ${facts.operation.backend_compensation}.`;
    if (facts.operation.backend_compensation === "completed") {
      notice.details.push({ label: "Backend cleanup", value: "completed" });
    } else notice.compensation.push(text);
  }
  if (facts.operation.finalize) {
    if (facts.operation.finalize.compensation !== "completed") {
      notice.compensation.push(
        `Finalization cleanup: ${facts.operation.finalize.compensation}.`,
      );
    } else {notice.details.push({
        label: "Finalization cleanup",
        value: "completed",
      });}
    notice.details.push({
      label: "Finalization stage",
      value: facts.operation.finalize.stage,
    });
  }
  if (facts.resources) {
    const cleanup = facts.resources.cleanup;
    if (typeof cleanup !== "string" || cleanup === "required") {
      notice.compensation.push(
        `Runtime cleanup: ${
          typeof cleanup === "string" ? cleanup : `failed (${cleanup.failed})`
        }.`,
      );
    }
    notice.details.push({
      label: "Runtime resource observation",
      value:
        `${facts.resources.resources}; durable effects possible: ${facts.resources.durable_effects_possible}; cleanup: ${
          typeof cleanup === "string" ? cleanup : cleanup.failed
        }`,
    });
  }
  for (const item of facts.secondary) {
    if (item.kind !== "persistence" || item.outcome !== "persisted") {
      notice.supplements.push(`Secondary ${item.kind} result needs attention.`);
    }
    notice.details.push({
      label: "Secondary result",
      value: `${item.kind === "cleanup" ? "cleanup" : item.phase}: ${
        item.kind === "cleanup"
          ? item.failure
          : item.kind === "persistence"
          ? item.outcome
          : item.kind
      }`,
    });
  }
  for (
    const [label, value] of [
      ["Session metadata persistence", facts.persistence],
      ["Backend result persistence", facts.backend_result_persistence],
      ["I/O kind", facts.io_kind],
    ]
  ) {
    if (value) notice.details.push({ label: label!, value });
    if (value && value !== "persisted" && label !== "I/O kind") {
      notice.supplements.push(`${label}: ${value}.`);
    }
  }
  return notice;
}

// 送信/受信の失敗はremote側の確定拒否ではない。新しいPOSTへの自動再送は行わない。
export async function submitWorkerLaunch(
  fetchFn: typeof fetch,
  path: string,
  request:
    import("#lib/generated/worker-launch-api.ts").CreateWorkspaceWorkerRequest,
): Promise<
  | {
    kind: "created";
    response:
      import("#lib/generated/worker-launch-api.ts").BrowserCreateWorkerResponse;
  }
  | { kind: "failed"; receipt: StartupReceipt }
> {
  let response: Response;
  try {
    response = await fetchFn(path, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(request),
    });
  } catch {
    return { kind: "failed", receipt: { kind: "transport" } };
  }
  if (!response.ok) {
    return {
      kind: "failed",
      receipt: await receiveWorkerStartupProblem(response),
    };
  }
  try {
    return {
      kind: "created",
      response: parseBrowserCreateWorkerResponse(
        await readBoundedJson(response, 8 * 1024 * 1024),
      ),
    };
  } catch {
    return { kind: "failed", receipt: { kind: "invalid" } };
  }
}

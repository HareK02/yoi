import type { Category } from "#lib/generated/error-contract.ts";
import type {
  RuntimeControlCause,
  RuntimeWorkerLifecycleResult,
} from "#lib/generated/runtime-api.ts";
export type { RuntimeControlCause } from "#lib/generated/runtime-api.ts";
import type { ApiResult } from "#lib/workspace/api/http.ts";
import { readBoundedJson } from "#lib/workspace/api/http.ts";
import { failureNotice } from "./model.ts";
import { failureString, receiveProblem } from "./receive.ts";
import type { DetailsDecoder, FailureReceipt } from "./receive.ts";

// protocol::control_failureの公開enum。共通envelope/Problem検証はreceiveが所有する。
const CAUSES = {
  runtime_missing: "dependency",
  worker_missing: "dependency",
  invalid_worker_identifier: "invalid_input",
  runtime_stopped: "dependency",
  execution_unavailable: "dependency",
  execution_failed: "internal",
  execution_rejected: "conflict",
  restore_observation_stale: "conflict",
  restore_observation_conflict: "conflict",
  restore_admission_busy: "conflict",
  stop_observation_conflict: "conflict",
  stop_request_conflict: "conflict",
  stop_admission_busy: "conflict",
  create_request_conflict: "conflict",
  invalid_request: "invalid_input",
  workspace_binding_mismatch: "authorization",
  configuration_unavailable: "dependency",
  working_directory_failed: "dependency",
  persistence_failed: "internal",
  persistence_outcome_unknown: "unknown",
  store_unavailable: "internal",
  recording_source_unavailable: "dependency",
  recording_ownership_unknown: "unknown",
  recording_execution_unconfirmed: "unknown",
  state_unavailable: "internal",
  authorization_failed: "authorization",
  connection_failed: "dependency",
  transport_failed: "unknown",
  timed_out: "unknown",
  request_encoding_failed: "invalid_input",
  client_setup_failed: "internal",
  response_too_large: "dependency",
  invalid_response: "dependency",
  result_unavailable: "unknown",
  unsupported_operation: "invalid_input",
  remote_rejected: "dependency",
} as const satisfies Record<RuntimeControlCause, Category>;
const causes: ReadonlySet<string> = new Set(Object.keys(CAUSES));

export type RuntimeControlEvidence =
  & Pick<RuntimeWorkerLifecycleResult, "control" | "control_explanation">
  & {
    controlUnsupported?: boolean;
  };

export function parseRuntimeControl(
  value: unknown,
  explanation?: unknown,
): RuntimeControlEvidence {
  const cause: RuntimeControlEvidence = value === undefined
    ? {}
    : value === null
    ? { control: null }
    : typeof value === "string" && causes.has(value)
    ? { control: value as RuntimeControlCause }
    : { controlUnsupported: true };
  if (explanation === undefined || explanation === null) {
    return {
      ...cause,
      ...(explanation === null ? { control_explanation: null } : {}),
    };
  }
  try {
    const text = failureString(explanation);
    return text.trim() ? { ...cause, control_explanation: text } : cause;
  } catch {
    // 説明の欠落・不正で、別の正本が持つ操作stateやcauseを上書きしない。
    return cause;
  }
}

// 操作stateやdiagnosticsから主原因を推測せず、ownerが組にした公開説明だけを使う。
export function runtimeControlMessage(
  result: RuntimeControlEvidence,
  fallback: string,
): string {
  if (result.controlUnsupported) {
    return `${fallback}. The Runtime returned an unsupported control cause.`;
  }
  if (!result.control) {
    return `${fallback}. No supported Runtime control cause was provided.`;
  }
  return result.control_explanation ??
    `${fallback}. The Runtime control explanation is unavailable.`;
}

export const decodeRuntimeControl: DetailsDecoder<RuntimeControlCause> = (
  failure,
) => {
  if (
    failure.domain !== "runtime_control" || failure.details_version !== 1 ||
    !causes.has(failure.reason)
  ) {
    return { kind: "unsupported" };
  }
  if (
    failure.details !== failure.reason ||
    failure.category !== CAUSES[failure.reason as RuntimeControlCause]
  ) return { kind: "invalid" };
  return { kind: "known", details: failure.details as RuntimeControlCause };
};

export class RuntimeControlHttpError extends Error {
  readonly notice;
  constructor(
    readonly status: number,
    readonly receipt: FailureReceipt<RuntimeControlCause>,
  ) {
    const notice = failureNotice(receipt);
    super(notice.explanation);
    this.name = "RuntimeControlHttpError";
    this.notice = notice;
  }
}

export async function runtimeControlHttpError(
  response: Response,
): Promise<RuntimeControlHttpError> {
  return new RuntimeControlHttpError(
    response.status,
    await receiveProblem(response, decodeRuntimeControl),
  );
}

export async function runtimeControlJson<T>(
  response: Response,
  parse: (value: unknown) => T,
): Promise<T> {
  if (!response.ok) throw await runtimeControlHttpError(response);
  return parse(await readBoundedJson(response, 8 * 1024 * 1024));
}

// RuntimeのGETにも同じ受信境界を使う。http.tsの汎用legacy表示へraw bodyを渡さない。
export async function loadRuntimeJson<T>(
  fetchFn: typeof fetch,
  path: string,
  init: RequestInit | undefined,
  parse: (value: unknown) => T,
): Promise<ApiResult<T>> {
  try {
    return {
      data: await runtimeControlJson(await fetchFn(path, init), parse),
      error: null,
    };
  } catch (error) {
    return {
      data: null,
      error: error instanceof RuntimeControlHttpError
        ? error.message
        : "Runtime response could not be loaded safely.",
      ...(error instanceof RuntimeControlHttpError
        ? { status: error.status }
        : {}),
    };
  }
}

import type {
  SubjectBodyFailedRequest,
  SubjectBodyMutationResponse,
  SubjectBodyObservation,
  SubjectBodyRef,
  SubjectBodyResponse,
  SubjectBodyStartRequest,
  SubjectBodyStopOutcomeResponse,
  SubjectBodyStopRequest,
} from "#lib/generated/subject-body-api.ts";
import { parseWorkerSummary } from "#lib/workspace/api/workers.ts";
import { readBoundedJson, workspaceApiPath } from "#lib/workspace/api/http.ts";

type Diagnostic = SubjectBodyMutationResponse["diagnostics"][number];
const MAX_RESPONSE_BYTES = 2 * 1024 * 1024;
const MAX_SAVED_LENGTH = 16 * 1024;

function record(value: unknown, keys: string[]): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new Error("Subject body value must be an object");
  }
  if (Object.keys(value).some((key) => !keys.includes(key))) {
    throw new Error("Subject body value contains unknown fields");
  }
  return value as Record<string, unknown>;
}

function text(value: unknown, max = 4096): string {
  if (typeof value !== "string" || value.length === 0 || value.length > max) {
    throw new Error("Subject body string is invalid");
  }
  return value;
}

function parseRequestId(value: unknown): string {
  const result = text(value, 128);
  if (
    new TextEncoder().encode(result).byteLength > 128 || /\p{Cc}/u.test(result)
  ) {
    throw new Error(
      "Subject body request ID must be at most 128 UTF-8 bytes and contain no control characters",
    );
  }
  return result;
}

function token(value: unknown): string {
  const result = text(value, 256);
  if (new TextEncoder().encode(result).byteLength > 256) {
    throw new Error("Subject body observation token is too long");
  }
  return result;
}

function flag(value: unknown): boolean {
  if (typeof value !== "boolean") {
    throw new Error("Subject body flag is invalid");
  }
  return value;
}

function diagnostics(value: unknown): Diagnostic[] {
  if (!Array.isArray(value) || value.length > 128) {
    throw new Error("Subject body diagnostics exceed their limit");
  }
  return value.map((entry) => {
    const item = record(entry, ["code", "severity", "message"]);
    const severity = item.severity;
    if (severity !== "info" && severity !== "warning" && severity !== "error") {
      throw new Error("Subject body diagnostic severity is invalid");
    }
    return {
      code: text(item.code),
      severity,
      message: text(item.message, 65536),
    };
  });
}

function bodyRef(value: unknown): SubjectBodyRef {
  const item = record(value, ["runtime_id", "worker_id", "generation"]);
  const generation = text(item.generation, 20);
  if (
    !/^(0|[1-9][0-9]*)$/.test(generation) ||
    (generation.length === 20 && generation > "18446744073709551615")
  ) {
    throw new Error("Subject body generation must be unsigned decimal text");
  }
  return {
    runtime_id: text(item.runtime_id),
    worker_id: text(item.worker_id),
    generation,
  };
}

export function parseSubjectBodyResponse(
  value: unknown,
  workspaceId: string,
  subjectId: string,
): SubjectBodyResponse {
  const item = record(value, [
    "workspace_id",
    "subject_id",
    "current",
    "observation",
    "can_start",
    "can_stop",
  ]);
  const workspace = text(item.workspace_id);
  const subject = text(item.subject_id);
  if (workspace !== workspaceId || subject !== subjectId) {
    throw new Error(
      "Subject body response does not match the requested target",
    );
  }
  const current = item.current === null ? null : bodyRef(item.current);
  const observed = record(item.observation, ["kind", "worker", "diagnostics"]);
  let observation: SubjectBodyObservation;
  switch (observed.kind) {
    case "absent":
      record(observed, ["kind"]);
      if (current !== null) throw new Error("Absent body cannot have an owner");
      observation = { kind: "absent" };
      break;
    case "unavailable":
      record(observed, ["kind", "diagnostics"]);
      if (current === null) {
        throw new Error("Unavailable body must retain its owner");
      }
      observation = {
        kind: "unavailable",
        diagnostics: diagnostics(observed.diagnostics),
      };
      break;
    case "available": {
      record(observed, ["kind", "worker"]);
      const worker = parseWorkerSummary(observed.worker);
      if (
        current === null || worker.runtime_id !== current.runtime_id ||
        worker.worker_id !== current.worker_id ||
        (worker.workspace.workspace_id != null &&
          worker.workspace.workspace_id !== workspaceId)
      ) {
        throw new Error("Subject body Worker does not match the current owner");
      }
      observation = { kind: "available", worker };
      break;
    }
    default:
      throw new Error("Subject body observation kind is invalid");
  }
  return {
    workspace_id: workspace,
    subject_id: subject,
    current,
    observation,
    can_start: flag(item.can_start),
    can_stop: flag(item.can_stop),
  };
}

export function parseSubjectBodyMutationResponse(
  value: unknown,
  workspaceId: string,
  subjectId: string,
  requestId: string,
): SubjectBodyMutationResponse {
  const item = record(value, [
    "request_id",
    "disposition",
    "diagnostics",
    "body",
  ]);
  const request_id = parseRequestId(item.request_id);
  if (request_id !== requestId) {
    throw new Error("Subject body receipt does not match the request");
  }
  const disposition = item.disposition;
  if (
    disposition !== "accepted" && disposition !== "rejected" &&
    disposition !== "unknown"
  ) {
    throw new Error("Subject body disposition is invalid");
  }
  return {
    request_id,
    disposition,
    diagnostics: diagnostics(item.diagnostics),
    body: item.body === null
      ? null
      : parseSubjectBodyResponse(item.body, workspaceId, subjectId),
  };
}

export function parseSubjectBodyStopOutcomeResponse(
  value: unknown,
  workspaceId: string,
  subjectId: string,
  requestId: string,
): SubjectBodyStopOutcomeResponse {
  const item = record(value, ["request_id", "outcome", "body"]);
  const request_id = parseRequestId(item.request_id);
  if (request_id !== requestId) {
    throw new Error("Subject body stop outcome does not match the request");
  }
  const outcome = item.outcome;
  if (
    outcome !== "not_found" && outcome !== "pending" && outcome !== "stopped"
  ) {
    throw new Error("Subject body stop outcome is invalid");
  }
  return {
    request_id,
    outcome,
    body: item.body === null
      ? null
      : parseSubjectBodyResponse(item.body, workspaceId, subjectId),
  };
}

function bodyPath(workspaceId: string, subjectId: string): string {
  return workspaceApiPath(
    workspaceId,
    `/subjektiv/subjects/${encodeURIComponent(subjectId)}/body`,
  );
}

export class SubjectBodyApiError extends Error {
  constructor(
    message: string,
    readonly request: SubjectBodyFailedRequest | null = null,
  ) {
    super(message);
    this.name = "SubjectBodyApiError";
  }
}

type RequestIdentity = {
  workspaceId: string;
  subjectId: string;
  requestId: string;
};

function failedRequest(
  value: unknown,
  expected: RequestIdentity,
): SubjectBodyFailedRequest {
  const item = record(value, [
    "workspace_id",
    "subject_id",
    "request_id",
    "resolution",
  ]);
  const workspace_id = text(item.workspace_id);
  const subject_id = text(item.subject_id);
  const request_id = parseRequestId(item.request_id);
  if (
    workspace_id !== expected.workspaceId ||
    subject_id !== expected.subjectId ||
    request_id !== expected.requestId
  ) {
    throw new Error("Subject body rejection does not match the frozen request");
  }
  const resolution = item.resolution;
  if (
    resolution !== "not_accepted" && resolution !== "superseded" &&
    resolution !== "unresolved"
  ) {
    throw new Error("Subject body request resolution is invalid");
  }
  return { workspace_id, subject_id, request_id, resolution };
}

async function checkedJson(
  response: Response,
  expected?: RequestIdentity,
): Promise<unknown> {
  if (!response.ok) {
    // GETやHTTP statusだけを、保存済み依頼の決済根拠にしない。
    let detail = "";
    let request: SubjectBodyFailedRequest | null = null;
    try {
      const error = record(
        await readBoundedJson(response, MAX_RESPONSE_BYTES),
        ["error", "message", "diagnostics", "request"],
      );
      text(error.error);
      const message = text(error.message, 65536);
      const publicDiagnostics = diagnostics(error.diagnostics);
      detail = (publicDiagnostics.map((item) =>
        item.message
      ).join("; ") || message)
        .slice(0, 1024);
      if (expected && error.request != null) {
        const bound = failedRequest(error.request, expected);
        // terminalなstale拒否は専用409契約だけ。未認可/別statusには流用しない。
        if (response.status === 409) request = bound;
      }
    } catch {
      // 不正なbodyやidentity不一致は決済せず、元の正確な入力を保持する。
      request = null;
    }
    throw new SubjectBodyApiError(
      `Subject body request failed with HTTP ${response.status}${
        detail ? `: ${detail}` : ""
      }`,
      request,
    );
  }
  return readBoundedJson(response, MAX_RESPONSE_BYTES);
}

export async function getSubjectBody(
  fetchFn: typeof fetch,
  workspaceId: string,
  subjectId: string,
): Promise<SubjectBodyResponse> {
  return parseSubjectBodyResponse(
    await checkedJson(
      await fetchFn(bodyPath(workspaceId, subjectId), { cache: "no-store" }),
    ),
    workspaceId,
    subjectId,
  );
}

async function mutateSubjectBody(
  fetchFn: typeof fetch,
  workspaceId: string,
  subjectId: string,
  action: "start" | "stop",
  request: SubjectBodyStartRequest | SubjectBodyStopRequest,
): Promise<SubjectBodyMutationResponse> {
  const requestId = parseRequestId(request.request_id);
  const response = await fetchFn(
    `${bodyPath(workspaceId, subjectId)}/${action}`,
    {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(request),
    },
  );
  return parseSubjectBodyMutationResponse(
    await checkedJson(response, { workspaceId, subjectId, requestId }),
    workspaceId,
    subjectId,
    requestId,
  );
}

export function startSubjectBody(
  fetchFn: typeof fetch,
  workspaceId: string,
  subjectId: string,
  request: SubjectBodyStartRequest,
): Promise<SubjectBodyMutationResponse> {
  return mutateSubjectBody(fetchFn, workspaceId, subjectId, "start", request);
}

export function stopSubjectBody(
  fetchFn: typeof fetch,
  workspaceId: string,
  subjectId: string,
  request: SubjectBodyStopRequest,
): Promise<SubjectBodyMutationResponse> {
  return mutateSubjectBody(fetchFn, workspaceId, subjectId, "stop", request);
}

// POSTは凍結入力を照合するためであり、stopの再実行ではない。
export async function getSubjectBodyStopOutcome(
  fetchFn: typeof fetch,
  workspaceId: string,
  subjectId: string,
  request: SubjectBodyStopRequest,
): Promise<SubjectBodyStopOutcomeResponse> {
  const requestId = parseRequestId(request.request_id);
  const response = await fetchFn(
    `${bodyPath(workspaceId, subjectId)}/stop/outcome`,
    {
      method: "POST",
      headers: { "content-type": "application/json" },
      cache: "no-store",
      body: JSON.stringify(request),
    },
  );
  // 認可拒否・通信失敗・不正な応答をnot_foundに読み替えない。
  return parseSubjectBodyStopOutcomeResponse(
    await checkedJson(response, { workspaceId, subjectId, requestId }),
    workspaceId,
    subjectId,
    requestId,
  );
}

export function subjectBodyControls(
  body: SubjectBodyResponse,
): { start: boolean; stop: boolean } {
  if (body.observation.kind === "absent") {
    return { start: body.can_start, stop: false };
  }
  if (body.observation.kind !== "available") {
    return { start: false, stop: false };
  }
  const worker = body.observation.worker;
  const observed = worker.availability === "observed" &&
    Boolean(worker.restore_observation_token);
  return {
    start: body.can_start && observed && worker.state === "stopped" &&
      worker.worker_state == null,
    stop: body.can_stop && observed && worker.worker_state != null &&
      ["idle", "running", "paused"].includes(worker.state),
  };
}

// accountIdはwhoami由来のローカルな隔離境界であり、Serverへ送るactor指定ではない。
export type SubjectBodyAttempt =
  & {
    accountId: string;
    workspaceId: string;
    subjectId: string;
  }
  & (
    | { action: "start"; request: SubjectBodyStartRequest }
    | { action: "stop"; request: SubjectBodyStopRequest }
  );

export function subjectBodyStorageKey(
  accountId: string,
  workspaceId: string,
  subjectId: string,
): string {
  // accountを特定できない旧2要素keyは読取り・移行・削除しない。
  return `yoi.subject-body:${
    JSON.stringify([text(accountId), workspaceId, subjectId])
  }`;
}

function parseAttempt(
  value: unknown,
  accountId: string,
  workspaceId: string,
  subjectId: string,
): SubjectBodyAttempt {
  text(accountId);
  const item = record(value, [
    "accountId",
    "workspaceId",
    "subjectId",
    "action",
    "request",
  ]);
  if (
    item.accountId !== accountId || item.workspaceId !== workspaceId ||
    item.subjectId !== subjectId
  ) {
    throw new Error(
      "Saved Subject body request belongs to a different account or target",
    );
  }
  const input = record(item.request, [
    "request_id",
    "expected_current",
    "expected_observation_token",
  ]);
  const request_id = parseRequestId(input.request_id);
  const expected_current = input.expected_current === null
    ? null
    : Object.freeze(bodyRef(input.expected_current));
  const expected_observation_token = input.expected_observation_token === null
    ? null
    : token(input.expected_observation_token);
  if (
    item.action === "stop" && expected_current !== null &&
    expected_observation_token !== null
  ) {
    return Object.freeze({
      accountId,
      workspaceId,
      subjectId,
      action: "stop",
      request: Object.freeze({
        request_id,
        expected_current,
        expected_observation_token,
      }),
    });
  }
  if (
    item.action === "start" &&
    ((expected_current === null) === (expected_observation_token === null))
  ) {
    return Object.freeze({
      accountId,
      workspaceId,
      subjectId,
      action: "start",
      request: Object.freeze({
        request_id,
        expected_current,
        expected_observation_token,
      }),
    });
  }
  throw new Error(
    "Saved Subject body request is invalid; do not create a replacement request",
  );
}

export function createSubjectBodyAttempt(
  accountId: string,
  body: SubjectBodyResponse,
  action: "start" | "stop",
  requestId: string,
): SubjectBodyAttempt {
  if (!subjectBodyControls(body)[action]) {
    throw new Error("Refresh Subject body before this action");
  }
  return parseAttempt(
    {
      accountId,
      workspaceId: body.workspace_id,
      subjectId: body.subject_id,
      action,
      request: {
        request_id: requestId,
        expected_current: body.current,
        expected_observation_token: body.observation.kind === "available"
          ? body.observation.worker.restore_observation_token
          : null,
      },
    },
    accountId,
    body.workspace_id,
    body.subject_id,
  );
}

export function readSubjectBodyAttempt(
  storage: Pick<Storage, "getItem">,
  accountId: string,
  workspaceId: string,
  subjectId: string,
): SubjectBodyAttempt | null {
  const raw = storage.getItem(
    subjectBodyStorageKey(accountId, workspaceId, subjectId),
  );
  if (raw === null) return null;
  if (raw.length > MAX_SAVED_LENGTH) {
    throw new Error("Saved Subject body request is too large");
  }
  return parseAttempt(JSON.parse(raw), accountId, workspaceId, subjectId);
}

export function saveSubjectBodyAttempt(
  storage: Pick<Storage, "getItem" | "setItem">,
  accountId: string,
  attempt: SubjectBodyAttempt,
): void {
  const safe = parseAttempt(
    attempt,
    accountId,
    attempt.workspaceId,
    attempt.subjectId,
  );
  const raw = JSON.stringify(safe);
  if (raw.length > MAX_SAVED_LENGTH) {
    throw new Error("Subject body request is too large to retain");
  }
  const previous = readSubjectBodyAttempt(
    storage,
    accountId,
    safe.workspaceId,
    safe.subjectId,
  );
  if (previous !== null && JSON.stringify(previous) !== raw) {
    throw new Error(
      "An unresolved Subject body request already exists; retry that request",
    );
  }
  storage.setItem(
    subjectBodyStorageKey(accountId, safe.workspaceId, safe.subjectId),
    raw,
  );
}

export function clearSubjectBodyAttempt(
  storage: Pick<Storage, "getItem" | "removeItem">,
  accountId: string,
  attempt: SubjectBodyAttempt,
): void {
  // 呼出し時のaccountと凍結入力を、storageアクセスより前に照合する。
  const safe = parseAttempt(
    attempt,
    accountId,
    attempt.workspaceId,
    attempt.subjectId,
  );
  const saved = readSubjectBodyAttempt(
    storage,
    accountId,
    safe.workspaceId,
    safe.subjectId,
  );
  if (saved && JSON.stringify(saved) !== JSON.stringify(safe)) {
    throw new Error(
      "The saved Subject body request changed; do not replace it",
    );
  }
  storage.removeItem(
    subjectBodyStorageKey(accountId, safe.workspaceId, safe.subjectId),
  );
}

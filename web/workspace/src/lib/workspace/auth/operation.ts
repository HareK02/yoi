import type { PasskeyAdditionResultRequest } from "#lib/generated/auth-api.ts";
import type { WhoamiResponse } from "./model.ts";

export type AuthStage =
  | "session-check"
  | "options"
  | "authenticator"
  | "conversion"
  | "before-complete"
  | "complete"
  | "confirmation";
export type AuthFailure =
  | "cancelled-or-unavailable"
  | "excluded"
  | "unsupported"
  | "environment"
  | "conversion"
  | "session-changed"
  | "rejected"
  | "transport"
  | "invalid-response"
  | "stopped";
export type AuthOutcome =
  | "not-sent"
  | "rejected"
  | "unknown"
  | "saved-session-unconfirmed";

// 診断は固定分類だけを持つ。cause、例外本文、HTTP body、credentialやceremony IDを保持しない。
export class AuthOperationError extends Error {
  constructor(
    readonly stage: AuthStage,
    readonly reason: AuthFailure,
    readonly outcome: AuthOutcome,
    readonly httpStatus?: number,
  ) {
    super(`Passkey ${stage}: ${reason} (${outcome}).`);
  }
}
type Observer = {
  phase: (stage: AuthStage) => void;
  verifySession: () => Promise<void>;
  expected: WhoamiResponse | null;
  additionAttempt: (attempt: PasskeyAdditionResultRequest) => void;
};
const observers = new WeakMap<AbortSignal, Observer>();
export function observeAuthOperation(
  signal: AbortSignal,
  observer: Observer,
): void {
  observers.set(signal, observer);
}
export function recordAdditionAttempt(
  signal: AbortSignal | undefined,
  attempt: PasskeyAdditionResultRequest,
): void {
  if (signal) observers.get(signal)?.additionAttempt(attempt);
}
export function expectedAuthSession(
  signal?: AbortSignal,
): WhoamiResponse | null {
  return signal ? observers.get(signal)?.expected ?? null : null;
}
export async function authCheckpoint(
  signal: AbortSignal | undefined,
  stage: AuthStage,
): Promise<void> {
  signal?.throwIfAborted();
  const observer = signal ? observers.get(signal) : undefined;
  observer?.phase(stage);
  if (stage === "options" || stage === "before-complete") {
    await observer?.verifySession();
  }
  signal?.throwIfAborted();
}
export function classifyAuthFailure(
  error: unknown,
  stage: AuthStage,
  sent: boolean,
): AuthOperationError {
  if (error instanceof AuthOperationError) return error;
  const outcome = sent ? "unknown" : "not-sent";
  if (
    error instanceof DOMException &&
    (stage === "authenticator" || error.name === "AbortError")
  ) {
    switch (error.name) {
      case "AbortError":
        return new AuthOperationError(stage, "stopped", outcome);
      case "InvalidStateError":
        return new AuthOperationError(stage, "excluded", outcome);
      case "NotAllowedError":
        return new AuthOperationError(
          stage,
          "cancelled-or-unavailable",
          outcome,
        );
      case "NotSupportedError":
        return new AuthOperationError(stage, "unsupported", outcome);
      case "SecurityError":
        return new AuthOperationError(stage, "environment", outcome);
    }
  }
  return new AuthOperationError(
    stage,
    stage === "conversion" ? "conversion" : "transport",
    outcome,
  );
}
export function operationErrorMessage(error: AuthOperationError): string {
  const reason: Record<AuthFailure, string> = {
    "cancelled-or-unavailable":
      "The browser did not complete verification. It may have been cancelled, timed out, or found no usable key; the browser does not distinguish these reasons.",
    excluded:
      "The authenticator reported an existing key (duplicate exclusion). Keep the older key and choose a different device, security key, or passkey provider; do not delete your only working key.",
    unsupported:
      "This browser or key cannot complete this passkey operation. Use a supported device that can save passkeys and verify with a PIN or biometrics.",
    environment:
      "The browser blocked this operation because of an origin, security, or environment constraint. Check that you are using the intended secure site; the exact cause is not known.",
    conversion:
      "The device may have created a key, but its response could not be converted for the server. Registration was not sent. Keep existing keys; a device-side entry alone does not mean it was saved to this account.",
    "session-changed":
      "The account or session changed, or could not be confirmed. The result cannot be applied to the previously displayed account.",
    rejected:
      "The server rejected the request or verification. A challenge may be invalid or expired. Check your account and the operation result before starting a new ceremony.",
    transport: "Communication could not be confirmed.",
    "invalid-response": "The server response could not be confirmed.",
    stopped: "Operation stopped.",
  };
  const outcome = error.outcome === "not-sent"
    ? "Completion was not sent."
    : error.outcome === "rejected"
    ? "The server did not accept completion."
    : error.outcome === "saved-session-unconfirmed"
    ? "The server confirmed the operation, but the expected browser session is not confirmed."
    : "Completion may already have reached the server. Its result is unknown; check the operation result before adding another key. Stopping the browser does not undo a submitted request.";
  return `${reason[error.reason]} ${outcome} [${error.stage}/${error.reason}${
    error.httpStatus ? `/HTTP-${error.httpStatus}` : ""
  }]`;
}

import type {
  DeviceLoginApproveRequest,
  PasskeyAdditionOptionsRequest,
  PasskeyAdditionResultRequest,
  PasskeyLoginCompleteRequest,
  PasskeyLoginOptionsRequest,
  PasskeyRegistrationCompleteRequest,
  PasskeyRegistrationOptionsRequest,
} from "#lib/generated/auth-api.ts";
import {
  authenticationCredentialToJson,
  AuthPayloadError,
  type DeviceApprovalResponse,
  parseAdditionResult,
  parseAuthUserResponse,
  parseDeviceApprovalResponse,
  parseLogoutResponse,
  parsePasskeyList,
  parseWhoamiResponse,
  type PasskeyUserResponse,
  prepareLoginOptions,
  prepareRegistrationOptions,
  registrationCredentialToJson,
  type WhoamiResponse,
} from "#lib/workspace/auth/model.ts";

import {
  authCheckpoint,
  AuthOperationError,
  type AuthStage,
  classifyAuthFailure,
  expectedAuthSession,
  recordAdditionAttempt,
} from "./operation.ts";

export class AuthRequestError extends Error {
  constructor(readonly status: number, readonly code: string | null = null) {
    super(`Auth request failed (${status}).`);
  }
}

class AuthResponseError extends Error {}

const MAX_AUTH_RESPONSE_BYTES = 256 * 1024;

export async function readBoundedAuthResponseJson(
  response: Response,
): Promise<unknown> {
  const contentLength = response.headers.get("content-length");
  if (contentLength !== null) {
    const parsed = Number(contentLength);
    if (
      !Number.isSafeInteger(parsed) || parsed < 0 ||
      parsed > MAX_AUTH_RESPONSE_BYTES
    ) {
      await response.body?.cancel();
      throw new AuthResponseError(
        "Invalid auth response: response body exceeds the size limit.",
      );
    }
  }
  if (response.body === null) {
    throw new AuthResponseError(
      "Invalid auth response: response body is missing.",
    );
  }

  const reader = response.body.getReader();
  const chunks: Uint8Array[] = [];
  let totalBytes = 0;
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      totalBytes += value.byteLength;
      if (totalBytes > MAX_AUTH_RESPONSE_BYTES) {
        await reader.cancel();
        throw new AuthResponseError(
          "Invalid auth response: response body exceeds the size limit.",
        );
      }
      chunks.push(value);
    }
  } finally {
    reader.releaseLock();
  }

  const bytes = new Uint8Array(totalBytes);
  let offset = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, offset);
    offset += chunk.byteLength;
  }
  try {
    return JSON.parse(
      new TextDecoder("utf-8", { fatal: true }).decode(bytes),
    ) as unknown;
  } catch {
    throw new AuthResponseError(
      "Invalid auth response: response body is not valid JSON.",
    );
  }
}

async function requestJson(path: string, init?: RequestInit): Promise<unknown> {
  const response = await fetch(path, {
    credentials: "same-origin",
    mode: "same-origin",
    redirect: "error",
    cache: "no-store",
    ...init,
    headers: {
      "content-type": "application/json",
      ...(init?.headers ?? {}),
    },
  });
  if (!response.ok) {
    let code: string | null = null;
    try {
      const value = await readBoundedAuthResponseJson(response);
      const message = typeof value === "object" && value !== null
        ? (value as { message?: unknown }).message
        : null;
      if (typeof message === "string") {
        code = [
          "auth_required",
          "passkey_authentication_changed",
          "passkey_registration_binding_mismatch",
          "webauthn_registration_verification_failed",
          "webauthn_login_verification_failed",
          "invalid_passkey_challenge",
          "expired_passkey_challenge",
          "self_registration_disabled",
          "passkey_activation_required",
        ].find((code) => message.startsWith(code + ":")) ?? null;
      }
    } catch { /* 本文・解析例外は保存しない。statusだけでも失敗として扱う。 */ }
    throw new AuthRequestError(response.status, code);
  }
  return await readBoundedAuthResponseJson(response);
}

export async function loadWhoami(
  signal?: AbortSignal,
): Promise<WhoamiResponse> {
  return parseWhoamiResponse(await requestJson("/api/auth/whoami", { signal }));
}

export async function logout(signal?: AbortSignal): Promise<void> {
  parseLogoutResponse(
    await requestJson("/api/auth/logout", {
      method: "POST",
      body: "{}",
      signal,
    }),
  );
}

export async function registerPasskey(
  handle: string,
  displayName?: string,
  signal?: AbortSignal,
): Promise<PasskeyUserResponse> {
  const optionsRequest: PasskeyRegistrationOptionsRequest = {
    handle,
    display_name: displayName ?? null,
    browser_origin: window.location.origin,
  };
  return await createPasskey("registration", optionsRequest, signal);
}

export async function addPasskey(
  signal?: AbortSignal,
): Promise<PasskeyUserResponse> {
  const expected = expectedAuthSession(signal) ?? await loadWhoami(signal);
  if (!expected.actor || !expected.authentication_reference) {
    throw new AuthOperationError(
      "session-check",
      "session-changed",
      "not-sent",
    );
  }
  const optionsRequest: PasskeyAdditionOptionsRequest = {
    expected_user_id: expected.actor.user_id,
    expected_account_id: expected.actor.account_id,
    expected_authentication_reference: expected.authentication_reference,
    browser_origin: window.location.origin,
  };
  return await createPasskey("addition", optionsRequest, signal);
}

async function createPasskey(
  purpose: "registration" | "addition",
  optionsRequest:
    | PasskeyRegistrationOptionsRequest
    | PasskeyAdditionOptionsRequest,
  signal?: AbortSignal,
): Promise<PasskeyUserResponse> {
  let stage: AuthStage = "options";
  let sent = false;
  try {
    await authCheckpoint(signal, stage);
    const options = await requestJson(`/api/auth/passkeys/${purpose}/options`, {
      method: "POST",
      body: JSON.stringify(optionsRequest),
      signal,
    });
    const publicKey = prepareRegistrationOptions(options);
    const challengeId = (options as { challenge_id: string }).challenge_id;
    stage = "authenticator";
    await authCheckpoint(signal, stage);
    if (typeof navigator.credentials?.create !== "function") {
      throw new DOMException("", "NotSupportedError");
    }
    const credential = await navigator.credentials.create({
      signal,
      publicKey,
    });
    if (!credential) throw new DOMException("", "NotAllowedError");
    signal?.throwIfAborted();
    stage = "conversion";
    await authCheckpoint(signal, stage);
    const completeRequest: PasskeyRegistrationCompleteRequest = {
      challenge_id: challengeId,
      credential: registrationCredentialToJson(credential),
    };
    stage = "before-complete";
    await authCheckpoint(signal, stage);
    stage = "complete";
    await authCheckpoint(signal, stage);
    if (purpose === "addition") {
      const expected = optionsRequest as PasskeyAdditionOptionsRequest;
      recordAdditionAttempt(signal, {
        challenge_id: challengeId,
        expected_user_id: expected.expected_user_id,
        expected_account_id: expected.expected_account_id,
        expected_authentication_reference:
          expected.expected_authentication_reference,
      });
    }
    sent = true;
    const result = parseAuthUserResponse(
      await requestJson(`/api/auth/passkeys/${purpose}/complete`, {
        method: "POST",
        body: JSON.stringify(completeRequest),
        signal,
      }),
    );
    signal?.throwIfAborted();
    return result;
  } catch (error) {
    if (error instanceof AuthRequestError) {
      throw new AuthOperationError(
        stage,
        error.code === "auth_required" ||
          error.code === "passkey_authentication_changed" ||
          error.code === "passkey_registration_binding_mismatch"
          ? "session-changed"
          : "rejected",
        sent ? error.status >= 500 ? "unknown" : "rejected" : "not-sent",
        error.status,
      );
    }
    if (
      error instanceof AuthResponseError || error instanceof AuthPayloadError
    ) {
      throw new AuthOperationError(
        stage,
        stage === "conversion" ? "conversion" : "invalid-response",
        sent ? "unknown" : "not-sent",
      );
    }
    throw classifyAuthFailure(error, stage, sent);
  }
}

export async function loginWithPasskey(
  signal?: AbortSignal,
): Promise<PasskeyUserResponse> {
  return await authenticateWithPasskey({
    browser_origin: window.location.origin,
  }, signal);
}

export async function loginWithLegacyPasskey(
  handle: string,
  signal?: AbortSignal,
): Promise<PasskeyUserResponse> {
  const loginHandle = handle.trim();
  if (!loginHandle) {
    throw new Error("User handle is required for legacy passkey login.");
  }
  return await authenticateWithPasskey({
    handle: loginHandle,
    browser_origin: window.location.origin,
  }, signal);
}

async function authenticateWithPasskey(
  optionsRequest: PasskeyLoginOptionsRequest,
  signal?: AbortSignal,
): Promise<PasskeyUserResponse> {
  let stage: AuthStage = "options";
  let sent = false;
  try {
    await authCheckpoint(signal, stage);
    const options = await requestJson("/api/auth/passkeys/login/options", {
      method: "POST",
      body: JSON.stringify(optionsRequest),
      signal,
    });
    const publicKey = prepareLoginOptions(options);
    const challengeId = (options as { challenge_id: string }).challenge_id;
    stage = "authenticator";
    await authCheckpoint(signal, stage);
    if (typeof navigator.credentials?.get !== "function") {
      throw new DOMException("", "NotSupportedError");
    }
    const credential = await navigator.credentials.get({ signal, publicKey });
    if (!credential) throw new DOMException("", "NotAllowedError");
    signal?.throwIfAborted();
    stage = "conversion";
    await authCheckpoint(signal, stage);
    const completeRequest: PasskeyLoginCompleteRequest = {
      challenge_id: challengeId,
      credential: authenticationCredentialToJson(credential),
    };
    stage = "before-complete";
    await authCheckpoint(signal, stage);
    stage = "complete";
    await authCheckpoint(signal, stage);
    sent = true;
    const result = parseAuthUserResponse(
      await requestJson("/api/auth/passkeys/login/complete", {
        method: "POST",
        body: JSON.stringify(completeRequest),
        signal,
      }),
    );
    signal?.throwIfAborted();
    return result;
  } catch (error) {
    if (error instanceof AuthRequestError) {
      throw new AuthOperationError(
        stage,
        error.code === "auth_required" ||
          error.code === "passkey_authentication_changed" ||
          error.code === "passkey_registration_binding_mismatch"
          ? "session-changed"
          : "rejected",
        sent ? error.status >= 500 ? "unknown" : "rejected" : "not-sent",
        error.status,
      );
    }
    if (
      error instanceof AuthResponseError || error instanceof AuthPayloadError
    ) {
      throw new AuthOperationError(
        stage,
        stage === "conversion" ? "conversion" : "invalid-response",
        sent ? "unknown" : "not-sent",
      );
    }
    throw classifyAuthFailure(error, stage, sent);
  }
}

export async function approveDeviceLogin(
  userCode: string,
  signal?: AbortSignal,
): Promise<DeviceApprovalResponse> {
  await authCheckpoint(signal, "before-complete");
  const request: DeviceLoginApproveRequest = { user_code: userCode };
  return parseDeviceApprovalResponse(
    await requestJson("/api/auth/device-login/approve", {
      method: "POST",
      body: JSON.stringify(request),
      signal,
    }),
  );
}

export async function loadPasskeys(signal?: AbortSignal) {
  return parsePasskeyList(await requestJson("/api/auth/passkeys", { signal }));
}
export async function readAdditionResult(
  request: PasskeyAdditionResultRequest,
  signal?: AbortSignal,
) {
  return parseAdditionResult(
    await requestJson("/api/auth/passkeys/addition/result", {
      method: "POST",
      body: JSON.stringify(request),
      signal,
    }),
  );
}

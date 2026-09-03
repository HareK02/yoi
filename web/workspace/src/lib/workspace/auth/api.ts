import type {
  DeviceLoginApproveRequest,
  PasskeyLoginCompleteRequest,
  PasskeyLoginOptionsRequest,
  PasskeyRegistrationCompleteRequest,
  PasskeyRegistrationOptionsRequest,
} from "$lib/generated/auth-api.ts";
import {
  authenticationCredentialToJson,
  type AuthUser,
  type DeviceApprovalResponse,
  parseAuthUserResponse,
  parseDeviceApprovalResponse,
  parseLogoutResponse,
  parseWhoamiResponse,
  prepareLoginOptions,
  prepareRegistrationOptions,
  registrationCredentialToJson,
  type WhoamiResponse,
} from "$lib/workspace/auth/model";

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
      throw new Error(
        "Invalid auth response: response body exceeds the size limit.",
      );
    }
  }
  if (response.body === null) {
    throw new Error("Invalid auth response: response body is missing.");
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
        throw new Error(
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
    throw new Error("Invalid auth response: response body is not valid JSON.");
  }
}

async function requestJson(path: string, init?: RequestInit): Promise<unknown> {
  const response = await fetch(path, {
    credentials: "same-origin",
    ...init,
    headers: {
      "content-type": "application/json",
      ...(init?.headers ?? {}),
    },
  });
  if (!response.ok) {
    await response.body?.cancel();
    throw new Error(`Auth request failed (${response.status}).`);
  }
  return await readBoundedAuthResponseJson(response);
}

export async function loadWhoami(): Promise<WhoamiResponse> {
  return parseWhoamiResponse(await requestJson("/api/auth/whoami"));
}

export async function logout(): Promise<void> {
  parseLogoutResponse(
    await requestJson("/api/auth/logout", {
      method: "POST",
      body: "{}",
    }),
  );
}

export async function registerPasskey(
  handle: string,
  displayName?: string,
): Promise<AuthUser> {
  const optionsRequest: PasskeyRegistrationOptionsRequest = {
    handle,
    display_name: displayName ?? null,
    browser_origin: window.location.origin,
  };
  const options = await requestJson("/api/auth/passkeys/registration/options", {
    method: "POST",
    body: JSON.stringify(optionsRequest),
  });
  const optionsRecord = typeof options === "object" && options !== null
    ? options as Record<string, unknown>
    : null;
  const challengeId = optionsRecord?.challenge_id;
  if (typeof challengeId !== "string" || challengeId.length === 0) {
    throw new Error(
      "Invalid auth payload: registration_options.challenge_id is required.",
    );
  }
  const credential = await navigator.credentials.create({
    publicKey: prepareRegistrationOptions(options),
  });
  if (!credential) throw new Error("Passkey registration was cancelled");

  const completeRequest: PasskeyRegistrationCompleteRequest = {
    challenge_id: challengeId,
    credential: registrationCredentialToJson(credential),
  };
  const result = parseAuthUserResponse(
    await requestJson("/api/auth/passkeys/registration/complete", {
      method: "POST",
      body: JSON.stringify(completeRequest),
    }),
  );
  return result.user;
}

export async function loginWithPasskey(handle?: string): Promise<AuthUser> {
  const optionsRequest: PasskeyLoginOptionsRequest = {
    handle: handle ?? null,
    browser_origin: window.location.origin,
  };
  const options = await requestJson("/api/auth/passkeys/login/options", {
    method: "POST",
    body: JSON.stringify(optionsRequest),
  });
  const optionsRecord = typeof options === "object" && options !== null
    ? options as Record<string, unknown>
    : null;
  const challengeId = optionsRecord?.challenge_id;
  if (typeof challengeId !== "string" || challengeId.length === 0) {
    throw new Error(
      "Invalid auth payload: login_options.challenge_id is required.",
    );
  }
  const credential = await navigator.credentials.get({
    publicKey: prepareLoginOptions(options),
  });
  if (!credential) throw new Error("Passkey login was cancelled");

  const completeRequest: PasskeyLoginCompleteRequest = {
    challenge_id: challengeId,
    credential: authenticationCredentialToJson(credential),
  };
  const result = parseAuthUserResponse(
    await requestJson("/api/auth/passkeys/login/complete", {
      method: "POST",
      body: JSON.stringify(completeRequest),
    }),
  );
  return result.user;
}

export async function approveDeviceLogin(
  userCode: string,
): Promise<DeviceApprovalResponse> {
  const request: DeviceLoginApproveRequest = { user_code: userCode };
  return parseDeviceApprovalResponse(
    await requestJson("/api/auth/device-login/approve", {
      method: "POST",
      body: JSON.stringify(request),
    }),
  );
}

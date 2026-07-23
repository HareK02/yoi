import {
  authenticationCredentialToJson,
  isPublicKeyCredential,
  prepareLoginOptions,
  prepareRegistrationOptions,
  registrationCredentialToJson,
  type DeviceApprovalResponse,
  type PasskeyLoginOptionsResponse,
  type PasskeyRegistrationOptionsResponse,
  type PasskeyUserResponse,
  type WhoamiResponse,
} from "./model";

async function jsonOrThrow<T>(response: Response): Promise<T> {
  const text = await response.text();
  if (!response.ok) {
    throw new Error(`${response.status} ${response.statusText}${text ? `: ${text}` : ""}`);
  }
  return text ? JSON.parse(text) as T : (null as T);
}

function browserOrigin(): string | null {
  return globalThis.location?.origin ?? null;
}

export async function loadWhoami(fetcher: typeof fetch = fetch): Promise<WhoamiResponse> {
  return await fetcher("/api/auth/whoami", { credentials: "same-origin" }).then(jsonOrThrow<WhoamiResponse>);
}

export async function registerPasskey(
  handle: string,
  displayName: string,
  fetcher: typeof fetch = fetch,
): Promise<PasskeyUserResponse> {
  const options = await fetcher("/api/auth/passkeys/registration/options", {
    method: "POST",
    headers: { "content-type": "application/json" },
    credentials: "same-origin",
    body: JSON.stringify({ handle, display_name: displayName, browser_origin: browserOrigin() }),
  }).then(jsonOrThrow<PasskeyRegistrationOptionsResponse>);

  const credential = await navigator.credentials.create({
    publicKey: prepareRegistrationOptions(options),
  });
  if (!isPublicKeyCredential(credential)) {
    throw new Error("Passkey registration did not return a public-key credential.");
  }

  return await fetcher("/api/auth/passkeys/registration/complete", {
    method: "POST",
    headers: { "content-type": "application/json" },
    credentials: "same-origin",
    body: JSON.stringify({
      challenge_id: options.challenge_id,
      credential: registrationCredentialToJson(credential),
    }),
  }).then(jsonOrThrow<PasskeyUserResponse>);
}

export async function loginWithPasskey(
  handle: string,
  fetcher: typeof fetch = fetch,
): Promise<PasskeyUserResponse> {
  const options = await fetcher("/api/auth/passkeys/login/options", {
    method: "POST",
    headers: { "content-type": "application/json" },
    credentials: "same-origin",
    body: JSON.stringify({ handle, browser_origin: browserOrigin() }),
  }).then(jsonOrThrow<PasskeyLoginOptionsResponse>);

  const credential = await navigator.credentials.get({
    publicKey: prepareLoginOptions(options),
  });
  if (!isPublicKeyCredential(credential)) {
    throw new Error("Passkey login did not return a public-key credential.");
  }

  return await fetcher("/api/auth/passkeys/login/complete", {
    method: "POST",
    headers: { "content-type": "application/json" },
    credentials: "same-origin",
    body: JSON.stringify({
      challenge_id: options.challenge_id,
      credential: authenticationCredentialToJson(credential),
    }),
  }).then(jsonOrThrow<PasskeyUserResponse>);
}

export async function logout(fetcher: typeof fetch = fetch): Promise<void> {
  await fetcher("/api/auth/logout", {
    method: "POST",
    credentials: "same-origin",
  }).then(jsonOrThrow<unknown>);
}

export async function approveDeviceLogin(
  userCode: string,
  fetcher: typeof fetch = fetch,
): Promise<DeviceApprovalResponse> {
  return await fetcher("/api/auth/device-login/approve", {
    method: "POST",
    headers: { "content-type": "application/json" },
    credentials: "same-origin",
    body: JSON.stringify({ user_code: userCode }),
  }).then(jsonOrThrow<DeviceApprovalResponse>);
}

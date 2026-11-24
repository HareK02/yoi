import { loadWhoami, readBoundedAuthResponseJson } from "./api.ts";
import { browserSession } from "./session-state.ts";
import { normalizeActivationHandle } from "./hosting-accounts.ts";
import {
  type AuthUser,
  parseAuthUser,
  parseAuthUserResponse,
  prepareRegistrationOptions,
  registrationCredentialToJson,
} from "./model.ts";
import type {
  PasskeyAdditionCodeResponse,
  PasskeyAdditionCodeStatusResponse,
} from "#lib/generated/auth-api.ts";

export type CodeStatus = PasskeyAdditionCodeStatusResponse;
export type IssuedCode = PasskeyAdditionCodeResponse;
type Owner = Pick<AuthUser, "user_id" | "account_id">;
export class AdditionCodeError extends Error {
  constructor(
    readonly kind:
      | "recent-auth"
      | "rejected"
      | "unconfirmed"
      | "session-changed",
  ) {
    super(
      kind === "recent-auth"
        ? "Verify your identity with a passkey, then generate the code."
        : kind === "session-changed"
        ? "The signed-in account or session changed. Refresh before continuing."
        : kind === "rejected"
        ? "The request was rejected. Check the code, account ID and destination, or generate a new code."
        : "The result could not be confirmed. Check status or try passkey login before starting again.",
    );
  }
}
function record(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error();
  }
  return value as Record<string, unknown>;
}
function text(value: unknown, limit = 4096): string {
  if (typeof value !== "string" || !value || value.length > limit) {
    throw new Error();
  }
  return value;
}
function origin(value: unknown): string {
  const raw = text(value);
  const parsed = new URL(raw);
  if (parsed.origin !== raw || !["https:", "http:"].includes(parsed.protocol)) {
    throw new Error();
  }
  return raw;
}
function metadata(value: unknown) {
  const r = record(value);
  const expires_at = text(r.expires_at, 128);
  if (!Number.isFinite(Date.parse(expires_at))) throw new Error();
  return {
    issuance_id: text(r.issuance_id, 256),
    target_origin: origin(r.target_origin),
    expires_at,
  };
}
function sameOwner(a: Owner, b: Owner) {
  return a.user_id === b.user_id && a.account_id === b.account_id;
}
function guard(signal: AbortSignal) {
  const generation = browserSession.authenticationGeneration;
  const stamp = browserSession.authenticationStamp;
  return () => {
    signal.throwIfAborted();
    if (
      generation !== browserSession.authenticationGeneration ||
      stamp !== browserSession.authenticationStamp
    ) throw new AdditionCodeError("session-changed");
  };
}
async function request(
  path: string,
  signal: AbortSignal,
  body?: unknown,
): Promise<unknown> {
  const response = await fetch(path, {
    method: body === undefined ? "GET" : "POST",
    credentials: "same-origin",
    mode: "same-origin",
    redirect: "error",
    cache: "no-store",
    referrerPolicy: "no-referrer",
    signal,
    headers: body === undefined
      ? undefined
      : { "content-type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (!response.ok) {
    let recent = false;
    try {
      const value = record(await readBoundedAuthResponseJson(response));
      recent = Array.isArray(value.diagnostics) &&
        value.diagnostics.some((d: unknown) =>
          record(d).code === "passkey_recent_authentication_required"
        );
    } catch {
      /* Never expose secret-bearing response bodies or transport errors. */
    }
    throw new AdditionCodeError(
      recent
        ? "recent-auth"
        : response.status >= 500
        ? "unconfirmed"
        : "rejected",
    );
  }
  return await readBoundedAuthResponseJson(response);
}
async function owned<T>(
  owner: Owner,
  signal: AbortSignal,
  operation: (
    binding: {
      expected_user_id: string;
      expected_account_id: string;
      expected_authentication_reference: string;
    },
  ) => Promise<T>,
): Promise<T> {
  const check = guard(signal);
  try {
    const before = await loadWhoami(signal);
    check();
    if (
      !before.actor || !before.authentication_reference ||
      before.actor.auth_method !== "browser_session" ||
      !sameOwner(before.actor, owner)
    ) throw new AdditionCodeError("session-changed");
    const result = await operation({
      expected_user_id: owner.user_id,
      expected_account_id: owner.account_id,
      expected_authentication_reference: before.authentication_reference,
    });
    check();
    const after = await loadWhoami(signal);
    check();
    if (
      !after.actor || !sameOwner(after.actor, owner) ||
      after.authentication_reference !== before.authentication_reference
    ) throw new AdditionCodeError("session-changed");
    return result;
  } catch (error) {
    throw error instanceof AdditionCodeError
      ? error
      : new AdditionCodeError("unconfirmed");
  }
}
export async function loadAdditionCodeStatus(
  owner: Owner,
  signal: AbortSignal,
): Promise<CodeStatus> {
  return await owned(owner, signal, async () => {
    const r = record(await request("/api/auth/passkeys/addition-code", signal));
    const user = parseAuthUser(r.user);
    if (
      !sameOwner(user, owner) || !Array.isArray(r.public_urls) ||
      r.public_urls.length > 100
    ) throw new Error();
    const public_urls = r.public_urls.map(origin);
    const active = r.active === null ? null : metadata(r.active);
    return { user, public_urls, active };
  });
}
export async function issueAdditionCode(
  owner: Owner,
  target: string,
  signal: AbortSignal,
): Promise<IssuedCode> {
  return await owned(owner, signal, async (binding) => {
    const r = record(
      await request("/api/auth/passkeys/addition-code", signal, {
        ...binding,
        target_origin: origin(target),
      }),
    );
    const user = parseAuthUser(r.user);
    const meta = metadata(r);
    if (!sameOwner(user, owner) || meta.target_origin !== target) {
      throw new Error();
    }
    return { user, ...meta, code: text(r.code) };
  });
}
export async function revokeAdditionCode(
  owner: Owner,
  issuance_id: string,
  signal: AbortSignal,
): Promise<boolean> {
  return await owned(owner, signal, async (binding) => {
    const r = record(
      await request("/api/auth/passkeys/addition-code/revoke", signal, {
        ...binding,
        issuance_id,
      }),
    );
    if (typeof r.revoked !== "boolean") throw new Error();
    return r.revoked;
  });
}

// The code is sent only to enrollment options, never as normal authentication or in a URL.
// A completed credential may survive response loss: do not automatically replay enrollment.
export async function redeemAdditionCode(
  handle: string,
  code: string,
  signal: AbortSignal,
): Promise<AuthUser> {
  const check = guard(signal);
  let completionSent = false;
  try {
    handle = normalizeActivationHandle(handle);
    if (window.isSecureContext !== true || !code || code.length > 4096) {
      throw new Error();
    }
    check();
    const options = await request(
      "/api/auth/passkeys/addition-code/options",
      signal,
      { handle, code, browser_origin: window.location.origin },
    );
    code = "";
    check();
    const publicKey = prepareRegistrationOptions(options);
    const challenge_id = text(record(options).challenge_id, 256);
    const credential = await navigator.credentials.create({
      publicKey,
      signal,
    });
    if (!credential) throw new Error();
    check();
    const body = {
      challenge_id,
      credential: registrationCredentialToJson(credential),
    };
    completionSent = true;
    const result = parseAuthUserResponse(
      await request("/api/auth/passkeys/addition-code/complete", signal, body),
    );
    check();
    if (result.user.handle !== handle) throw new Error();
    const current = await loadWhoami(signal);
    check();
    if (
      current.session.kind !== "authenticated" ||
      current.actor?.auth_method !== "browser_session" ||
      !sameOwner(current.actor, result.user) ||
      current.authentication_reference !== result.authentication_reference
    ) throw new Error();
    return result.user;
  } catch (error) {
    if (error instanceof AdditionCodeError) throw error;
    throw new AdditionCodeError(completionSent ? "unconfirmed" : "rejected");
  } finally {
    code = "";
  }
}

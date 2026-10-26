import type {
  PasskeyAdditionOptionsRequest,
  PasskeyRegistrationCompleteRequest,
} from "#lib/generated/auth-api.ts";
import { readBoundedAuthResponseJson } from "./api.ts";
import {
  type AuthUser,
  parseAuthUserResponse,
  parseWhoamiResponse,
  prepareRegistrationOptions,
  registrationCredentialToJson,
} from "./model.ts";

// Host発行secretはこの操作のmemoryだけに置く。汎用API clientや永続sessionへ渡さない。
// Cookieが既にあっても、本人の選択はServerがBearerから導出する。
export async function enrollHostingPasskey(
  token: string,
  signal: AbortSignal,
): Promise<AuthUser> {
  try {
    // WebAuthnが使えない非secure contextへsecretを送る前に拒否する。
    if (window.isSecureContext !== true) {
      throw new Error("Secure context required");
    }
    if (!/^[A-Za-z0-9._~-]{1,4096}$/.test(token)) {
      throw new Error("Invalid setup token");
    }
    const request = async (
      path:
        | "/api/auth/whoami"
        | "/api/auth/passkeys/addition/options"
        | "/api/auth/passkeys/addition/complete",
      body?: PasskeyAdditionOptionsRequest | PasskeyRegistrationCompleteRequest,
    ): Promise<unknown> => {
      signal.throwIfAborted();
      const response = await fetch(path, {
        method: path === "/api/auth/whoami" ? "GET" : "POST",
        // Bearerの失効を既存cookieで補わない。設定用tokenだけで本人を決め、
        // completionのcookieも受け取らず、次の明示Passkey loginでsessionを作る。
        credentials: "omit",
        mode: "same-origin",
        redirect: "error",
        cache: "no-store",
        referrerPolicy: "no-referrer",
        signal,
        headers: {
          "content-type": "application/json",
          authorization: `Bearer ${token}`,
        },
        body: JSON.stringify(body),
      });
      if (!response.ok) {
        await response.body?.cancel();
        throw new Error("Setup request rejected");
      }
      return await readBoundedAuthResponseJson(response);
    };
    const identity = parseWhoamiResponse(await request("/api/auth/whoami"));
    if (!identity.actor || !identity.authentication_reference || identity.actor.auth_method !== "api_token") throw new Error("Setup identity unconfirmed");
    const options = await request("/api/auth/passkeys/addition/options", {
      expected_user_id: identity.actor.user_id,
      expected_account_id: identity.actor.account_id,
      expected_authentication_reference: identity.authentication_reference,
      browser_origin: window.location.origin,
    });
    const publicKey = prepareRegistrationOptions(options);
    // prepareRegistrationOptionsでchallenge_idも含めて検証済み。
    const challengeId = (options as { challenge_id: string }).challenge_id;
    signal.throwIfAborted();
    const credential = await navigator.credentials.create({
      publicKey,
      signal,
    });
    if (!credential) throw new Error("Credential creation cancelled");
    signal.throwIfAborted();
    const result = await request("/api/auth/passkeys/addition/complete", {
      challenge_id: challengeId,
      credential: registrationCredentialToJson(credential),
    });
    return parseAuthUserResponse(result).user;
  } catch {
    // fetch/WebAuthn/responseの例外にはsecretを含む可能性があるため転載しない。
    throw new Error("Passkey enrollment could not be confirmed.");
  } finally {
    token = "";
  }
}

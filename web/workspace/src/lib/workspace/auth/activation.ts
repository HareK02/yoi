import type {
  ActivationOptionsRequest,
  PasskeyRegistrationCompleteRequest,
} from "#lib/generated/auth-api.ts";
import { loadWhoami, readBoundedAuthResponseJson } from "./api.ts";
import { browserSession } from "./session-state.ts";
import { normalizeActivationHandle } from "./hosting-accounts.ts";
import {
  type AuthUser,
  parseAuthUserResponse,
  prepareRegistrationOptions,
  registrationCredentialToJson,
} from "./model.ts";

// 一時codeは通常auth clientや本人の鍵追加APIへ渡さない。options取得はsessionではない。
// 再開はID+codeの再入力から行い、ceremony/codeをURL・storageへ保存しない。
// Cookieが存在しても対象はServerがcodeから決め、complete成功時だけ新sessionを受け取る。
export async function activateWithPasskey(
  handle: string,
  code: string,
  signal: AbortSignal,
): Promise<AuthUser> {
  const generation = browserSession.authenticationGeneration;
  const stamp = browserSession.authenticationStamp;
  // 別tabのstorage event配送より先にcredential/応答が返っても送信・適用しない。
  const assertCurrent = () => {
    signal.throwIfAborted();
    if (
      generation !== browserSession.authenticationGeneration ||
      stamp !== browserSession.authenticationStamp
    ) throw new Error("Activation superseded");
  };
  try {
    handle = normalizeActivationHandle(handle);
    if (window.isSecureContext !== true || !code || code.length > 4096) {
      throw new Error("Activation unavailable");
    }
    const request = async (
      path: "/api/auth/activation/options" | "/api/auth/activation/complete",
      body: ActivationOptionsRequest | PasskeyRegistrationCompleteRequest,
    ): Promise<unknown> => {
      assertCurrent();
      const response = await fetch(path, {
        method: "POST",
        credentials: "same-origin",
        mode: "same-origin",
        redirect: "error",
        cache: "no-store",
        referrerPolicy: "no-referrer",
        signal,
        headers: { "content-type": "application/json" },
        body: JSON.stringify(body),
      });
      if (!response.ok) {
        await response.body?.cancel();
        throw new Error("Activation rejected");
      }
      const result = await readBoundedAuthResponseJson(response);
      assertCurrent();
      return result;
    };
    const options = await request("/api/auth/activation/options", {
      handle,
      code,
      browser_origin: window.location.origin,
    });
    code = "";
    const publicKey = prepareRegistrationOptions(options);
    const challengeId = (options as { challenge_id: string }).challenge_id;
    assertCurrent();
    const credential = await navigator.credentials.create({
      publicKey,
      signal,
    });
    if (!credential) throw new Error("Registration cancelled");
    assertCurrent();
    const result = parseAuthUserResponse(
      await request("/api/auth/activation/complete", {
        challenge_id: challengeId,
        credential: registrationCredentialToJson(credential),
      }),
    );
    assertCurrent();
    if (result.user.handle !== handle) {
      throw new Error("Activation target mismatch");
    }
    const current = await loadWhoami(signal);
    assertCurrent();
    if (
      current.session.kind !== "authenticated" ||
      current.actor?.auth_method !== "browser_session" ||
      current.actor.user_id !== result.user.user_id ||
      current.actor.account_id !== result.user.account_id ||
      current.authentication_reference !== result.authentication_reference
    ) throw new Error("Activation session unconfirmed");
    return result.user;
  } catch {
    // HTTP/WebAuthn/JSON例外はsecretを含む可能性があるため、本文も例外も転載しない。
    throw new Error("Activation could not be confirmed.");
  } finally {
    code = "";
  }
}

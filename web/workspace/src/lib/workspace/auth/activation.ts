import type {
  ActivationOptionsRequest,
  PasskeyRegistrationCompleteRequest,
} from "#lib/generated/auth-api.ts";
import { readBoundedAuthResponseJson } from "./api.ts";
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
  try {
    handle = normalizeActivationHandle(handle);
    if (window.isSecureContext !== true || !code || code.length > 4096) {
      throw new Error("Activation unavailable");
    }
    const request = async (
      path: "/api/auth/activation/options" | "/api/auth/activation/complete",
      body: ActivationOptionsRequest | PasskeyRegistrationCompleteRequest,
    ): Promise<unknown> => {
      signal.throwIfAborted();
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
      return await readBoundedAuthResponseJson(response);
    };
    const options = await request("/api/auth/activation/options", {
      handle,
      code,
      browser_origin: window.location.origin,
    });
    code = "";
    const publicKey = prepareRegistrationOptions(options);
    const challengeId = (options as { challenge_id: string }).challenge_id;
    signal.throwIfAborted();
    const credential = await navigator.credentials.create({
      publicKey,
      signal,
    });
    if (!credential) throw new Error("Registration cancelled");
    signal.throwIfAborted();
    const result = parseAuthUserResponse(
      await request("/api/auth/activation/complete", {
        challenge_id: challengeId,
        credential: registrationCredentialToJson(credential),
      }),
    ).user;
    signal.throwIfAborted();
    if (result.handle !== handle) throw new Error("Activation target mismatch");
    return result;
  } catch {
    // HTTP/WebAuthn/JSON例外はsecretを含む可能性があるため、本文も例外も転載しない。
    throw new Error("Activation could not be confirmed.");
  } finally {
    code = "";
  }
}

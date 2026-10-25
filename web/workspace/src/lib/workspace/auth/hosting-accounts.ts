import type { ActivationCodeResponse } from "#lib/generated/auth-api.ts";
import { readBoundedAuthResponseJson } from "./api.ts";
import { HostingRequestError } from "./hosting.ts";
import { parseAuthUser } from "./model.ts";

// Serverのnormalize_handleと同じASCII正規化。Unicode case foldingはしない。
export function normalizeActivationHandle(value: string): string {
  const handle = value.trim().replace(
    /[A-Z]/g,
    (letter) => letter.toLowerCase(),
  );
  if (!/^[a-z0-9_-]{1,64}$/.test(handle)) {
    throw new Error("Invalid account ID.");
  }
  return handle;
}

// 管理者は新規IDを作成して、発行結果のcodeと/activateの場所を本人へ別々に渡す。
// 再発行は仮accountにだけ行い、旧code/ceremonyは失効する。応答喪失時は自動再送せず、
// 本人のPasskey login確認後に必要なら明示再発行する。codeは発行結果のmemoryだけで扱う。
export async function issueActivationCode(
  handle: string,
  reissue: boolean,
  signal: AbortSignal,
): Promise<ActivationCodeResponse> {
  try {
    handle = normalizeActivationHandle(handle);
    const response = await fetch(
      reissue
        ? "/api/hosting/accounts/activation-code"
        : "/api/hosting/accounts",
      {
        method: "POST",
        credentials: "same-origin",
        mode: "same-origin",
        redirect: "error",
        cache: "no-store",
        referrerPolicy: "no-referrer",
        signal,
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ handle }),
      },
    );
    if (!response.ok) {
      await response.body?.cancel();
      throw new HostingRequestError(response.status);
    }
    const value = await readBoundedAuthResponseJson(response);
    if (!value || typeof value !== "object" || Array.isArray(value)) {
      throw new Error();
    }
    const record = value as Record<string, unknown>;
    if (
      Object.keys(record).sort().join(",") !== "code,expires_at,user" ||
      typeof record.code !== "string" || !record.code ||
      record.code.length > 4096 ||
      typeof record.expires_at !== "string" ||
      !Number.isFinite(Date.parse(record.expires_at))
    ) throw new Error();
    const user = parseAuthUser(record.user);
    if (user.handle !== handle) throw new Error();
    signal.throwIfAborted();
    return { user, code: record.code, expires_at: record.expires_at };
  } catch (error) {
    if (error instanceof HostingRequestError) throw error;
    // HTTP本文、JSON/transport元例外には秘密が含まれ得る。
    throw new Error("Code issuance could not be confirmed.");
  }
}

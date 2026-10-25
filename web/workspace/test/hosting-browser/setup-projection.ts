// Setupの表示条件だけを代替する。Bearerやrequest bodyは読まず、記録もしない。
// WebAuthnは固定の非暗号credentialを返すため、本物の認証・署名検証ではない。
export const setupWebAuthnProjection = `
(() => {
  const bytes = () => new Uint8Array([117, 105]).buffer;
  const core = () => ({ id: 'ui-projection-only', rawId: bytes(), type: 'public-key', getClientExtensionResults: () => ({}) });
  Object.defineProperty(navigator.credentials, 'create', { configurable: true, value: async () => ({ ...core(), response: { clientDataJSON: bytes(), attestationObject: bytes(), getTransports: () => ['internal'] } }) });
  Object.defineProperty(navigator.credentials, 'get', { configurable: true, value: async () => ({ ...core(), response: { clientDataJSON: bytes(), authenticatorData: bytes(), signature: bytes(), userHandle: null } }) });
})();
`;
const user = {
  user_id: "fixture-user",
  account_id: "fixture-account",
  handle: "alice",
  display_name: "Alice",
};
const stages = new Map<string, "enrolled" | "login">();
export function setupProjectionResponse(
  path: string,
  mode: string,
  reset: boolean,
  tokenRequest = false,
): Response | null {
  if (!mode.startsWith("setup-")) return null;
  if (reset) stages.delete(mode);
  if (path === "/__ui_fixture/webauthn.js") {
    return new Response(setupWebAuthnProjection, {
      headers: { "content-type": "text/javascript" },
    });
  }
  if (path === "/api/auth/whoami") {
    if (tokenRequest) {
      return Response.json({
        actor: { ...user, auth_method: "api_token" },
        authentication_reference: "fixture-token-binding",
        session: { kind: "authenticated", expires_at: "2099-01-01T00:00:00Z" },
      });
    }
    return Response.json({
      authentication_reference: stages.get(mode) === "login"
        ? "fixture-authentication"
        : null,
      session: stages.get(mode) === "login"
        ? { kind: "authenticated", expires_at: "2099-01-01T00:00:00Z" }
        : { kind: "anonymous" },
      actor: stages.get(mode) === "login"
        ? { ...user, auth_method: "browser_session" }
        : null,
    });
  }
  if (path === "/api/hosting/access") {
    return Response.json({ is_admin: stages.get(mode) === "login" });
  }
  if (path === "/api/auth/passkeys/addition/options") {
    if (mode === "setup-reject") {
      return new Response("synthetic enrollment rejection", { status: 403 });
    }
    return Response.json({
      challenge_id: "fixture-add",
      public_key: {
        challenge: "dWk",
        rp: { name: "UI projection" },
        user: { id: "dWk", name: "alice", displayName: "Alice" },
        pubKeyCredParams: [{ type: "public-key", alg: -7 }],
      },
    });
  }
  if (path === "/api/auth/passkeys/addition/complete") {
    stages.set(mode, "enrolled");
    return Response.json({
      user,
      authentication_reference: "fixture-authentication",
    });
  }
  if (path === "/api/auth/passkeys/login/options") {
    return Response.json({
      challenge_id: "fixture-login",
      public_key: { challenge: "dWk" },
    });
  }
  if (path === "/api/auth/passkeys/login/complete") {
    stages.set(mode, "login");
    return Response.json({
      user,
      authentication_reference: "fixture-authentication",
    });
  }
  return null;
}

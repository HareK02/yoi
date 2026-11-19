/** Synthetic signed-in shell state only; not evidence of Backend authentication. */
export function authenticatedFixtureResponse(request: Request): Response | null {
  if (request.method !== "GET") return null;
  const path = new URL(request.url).pathname;
  if (path === "/api/auth/whoami") {
    return Response.json({
      authentication_reference: "fixture-authentication",
      session: { kind: "authenticated", expires_at: "2099-01-01T00:00:00Z" },
      actor: {
        user_id: "fixture-user",
        account_id: "fixture-owner",
        handle: "alice",
        display_name: "Alice",
        auth_method: "browser_session",
      },
    });
  }
  if (path === "/api/hosting/access") return Response.json({ is_admin: false });
  return null;
}

declare const Deno: { test(name: string, run: () => void): void };
function assertEquals(
  actual: string,
  expected: string,
  context?: string,
): void {
  if (actual !== expected) {
    throw new Error(
      `${context ?? "return destination"}: expected ${expected}, got ${actual}`,
    );
  }
}
import { authEntryUrl, loginUrl, safeReturnTo } from "./routes.ts";

Deno.test("return destination permits known app screens without forwarding arbitrary query or fragment", () => {
  for (
    const [input, expected] of [
      ["/", "/"],
      ["/account", "/account"],
      ["/account?returnTo=//evil.invalid#secret", "/account"],
      [
        "/w/workspace-id/tickets/T-123?filter=all#entry",
        "/w/workspace-id/tickets/T-123",
      ],
      [
        "/login/device?user_code=ABCD-EFGH",
        "/login/device?user_code=ABCD-EFGH",
      ],
      [
        "/login/device?code=abcd-1234&returnTo=//evil.invalid",
        "/login/device?user_code=abcd-1234",
      ],
      ["/login/device?user_code=%252f%252fevil.invalid", "/login/device"],
    ]
  ) assertEquals(safeReturnTo(input), expected, input);
});

Deno.test("return destination rejects external, ambiguous encoding and authentication loops", () => {
  for (
    const input of [
      null,
      "",
      "https://evil.invalid",
      "//evil.invalid",
      "///evil.invalid",
      "javascript:alert(1)",
      "account",
      "/\\evil.invalid",
      "/%2f%2fevil.invalid",
      "/%252f%252fevil.invalid",
      "/%61ccount",
      "/login",
      "/register",
      "/activate",
      "/login/",
      "/login?returnTo=/account",
      "/login/device/",
      "/login/../account",
      "/w/a/../../login",
      "/w/a//evil",
      "/w/a/./b",
      "/account\n",
      "/account\t",
      "/account\u0000",
      "/account%00",
      "/api/auth/logout",
      "/hosting/setup",
      "/not-an-app-route",
      "/" + "a".repeat(2049),
    ]
  ) assertEquals(safeReturnTo(input), "/", String(input));
});

Deno.test("login links encode one validated destination and discard nested auth loops", () => {
  assertEquals(
    loginUrl("/login/device?user_code=ABCD-EFGH"),
    "/login?returnTo=%2Flogin%2Fdevice%3Fuser_code%3DABCD-EFGH",
  );
  assertEquals(
    authEntryUrl("/register", "/account"),
    "/register?returnTo=%2Faccount",
  );
  assertEquals(loginUrl("/login?returnTo=/login"), "/login?returnTo=%2F");
});

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
import {
  authEntryUrl,
  isAuthPage,
  isPublicPage,
  loginUrl,
  safeReturnTo,
} from "./routes.ts";

Deno.test("account uses the authenticated app shell, not the standalone auth entry shell", () => {
  assertEquals(String(isAuthPage("/account")), "false");
  assertEquals(String(isPublicPage("/account")), "false");
  for (const path of ["/login", "/register", "/activate", "/login/device"]) {
    assertEquals(String(isAuthPage(path)), "true", path);
    assertEquals(String(isPublicPage(path)), "true", path);
  }
});

Deno.test("return destination preserves query and fragment only on known app screens", () => {
  for (
    const [input, expected] of [
      ["/", "/dashboard"],
      ["/dashboard?query=keep#fragment", "/dashboard?query=keep#fragment"],
      ["/account", "/account"],
      ["/account?returnTo=//evil.invalid#secret", "/account"],
      [
        "/w/workspace-id/tickets/T-123?filter=all#entry",
        "/w/workspace-id/tickets/T-123?filter=all#entry",
      ],
      [
        "/login/device?user_code=ABCD-EFGH",
        "/login/device?user_code=ABCD-EFGH",
      ],
      [
        "/login/device?code=abcd-1234&returnTo=//evil.invalid",
        "/login/device?user_code=abcd-1234",
      ],
      ["/login/device?user_code=%252f%252fevil.invalid", "/dashboard"],
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
      "/account?q=%255c",
      "/account?q=%0A",
      "/account#%5c",
      "/account?q=%ZZ",
      "/account?q=%252f%252fevil.invalid",
      "/api/auth/logout",
      "/hosting/setup",
      "/not-an-app-route",
      "/" + "a".repeat(2049),
    ]
  ) assertEquals(safeReturnTo(input), "/dashboard", String(input));
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
  assertEquals(
    loginUrl("/login?returnTo=/login"),
    "/login?returnTo=%2Fdashboard",
  );
});

Deno.test("return destination accepts canonical UTF-8 resource slugs but not encoded ASCII or delimiters", () => {
  const target = `/w/team/tickets/T-123-${
    encodeURIComponent("未ログイン")
  }?view=comments#entry-2`;
  assertEquals(safeReturnTo(target), target);
  for (
    const [input, expected] of [
      [
        "/w/team/tickets?filter=100%25#entry",
        "/w/team/tickets?filter=100%25#entry",
      ],
      [
        "/w/team/tickets/%e6%9c%aa?filter=100%25#part%25",
        "/w/team/tickets/%e6%9c%aa?filter=100%25#part%25",
      ],
      [
        "/w/team/tickets/未?query=a%2fb#entry%2f2",
        "/w/team/tickets/%E6%9C%AA?query=a%2fb#entry%2f2",
      ],
    ]
  ) {
    assertEquals(safeReturnTo(input), expected);
  }
  for (
    const input of [
      "/w/team/tickets/%54-123",
      "/w/team/tickets/T-123%2fother",
      "/w/team/tickets/%E0%80%AF",
      "/w/team/tickets/%252f",
    ]
  ) {
    assertEquals(safeReturnTo(input), "/dashboard");
  }
});

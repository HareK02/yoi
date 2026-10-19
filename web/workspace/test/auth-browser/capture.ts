// shared captureがscreenshot/snapshot/review-contextを採取する。合否と本人目視は呼出元が行う。
// 本番buildに合成HTTP/WebAuthnだけを接続し、撮影専用CSSや認証情報は使わない。
import { resolve } from "@std/path";
import { capture } from "../../../../tools/web-ux/src/capture.ts";
import type {
  Interaction,
  RouteScenario,
  Scenario,
} from "../../../../tools/web-ux/src/types.ts";

const root = resolve(import.meta.dirname!, "../../../..");
const phase = Deno.args[0] ?? "after";
const coverage = Deno.args[1] ?? "all";
if (coverage !== "all" && coverage !== "session") {
  throw new Error("Expected capture coverage: all or session");
}
if (!/^[a-zA-Z0-9_-]+$/.test(phase)) {
  throw new Error("Expected safe capture label");
}
const ready = (selector: string) => ({ kind: "selector" as const, selector });
const click = (selector: string): Interaction => ({
  action: "click",
  selector,
});
const fill = (selector: string, value: string): Interaction => ({
  action: "fill",
  selector,
  value,
});
const id = 'label:has-text("ID (handle)") input';
const loginButton = 'button:text-is("Log in with passkey")';
const login = [click(loginButton)];
const legacyLogin = [
  click('summary:text-is("Use an older key with account ID")'),
  fill(id, "alice"),
  click('button:text-is("Log in with older key")'),
];
const register = [
  fill(id, "alice"),
  fill('label:has-text("Display name") input', "Alice"),
  click('button:text-is("Register new account")'),
];
const route = (
  id: string,
  path: string,
  selector: string,
  capturePoints: RouteScenario["capturePoints"] = [{
    id: "initial",
    label: id,
    fullPage: true,
  }],
): RouteScenario => ({
  id,
  label: id,
  path,
  goal:
    "Review standalone production auth UI, navigation, focus, state and copy; not Server authorization.",
  dataState: `Synthetic ${id}; no real session, credential or activation code.`,
  ready: ready(selector),
  capturePoints,
});
const routes: RouteScenario[] = [
  route("login", "/login?fixture=anonymous", `${loginButton}:enabled`, [
    {
      id: "initial",
      label: "ID-free primary login; explicit older-key disclosure",
      fullPage: true,
    },
    {
      id: "keyboard-focus",
      label: "Login keyboard focus",
      fullPage: true,
      interaction: [{ action: "press", selector: loginButton, key: "Tab" }, {
        action: "press",
        selector: 'summary:text-is("Use an older key with account ID")',
        key: "Shift+Tab",
      }],
      ready: ready('button:text-is("Log in with passkey"):focus-visible'),
    },
    {
      id: "legacy-expanded",
      label: "Explicit old-key compatibility and safe migration guidance",
      fullPage: true,
      interaction: [
        click('summary:text-is("Use an older key with account ID")'),
      ],
      ready: ready(`${id}:visible`),
    },
  ]),
  route(
    "account-redirect",
    "/account?fixture=anonymous",
    'h1:text-is("Yoi")',
  ),
  route(
    "expired-account",
    "/account?fixture=session-expired&tab=keys#passkeys",
    '[role="status"]:has-text("Your browser session expired")',
  ),
  route(
    "expired-login",
    "/login?fixture=session-expired-cancel-once&returnTo=%2Faccount%3Ftab%3Dkeys%23passkeys",
    '[role="status"]:has-text("Your browser session expired")',
    [
      {
        id: "initial",
        label: "Explicit expiry and login continuation, not anonymous entry",
        fullPage: true,
      },
      {
        id: "cancelled",
        label: "Cancelled passkey keeps expiry context and retry destination",
        fullPage: true,
        interaction: login,
        ready: ready('[role="alert"]:has-text("cancelled")'),
      },
      {
        id: "returned",
        label:
          "Retry confirms authentication before returning to the requested account",
        fullPage: true,
        interaction: login,
        ready: ready('button:text-is("Add passkey to alice")'),
      },
    ],
  ),
  route("register", "/register?fixture=anonymous", `${id}:enabled`, [
    {
      id: "initial",
      label: "ID and distinct display name; new account only",
      fullPage: true,
    },
    {
      id: "keyboard-focus",
      label: "Register keyboard focus",
      fullPage: true,
      interaction: [
        fill(id, "alice"),
        fill('label:has-text("Display name") input', "Alice"),
        {
          action: "press",
          selector: 'label:has-text("Display name") input',
          key: "Tab",
        },
      ],
      ready: ready('button:text-is("Register new account"):focus-visible'),
    },
  ]),
  route(
    "account",
    "/account?fixture=account",
    'button:text-is("Add passkey to alice")',
    [
      {
        id: "initial",
        label: "Own account only; no target handle field",
        fullPage: true,
      },
      {
        id: "addition-confirmed",
        label: "Own Passkey addition confirmed",
        fullPage: true,
        interaction: [click('button:text-is("Add passkey to alice")')],
        ready: ready('[role="status"]:has-text("Passkey added")'),
      },
      {
        id: "logout",
        label: "Log out forgets expiry and returns to public home",
        fullPage: true,
        interaction: [click('button:text-is("Log out")')],
        ready: ready('h1:text-is("Yoi")'),
      },
    ],
  ),
  route(
    "device-anonymous",
    "/login/device?user_code=ABCD-EFGH&fixture=anonymous",
    'a:text-is("Log in to continue")',
    [
      {
        id: "initial",
        label: "User code and login continuation; no implicit approval",
        fullPage: true,
      },
      {
        id: "login",
        label: "Dedicated login with device returnTo",
        fullPage: true,
        interaction: [click('a:text-is("Log in to continue")')],
        ready: ready(`${loginButton}:enabled`),
      },
      {
        id: "authenticated-unapproved",
        label: "Login complete; explicit approval still required",
        fullPage: true,
        interaction: login,
        ready: ready('button:text-is("Approve device login"):enabled'),
      },
      {
        id: "approved",
        label: "Separate explicit approval confirmed",
        fullPage: true,
        interaction: [click('button:text-is("Approve device login")')],
        ready: ready(
          'text="Device login approved. You can return to the CLI."',
        ),
      },
    ],
  ),
  route(
    "session-loading",
    "/login?fixture=session-loading",
    'text="Loading session…"',
  ),
  route(
    "session-error",
    "/login?fixture=session-error",
    'button:text-is("Retry session")',
  ),
  route(
    "account-session-error",
    "/account?fixture=session-error",
    "text=/Login status is unknown/",
  ),
  route(
    "register-session-error",
    "/register?fixture=session-error",
    'button:text-is("Retry session")',
  ),
  route(
    "device-session-error",
    "/login/device?user_code=ABCD-EFGH&fixture=session-error",
    'button:text-is("Retry session")',
  ),
  route(
    "policy-disabled",
    "/register?fixture=policy-disabled",
    'text="New account registration is disabled. Existing users can still log in."',
  ),
  route("policy-error", "/register?fixture=policy-error", '[role="alert"]'),
  route("policy-stopped", "/register?fixture=policy-stopped", `${id}:enabled`, [
    {
      id: "completion-403",
      label:
        "Policy refreshed to disabled after intentional registration HTTP403",
      fullPage: true,
      interaction: register,
      ready: ready(
        'text="New account registration is disabled. Existing users can still log in."',
      ),
    },
  ]),
  ...["login", "registration", "addition"].flatMap((purpose) =>
    ["pending", "cancel", "unsupported", "error", "expired"].map((state) => {
      const path = purpose === "registration"
        ? "/register"
        : purpose === "addition"
        ? "/account"
        : "/login";
      const interaction = purpose === "login"
        ? login
        : purpose === "registration"
        ? register
        : [click('button:text-is("Add passkey to alice")')];
      return route(
        `${purpose}-${state}`,
        `${path}?fixture=${purpose}-${state}`,
        purpose === "addition"
          ? 'button:text-is("Add passkey to alice")'
          : purpose === "login"
          ? `${loginButton}:enabled`
          : `${id}:enabled`,
        [{
          id: "result",
          label: `${purpose} ${state}; not a successful ceremony`,
          fullPage: true,
          interaction,
          ready: ready(
            state === "pending" ? '[role="status"]' : '[role="alert"]',
          ),
        }],
      );
    })
  ),
  ...["pending", "cancel", "unsupported", "error", "expired"].map((state) =>
    route(
      `legacy-login-${state}`,
      `/login?fixture=login-${state}`,
      `${loginButton}:enabled`,
      [{
        id: "result",
        label:
          `Explicit compatibility ${state}; not confirmed login or enrollment`,
        fullPage: true,
        interaction: legacyLogin,
        ready: ready(
          state === "pending" ? '[role="status"]' : '[role="alert"]',
        ),
      }],
    )
  ),
  route(
    "activation-entry",
    "/login?fixture=activation-pending",
    'a:text-is("Activate an account with a code")',
    [
      {
        id: "entry",
        label: "Existing activation route from login",
        fullPage: true,
        interaction: [click('a:text-is("Activate an account with a code")')],
        ready: ready("#activation-code:enabled"),
      },
      {
        id: "limited",
        label:
          "No ordinary session or device approval before Passkey completion",
        fullPage: true,
        interaction: [
          fill("#activation-handle", "alice"),
          fill("#activation-code", "synthetic-input-only"),
          click('button:text-is("Register Passkey and activate")'),
        ],
        ready: ready('button:text-is("Cancel registration")'),
      },
      {
        id: "cancelled",
        label: "Activation cancellation and login recovery entry",
        fullPage: true,
        interaction: [click('button:text-is("Cancel registration")')],
        ready: ready('[role="alert"]'),
      },
    ],
  ),
];
const sessionCaptureIds = new Set([
  "login",
  "account-redirect",
  "expired-account",
  "expired-login",
  "session-error",
  "account-session-error",
]);
await Deno.mkdir(`${root}/target/web-ux`, { recursive: true });
for (const colorScheme of ["light", "dark"] as const) {
  for (const width of [1440, 768, 390]) {
    const scenario: Scenario = {
      schemaVersion: 1,
      id: `auth-${colorScheme}-${width}`,
      title:
        "Standalone authentication synthetic projection on production build",
      baseUrl: "http://127.0.0.1:15392",
      colorScheme,
      reducedMotion: "reduce",
      personas: [{
        id: "fixture",
        label: "Synthetic HTTP/WebAuthn only",
        auth: { kind: "anonymous" },
      }],
      viewports: [{ width, height: 900, label: `${width}` }],
      routes: coverage === "session"
        ? routes.filter((route) => sessionCaptureIds.has(route.id))
        : routes,
      redact: {
        // 初期撮影後にもfillする。空の入力まで恒久的に隠すと次の操作が不能になる。
        // 合成codeはsubmit直後に破棄され、pendingのdisabled入力だけを隠す。
        selectors: ["#activation-code:disabled", ".secret"],
        text: ["synthetic-input-only"],
      },
      processes: [{
        id: "auth-projection",
        command: "deno",
        cwd: root,
        args: [
          "run",
          "--config",
          "tools/web-ux/deno.json",
          "--allow-read",
          "--allow-net",
          "web/workspace/test/auth-browser/fixture-server.ts",
          "15392",
          "web/workspace/build",
        ],
        readyUrl: "http://127.0.0.1:15392/health",
      }],
    };
    const scenarioPath =
      `${root}/target/web-ux/auth-${phase}-${colorScheme}-${width}.json`;
    await Deno.writeTextFile(scenarioPath, JSON.stringify(scenario, null, 2));
    const result = await capture({
      scenarioPath,
      outputDirectory: `${root}/target/web-ux`,
      runId: `auth-${phase}-${colorScheme}-${width}`,
    });
    console.log(
      JSON.stringify({
        colorScheme,
        width,
        status: result.status,
        captures: result.captures.length,
      }),
    );
  }
}

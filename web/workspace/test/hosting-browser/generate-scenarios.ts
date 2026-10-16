import { resolve } from "@std/path";
import type { RouteScenario, Scenario } from "../../../../tools/web-ux/src/types.ts";
const root = resolve(import.meta.dirname!, "../../../..");
const selector = (selector: string) => ({ kind: "selector" as const, selector });
const click = (selector: string) => ({ action: "click" as const, selector });
const press = (selector: string, key: string) => ({ action: "press" as const, selector, key });
const route = (
  id: string,
  path: string,
  ready: string,
  dataState: string,
  points = [{ id: "initial", label: "Initial state" }],
): RouteScenario => ({
  id,
  label: id,
  path: `${path}?fixture=${id}`,
  goal: "Verify production shell UI projection, not server authorization.",
  dataState,
  ready: selector(ready),
  capturePoints: points,
});
const routes: RouteScenario[] = [
  ...["on", "off"].map((policy) =>
    route(
      `admin-${policy}`,
      "/hosting/settings",
      'button:has-text("Edit registration policy")',
      `Synthetic admin; policy ${policy}.`,
      [
        { id: "initial", label: "Loaded hosting-wide policy" },
        {
          id: "edit",
          label: "Keyboard edit focus",
          interaction: [press('button:has-text("Edit registration policy")', "Enter")],
          ready: selector('input[type="checkbox"]:focus'),
        },
        {
          id: "saved",
          label: "Confirmed save",
          interaction: [
            press('input[type="checkbox"]', "Space"),
            click('button:has-text("Save settings")'),
          ],
          ready: selector('[role="status"]:has-text("Hosting settings saved.")'),
        },
      ] as RouteScenario["capturePoints"],
    )
  ),
  route(
    "ordinary",
    "/hosting/settings",
    'p:has-text("Hosting administrator access is required.")',
    "Synthetic authenticated ordinary user, no admin controls.",
  ),
  route(
    "anonymous",
    "/hosting/settings",
    'a:has-text("Open Account")',
    "Synthetic anonymous visitor; sign-in guidance, no controls.",
  ),
  route(
    "admin-loading",
    "/hosting/settings",
    '[role="status"]:has-text("Loading hosting settings")',
    "Settings GET intentionally held pending; shell remains usable.",
  ),
  route(
    "admin-load-error",
    "/hosting/settings",
    'button:has-text("Retry loading settings")',
    "Intentional HTTP503; expected console resource error and visible alert.",
  ),
  ...["saving", "save-error"].map((state) =>
    route(
      `admin-${state}`,
      "/hosting/settings",
      'button:has-text("Edit registration policy")',
      state === "saving"
        ? "PUT held pending; last loaded policy retained, edit controls disabled."
        : "Intentional PUT HTTP503; outcome unknown, retry blocked until reload.",
      [
        {
          id: state,
          label: state,
          interaction: [
            click('button:has-text("Edit registration policy")'),
            press('input[type="checkbox"]', "Space"),
            click('button:has-text("Save settings")'),
          ],
          ready: selector(state === "saving" ? 'button:has-text("Saving…")' : '[role="alert"]'),
        },
      ] as RouteScenario["capturePoints"],
    )
  ),
  route(
    "account-on",
    "/account",
    'button:has-text("Register new account"):enabled',
    "Anonymous policy on; registration and login available.",
  ),
  route(
    "account-off",
    "/account",
    'p:has-text("New account registration is disabled.")',
    "Anonymous policy off; no registration action, login available.",
  ),
  route(
    "admin-account",
    "/account",
    'button:has-text("Add passkey to alice")',
    "Authenticated admin; own passkey addition and global settings link.",
  ),
  route(
    "ordinary-account-off",
    "/account",
    'button:has-text("Add passkey to alice")',
    "Authenticated ordinary user with policy off; own passkey addition available without admin nav.",
  ),
  route(
    "account-error",
    "/account",
    'button:has-text("Retry registration availability")',
    "Intentional policy HTTP503; expected console resource error and visible alert; login retained.",
  ),
  route(
    "account-loading",
    "/account",
    '[role="status"]:has-text("Checking registration availability")',
    "Policy request held pending; login remains available after session loads.",
  ),
];
for (const account of routes.filter((item) => item.path.startsWith("/account"))) {
  account.capturePoints[0].fullPage = true;
}
for (const theme of ["light", "dark"] as const) {
  const scenario: Scenario = {
    schemaVersion: 1,
    id: `hosting-settings-${theme}`,
    title: "Hosting settings: synthetic UI conditions on production shell",
    baseUrl: "http://127.0.0.1:15378",
    colorScheme: theme,
    reducedMotion: "reduce",
    personas: [{
      id: "fixture",
      label: "Synthetic API projection; no real authentication",
      auth: { kind: "anonymous" },
    }],
    viewports: [{ label: "desktop", width: 1440, height: 900 }, {
      label: "narrow",
      width: 768,
      height: 900,
    }, { label: "mobile", width: 390, height: 844 }],
    routes,
    processes: [{
      id: "hosting-ui-fixture",
      command: "deno",
      cwd: "../../..",
      args: [
        "run",
        "--config",
        "tools/web-ux/deno.json",
        "--allow-read",
        "--allow-net",
        "web/workspace/test/hosting-browser/fixture-server.ts",
        "15378",
        "web/workspace/build",
      ],
      readyUrl: "http://127.0.0.1:15378/health",
    }],
  };
  await Deno.writeTextFile(
    `${root}/tools/web-ux/scenarios/hosting-settings-${theme}.json`,
    JSON.stringify(scenario, null, 2) + "\n",
  );
}

// 合成HTTPとWebAuthn境界だけを使い、本番build/shellをshared web-uxで採取する。
// ServerE2Eとは別物。secret fieldは撮影前に隠し、snapshot/診断もredactする。
import { resolve } from "@std/path";
import { capture } from "../../../../tools/web-ux/src/capture.ts";
import type {
  Interaction,
  RouteScenario,
  Scenario,
} from "../../../../tools/web-ux/src/types.ts";
const root = resolve(import.meta.dirname!, "../../../..");
Deno.env.set("WEB_UX_SYNTHETIC_ACTIVATION_INPUT", "synthetic-input-only");
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
const activation = [
  fill("#activation-handle", "alice"),
  fill("#activation-code", "${WEB_UX_SYNTHETIC_ACTIVATION_INPUT}"),
  click('button:has-text("Register Passkey and activate")'),
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
  goal: "Check production shell and state/copy; not Server authorization.",
  dataState: `Synthetic ${id}. No real code, account, credential or session.`,
  ready: ready(selector),
  capturePoints,
});
const routes: RouteScenario[] = [
  route(
    "settings",
    "/hosting/settings?fixture=admin",
    'a[href="/hosting/accounts"]',
  ),
  route("admin", "/hosting/accounts?fixture=admin", "#account-handle:enabled", [
    { id: "initial", label: "ID-only creation", fullPage: true },
    {
      id: "link-focus",
      label: "Hosting settings link keyboard focus",
      interaction: [{ action: "press", selector: "summary", key: "Tab" }],
      ready: ready('a[href="/hosting/settings"]:focus-visible'),
      fullPage: true,
    },
    {
      id: "issued",
      label: "Issued; code redacted",
      interaction: [
        fill("#account-handle", "alice"),
        click('button:has-text("Create account and issue code")'),
      ],
      ready: ready("#issued-heading"),
      fullPage: true,
    },
    {
      id: "reissue",
      label: "Explicit reissue and invalidation warning",
      interaction: [click('button:has-text("Hide code")'), {
        action: "press",
        selector: "#account-operation",
        key: "ArrowDown",
      }],
      ready: ready('button:has-text("Reissue activation code")'),
      fullPage: true,
    },
  ]),
  route(
    "anonymous",
    "/hosting/accounts?fixture=anonymous",
    'a[href="/account"]',
  ),
  route(
    "ordinary",
    "/hosting/accounts?fixture=ordinary",
    'p:has-text("Hosting administrator access is required.")',
  ),
  route("loading", "/hosting/accounts?fixture=loading", '[role="status"]'),
  route(
    "access-error",
    "/hosting/accounts?fixture=access-error",
    '[role="alert"]',
  ),
  route(
    "issue-error",
    "/hosting/accounts?fixture=issue-error",
    "#account-handle:enabled",
    [{
      id: "unconfirmed",
      label: "Intentional HTTP409; unconfirmed issuance",
      interaction: [
        fill("#account-handle", "alice"),
        click('button:has-text("Create account and issue code")'),
      ],
      ready: ready('[role="alert"]'),
      fullPage: true,
    }],
  ),
  route(
    "activation-pending",
    "/activate?fixture=activation-pending",
    "#activation-code:enabled",
    [
      {
        id: "initial",
        label: "ID and code before mandatory Passkey",
        fullPage: true,
      },
      {
        id: "link-focus",
        label: "Account recovery link keyboard focus",
        interaction: [{
          action: "press",
          selector: "summary",
          key: "Shift+Tab",
        }],
        ready: ready('a[href="/account"]:focus-visible'),
        fullPage: true,
      },
      {
        id: "pending",
        label: "Pending prompt, no activation claim",
        interaction: activation,
        ready: ready('button:has-text("Cancel registration")'),
        fullPage: true,
      },
      {
        id: "cancelled",
        label: "Cancelled; code cleared and restart guidance",
        interaction: [click('button:has-text("Cancel registration")')],
        ready: ready('[role="alert"]'),
        fullPage: true,
      },
    ],
  ),
  ...[
    "activation-cancel",
    "activation-unknown",
    "activation-reject",
    "activation-success",
  ].map((mode) =>
    route(mode, `/activate?fixture=${mode}`, "#activation-code:enabled", [{
      id: "result",
      label: mode,
      interaction: activation,
      ready: ready(
        mode === "activation-success" ? '[role="status"]' : '[role="alert"]',
      ),
      fullPage: true,
    }])
  ),
];
for (const colorScheme of ["light", "dark"] as const) {
  for (const width of [1440, 768, 390]) {
    const scenario: Scenario = {
      schemaVersion: 1,
      id: `activation-${colorScheme}-${width}`,
      title: "Activation synthetic projection on production build",
      baseUrl: "http://127.0.0.1:15382",
      colorScheme,
      reducedMotion: "reduce",
      personas: [{
        id: "fixture",
        label: "Synthetic HTTP/WebAuthn only",
        auth: { kind: "anonymous" },
      }],
      viewports: [{ width, height: 900, label: `${width}` }],
      routes,
      redact: {
        selectors: [".secret"],
        text: ["synthetic-output-only", "${WEB_UX_SYNTHETIC_ACTIVATION_INPUT}"],
      },
      processes: [{
        id: "activation-projection",
        command: "deno",
        cwd: root,
        args: [
          "run",
          "--config",
          "tools/web-ux/deno.json",
          "--allow-read",
          "--allow-net",
          "web/workspace/test/activation-browser/fixture-server.ts",
          "15382",
          "web/workspace/build",
        ],
        readyUrl: "http://127.0.0.1:15382/health",
      }],
    };
    const scenarioPath =
      `${root}/target/web-ux/activation-${colorScheme}-${width}.json`;
    await Deno.writeTextFile(scenarioPath, JSON.stringify(scenario, null, 2));
    const result = await capture({
      scenarioPath,
      outputDirectory: `${root}/target/web-ux`,
      runId: `activation-after-v3-${colorScheme}-${width}`,
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

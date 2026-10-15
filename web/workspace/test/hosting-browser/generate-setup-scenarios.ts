import { resolve } from "@std/path";
import type { Interaction, Scenario } from "../../../../tools/web-ux/src/types.ts";
const root = resolve(import.meta.dirname!, "../../../..");
const submit: Interaction[] = [{
  action: "fill",
  selector: "#host-setup-token",
  value: "${WEB_UX_SYNTHETIC_SETUP_INPUT}",
}, { action: "click", selector: 'button:has-text("Add administrator passkey")' }];
for (const theme of ["light", "dark"]) {
  const scenario: Scenario = JSON.parse(
    await Deno.readTextFile(`${root}/tools/web-ux/scenarios/hosting-settings-${theme}.json`),
  );
  scenario.id = `hosting-setup-${theme}`;
  scenario.title =
    "Hosting setup production UI: synthetic HTTP and navigator.credentials boundary only";
  scenario.redact = { text: ["${WEB_UX_SYNTHETIC_SETUP_INPUT}"] };
  scenario.routes = [{
    id: "setup-flow",
    label: "Setup enrollment and login UI",
    path: "/hosting/setup?fixture=setup-flow",
    goal:
      "Separate host-token enrollment from explicit passkey login and verified settings access.",
    dataState:
      "Synthetic actor, API and non-cryptographic navigator.credentials; no real token/authentication. Test-only bootstrap changes browser API boundary, not production UI modules/styles.",
    ready: { kind: "selector", selector: "#host-setup-token:enabled" },
    capturePoints: [
      { id: "initial", label: "Empty password field; enrollment disabled", fullPage: true },
      {
        id: "host-help",
        label: "Trusted-host instructions expanded",
        interaction: [{ action: "press", selector: "summary", key: "Enter" }],
        ready: { kind: "selector", selector: "details[open]" },
        fullPage: true,
      },
      {
        id: "masked-entry",
        label: "Synthetic input password-masked; action enabled",
        interaction: [{ action: "press", selector: "summary", key: "Enter" }, submit[0]],
        ready: {
          kind: "selector",
          selector: 'button:has-text("Add administrator passkey"):enabled',
        },
        fullPage: true,
      },
      {
        id: "enrolled",
        label: "Enrollment is not login; explicit login handoff",
        interaction: [submit[1]],
        ready: {
          kind: "selector",
          selector: 'button:has-text("Sign in with new passkey"):enabled',
        },
        fullPage: true,
      },
      {
        id: "verified",
        label: "Matching actor/session/access projected verified; settings link",
        interaction: [{
          action: "press",
          selector: 'button:has-text("Sign in with new passkey")',
          key: "Enter",
        }],
        ready: { kind: "selector", selector: 'a:has-text("Open hosting settings")' },
        fullPage: true,
      },
    ],
  }, {
    id: "setup-reject",
    label: "Rejected setup enrollment",
    path: "/hosting/setup?fixture=setup-reject",
    goal:
      "Show safe unconfirmed-enrollment guidance with empty cleared token and no settings action.",
    dataState:
      "Intentional addition/options HTTP403. Expected console/resource diagnostics; no real authorization exercised.",
    ready: { kind: "selector", selector: "#host-setup-token:enabled" },
    capturePoints: [{
      id: "rejected",
      label: "Rejected attempt; token cleared",
      interaction: submit,
      ready: { kind: "selector", selector: '[role="alert"]' },
      fullPage: true,
    }],
  }];
  await Deno.writeTextFile(
    `${root}/tools/web-ux/scenarios/hosting-setup-${theme}.json`,
    JSON.stringify(scenario, null, 2) + "\n",
  );
}

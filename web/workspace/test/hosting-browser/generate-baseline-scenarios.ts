import { resolve } from "@std/path";
import type { Scenario } from "../../../../tools/web-ux/src/types.ts";
const root = resolve(import.meta.dirname!, "../../../..");
for (const theme of ["light", "dark"]) {
  const scenario: Scenario = JSON.parse(
    await Deno.readTextFile(`${root}/tools/web-ux/scenarios/hosting-settings-${theme}.json`),
  );
  scenario.id = `hosting-baseline-${theme}`;
  scenario.title = "Before: tracked HEAD Account and global navigation; no hosting route existed";
  scenario.routes = [
    {
      id: "account-on",
      label: "Anonymous Account before",
      path: "/account?fixture=account-on",
      goal: "Compare existing Account registration/login before policy gating.",
      dataState: "Synthetic anonymous session; unchanged baseline production shell.",
      ready: { kind: "selector", selector: 'button:has-text("Register new account"):enabled' },
      capturePoints: [{ id: "initial", label: "Before anonymous Account" }],
    },
    {
      id: "admin-account",
      label: "Signed-in Account before",
      path: "/account?fixture=admin-account",
      goal: "Compare Account and global navigation before admin settings link.",
      dataState: "Synthetic signed-in session; no hosting route/link before.",
      ready: { kind: "selector", selector: 'button:has-text("Add passkey to alice")' },
      capturePoints: [{ id: "initial", label: "Before signed-in Account" }],
    },
  ];
  scenario.processes![0].cwd = root;
  scenario.processes![0].args![scenario.processes![0].args!.length - 1] =
    "target/web-ux/baseline-source/web/workspace/build";
  for (const route of scenario.routes) route.capturePoints[0].fullPage = true;
  await Deno.writeTextFile(
    `${root}/target/web-ux/baseline-${theme}.json`,
    JSON.stringify(scenario, null, 2),
  );
  for (const version of ["before", "after"]) {
    const nav = structuredClone(scenario);
    nav.id = `hosting-navigation-${version}-${theme}`;
    nav.title = `${version} mobile global navigation on production shell`;
    nav.viewports = nav.viewports.filter((item) => item.width === 390);
    for (const route of nav.routes) {
      route.capturePoints = [{
        id: "navigation",
        label: "Mobile global links",
        interaction: [{ action: "click", selector: 'button[aria-label="Show sidebar"]' }],
        ready: { kind: "selector", selector: 'nav[aria-label="Global pages"]' },
      }];
    }
    if (version === "after") {
      nav.processes![0].args![nav.processes![0].args!.length - 1] = "web/workspace/build";
    }
    await Deno.writeTextFile(
      `${root}/target/web-ux/navigation-${version}-${theme}.json`,
      JSON.stringify(nav, null, 2),
    );
  }
}

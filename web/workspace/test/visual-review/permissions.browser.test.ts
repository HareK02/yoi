/// <reference lib="dom" />
import { assertEquals } from "jsr:@std/assert@1.0.19";
import { chromium } from "npm:playwright@1.59.1";
import {
  startOwnedProcesses,
  stopOwnedProcesses,
} from "../../../../tools/web-ux/src/processes.ts";
import { freePort } from "../../../../tools/web-ux/browser-tests/memory_document_harness.ts";
import {
  createSubjectPermissionsFixture,
  fixtureLongNames,
} from "../../../../tools/web-ux/browser-tests/subject_permissions_fixture.ts";
import { sha256File } from "../../../../tools/web-ux/src/artifacts.ts";

type State =
  | "normal"
  | "unset"
  | "removed"
  | "revoked"
  | "explicit-empty"
  | "excluded"
  | "longnames"
  | "reader"
  | "legacy"
  | "diagnostics"
  | "auth401"
  | "auth403"
  | "unavailable"
  | "missing-resolution"
  | "loading";
type Diagnostic = {
  kind: "page" | "console" | "request" | "http";
  message: string;
  url?: string;
  method?: string;
  status?: number;
};
const subjects: Record<State, string> = {
  normal: "permissions-edit",
  unset: "permissions-empty",
  removed: "permissions-removed",
  revoked: "permissions-revoked",
  "explicit-empty": "permissions-explicit-empty",
  excluded: "permissions-excluded",
  longnames: "permissions-longnames",
  reader: "permissions-reader",
  legacy: "permissions-legacy",
  diagnostics: "permissions-diagnostics",
  auth401: "permissions-anonymous",
  auth403: "permissions-forbidden",
  unavailable: "permissions-unavailable",
  "missing-resolution": "permissions-edit",
  loading: "permissions-edit",
};
const alerts: Partial<Record<State, string>> = {
  auth401: "Sign in to read the current Subject policy.",
  auth403: "You do not have permission to read the current Subject policy.",
  unavailable:
    "Current Subject policy could not be read. No effective permission is inferred. Reload to try again.",
  "missing-resolution":
    "Current Subject policy could not be read. No effective permission is inferred. Reload to try again.",
};

// Actual production UI with synthetic HTTP, not Backend authentication,
// persistence, resolver, revocation or Worker-effect evidence. This UI is read-only:
// no mutation/CAS scenario belongs here. --before uses a separately built HEAD
// baseline copied to build/ by the caller; its old wire contract is historical only.
Deno.test("Current Subject policy responsive visual review", async () => {
  const before = Deno.args.includes("--before");
  const root = new URL("../../../../", import.meta.url).pathname;
  const output = `${root}target/web-ux/subject-permission-policy/${
    before ? "before" : "after"
  }`;
  // Load the old source fixture ONLY for historical capture. Current-policy
  // responses must come from the shared dedicated endpoint fixture below.
  const historicalFixture = before
    ? await import("../permissions-fixture.ts")
    : null;
  await Deno.mkdir(output, { recursive: true });
  const port = await freePort();
  const processes = await startOwnedProcesses(
    [{
      id: "subject-permissions-fixture",
      command: Deno.execPath(),
      cwd: root,
      args: [
        "run",
        "-A",
        "tools/web-ux/browser-tests/memory_document_fixture_server.ts",
        String(port),
        `${root}web/workspace/build`,
        "3",
        "permissions",
      ],
      readyUrl: `http://127.0.0.1:${port}/health`,
    }],
    `${output}/scenario.json`,
    output,
    [],
  );
  const browser = await chromium.launch({ headless: true });
  const captures: unknown[] = [];
  const matrix: { state: State; theme: "light" | "dark"; width: number }[] = [];
  for (const theme of ["light", "dark"] as const) {
    for (const width of [1440, 768, 390]) {
      matrix.push({ state: "normal", theme, width });
    }
  }
  if (!before) {
    matrix.push(
      { state: "unset", theme: "light", width: 390 },
      { state: "removed", theme: "dark", width: 390 },
      { state: "revoked", theme: "light", width: 390 },
      { state: "explicit-empty", theme: "dark", width: 768 },
      { state: "excluded", theme: "dark", width: 768 },
      { state: "longnames", theme: "light", width: 390 },
      { state: "longnames", theme: "dark", width: 390 },
      { state: "longnames", theme: "light", width: 768 },
      { state: "reader", theme: "dark", width: 390 },
      { state: "legacy", theme: "light", width: 768 },
      { state: "diagnostics", theme: "dark", width: 390 },
      { state: "auth401", theme: "light", width: 390 },
      { state: "auth403", theme: "dark", width: 390 },
      { state: "unavailable", theme: "dark", width: 390 },
      { state: "missing-resolution", theme: "light", width: 768 },
      { state: "loading", theme: "dark", width: 390 },
    );
  }
  try {
    for (const { state, theme, width } of matrix) {
      const subjectId = before ? "permissions-legacy" : subjects[state];
      const policyPath =
        `/api/w/memory-review/subjektiv/subjects/${subjectId}/permission-policy`;
      const faultStatus = state === "auth401"
        ? 401
        : state === "auth403"
        ? 403
        : state === "unavailable"
        ? 503
        : null;
      const page = await browser.newPage({
        viewport: { width, height: 900 },
        colorScheme: theme,
        reducedMotion: "reduce",
      });
      const errors: Diagnostic[] = [];
      const permissionRequests: { path: string; method: string }[] = [];
      page.on("request", (request) => {
        const path = new URL(request.url()).pathname;
        if (
          path.startsWith("/api/") &&
          (/\/(?:config|adoption|permissions)(?:\/|$)/.test(path) ||
            path.endsWith("/permission-policy"))
        ) permissionRequests.push({ path, method: request.method() });
      });
      page.on("pageerror", (error) =>
        errors.push({ kind: "page", message: error.message }));
      page.on("console", (message) => {
        if (message.type() === "error") {
          errors.push({
            kind: "console",
            message: message.text(),
            url: message.location().url,
          });
        }
      });
      page.on("requestfailed", (request) =>
        errors.push({
          kind: "request",
          message: request.failure()?.errorText ?? "Unknown request failure",
          url: request.url(),
          method: request.method(),
        }));
      page.on("response", (response) => {
        if (response.status() >= 400) {
          errors.push({
            kind: "http",
            message: `HTTP ${response.status()}`,
            url: response.url(),
            status: response.status(),
          });
        }
      });
      let releaseLoad: (() => void) | undefined;
      const loadGate = new Promise<void>((resolve) => {
        releaseLoad = resolve;
      });
      if (before) {
        await page.route("**/config/adoption", async (route) => {
          await route.fulfill({
            json: historicalFixture!.permissionSource("memory-review", subjectId),
          });
        });
      } else {
        const fixture = createSubjectPermissionsFixture();
        await page.route("**/permission-policy", async (route) => {
          if (state === "loading") await loadGate;
          const response = await fixture(new Request(route.request().url(), {
            method: route.request().method(),
          }));
          if (!response) throw new Error("Dedicated policy fixture did not match");
          let body = await response.text();
          if (state === "missing-resolution") {
            // Deliberate malformed HTTP 200; all other fields remain coherent.
            const incomplete = JSON.parse(body);
            delete incomplete.execution_resolution;
            body = JSON.stringify(incomplete);
          }
          await route.fulfill({
            status: response.status,
            headers: Object.fromEntries(response.headers),
            body,
          });
        });
      }
      try {
        const response = await page.goto(
          `http://127.0.0.1:${port}/w/memory-review/subjektiv/${subjectId}/permissions`,
        );
        const permissions = page.locator(".permissions");
        await permissions.waitFor();
        const ready = !before && !alerts[state] && state !== "loading";
        if (before) {
          await page.getByRole("heading", {
            name: "Direct operations",
            exact: true,
          }).waitFor();
        } else {
          await permissions.getByRole("heading", {
            name: "Current Subject policy",
            exact: true,
          }).waitFor();
          if (state === "loading") {
            await permissions.getByRole("status").filter({
              hasText: "Reading current Subject policy…",
            }).waitFor();
          } else if (alerts[state]) {
            await permissions.getByRole("alert").waitFor();
          } else {
            await page.locator('[data-permission-state="ready"]').waitFor();
            // Reveal real production disclosures, not screenshot-only styling.
            const summaries = permissions.locator("details > summary");
            for (const summary of await summaries.all()) await summary.click();
          }
        }
        const keyboardOrder: (string | null)[] = [];
        if (!before && state === "normal") {
          await permissions.getByRole("button", { name: "Reload permissions" })
            .focus();
          keyboardOrder.push(await page.locator(":focus").textContent());
          await page.keyboard.press("Tab");
          keyboardOrder.push(await page.locator(":focus").textContent());
          await page.keyboard.press("Shift+Tab");
          keyboardOrder.push(await page.locator(":focus").textContent());
        }
        await permissions.getByRole("heading", { level: 2 }).scrollIntoViewIfNeeded();
        const id = `${state}-${theme}-${width}`;
        const image = `${output}/${id}.png`;
        await page.screenshot({ path: image, fullPage: true });
        const accessibility = await page.locator("body").ariaSnapshot();
        let catalogCapture: { image: string; sha256: string } | null = null;
        if (ready) {
          await permissions.locator(".catalog").scrollIntoViewIfNeeded();
          const catalogImage = `${output}/${id}-catalog.png`;
          await page.screenshot({ path: catalogImage });
          catalogCapture = {
            image: catalogImage,
            sha256: await sha256File(catalogImage),
          };
        }
        const overflow = await page.evaluate(() =>
          document.documentElement.scrollWidth > innerWidth
        );
        const contentWidth = await permissions.evaluate((element) => ({
          client: element.clientWidth,
          scroll: element.scrollWidth,
          overflow: element.scrollWidth > element.clientWidth + 1,
        }));
        // Existing Sidebar cancels superseded worker-catalog GETs. Keep these
        // separate, narrowly scoped evidence rather than suppressing all aborts.
        const shellCancellations = errors.filter((error) =>
          error.kind === "request" && error.method === "GET" &&
          error.message === "net::ERR_ABORTED" &&
          new URL(error.url!).pathname === "/api/w/memory-review/workers"
        );
        const isPolicyUrl = (url: string | undefined) =>
          !!url && new URL(url).pathname === policyPath;
        const expectedFaultErrors = errors.filter((error) =>
          faultStatus !== null && (
            (error.kind === "http" && error.status === faultStatus &&
              isPolicyUrl(error.url)) ||
            (error.kind === "console" &&
              (!error.url || isPolicyUrl(error.url)) &&
              error.message.startsWith("Failed to load resource:") &&
              error.message.includes(`status of ${faultStatus}`)) ||
            (error.kind === "request" && error.method === "GET" &&
              isPolicyUrl(error.url) && error.message === "net::ERR_ABORTED")
          )
        );
        const unexpectedErrors = errors.filter((error) =>
          !shellCancellations.includes(error) &&
          !expectedFaultErrors.includes(error)
        );
        const visibleAlerts = await page.getByRole("alert").allTextContents();
        captures.push({
          id,
          state,
          theme,
          viewport: { width, height: 900 },
          route: new URL(page.url()).pathname,
          subjectId,
          persona: before ? "historical-owner" : state === "reader"
            ? "read-only reader"
            : state === "auth401" || state === "auth403"
            ? "synthetic endpoint refusal; shell session remains authenticated"
            : "synthetic owner",
          documentStatus: response?.status(),
          errors,
          permissionRequests: [...permissionRequests],
          expectedFault: faultStatus !== null || state === "missing-resolution",
          expectedAlert: alerts[state] ?? null,
          expectedDiagnostic: faultStatus !== null
            ? `HTTP ${faultStatus} from ${policyPath}; endpoint refusal, not real authentication evidence`
            : state === "missing-resolution"
            ? "HTTP 200 current policy missing top-level execution_resolution; parser must reject without inferring permissions"
            : state === "loading"
            ? "Dedicated GET held until after capture; no response failure expected"
            : null,
          expectedFaultErrors,
          unexpectedErrors,
          shellCancellations,
          overflow,
          contentWidth,
          keyboardOrder,
          image,
          catalogCapture,
          sha256: await sha256File(image),
          accessibility,
          alerts: visibleAlerts,
        });
        assertEquals(response?.status(), 200);
        assertEquals(overflow, false);
        assertEquals(contentWidth.overflow, false);
        assertEquals(unexpectedErrors, []);
        if (!before) {
          // Exact evidence: one dedicated GET, no config/adoption/validate,
          // permission writes, or mutation/CAS traffic from this read-only UI.
          assertEquals(permissionRequests, [{ path: policyPath, method: "GET" }]);
          assertEquals(
            await permissions.getByRole("button").allTextContents(),
            ["Reload permissions"],
          );
          assertEquals(
            await permissions.locator("input, textarea, select, form").count(),
            0,
          );
          assertEquals(
            await page.getByRole("link", { name: /Edit configuration/i }).count(),
            0,
          );
          assertEquals(await permissions.locator("a").count(), 0);
          assertEquals(
            await permissions.getByText(
              /configuration source|draft|adoption|source identity/i,
            ).count(),
            0,
          );
          assertEquals(visibleAlerts, alerts[state] ? [alerts[state]] : []);
          if (faultStatus !== null) {
            assertEquals(
              expectedFaultErrors.some((error) => error.kind === "http"),
              true,
            );
          }
          if (state === "loading") {
            assertEquals(await permissions.getAttribute("aria-busy"), "true");
            assertEquals(
              await permissions.getByRole("status").textContent(),
              "Reading current Subject policy…",
            );
            assertEquals(
              await permissions.getByRole("button", { name: "Reload permissions" })
                .isDisabled(),
              true,
            );
          }
          if (alerts[state] || state === "loading") {
            assertEquals(
              await permissions.locator(".lane, .catalog, .policy-change").count(),
              0,
            );
          }
          if (ready) {
            assertEquals(
              await permissions.locator(".policy-change summary").textContent(),
              "Policy change",
            );
            assertEquals(
              await permissions.getByText("Read-only policy access.", {
                exact: true,
              }).count(),
              state === "reader" || state === "legacy" ? 1 : 0,
            );
            const assignmentState = state === "unset" || state === "removed"
              ? "Unset assignment"
              : state === "revoked"
              ? "Revoked assignment"
              : state === "explicit-empty"
              ? "Explicitly empty assignment"
              : null;
            if (assignmentState) {
              assertEquals(
                await permissions.locator(".assignment-state").allTextContents(),
                [assignmentState, assignmentState],
              );
              assertEquals(await permissions.locator(".resolved > li").count(), 0);
            }
            if (state === "unset" || state === "removed") {
              assertEquals(
                await permissions.locator(".policy-change dd").textContent(),
                state === "unset"
                  ? "No policy change recorded"
                  : "fixture-removal-permissions-removed",
              );
            }
            if (state === "excluded") {
              assertEquals(
                await permissions.locator(".resolved > li > code").allTextContents(),
                ["ticket.read"],
              );
              assertEquals(
                await permissions.getByRole("heading", {
                  name: "Exclusions",
                  exact: true,
                }).count(),
                2,
              );
            }
            if (state === "longnames") {
              assertEquals(
                await permissions.locator(".resolved > li > code").allTextContents(),
                [fixtureLongNames.key, fixtureLongNames.key],
              );
              assertEquals(
                await permissions.locator(".catalog").getByText(
                  fixtureLongNames.namespace,
                  { exact: true },
                ).count() > 0,
                true,
              );
            }
            if (state === "diagnostics") {
              const direct = permissions.getByRole("region", {
                name: "Direct operations",
                exact: true,
              });
              for (const diagnostic of [
                "unknown_set · missing-set",
                "unknown_namespace · missing-namespace",
                "unknown_key · ticket.unknown",
                "disabled_feature · worker",
              ]) {
                assertEquals(
                  await direct.getByText(new RegExp(`^${diagnostic.replace(" · ", "\\s*·\\s*")}$`)).isVisible(),
                  true,
                  await direct.innerText(),
                );
              }
              assertEquals(await permissions.locator(".resolved > li").count(), 0);
            }
            if (state === "legacy") {
              assertEquals(
                await permissions.locator(".legacy pre").isVisible(),
                true,
              );
              assertEquals(
                (await permissions.locator(".legacy pre").textContent())
                  ?.includes("T-802"),
                true,
              );
            }
            if (state === "normal") {
              assertEquals(
                await permissions.locator(".resolved > li > code").allTextContents(),
                ["ticket.read", "ticket.read", "ticket.comment"],
              );
              assertEquals(keyboardOrder, [
                "Reload permissions",
                "Permission origin",
                "Reload permissions",
              ]);
            }
          }
        }
      } finally {
        releaseLoad?.();
        try {
          if (!before && state === "loading") {
            await page.locator('[data-permission-state="ready"]').waitFor();
          }
        } finally {
          await page.close();
        }
      }
    }
  } finally {
    try {
      await Deno.writeTextFile(
        `${output}/review-context.json`,
        JSON.stringify({
          evidence: "Actual production Web build; synthetic HTTP only, not Backend persistence, authorization or revocation",
          before,
          wireContract: before ? "historical configuration-adoption GET only" : "dedicated current Subject permission-policy GET only",
          visualReview: "Pending parent inspection of actual before/after PNGs and this context; automated assertions are not visual approval",
          browser: browser.version(),
          buildSha256: await sha256File(`${root}web/workspace/build/index.html`),
          // In --before, source files still belong to the current checkout, not
          // the isolated baseline. Hash the actual served build separately.
          sourceProvenance: "Current checkout; not necessarily the source of the historical baseline build",
          sourceHashes: await Promise.all([
            "web/workspace/src/lib/workspace/memory/permissions.ts",
            "web/workspace/src/lib/workspace/memory/SubjectPermissions.svelte",
            "tools/web-ux/browser-tests/subject_permissions_fixture.ts",
            "tools/web-ux/browser-tests/memory_document_fixture_server.ts",
            "web/workspace/test/visual-review/permissions.browser.test.ts",
            ...(before ? ["web/workspace/test/permissions-fixture.ts"] : []),
          ].map(async (file) => ({ file, sha256: await sha256File(`${root}${file}`) }))),
          captures,
        }, null, 2),
      );
    } finally {
      try {
        await browser.close();
      } finally {
        await stopOwnedProcesses(processes);
      }
    }
  }
});

#!/usr/bin/env -S deno run -A
/** Actual Web build with synthetic HTTP; Rust owns real authentication/CAS evidence. */
import { assertEquals } from "@std/assert";
import { dirname, fromFileUrl, join, resolve } from "@std/path";
import { chromium } from "playwright";
import { sha256File } from "../src/artifacts.ts";
import { startOwnedProcesses, stopOwnedProcesses } from "../src/processes.ts";
import { assertNoPageWideOverflow, freePort } from "./memory_document_harness.ts";
import missingCredential from "../../../web/workspace/test/repository-access/fixtures/credential-not-found.json" with {
  type: "json",
};

const root = resolve(dirname(fromFileUrl(import.meta.url)), "../../..");
const buildRoot = resolve(Deno.args[0]);
const runId = Deno.args[1];
const before = Deno.args[2] === "before";
if (!runId || !/^[a-zA-Z0-9._-]+$/.test(runId)) throw new Error("supply a safe unique run id");
const output = join(root, "target/web-ux/t815", runId);
await Deno.mkdir(dirname(output), { recursive: true });
await Deno.mkdir(output);
const port = await freePort();
const base = `http://127.0.0.1:${port}`;
const processes = await startOwnedProcesses(
  [{
    id: "repository-access-synthetic-http",
    command: Deno.execPath(),
    cwd: root,
    args: [
      "run",
      "--allow-net",
      "--allow-read",
      join(root, "tools/web-ux/browser-tests/workspace_home_fixture_server.ts"),
      String(port),
      buildRoot,
    ],
    readyUrl: `${base}/health`,
  }],
  import.meta.url,
  join(output, "processes"),
  [],
);
const source = await new Deno.Command("git", { args: ["rev-parse", "HEAD"], cwd: root }).output();
const manifest = {
  evidence: "actual-Web-with-synthetic-HTTP-NOT-real-Backend",
  source: new TextDecoder().decode(source.stdout).trim(),
  before,
  buildRoot,
  buildIndexSha256: await sha256File(join(buildRoot, "index.html")),
  visualVerdict: "UNREVIEWED",
  captures: [] as unknown[],
  failures: [] as string[],
};
const browser = await chromium.launch({ headless: true });
try {
  for (const colorScheme of ["light", "dark"] as const) {
    for (const width of [1440, 768, 390]) {
      const context = await browser.newContext({
        viewport: { width, height: 900 },
        colorScheme,
        reducedMotion: "reduce",
      });
      try {
        await context.route("**/api/auth/whoami", (route) =>
          route.fulfill({
            json: {
              authentication_reference: "fixture-authentication",
              session: { kind: "authenticated", expires_at: "2099-01-01T00:00:00Z" },
              actor: {
                user_id: "fixture-user",
                account_id: "fixture-owner",
                handle: "reviewer",
                display_name: "Repository access review",
                auth_method: "browser_session",
              },
            },
          }));
        await context.route(
          "**/api/hosting/access",
          (route) => route.fulfill({ json: { is_admin: false } }),
        );
        await context.route("**/api/w/*/workspace", async (route) => {
          const response = await route.fetch();
          const data = await response.json();
          data.permissions.delete_subjects = true;
          await route.fulfill({ json: data });
        });
        let mutations = 0;
        const page = await context.newPage();
        const errors: { kind: string; message: string; expected: boolean }[] = [];
        let documentStatus: number | null = null;
        page.on("response", (r) => {
          if (r.request().isNavigationRequest()) documentStatus = r.status();
          if (r.status() >= 400) {
            errors.push({
              kind: "http",
              message: `${r.status()} ${new URL(r.url()).pathname}`,
              expected: r.status() === 404 && r.url().endsWith("/deploy/rotate"),
            });
          }
        });
        page.on(
          "pageerror",
          (e) => errors.push({ kind: "page", message: e.message, expected: false }),
        );
        page.on("console", (e) => {
          if (e.type() === "error") {
            errors.push({ kind: "console", message: e.text(), expected: e.text().includes("404") });
          }
        });
        page.on(
          "requestfailed",
          (r) =>
            errors.push({
              kind: "request",
              message: `${r.method()} ${new URL(r.url()).pathname}: ${r.failure()?.errorText}`,
              expected: before && r.url().endsWith("/deploy/rotate") &&
                r.failure()?.errorText === "net::ERR_ABORTED",
            }),
        );
        await page.route("**/api/w/*/settings/repository-access**", (route) => {
          const path = new URL(route.request().url()).pathname;
          if (path.endsWith("/deploy/rotate")) {
            mutations++;
            return route.fulfill({
              status: 404,
              contentType: "application/problem+json",
              body: JSON.stringify(missingCredential),
            });
          }
          if (path.endsWith("/credentials")) {
            return route.fulfill({
              json: [{
                credential_id: "deploy",
                workspace_id: "home-owner",
                name: "Deploy key",
                public_key_algorithm: "ssh-ed25519",
                public_key_fingerprint: "SHA256:fixture-current",
                status: "active",
                created_at: "2026-01-01T00:00:00Z",
                rotated_at: null,
                referenced_repositories: [],
              }],
            });
          }
          if (path.endsWith("/host-trusts")) return route.fulfill({ json: [] });
          return route.fulfill({
            json: { workspace_id: "home-owner", projection_digest: "sha256:fixture", bindings: [] },
          });
        });
        manifest.captures.push({ id: `${colorScheme}-${width}-trace`, errors });
        await page.goto(`${base}/w/home-owner/settings/repository-access`);
        await page.getByRole("button", { name: "Rotate", exact: true }).click();
        await page.getByRole("textbox", { name: /^New OpenSSH private key/ }).fill(
          "synthetic-key-placeholder",
        );
        await page.getByRole("button", { name: "Rotate credential", exact: true }).click();
        const alert = page.getByRole("alert");
        await alert.waitFor();
        const expected = before
          ? "The item for rotate Deploy key no longer exists. Reload the page to see the current state."
          : missingCredential.detail;
        assertEquals(await alert.textContent(), expected);
        assertEquals(mutations, 1, "display must not resend a mutation");
        await alert.scrollIntoViewIfNeeded();
        await assertNoPageWideOverflow(page);
        const id = `${colorScheme}-${width}-credential-missing`;
        const image = join(output, `${id}.png`);
        await page.screenshot({ path: image, mask: [page.locator("[data-web-ux-redact]")] });
        await Deno.writeTextFile(
          join(output, `${id}.aria.txt`),
          await page.locator("body").ariaSnapshot(),
        );
        manifest.captures.push({
          id,
          image,
          imageSha256: await sha256File(image),
          route: new URL(page.url()).pathname,
          persona: "synthetic-owner",
          documentStatus,
          viewport: page.viewportSize(),
          colorScheme,
          visibleAlerts: await alert.allTextContents(),
          errors,
          mutations,
        });
        assertEquals(errors.filter((e) => !e.expected), [], "unexpected browser diagnostics");
      } catch (error) {
        const failedPage = context.pages()[0];
        if (failedPage) {
          await failedPage.screenshot({
            path: join(output, "failed.png"),
            mask: [failedPage.locator("[data-web-ux-redact]")],
          });
          await Deno.writeTextFile(
            join(output, "failed.aria.txt"),
            await failedPage.locator("body").ariaSnapshot(),
          );
        }
        throw error;
      } finally {
        await context.close();
      }
    }
  }
} catch (error) {
  manifest.failures.push(error instanceof Error ? error.message : String(error));
  throw error;
} finally {
  await browser.close();
  await stopOwnedProcesses(processes);
  await Deno.writeTextFile(join(output, "review-context.json"), JSON.stringify(manifest, null, 2));
}
console.log(output);

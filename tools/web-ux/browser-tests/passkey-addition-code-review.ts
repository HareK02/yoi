// Production pages with synthetic HTTP responses; backend authorization is tested separately.
import { chromium } from "playwright";
import { resolve } from "@std/path";
import { assertEquals } from "@std/assert";
import { startOwnedProcesses, stopOwnedProcesses } from "../src/processes.ts";
const root = resolve(import.meta.dirname!, "../../..");
const phase = Deno.args[0] ?? "after";
const base = "http://127.0.0.1:15397";
const output = `${root}/target/web-ux/passkey-addition-code/${phase}`;
const processes = await startOwnedProcesses(
  [{
    id: "addition-code",
    command: "deno",
    cwd: root,
    args: [
      "run",
      "--config",
      "tools/web-ux/deno.json",
      "--allow-read",
      "--allow-net",
      "web/workspace/test/auth-browser/fixture-server.ts",
      "15397",
      "web/workspace/build",
    ],
    readyUrl: `${base}/health`,
  }],
  `${root}/tools/web-ux/scenarios/anonymous-entry.json`,
  `${output}/processes`,
  [],
);
let browser: Awaited<ReturnType<typeof chromium.launch>> | undefined;
try {
  browser = await chromium.launch(Deno.args[1] ? { executablePath: Deno.args[1] } : {});
  for (const theme of ["light", "dark"] as const) {
    for (const width of [1440, 768, 390]) {
      const context = await browser.newContext({
        viewport: { width, height: 1000 },
        colorScheme: theme,
        reducedMotion: "reduce",
      });
      const page = await context.newPage();
      const errors: string[] = [];
      const documentStatuses: number[] = [];
      page.on("pageerror", () => errors.push("pageerror"));
      page.on("requestfailed", (request) => {
        if (request.failure()?.errorText !== "net::ERR_ABORTED") {
          errors.push(`request failed: ${new URL(request.url()).pathname}`);
        }
      });
      page.on("console", (message) => {
        if (
          message.type() === "error" &&
          !(theme === "light" && width === 390 && message.text().includes("403"))
        ) errors.push("console error");
      });
      page.on("response", (response) => {
        if (response.request().isNavigationRequest()) documentStatuses.push(response.status());
      });
      let active: { issuance_id: string; target_origin: string; expires_at: string } | null = null;
      const user = {
        user_id: "fixture-user",
        account_id: "fixture-account",
        handle: "alice",
        display_name: "Alice",
      };
      const requests: string[] = [];
      let requireRecentAuth = theme === "light" && width === 390;
      if (phase === "after") {
        await page.route("**/api/auth/passkeys/addition-code**", async (route) => {
          const request = route.request();
          const path = new URL(request.url()).pathname;
          requests.push(`${request.method()} ${path}`);
          let value: unknown;
          if (path.endsWith("/revoke")) {
            active = null;
            value = { revoked: true };
          } else if (path.endsWith("/options")) {
            value = {
              challenge_id: "synthetic-code-challenge",
              public_key: {
                challenge: "dWk",
                rp: { name: "Yoi", id: "127.0.0.1" },
                user: { id: "dWk", name: "alice", displayName: "Alice" },
                pubKeyCredParams: [{ type: "public-key", alg: -7 }],
                authenticatorSelection: {
                  residentKey: "required",
                  requireResidentKey: true,
                  userVerification: "required",
                },
              },
            };
          } else if (path.endsWith("/complete")) {
            active = null;
            value = { user, authentication_reference: "fixture-authentication" };
          } else if (request.method() === "GET") {
            value = { user, public_urls: [base, "https://dev.example.test"], active };
          } else {
            if (requireRecentAuth) {
              requireRecentAuth = false;
              await route.fulfill({
                status: 403,
                json: {
                  error: "Forbidden",
                  diagnostics: [{ code: "passkey_recent_authentication_required" }],
                },
              });
              return;
            }
            active = {
              issuance_id: "synthetic-issuance",
              target_origin: request.postDataJSON().target_origin,
              expires_at: "2099-01-01T00:00:00Z",
            };
            value = { user, ...active, code: "synthetic-ui-code-not-a-secret" };
          }
          await route.fulfill({ json: value });
        });
      }
      await page.request.get(`${base}/?fixture=account`);
      await page.goto(`${base}/account`);
      await page.getByRole("heading", { name: "Account", exact: true }).waitFor({ timeout: 5000 })
        .catch(async () => {
          throw new Error(
            `Account unavailable: ${page.url()} ${await page.locator("body").innerText()} ${
              errors.join(",")
            }`,
          );
        });
      await page.getByRole("button", { name: "Add passkey to alice", exact: true }).waitFor();
      const dir = `${output}/${theme}-${width}`;
      await Deno.mkdir(dir, { recursive: true });
      const captures: unknown[] = [];
      async function capture(name: string) {
        const overflow = await page.evaluate(() =>
          document.documentElement.scrollWidth > innerWidth
        );
        assertEquals(overflow, false);
        const bytes = await page.screenshot({ path: `${dir}/${name}.png`, fullPage: true });
        const digest = await crypto.subtle.digest("SHA-256", new Uint8Array(bytes).buffer);
        captures.push({
          name,
          overflow,
          sha256: Array.from(new Uint8Array(digest), (byte) => byte.toString(16).padStart(2, "0"))
            .join(""),
          accessibility: await page.locator("body").ariaSnapshot(),
          visibleErrors: await page.locator('[role="alert"]:visible').allTextContents(),
        });
      }
      if (phase === "after") {
        await page.getByText("Add a passkey on another domain or device", { exact: true }).click();
        await page.getByLabel("Destination URL").selectOption(base);
        await page.getByRole("button", { name: "Generate addition code", exact: true }).click();
        if (theme === "light" && width === 390) {
          await page.getByText("Verify your identity with a passkey, then generate the code.", {
            exact: true,
          }).waitFor();
          await page.getByRole("button", { name: "Verify identity with passkey", exact: true })
            .click();
          await page.getByText("Identity verified. You can now generate an addition code.", {
            exact: true,
          }).waitFor();
          await page.getByText("Add a passkey on another domain or device", { exact: true })
            .click();
          await page.getByLabel("Destination URL").selectOption(base);
          await page.getByRole("button", { name: "Generate addition code", exact: true }).click();
        }
        await page.getByLabel("One-time addition code").waitFor();
        assertEquals(
          await page.getByLabel("One-time addition code").getAttribute("type"),
          "password",
        );
      }
      await capture("account");
      if (phase === "after") {
        await page.getByRole("link", { name: `${base}/account/add-passkey`, exact: true })
          .scrollIntoViewIfNeeded();
        await capture("issued-code");
        await page.getByRole("button", { name: "Revoke active code", exact: true }).click();
        await page.getByText("Code revoked. Existing passkeys and sessions are unchanged.", {
          exact: true,
        }).waitFor();
        await page.request.get(`${base}/?fixture=anonymous`);
        await page.goto(`${base}/account/add-passkey`);
        await page.getByRole("heading", { name: "Add passkey with a code", exact: true }).waitFor();
        await capture("redeem");
        await page.getByLabel("Account ID (handle)").fill("alice");
        await page.getByLabel("Addition code", { exact: true }).fill(
          "synthetic-ui-code-not-a-secret",
        );
        // Completion's synthetic cookie effect is projected by whoami, not by injecting a real credential.
        await page.route("**/api/auth/whoami", (route) =>
          route.fulfill({
            json: {
              actor: { ...user, auth_method: "browser_session" },
              authentication_reference: "fixture-authentication",
              session: { kind: "authenticated", expires_at: "2099-01-01T00:00:00Z" },
            },
          }));
        await page.getByRole("button", { name: "Add passkey to existing account", exact: true })
          .click();
        await page.getByText("Passkey added to alice. You are signed in on this domain.", {
          exact: true,
        }).waitFor();
        await capture("success");
        assertEquals(
          requests.filter((p) => p.endsWith("/options")).length,
          1,
        );
        assertEquals(requests.filter((p) => p.endsWith("/complete")).length, 1);
        assertEquals(
          await page.evaluate(() =>
            JSON.stringify(localStorage).includes("synthetic-ui-code-not-a-secret")
          ),
          false,
        );
      }
      const overflow = await page.evaluate(() => document.documentElement.scrollWidth > innerWidth);
      assertEquals(overflow, false);
      await Deno.writeTextFile(
        `${dir}/review-context.json`,
        JSON.stringify({
          phase,
          theme,
          width,
          documentStatuses,
          captures,
          errors,
          overflow,
          status: errors.length ? "fail" : "pass",
        }),
      );
      assertEquals(errors, []);
      await context.close();
    }
  }
} finally {
  await browser?.close();
  await stopOwnedProcesses(processes);
}

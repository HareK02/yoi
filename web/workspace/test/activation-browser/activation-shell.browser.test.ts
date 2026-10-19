/// <reference lib="dom" />
import { assert, assertEquals } from "@std/assert";
import { resolve } from "@std/path";
import { chromium, type Locator } from "playwright";
import {
  startOwnedProcesses,
  stopOwnedProcesses,
} from "../../../../tools/web-ux/src/processes.ts";
const root = resolve(import.meta.dirname!, "../../../..");
// 本番shellとclientを接続する合成HTTP/WebAuthn試験。Server認証/atomic保存のE2Eではない。
Deno.test("activation standalone shell supports issuance, reissue, denial, restart and confirmed-only activation", async () => {
  const processes = await startOwnedProcesses(
    [{
      id: "activation-browser",
      command: "deno",
      cwd: root,
      args: [
        "run",
        "--config",
        "tools/web-ux/deno.json",
        "--allow-read",
        "--allow-net",
        "web/workspace/test/activation-browser/fixture-server.ts",
        "15381",
        "web/workspace/build",
      ],
      readyUrl: "http://127.0.0.1:15381/health",
    }],
    `${root}/tools/web-ux/scenarios/hosting-settings-light.json`,
    `${root}/target/web-ux/activation-interactions/processes`,
    [],
  );
  const browser = await chromium.launch();
  try {
    for (const colorScheme of ["light", "dark"] as const) {
      for (const width of [1440, 768, 390]) {
        const context = await browser.newContext({
          viewport: { width, height: 900 },
          colorScheme,
          reducedMotion: "reduce",
        });
        try {
          const page = await context.newPage();
          const requested: string[] = [];
          const errors: string[] = [];
          page.on("request", (request) => {
            if (new URL(request.url()).pathname.startsWith("/api/")) {
              requested.push(
                `${request.method()} ${new URL(request.url()).pathname}`,
              );
            }
          });
          page.on("pageerror", () => errors.push("pageerror"));
          const go = async (path: string, mode: string) => {
            await page.goto(`http://127.0.0.1:15381${path}?fixture=${mode}`);
          };
          const noOverflow = async () =>
            assertEquals(
              await page.evaluate(() =>
                document.documentElement.scrollWidth > innerWidth
              ),
              false,
            );
          const linkStyle = (link: Locator) =>
            link.evaluate((element) => {
              const style = getComputedStyle(element);
              const probe = document.createElement("span");
              probe.hidden = true;
              element.append(probe);
              const token = (name: string) => {
                probe.style.color = `var(${name})`;
                return getComputedStyle(probe).color;
              };
              const result = {
                color: style.color,
                outline: style.outlineColor,
                width: style.outlineWidth,
                kind: style.outlineStyle,
                underline: style.textDecorationLine,
                strong: token("--text-strong"),
                text: token("--text"),
                accent: token("--accent"),
              };
              probe.remove();
              return result;
            });
          const checkLink = async (link: Locator) => {
            await page.mouse.move(0, 0);
            const normal = await linkStyle(link);
            assertEquals(normal.color, normal.strong);
            assert(normal.underline.includes("underline"));
            await link.hover();
            const hovered = await linkStyle(link);
            assertEquals(hovered.color, hovered.text);
            await page.mouse.move(0, 0);
            await page.keyboard.press("Tab");
            await link.focus();
            const focused = await linkStyle(link);
            assertEquals(focused.kind, "solid");
            assertEquals(focused.width, "2px");
            assertEquals(focused.outline, focused.accent);
          };
          await go("/hosting/settings", "admin");
          await page.getByRole("link", {
            name: "Create accounts or reissue activation codes",
          }).click();
          await page.getByLabel("Account ID (handle)").waitFor();
          await checkLink(
            page.getByRole("link", { name: "Hosting settings", exact: true }),
          );
          const handle = page.getByLabel("Account ID (handle)");
          await handle.fill(" ALICE ");
          const issue = page.getByRole("button", {
            name: "Create account and issue code",
          });
          await handle.press("Tab");
          assert(
            await issue.evaluate((el) =>
              el === document.activeElement &&
              getComputedStyle(el).outlineStyle !== "none"
            ),
          );
          await page.keyboard.press("Enter");
          await page.getByRole("heading", { name: "Activation code issued" })
            .waitFor();
          await checkLink(
            page.getByRole("link", { name: "/activate", exact: true }),
          );
          assertEquals(await page.locator(".secret").count(), 1);
          await noOverflow();
          await page.getByRole("button", { name: "Hide code" }).click();
          assertEquals(await page.locator(".secret").count(), 0);
          await page.getByLabel("Operation").selectOption("reissue");
          await page.getByRole("button", { name: "Reissue activation code" })
            .click();
          await page.getByRole("heading", { name: "Activation code issued" })
            .waitFor();
          assert(
            requested.includes("POST /api/hosting/accounts/activation-code"),
          );
          for (const mode of ["anonymous", "ordinary"]) {
            const before = requested.filter((path) =>
              path.startsWith("POST ")
            ).length;
            await go("/hosting/accounts", mode);
            if (mode === "anonymous") {
              await page.waitForURL("http://127.0.0.1:15381/");
              await page.getByRole("heading", { name: "Yoi", exact: true })
                .waitFor();
            } else {await page.getByText(
                "Hosting administrator access is required.",
              ).waitFor();}
            assertEquals(
              await page.getByLabel("Account ID (handle)").count(),
              0,
            );
            assertEquals(
              requested.filter((path) => path.startsWith("POST ")).length,
              before,
            );
            await noOverflow();
          }
          const fillActivation = async () => {
            await page.getByLabel("Account ID (handle)").fill("alice");
            await page.getByLabel("Activation code", { exact: true }).fill(
              "synthetic-input-only",
            );
            await page.getByRole("button", {
              name: "Register Passkey and activate",
            }).click();
          };
          await go("/activate", "activation-pending");
          await page.getByLabel("Activation code", { exact: true }).waitFor();
          await checkLink(
            page.getByRole("link", {
              name:
                "Already registered or completion uncertain? Log in with a Passkey",
            }),
          );
          await fillActivation();
          await page.getByText(
            "Passkey registration pending. Account activation has not been confirmed.",
          ).waitFor();
          assertEquals(
            await page.getByLabel("Activation code", { exact: true })
              .inputValue(),
            "",
          );
          assertEquals(
            requested.filter((path) =>
              path === "POST /api/auth/activation/complete"
            ).length,
            0,
          );
          await page.getByRole("button", { name: "Cancel registration" })
            .click();
          await page.getByRole("alert").waitFor();
          assertEquals(await page.getByText(/You are signed in/).count(), 0);
          await noOverflow();
          await page.reload();
          await page.getByRole("button", {
            name: "Register Passkey and activate",
          }).waitFor();
          assertEquals(
            await page.getByLabel("Activation code", { exact: true })
              .inputValue(),
            "",
          );
          for (
            const mode of [
              "activation-cancel",
              "activation-unknown",
              "activation-reject",
            ]
          ) {
            await go("/activate", mode);
            await fillActivation();
            await page.getByRole("alert").waitFor();
            assertEquals(await page.getByText(/You are signed in/).count(), 0);
            await noOverflow();
          }
          await go("/activate", "activation-success");
          await fillActivation();
          await page.getByText(
            "Passkey registration and activation confirmed for alice. You are signed in.",
          ).waitFor();
          assertEquals(await page.locator(".app-shell").count(), 0);
          await noOverflow();
          assertEquals(
            requested.some((path) => path.includes("/api/workspaces")),
            false,
          );
          assertEquals(
            requested.some((path) =>
              path.includes("/addition/") || path.includes("/registration/") ||
              path.includes("/device")
            ),
            false,
          );
          assertEquals(
            await page.evaluate(() =>
              [...Object.values(localStorage), ...Object.values(sessionStorage)]
                .some((value) => value.includes("synthetic-"))
            ),
            false,
          );
          assertEquals(new URL(page.url()).searchParams.has("code"), false);
          assertEquals(errors, []);
        } finally {
          await context.close();
        }
      }
    }
  } finally {
    await browser.close();
    await stopOwnedProcesses(processes);
  }
});

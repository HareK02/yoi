#!/usr/bin/env -S deno run -A
/** 実Web/実WASMを合成HTTPで操作する。認証・保存の証拠はServer suiteが所有する。 */
import { assert, assertEquals } from "@std/assert";
import { dirname, fromFileUrl, join, resolve } from "@std/path";
import { createHash } from "node:crypto";
import { chromium, type Page } from "playwright";
import { sha256File } from "../src/artifacts.ts";
import { startOwnedProcesses, stopOwnedProcesses } from "../src/processes.ts";
import { assertNoPageWideOverflow, freePort } from "./memory_document_harness.ts";
import init, {
  apply_changes,
  compose_schema_bundle,
  set_snapshot,
} from "../../../web/workspace/src/lib/workspace/config-source/generated/config_source_wasm.js";
import type {
  ConfigTreeChange,
  ConfigTreeSnapshot,
  WorkspaceConfigSchemaBundle,
  WorkspaceConfigTreeResponse,
} from "../../../web/workspace/src/lib/workspace/config-source/types.ts";

const root = resolve(dirname(fromFileUrl(import.meta.url)), "../../..");
const buildRoot = resolve(Deno.args[0] ?? join(root, "web/workspace/build"));
const runId = Deno.args[1];
const before = Deno.args[2] === "before";
if (!runId || !/^[a-zA-Z0-9._-]+$/.test(runId)) throw new Error("supply unique safe run id");
const output = join(root, "target/web-ux/t838", runId);
await Deno.mkdir(dirname(output), { recursive: true });
await Deno.mkdir(output);
await init({
  module_or_path: await Deno.readFile(
    join(
      root,
      "web/workspace/src/lib/workspace/config-source/generated/config_source_wasm_bg.wasm",
    ),
  ),
});
const hash = (s: string) => `sha256:${createHash("sha256").update(s).digest("hex")}`;
const schema = compose_schema_bundle([]) as WorkspaceConfigSchemaBundle;
const content =
  "{\n    permissions = { subjects = []; remove_subjects = []; };\n} as WorkspaceConfigSchema\n";
const treeHash = createHash("sha256").update("yoi-config-tree-v1\0").update(
  "main.dcdl\0text/x-decodal\0",
).update(content).update("\0");
const initial: WorkspaceConfigTreeResponse = {
  snapshot: {
    digest: `sha256:${treeHash.digest("hex")}`,
    entries: {
      "main.dcdl": {
        path: "main.dcdl",
        content_type: "decodal",
        content,
        content_digest: hash(content),
      },
    },
  },
  contract: {
    contract_version: 2,
    decodal_version: "0.4.0",
    schema_version: 1,
    entrypoints: ["main.dcdl"],
    import_policy_version: 1,
    schema_bundle: schema,
    fingerprint: hash(JSON.stringify([2, "0.4.0", 1, ["main.dcdl"], 1, schema.fingerprint])),
  },
  projection_digest: hash("synthetic-empty-policy-projection"),
};
const port = await freePort();
const base = `http://127.0.0.1:${port}`;
const owned = await startOwnedProcesses(
  [{
    id: "configuration-synthetic-http",
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
  evidence: "actual-Web-and-WASM-with-synthetic-HTTP-NOT-real-Backend",
  before,
  source: new TextDecoder().decode(source.stdout).trim(),
  buildRoot,
  buildIndexSha256: await sha256File(join(buildRoot, "index.html")),
  visualVerdict: "UNREVIEWED",
  captures: [] as unknown[],
  failures: [] as string[],
};
const browser = await chromium.launch({ headless: true });
const documents = new WeakMap<Page, number>();
async function capture(page: Page, id: string, errors: unknown[], expectedErrors: string[] = []) {
  if (id.endsWith("-unknown")) {
    await page.locator(".config-source-conflict").scrollIntoViewIfNeeded();
  } else if (id.endsWith("-late-receipt")) {
    await page.locator(".config-source-status").scrollIntoViewIfNeeded();
  }
  const image = join(output, `${id}.png`);
  await page.screenshot({ path: image });
  const accessibilitySnapshot = `${id}.aria.txt`;
  await Deno.writeTextFile(
    join(output, accessibilitySnapshot),
    await page.locator("body").ariaSnapshot(),
  );
  manifest.captures.push({
    id,
    route: new URL(page.url()).pathname,
    documentStatus: documents.get(page) ?? null,
    viewport: page.viewportSize(),
    image,
    imageSha256: await sha256File(image),
    accessibilitySnapshot,
    visibleAlerts: await page.getByRole("alert").allTextContents(),
    errors: [...errors],
    expectedErrors,
  });
  await assertNoPageWideOverflow(page);
}
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
                display_name: "Configuration review",
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
          data.permissions.delete_subjects = !route.request().url().includes("home-member");
          await route.fulfill({ json: data });
        });
        await context.route("**/*.wasm", async (route) => {
          const response = await route.fetch();
          await route.fulfill({
            response,
            headers: { ...response.headers(), "content-type": "application/wasm" },
          });
        });
        const page = await context.newPage();
        const errors: { kind: string; message: string }[] = [];
        page.on("response", (r) => {
          if (r.request().isNavigationRequest()) documents.set(page, r.status());
          if (r.status() >= 400) {
            errors.push({ kind: "http", message: `${r.status()} ${new URL(r.url()).pathname}` });
          }
        });
        page.on("pageerror", (e) => errors.push({ kind: "page", message: e.message }));
        page.on("console", (e) => {
          if (e.type() === "error" || e.type() === "warning") {
            errors.push({ kind: "console", message: e.text() });
          }
        });
        page.on(
          "requestfailed",
          (r) =>
            errors.push({
              kind: "request",
              message: `${r.method()} ${new URL(r.url()).pathname}: ${r.failure()?.errorText}`,
            }),
        );
        let state = structuredClone(initial);
        let request: unknown = null;
        let savedReceipt: unknown = null;
        let head = "head:observed";
        const prefix = `${colorScheme}-${width}`;
        await page.route("**/api/w/*/config/**", async (route) => {
          const path = new URL(route.request().url()).pathname;
          if (path.includes("/home-member/")) {
            return await route.fulfill({ status: 403, body: "Workspace owner required" });
          }
          if (path.endsWith("/source-tree")) return await route.fulfill({ json: state });
          if (path.endsWith("/adoption")) {
            return await route.fulfill({
              json: {
                state,
                history_head: head,
                source_stored: true,
                catalog: { workspace: "home-owner", namespaces: [] },
                catalog_digest: hash("catalog"),
                expected_legacy_export: null,
                policies: [],
                migration_diagnostics: [],
              },
            });
          }
          const input = route.request().postDataJSON();
          if (path.endsWith("/preview")) {
            set_snapshot(state.snapshot);
            const candidate = apply_changes(
              input.source.changes as ConfigTreeChange[],
            ) as ConfigTreeSnapshot;
            return await route.fulfill({
              json: {
                expected_head: head,
                state: { ...state, snapshot: candidate },
                changed_paths: ["module.dcdl"],
                changed_subjects: [],
              },
            });
          }
          if (path.endsWith("/commit")) {
            request = input;
            set_snapshot(state.snapshot);
            state = {
              ...state,
              snapshot: apply_changes(input.source.changes) as ConfigTreeSnapshot,
            };
            head = "head:adopted";
            savedReceipt = {
              workspace_id: "home-owner",
              operation_id: input.operation_id,
              history_head: head,
              state,
              source_stored: true,
              policy_committed: true,
              runtime_application: "next_current_policy_check",
            };
            return await route.abort("failed");
          }
          if (path.endsWith("/reconcile")) {
            assertEquals(input, request);
            return await route.fulfill({ json: { status: "not_observed" } });
          }
          throw new Error(`unexpected config request: ${path}`);
        });
        await page.goto(`${base}/w/home-owner/settings/configuration`);
        await page.locator(".cm-content[contenteditable=true]").waitFor();
        await page.locator(".config-source-shell").scrollIntoViewIfNeeded();
        await capture(page, `${prefix}-ready`, errors);
        assertEquals(errors, []);
        if (!before) {
          await page.getByRole("button", { name: "Create local source" }).click();
          const commit = page.getByRole("button", { name: "Commit", exact: true });
          await commit.click();
          await page.getByRole("button", { name: "Reconcile adoption" }).waitFor();
          await page.getByText(/not observed does not mean failed/).first().waitFor();
          await capture(page, `${prefix}-unknown`, errors, [
            "Intentional failed POST commit response",
          ]);
          // 後続draftを編集してから古いreceiptを返し、実editorでも消えないことを確認する。
          const editor = page.locator(".cm-content");
          await editor.fill("{ later = true; }\n");
          await page.unroute("**/api/w/*/config/**");
          await page.route("**/api/w/*/config/adoption/reconcile", async (route) => {
            assertEquals(route.request().postDataJSON(), request);
            await route.fulfill({ json: { status: "adopted", receipt: savedReceipt } });
          });
          await page.getByRole("button", { name: "Reconcile adoption" }).click();
          await page.getByText(/Later local edits remain unsaved/).waitFor();
          assert((await editor.innerText()).includes("later"));
          await capture(page, `${prefix}-late-receipt`, errors, [
            "Intentional failed POST commit response",
          ]);
        }
        await page.route(
          "**/api/w/home-member/config/**",
          (route) => route.fulfill({ status: 403, body: "Workspace owner required" }),
        );
        await page.goto(`${base}/w/home-member/settings/configuration`);
        await page.getByRole("alert").filter({ hasText: "Unable to load workspace configuration" })
          .waitFor();
        assertEquals(await page.locator(".cm-content").count(), 0);
        await capture(page, `${prefix}-non-owner`, errors, [
          "HTTP403 owner rejection",
          ...(!before ? ["Intentional failed POST commit response"] : []),
        ]);
      } catch (error) {
        const page = context.pages()[0];
        if (page) {
          await page.screenshot({ path: join(output, `failure-${colorScheme}-${width}.png`) });
          await Deno.writeTextFile(
            join(output, `failure-${colorScheme}-${width}.txt`),
            await page.locator("body").innerText(),
          );
        }
        throw error;
      } finally {
        await context.close();
      }
    }
  }
} catch (e) {
  manifest.failures.push(String(e));
  throw e;
} finally {
  await browser.close();
  await stopOwnedProcesses(owned);
  await Deno.writeTextFile(join(output, "review-context.json"), JSON.stringify(manifest, null, 2));
}

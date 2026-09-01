import { basename, dirname, join, relative, resolve } from "@std/path";
import { type Browser, chromium, type Page, type Response } from "playwright";
import {
  assertBundleIsSecretFree,
  bounded,
  ensurePrivateDirectory,
  makePrivate,
  redactText,
  safeUrl,
  sha256File,
} from "./artifacts.ts";
import { type RunningProcess, startOwnedProcesses, stopOwnedProcesses } from "./processes.ts";
import {
  interpolateEnvironment,
  loadScenario,
  resolveScenarioPath,
  validateBaseUrl,
} from "./scenario.ts";
import type {
  CaptureError,
  CaptureEvidence,
  CapturePoint,
  Interaction,
  Persona,
  ReadyCondition,
  ReviewContext,
  RouteScenario,
  Scenario,
  ScreenshotEvidence,
  Viewport,
} from "./types.ts";

export type CaptureOptions = {
  scenarioPath: string;
  outputDirectory: string;
  baseUrl?: string;
  runId?: string;
  personas?: string[];
  routes?: string[];
  viewports?: string[];
  headed?: boolean;
};

type SourceState = { revision: string | null; dirty: boolean | null };

function slug(value: string): string {
  return value.replaceAll(/[^a-zA-Z0-9.-]+/g, "-").replaceAll(/^-+|-+$/g, "").toLowerCase();
}

function viewportId(viewport: Viewport): string {
  return viewport.label ?? `${viewport.width}x${viewport.height}`;
}

function timestampId(): string {
  return new Date().toISOString().replaceAll(/[:.]/g, "-");
}

async function sourceState(): Promise<SourceState> {
  try {
    const [revision, status] = await Promise.all([
      new Deno.Command("git", { args: ["rev-parse", "HEAD"], stdout: "piped", stderr: "null" })
        .output(),
      new Deno.Command("git", { args: ["status", "--porcelain"], stdout: "piped", stderr: "null" })
        .output(),
    ]);
    return {
      revision: revision.success ? new TextDecoder().decode(revision.stdout).trim() : null,
      dirty: status.success ? new TextDecoder().decode(status.stdout).trim().length > 0 : null,
    };
  } catch {
    return { revision: null, dirty: null };
  }
}

function selectById<T extends { id: string }>(
  values: T[],
  requested: string[] | undefined,
  kind: string,
): T[] {
  if (!requested || requested.length === 0) return values;
  const requestedSet = new Set(requested);
  const selected = values.filter((value) => requestedSet.has(value.id));
  const missing = [...requestedSet].filter((id) => !selected.some((value) => value.id === id));
  if (missing.length > 0) throw new Error(`unknown ${kind}: ${missing.join(", ")}`);
  return selected;
}

function selectViewports(values: Viewport[], requested: string[] | undefined): Viewport[] {
  if (!requested || requested.length === 0) return values;
  const requestedSet = new Set(requested);
  const selected = values.filter((value) => requestedSet.has(viewportId(value)));
  const missing = [...requestedSet].filter((id) =>
    !selected.some((value) => viewportId(value) === id)
  );
  if (missing.length > 0) throw new Error(`unknown viewports: ${missing.join(", ")}`);
  return selected;
}

function responseMatches(
  response: Response,
  ready: Extract<ReadyCondition, { kind: "response" }>,
): boolean {
  const pattern = new RegExp(ready.urlPattern);
  return pattern.test(response.url()) &&
    (ready.status === undefined || response.status() === ready.status);
}

async function waitReady(
  page: Page,
  ready: ReadyCondition,
  navigation?: Response | null,
): Promise<void> {
  const timeout = ready.timeoutMs ?? 15_000;
  if (ready.kind === "selector") {
    await page.locator(ready.selector).first().waitFor({ state: "visible", timeout });
    return;
  }
  if (ready.kind === "network-idle") {
    await page.waitForLoadState("networkidle", { timeout });
    return;
  }
  if (navigation && responseMatches(navigation, ready)) return;
  await page.waitForResponse((response) => responseMatches(response, ready), { timeout });
}

async function performInteraction(page: Page, interaction: Interaction): Promise<void> {
  if (interaction.action === "wait") return await waitReady(page, interaction.ready);
  const locator = page.locator(interaction.selector).first();
  const timeout = interaction.timeoutMs ?? 10_000;
  if (interaction.action === "click") return await locator.click({ timeout });
  if (interaction.action === "fill") {
    return await locator.fill(interpolateEnvironment(interaction.value), { timeout });
  }
  await locator.press(interaction.key, { timeout });
}

async function retry<T>(label: string, operation: () => Promise<T>): Promise<T> {
  let last: unknown;
  for (let attempt = 1; attempt <= 2; attempt++) {
    try {
      return await operation();
    } catch (error) {
      last = error;
      if (attempt < 2) await new Promise((resolve) => setTimeout(resolve, 350));
    }
  }
  throw new Error(
    `${label} failed after 2 attempts: ${last instanceof Error ? last.message : String(last)}`,
  );
}

async function hideRedactedSelectors(page: Page, selectors: string[]): Promise<void> {
  if (selectors.length === 0) return;
  const escaped = selectors.join(",\n");
  await page.addStyleTag({ content: `${escaped} { visibility: hidden !important; }` });
}

export function isVisibleUiErrorText(content: string): boolean {
  return /\b(error|failed|unauthorized|forbidden|not found)\b/i.test(content);
}

async function collectVisibleUiErrors(
  page: Page,
  errors: CaptureError[],
  secrets: string[],
): Promise<void> {
  const alerts = page.locator('[role="alert"], [aria-live="assertive"]');
  for (let index = 0; index < await alerts.count(); index++) {
    const alert = alerts.nth(index);
    if (!await alert.isVisible().catch(() => false)) continue;
    const content = (await alert.innerText().catch(() => "")).trim();
    if (!isVisibleUiErrorText(content)) continue;
    const message = `visible UI error: ${bounded(redactText(content, secrets), 500)}`;
    if (!errors.some((error) => error.kind === "document" && error.message === message)) {
      errors.push({ kind: "document", message });
    }
  }
}

async function capturePoint(
  page: Page,
  runDirectory: string,
  persona: Persona,
  route: RouteScenario,
  viewport: Viewport,
  point: CapturePoint,
  documentResponse: Response | null,
  errors: CaptureError[],
  scenario: Scenario,
): Promise<CaptureEvidence> {
  const startedAt = new Date().toISOString();
  for (const interaction of point.interaction ?? []) await performInteraction(page, interaction);
  if (point.ready) await waitReady(page, point.ready);
  await hideRedactedSelectors(page, scenario.redact?.selectors ?? []);
  await collectVisibleUiErrors(page, errors, scenario.redact?.text ?? []);
  const directory = join(
    runDirectory,
    "captures",
    persona.id,
    route.id,
    viewportId(viewport),
    point.id,
  );
  await ensurePrivateDirectory(directory);
  const viewportScreenshot = join(directory, "viewport.png");
  await page.screenshot({ path: viewportScreenshot, fullPage: false, animations: "disabled" });
  await makePrivate(viewportScreenshot);
  const screenshots: ScreenshotEvidence[] = [{
    kind: "viewport",
    path: relative(runDirectory, viewportScreenshot),
    sha256: await sha256File(viewportScreenshot),
  }];
  if (point.fullPage) {
    const fullPageScreenshot = join(directory, "full-page.png");
    await page.screenshot({ path: fullPageScreenshot, fullPage: true, animations: "disabled" });
    await makePrivate(fullPageScreenshot);
    screenshots.push({
      kind: "full-page",
      path: relative(runDirectory, fullPageScreenshot),
      sha256: await sha256File(fullPageScreenshot),
    });
  }
  let snapshotPath: string | null = null;
  try {
    const snapshot = await page.locator("body").ariaSnapshot({ timeout: 5_000 });
    const redacted = redactText(snapshot, scenario.redact?.text ?? []);
    const target = join(directory, "accessibility.md");
    await Deno.writeTextFile(target, redacted, { mode: 0o600 });
    snapshotPath = relative(runDirectory, target);
  } catch (error) {
    errors.push({
      kind: "tool",
      message: `accessibility snapshot failed: ${
        bounded(error instanceof Error ? error.message : String(error))
      }`,
    });
  }
  return {
    persona: { id: persona.id, label: persona.label },
    route: { id: route.id, path: route.path, goal: route.goal, dataState: route.dataState },
    viewport,
    theme: scenario.colorScheme ?? "light",
    capturePoint: { id: point.id, label: point.label },
    document: { url: safeUrl(page.url()), status: documentResponse?.status() ?? null },
    screenshots,
    snapshotPath,
    errors: [...errors],
    startedAt,
    finishedAt: new Date().toISOString(),
  };
}

function screenshotDataUrl(bytes: Uint8Array): string {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return `data:image/png;base64,${btoa(binary)}`;
}

function escapeHtml(value: string): string {
  return value.replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;").replaceAll(
    '"',
    "&quot;",
  );
}

async function createContactSheet(
  browser: Browser,
  runDirectory: string,
  captures: CaptureEvidence[],
): Promise<{ html: string | null; png: string | null }> {
  const cells: string[] = [];
  for (const capture of captures) {
    const screenshot = capture.screenshots.find((item) => item.kind === "viewport") ??
      capture.screenshots[0];
    if (!screenshot) continue;
    const bytes = await Deno.readFile(join(runDirectory, screenshot.path));
    cells.push(
      `<figure><img src="${screenshotDataUrl(bytes)}"><figcaption><strong>${
        escapeHtml(capture.persona.label)
      } · ${escapeHtml(capture.route.id)}</strong><br>${
        escapeHtml(viewportId(capture.viewport))
      } · ${escapeHtml(capture.capturePoint.label)}<br><small>${
        escapeHtml(capture.route.dataState)
      }</small>${
        capture.errors.length > 0
          ? `<br><strong class="errors">${capture.errors.length} captured error(s)</strong>`
          : ""
      }</figcaption></figure>`,
    );
  }
  if (cells.length === 0) return { html: null, png: null };
  const html =
    `<!doctype html><meta charset="utf-8"><title>Web UX review contact sheet</title><style>body{margin:0;padding:20px;background:#e8e8e8;color:#111;font:14px system-ui}main{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:20px}figure{margin:0;background:white;border:1px solid #aaa;padding:10px;box-shadow:0 2px 8px #0002}img{width:100%;height:auto;display:block;border:1px solid #ddd}figcaption{padding-top:8px;line-height:1.45}small{color:#555}.errors{color:#b42318}</style><main>${
      cells.join("")
    }</main>`;
  const htmlPath = join(runDirectory, "contact-sheet.html");
  const pngPath = join(runDirectory, "contact-sheet.png");
  await Deno.writeTextFile(htmlPath, html, { mode: 0o600 });
  const page = await browser.newPage({ viewport: { width: 1600, height: 1000 } });
  try {
    await page.setContent(html, { waitUntil: "load" });
    await page.screenshot({ path: pngPath, fullPage: true, animations: "disabled" });
    await makePrivate(pngPath);
  } finally {
    await page.close();
  }
  return { html: relative(runDirectory, htmlPath), png: relative(runDirectory, pngPath) };
}

export async function capture(options: CaptureOptions): Promise<ReviewContext> {
  const scenarioPath = resolve(options.scenarioPath);
  const scenario = await loadScenario(scenarioPath);
  const baseUrl = validateBaseUrl(
    interpolateEnvironment(
      options.baseUrl ?? Deno.env.get("WEB_UX_BASE_URL") ?? scenario.baseUrl ?? "",
    ),
  );
  const personas = selectById(scenario.personas, options.personas, "personas");
  const routes = selectById(scenario.routes, options.routes, "routes");
  const viewports = selectViewports(scenario.viewports, options.viewports);
  const runId = slug(options.runId ?? `${scenario.id}-${timestampId()}`);
  const runDirectory = resolve(options.outputDirectory, runId);
  try {
    await Deno.stat(runDirectory);
    throw new Error(`run directory already exists: ${runDirectory}`);
  } catch (error) {
    if (!(error instanceof Deno.errors.NotFound)) throw error;
  }
  await ensurePrivateDirectory(runDirectory);
  const secrets = scenario.redact?.text ?? [];
  let browser: Browser | null = null;
  let processes: RunningProcess[] = [];
  const captures: CaptureEvidence[] = [];
  const diagnostics: CaptureError[] = [];
  let contactSheet = { html: null as string | null, png: null as string | null };
  let browserVersion = "unknown";
  let status: ReviewContext["status"] = "completed";
  try {
    processes = await startOwnedProcesses(
      scenario.processes ?? [],
      scenarioPath,
      join(runDirectory, "process-logs"),
      secrets,
    );
    browser = await chromium.launch({ headless: !options.headed });
    browserVersion = browser.version();
    for (const persona of personas) {
      const storageState = persona.auth.kind === "storage-state"
        ? resolveScenarioPath(scenarioPath, persona.auth.path)
        : undefined;
      if (storageState) await Deno.stat(storageState);
      for (const viewport of viewports) {
        const context = await browser.newContext({
          storageState,
          viewport: { width: viewport.width, height: viewport.height },
          deviceScaleFactor: viewport.deviceScaleFactor ?? 1,
          locale: scenario.locale,
          timezoneId: scenario.timezone,
          colorScheme: scenario.colorScheme,
          reducedMotion: scenario.reducedMotion,
        });
        try {
          for (const route of routes) {
            const routeErrors: CaptureError[] = [];
            const page = await context.newPage();
            page.on("console", (message) => {
              if (message.type() === "error") {
                routeErrors.push({
                  kind: "console",
                  message: bounded(redactText(message.text(), secrets)),
                });
              }
            });
            page.on(
              "pageerror",
              (error) =>
                routeErrors.push({
                  kind: "page",
                  message: bounded(redactText(error.message, secrets)),
                }),
            );
            page.on(
              "requestfailed",
              (request) =>
                routeErrors.push({
                  kind: "request",
                  message: bounded(
                    redactText(request.failure()?.errorText ?? "request failed", secrets),
                  ),
                  url: safeUrl(request.url()),
                }),
            );
            page.on("response", (response) => {
              if (response.status() >= 400) {
                routeErrors.push({
                  kind: "request",
                  message: `HTTP ${response.status()}`,
                  url: safeUrl(response.url()),
                  status: response.status(),
                });
              }
            });
            try {
              const routePath = interpolateEnvironment(route.path);
              const targetUrl = new URL(routePath, `${baseUrl}/`).toString();
              const response = await retry(`navigate ${route.id}`, async () => {
                const ready = route.ready;
                const responseReady = ready.kind === "response"
                  ? page.waitForResponse(
                    (candidate) => responseMatches(candidate, ready),
                    { timeout: ready.timeoutMs ?? 15_000 },
                  )
                  : null;
                try {
                  const navigation = await page.goto(targetUrl, {
                    waitUntil: "domcontentloaded",
                    timeout: 20_000,
                  });
                  if (responseReady) await responseReady;
                  else await waitReady(page, ready, navigation);
                  return navigation;
                } catch (error) {
                  responseReady?.catch(() => undefined);
                  throw error;
                }
              });
              if (response && response.status() >= 400) {
                routeErrors.push({
                  kind: "document",
                  message: `document returned HTTP ${response.status()}`,
                  url: safeUrl(response.url()),
                  status: response.status(),
                });
              }
              for (const point of route.capturePoints) {
                captures.push(
                  await capturePoint(
                    page,
                    runDirectory,
                    persona,
                    route,
                    viewport,
                    point,
                    response,
                    routeErrors,
                    scenario,
                  ),
                );
              }
            } catch (error) {
              status = "completed-with-errors";
              routeErrors.push({
                kind: "tool",
                message: bounded(
                  redactText(error instanceof Error ? error.message : String(error), secrets),
                ),
              });
              captures.push({
                persona: { id: persona.id, label: persona.label },
                route: {
                  id: route.id,
                  path: route.path,
                  goal: route.goal,
                  dataState: route.dataState,
                },
                viewport,
                theme: scenario.colorScheme ?? "light",
                capturePoint: { id: "failed", label: "Capture failed" },
                document: { url: safeUrl(page.url()), status: null },
                screenshots: [],
                snapshotPath: null,
                errors: routeErrors,
                startedAt: new Date().toISOString(),
                finishedAt: new Date().toISOString(),
              });
            } finally {
              await page.close();
            }
          }
        } finally {
          await context.close();
        }
      }
    }
    contactSheet = await createContactSheet(browser, runDirectory, captures);
    if (captures.some((capture) => capture.errors.length > 0)) status = "completed-with-errors";
  } catch (error) {
    status = "failed";
    diagnostics.push({
      kind: "tool",
      message: bounded(redactText(error instanceof Error ? error.message : String(error), secrets)),
    });
  } finally {
    if (browser) {
      await browser.close().catch((error) =>
        diagnostics.push({
          kind: "tool",
          message: `browser cleanup failed: ${bounded(String(error))}`,
        })
      );
    }
    diagnostics.push(...await stopOwnedProcesses(processes));
  }
  if (diagnostics.length > 0 && status === "completed") status = "completed-with-errors";
  const manifest: ReviewContext = {
    schemaVersion: 1,
    runId,
    scenario: {
      id: scenario.id,
      title: scenario.title,
      sourcePath: relative(Deno.cwd(), scenarioPath),
    },
    source: await sourceState(),
    baseUrl: safeUrl(baseUrl),
    browser: { name: "chromium", version: browserVersion },
    createdAt: new Date().toISOString(),
    status,
    filters: {
      personas: personas.map((item) => item.id),
      routes: routes.map((item) => item.id),
      viewports: viewports.map(viewportId),
    },
    captures,
    contactSheet,
    diagnostics,
  };
  const serialized = `${JSON.stringify(manifest, null, 2)}\n`;
  assertBundleIsSecretFree(serialized, secrets);
  await Deno.writeTextFile(join(runDirectory, "review-context.json"), serialized, { mode: 0o600 });
  if (status === "failed") {
    throw new Error(`capture failed; inspect ${join(runDirectory, "review-context.json")}`);
  }
  return manifest;
}

export function describeCapture(manifest: ReviewContext, outputDirectory: string): string {
  return `${manifest.status}: ${manifest.captures.length} capture(s); ${
    join(outputDirectory, manifest.runId, "review-context.json")
  }`;
}

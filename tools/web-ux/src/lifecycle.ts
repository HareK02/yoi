import { dirname, resolve } from "@std/path";
import { chromium } from "playwright";
import { ensurePrivateDirectory, makePrivate, writePrivateJson } from "./artifacts.ts";
import { deleteAuthState, writeAuthMetadata } from "./auth_state.ts";
import {
  interpolateEnvironment,
  loadScenario,
  resolveScenarioPath,
  validateBaseUrl,
} from "./scenario.ts";

export type AuthOptions = {
  scenarioPath: string;
  personaId: string;
  baseUrl?: string;
  importState?: string;
  timeoutMs?: number;
  expiresInHours?: number;
  delete?: boolean;
  headless?: boolean;
};

function validateStorageState(value: unknown): { cookies: unknown[]; origins: unknown[] } {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new Error("storage state must be an object");
  }
  const source = value as Record<string, unknown>;
  if (!Array.isArray(source.cookies) || !Array.isArray(source.origins)) {
    throw new Error("storage state must contain cookies and origins arrays");
  }
  return { cookies: source.cookies, origins: source.origins };
}

export async function authenticate(options: AuthOptions): Promise<string> {
  const scenarioPath = resolve(options.scenarioPath);
  const scenario = await loadScenario(scenarioPath);
  const persona = scenario.personas.find((candidate) => candidate.id === options.personaId);
  if (!persona) throw new Error(`unknown persona: ${options.personaId}`);
  if (persona.auth.kind !== "storage-state") {
    throw new Error(`persona ${persona.id} is anonymous and has no auth state`);
  }
  const outputPath = resolveScenarioPath(scenarioPath, persona.auth.path);
  if (options.delete) {
    await deleteAuthState(outputPath);
    return outputPath;
  }
  await ensurePrivateDirectory(dirname(outputPath));
  const baseUrl = validateBaseUrl(
    interpolateEnvironment(
      options.baseUrl ?? Deno.env.get("WEB_UX_BASE_URL") ?? scenario.baseUrl ?? "",
    ),
  );
  const expiresInHours = options.expiresInHours ?? 12;
  if (options.importState) {
    const imported = validateStorageState(
      JSON.parse(await Deno.readTextFile(resolve(options.importState))),
    );
    await writePrivateJson(outputPath, imported);
    await writeAuthMetadata(outputPath, persona.id, baseUrl, expiresInHours);
    return outputPath;
  }
  if (!persona.login) {
    throw new Error(`persona ${persona.id} needs login configuration or --import-state`);
  }
  const browser = await chromium.launch({ headless: options.headless ?? false });
  try {
    const context = await browser.newContext();
    const page = await context.newPage();
    const loginUrl = new URL(interpolateEnvironment(persona.login.path ?? "/"), `${baseUrl}/`)
      .toString();
    await page.goto(loginUrl, { waitUntil: "domcontentloaded", timeout: 30_000 });
    const success = new RegExp(interpolateEnvironment(persona.login.successUrl));
    if (!success.test(page.url())) {
      await page.waitForURL((url) => success.test(url.toString()), {
        timeout: options.timeoutMs ?? 300_000,
      });
    }
    await context.storageState({ path: outputPath });
    await makePrivate(outputPath);
    await writeAuthMetadata(outputPath, persona.id, baseUrl, expiresInHours);
    return outputPath;
  } finally {
    await browser.close();
  }
}

export type CleanupOptions = {
  outputDirectory: string;
  keep: number;
  olderThanDays?: number;
  dryRun?: boolean;
};

export async function cleanup(options: CleanupOptions): Promise<string[]> {
  const outputDirectory = resolve(options.outputDirectory);
  const candidates: { path: string; modified: number }[] = [];
  try {
    for await (const entry of Deno.readDir(outputDirectory)) {
      if (!entry.isDirectory) continue;
      const path = resolve(outputDirectory, entry.name);
      try {
        await Deno.stat(resolve(path, "review-context.json"));
        const stat = await Deno.stat(path);
        candidates.push({ path, modified: stat.mtime?.getTime() ?? 0 });
      } catch {
        // Only complete review bundle directories are owned by cleanup.
      }
    }
  } catch (error) {
    if (error instanceof Deno.errors.NotFound) return [];
    throw error;
  }
  candidates.sort((left, right) => right.modified - left.modified);
  const cutoff = options.olderThanDays === undefined
    ? Number.POSITIVE_INFINITY
    : Date.now() - options.olderThanDays * 24 * 60 * 60 * 1000;
  const removed: string[] = [];
  for (const [index, candidate] of candidates.entries()) {
    if (index < options.keep || candidate.modified > cutoff) continue;
    removed.push(candidate.path);
    if (!options.dryRun) await Deno.remove(candidate.path, { recursive: true });
  }
  return removed;
}

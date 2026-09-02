import { isAbsolute, join, resolve } from "@std/path";
import type {
  CapturePoint,
  Persona,
  ReadyCondition,
  RouteScenario,
  Scenario,
  Viewport,
} from "./types.ts";

function record(value: unknown, at: string): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new Error(`${at} must be an object`);
  }
  return value as Record<string, unknown>;
}

function text(value: unknown, at: string): string {
  if (typeof value !== "string" || value.trim() === "") throw new Error(`${at} must be text`);
  return value;
}

function positiveInteger(value: unknown, at: string): number {
  if (!Number.isInteger(value) || (value as number) <= 0) {
    throw new Error(`${at} must be a positive integer`);
  }
  return value as number;
}

function identifier(value: unknown, at: string): string {
  const result = text(value, at);
  if (!/^[a-z0-9][a-z0-9-]*$/.test(result)) {
    throw new Error(`${at} must contain lowercase ASCII letters, digits, or hyphens`);
  }
  return result;
}

function stringArray(value: unknown, at: string): string[] {
  if (value === undefined) return [];
  if (!Array.isArray(value)) throw new Error(`${at} must be an array`);
  return value.map((item, index) => text(item, `${at}[${index}]`));
}

function parseReady(value: unknown, at: string): ReadyCondition {
  const source = record(value, at);
  const kind = text(source.kind, `${at}.kind`);
  const timeoutMs = source.timeoutMs === undefined
    ? undefined
    : positiveInteger(source.timeoutMs, `${at}.timeoutMs`);
  if (kind === "selector") {
    return { kind, selector: text(source.selector, `${at}.selector`), timeoutMs };
  }
  if (kind === "response") {
    const status = source.status === undefined
      ? undefined
      : positiveInteger(source.status, `${at}.status`);
    return { kind, urlPattern: text(source.urlPattern, `${at}.urlPattern`), status, timeoutMs };
  }
  if (kind === "network-idle") return { kind, timeoutMs };
  throw new Error(`${at}.kind is unsupported: ${kind}`);
}

function parseCapturePoint(value: unknown, at: string): CapturePoint {
  const source = record(value, at);
  const result: CapturePoint = {
    id: identifier(source.id, `${at}.id`),
    label: text(source.label, `${at}.label`),
    fullPage: source.fullPage === undefined ? false : Boolean(source.fullPage),
  };
  if (source.ready !== undefined) result.ready = parseReady(source.ready, `${at}.ready`);
  if (source.interaction !== undefined) {
    if (!Array.isArray(source.interaction)) throw new Error(`${at}.interaction must be an array`);
    if (source.interaction.length > 20) {
      throw new Error(`${at}.interaction must not exceed 20 items`);
    }
    result.interaction = source.interaction.map((raw, index) => {
      const action = record(raw, `${at}.interaction[${index}]`);
      const name = text(action.action, `${at}.interaction[${index}].action`);
      if (name === "wait") {
        return {
          action: name,
          ready: parseReady(action.ready, `${at}.interaction[${index}].ready`),
        };
      }
      const selector = text(action.selector, `${at}.interaction[${index}].selector`);
      const timeoutMs = action.timeoutMs === undefined
        ? undefined
        : positiveInteger(action.timeoutMs, `${at}.interaction[${index}].timeoutMs`);
      if (name === "click") return { action: name, selector, timeoutMs };
      if (name === "fill") {
        return {
          action: name,
          selector,
          value: text(action.value, `${at}.interaction[${index}].value`),
          timeoutMs,
        };
      }
      if (name === "press") {
        return {
          action: name,
          selector,
          key: text(action.key, `${at}.interaction[${index}].key`),
          timeoutMs,
        };
      }
      throw new Error(`${at}.interaction[${index}].action is unsupported: ${name}`);
    });
  }
  return result;
}

function parsePersona(value: unknown, at: string): Persona {
  const source = record(value, at);
  const auth = record(source.auth, `${at}.auth`);
  const kind = text(auth.kind, `${at}.auth.kind`);
  const persona: Persona = {
    id: identifier(source.id, `${at}.id`),
    label: text(source.label, `${at}.label`),
    auth: kind === "anonymous"
      ? { kind }
      : kind === "storage-state"
      ? { kind, path: text(auth.path, `${at}.auth.path`) }
      : (() => {
        throw new Error(`${at}.auth.kind is unsupported: ${kind}`);
      })(),
  };
  if (source.login !== undefined) {
    const login = record(source.login, `${at}.login`);
    persona.login = {
      path: login.path === undefined ? "/" : text(login.path, `${at}.login.path`),
      successUrl: text(login.successUrl, `${at}.login.successUrl`),
    };
  }
  return persona;
}

function parseViewport(value: unknown, at: string): Viewport {
  const source = record(value, at);
  return {
    width: positiveInteger(source.width, `${at}.width`),
    height: positiveInteger(source.height, `${at}.height`),
    label: source.label === undefined ? undefined : identifier(source.label, `${at}.label`),
    deviceScaleFactor: source.deviceScaleFactor === undefined
      ? 1
      : positiveInteger(source.deviceScaleFactor, `${at}.deviceScaleFactor`),
  };
}

function parseRoute(value: unknown, at: string): RouteScenario {
  const source = record(value, at);
  if (!Array.isArray(source.capturePoints) || source.capturePoints.length === 0) {
    throw new Error(`${at}.capturePoints must have at least one item`);
  }
  if (source.capturePoints.length > 12) {
    throw new Error(`${at}.capturePoints must not exceed 12 items`);
  }
  return {
    id: identifier(source.id, `${at}.id`),
    label: text(source.label, `${at}.label`),
    path: text(source.path, `${at}.path`),
    goal: text(source.goal, `${at}.goal`),
    dataState: text(source.dataState, `${at}.dataState`),
    ready: parseReady(source.ready, `${at}.ready`),
    capturePoints: source.capturePoints.map((point, index) =>
      parseCapturePoint(point, `${at}.capturePoints[${index}]`)
    ),
  };
}

function uniqueIds(values: { id: string }[], at: string): void {
  const seen = new Set<string>();
  for (const value of values) {
    if (seen.has(value.id)) throw new Error(`${at} contains duplicate id: ${value.id}`);
    seen.add(value.id);
  }
}

export function interpolateEnvironment(value: string, environment = Deno.env.toObject()): string {
  return value.replaceAll(/\$\{([A-Z][A-Z0-9_]*)\}/g, (_match, name: string) => {
    const replacement = environment[name];
    if (replacement === undefined) {
      throw new Error(`required environment variable is missing: ${name}`);
    }
    return replacement;
  });
}

export function validateBaseUrl(value: string): string {
  const url = new URL(value);
  if (url.protocol !== "http:" && url.protocol !== "https:") {
    throw new Error("base URL must use http or https");
  }
  if (url.username || url.password) throw new Error("base URL must not contain credentials");
  url.pathname = url.pathname.replace(/\/$/, "");
  return url.toString().replace(/\/$/, "");
}

export function resolveScenarioPath(sourcePath: string, value: string): string {
  const expanded = interpolateEnvironment(value);
  return isAbsolute(expanded) ? expanded : resolve(join(sourcePath, "..", expanded));
}

export async function loadScenario(sourcePath: string): Promise<Scenario> {
  const parsed = JSON.parse(await Deno.readTextFile(sourcePath));
  const source = record(parsed, "scenario");
  if (source.schemaVersion !== 1) throw new Error("scenario.schemaVersion must equal 1");
  if (!Array.isArray(source.personas) || source.personas.length === 0) {
    throw new Error("scenario.personas must have at least one item");
  }
  if (source.personas.length > 8) throw new Error("scenario.personas must not exceed 8 items");
  if (!Array.isArray(source.viewports) || source.viewports.length === 0) {
    throw new Error("scenario.viewports must have at least one item");
  }
  if (source.viewports.length > 8) throw new Error("scenario.viewports must not exceed 8 items");
  if (!Array.isArray(source.routes) || source.routes.length === 0) {
    throw new Error("scenario.routes must have at least one item");
  }
  if (source.routes.length > 40) throw new Error("scenario.routes must not exceed 40 items");
  const personas = source.personas.map((value, index) =>
    parsePersona(value, `scenario.personas[${index}]`)
  );
  const routes = source.routes.map((value, index) =>
    parseRoute(value, `scenario.routes[${index}]`)
  );
  const scenario: Scenario = {
    schemaVersion: 1,
    id: identifier(source.id, "scenario.id"),
    title: text(source.title, "scenario.title"),
    baseUrl: source.baseUrl === undefined ? undefined : text(source.baseUrl, "scenario.baseUrl"),
    locale: source.locale === undefined ? "en-US" : text(source.locale, "scenario.locale"),
    timezone: source.timezone === undefined ? "UTC" : text(source.timezone, "scenario.timezone"),
    colorScheme: source.colorScheme === "dark" ? "dark" : "light",
    reducedMotion: source.reducedMotion === "no-preference" ? "no-preference" : "reduce",
    redact: source.redact === undefined ? undefined : (() => {
      const redact = record(source.redact, "scenario.redact");
      return {
        selectors: stringArray(redact.selectors, "scenario.redact.selectors"),
        text: stringArray(redact.text, "scenario.redact.text").map((value) =>
          interpolateEnvironment(value)
        ),
      };
    })(),
    personas,
    viewports: source.viewports.map((value, index) =>
      parseViewport(value, `scenario.viewports[${index}]`)
    ),
    routes,
  };
  if (source.processes !== undefined) {
    if (!Array.isArray(source.processes)) throw new Error("scenario.processes must be an array");
    if (source.processes.length > 8) throw new Error("scenario.processes must not exceed 8 items");
    scenario.processes = source.processes.map((value, index) => {
      const at = `scenario.processes[${index}]`;
      const process = record(value, at);
      const env = process.env === undefined ? undefined : record(process.env, `${at}.env`);
      return {
        id: identifier(process.id, `${at}.id`),
        command: text(process.command, `${at}.command`),
        args: stringArray(process.args, `${at}.args`),
        cwd: process.cwd === undefined ? undefined : text(process.cwd, `${at}.cwd`),
        env: env === undefined ? undefined : Object.fromEntries(
          Object.entries(env).map((
            [key, raw],
          ) => [key, interpolateEnvironment(text(raw, `${at}.env.${key}`))]),
        ),
        readyUrl: process.readyUrl === undefined ? undefined : validateBaseUrl(
          interpolateEnvironment(text(process.readyUrl, `${at}.readyUrl`)),
        ),
        readyTimeoutMs: process.readyTimeoutMs === undefined
          ? undefined
          : positiveInteger(process.readyTimeoutMs, `${at}.readyTimeoutMs`),
      };
    });
    uniqueIds(scenario.processes, "scenario.processes");
  }
  uniqueIds(personas, "scenario.personas");
  uniqueIds(routes, "scenario.routes");
  for (const route of routes) uniqueIds(route.capturePoints, `route ${route.id} capturePoints`);
  const captureCount = personas.length * scenario.viewports.length *
    routes.reduce((total, route) => total + route.capturePoints.length, 0);
  if (captureCount > 200) {
    throw new Error(`scenario capture matrix must not exceed 200 items (received ${captureCount})`);
  }
  return scenario;
}

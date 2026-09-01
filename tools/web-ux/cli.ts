#!/usr/bin/env -S deno run --allow-env --allow-net --allow-read --allow-write --allow-run --allow-sys
import { dirname, fromFileUrl, resolve } from "@std/path";
import { authenticate, cleanup } from "./src/lifecycle.ts";
import { capture, describeCapture } from "./src/capture.ts";
import { compare } from "./src/compare.ts";

const DEFAULT_OUTPUT = resolve(dirname(fromFileUrl(import.meta.url)), "../..", "target/web-ux");

const HELP = `Web UX inspection workbench

Usage:
  deno task web-ux auth --scenario <file> --persona <id> [--base-url <url>] [--import-state <file>] [--expires-in-hours <hours>] [--headless]
  deno task web-ux auth --scenario <file> --persona <id> --delete
  deno task web-ux capture --scenario <file> [--output <directory>] [--base-url <url>] [--run-id <id>] [--personas <ids>] [--routes <ids>] [--viewports <ids>] [--headed]
  deno task web-ux compare --before <review-context.json> --after <review-context.json> --output <directory> [--threshold <0..1>]
  deno task web-ux cleanup --output <directory> [--keep <count>] [--older-than-days <days>] [--dry-run]

Comma-separate persona, route, and viewport ids. Auth state is local, mode 0600, and must not be committed.
`;

type Arguments = { command: string; values: Map<string, string[]>; flags: Set<string> };

export function parseArguments(args: string[]): Arguments {
  const command = args.shift() ?? "help";
  const values = new Map<string, string[]>();
  const flags = new Set<string>();
  for (let index = 0; index < args.length; index++) {
    const token = args[index];
    if (!token.startsWith("--")) throw new Error(`unexpected argument: ${token}`);
    const name = token.slice(2);
    const next = args[index + 1];
    if (next === undefined || next.startsWith("--")) {
      flags.add(name);
    } else {
      const items = values.get(name) ?? [];
      items.push(next);
      values.set(name, items);
      index++;
    }
  }
  return { command, values, flags };
}

function optional(args: Arguments, name: string): string | undefined {
  const values = args.values.get(name);
  if (!values) return undefined;
  if (values.length !== 1) throw new Error(`--${name} must be specified once`);
  return values[0];
}

function required(args: Arguments, name: string): string {
  const value = optional(args, name);
  if (!value) throw new Error(`--${name} is required`);
  return value;
}

function integer(args: Arguments, name: string, fallback?: number): number | undefined {
  const value = optional(args, name);
  if (value === undefined) return fallback;
  const parsed = Number(value);
  if (!Number.isInteger(parsed) || parsed < 0) {
    throw new Error(`--${name} must be a non-negative integer`);
  }
  return parsed;
}

function list(args: Arguments, name: string): string[] | undefined {
  const value = optional(args, name);
  return value?.split(",").map((item) => item.trim()).filter(Boolean);
}

function rejectUnknown(args: Arguments, allowedValues: string[], allowedFlags: string[]): void {
  for (const name of args.values.keys()) {
    if (!allowedValues.includes(name)) throw new Error(`unsupported option: --${name}`);
  }
  for (const name of args.flags) {
    if (!allowedFlags.includes(name)) throw new Error(`unsupported flag: --${name}`);
  }
}

export async function main(rawArgs: string[]): Promise<number> {
  const args = parseArguments([...rawArgs]);
  if (args.command === "help" || args.flags.has("help")) {
    console.log(HELP);
    return 0;
  }
  if (args.command === "auth") {
    rejectUnknown(
      args,
      ["scenario", "persona", "base-url", "import-state", "timeout-ms", "expires-in-hours"],
      ["headless", "delete"],
    );
    const deleting = args.flags.has("delete");
    if (deleting && optional(args, "import-state")) {
      throw new Error("--delete cannot be combined with --import-state");
    }
    const path = await authenticate({
      scenarioPath: required(args, "scenario"),
      personaId: required(args, "persona"),
      baseUrl: optional(args, "base-url"),
      importState: optional(args, "import-state"),
      timeoutMs: integer(args, "timeout-ms"),
      expiresInHours: integer(args, "expires-in-hours"),
      delete: deleting,
      headless: args.flags.has("headless"),
    });
    console.log(`auth state ${deleting ? "deleted" : "saved"}: ${path}`);
    return 0;
  }
  if (args.command === "capture") {
    rejectUnknown(args, [
      "scenario",
      "output",
      "base-url",
      "run-id",
      "personas",
      "routes",
      "viewports",
    ], ["headed"]);
    const outputDirectory = optional(args, "output") ?? DEFAULT_OUTPUT;
    const manifest = await capture({
      scenarioPath: required(args, "scenario"),
      outputDirectory,
      baseUrl: optional(args, "base-url"),
      runId: optional(args, "run-id"),
      personas: list(args, "personas"),
      routes: list(args, "routes"),
      viewports: list(args, "viewports"),
      headed: args.flags.has("headed"),
    });
    console.log(describeCapture(manifest, outputDirectory));
    return manifest.status === "completed" ? 0 : 2;
  }
  if (args.command === "compare") {
    rejectUnknown(args, ["before", "after", "output", "threshold"], []);
    const thresholdValue = optional(args, "threshold");
    const threshold = thresholdValue === undefined ? undefined : Number(thresholdValue);
    if (
      threshold !== undefined && (!Number.isFinite(threshold) || threshold < 0 || threshold > 1)
    ) {
      throw new Error("--threshold must be between 0 and 1");
    }
    const report = await compare({
      before: required(args, "before"),
      after: required(args, "after"),
      outputDirectory: required(args, "output"),
      threshold,
    });
    console.log(`comparison saved: ${report}`);
    return 0;
  }
  if (args.command === "cleanup") {
    rejectUnknown(args, ["output", "keep", "older-than-days"], ["dry-run"]);
    const removed = await cleanup({
      outputDirectory: required(args, "output"),
      keep: integer(args, "keep", 5)!,
      olderThanDays: integer(args, "older-than-days"),
      dryRun: args.flags.has("dry-run"),
    });
    for (const path of removed) {
      console.log(`${args.flags.has("dry-run") ? "would remove" : "removed"}: ${path}`);
    }
    return 0;
  }
  throw new Error(`unknown command: ${args.command}\n\n${HELP}`);
}

if (import.meta.main) {
  try {
    Deno.exit(await main(Deno.args));
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    Deno.exit(1);
  }
}

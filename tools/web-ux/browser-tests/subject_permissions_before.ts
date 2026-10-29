#!/usr/bin/env -S deno run -A
/** 変更前の実Web buildでSubject詳細を撮影する。Web自体はbuildしない。 */
import { dirname, fromFileUrl, join, resolve } from "@std/path";
import { capture } from "../src/capture.ts";
import type { Scenario } from "../src/types.ts";
import { freePort } from "./memory_document_harness.ts";

const repositoryRoot = resolve(dirname(fromFileUrl(import.meta.url)), "../../..");
const outputRoot = join(repositoryRoot, "target/web-ux/t-802");
const buildRoot = resolve(Deno.args[0] ?? join(repositoryRoot, "web/workspace/build"));
const runPrefix = Deno.args[1] ?? `before-${new Date().toISOString().replaceAll(/[:.]/g, "-")}`;
if (!/^[a-zA-Z0-9._-]+$/.test(runPrefix)) throw new Error("run id must be a safe filename");
// Preflightは必須。buildの欠落をfixture HTMLで代替しない。
for (const filename of ["index.html", "_app/immutable"]) {
  await Deno.stat(join(buildRoot, filename)).catch(() => {
    throw new Error(
      `Actual Web build missing: ${
        join(buildRoot, filename)
      }. Parent must build/supply it; this harness does not build Web or bypass preflight.`,
    );
  });
}
await Deno.mkdir(outputRoot, { recursive: true });
const index = await Deno.readFile(join(buildRoot, "index.html"));
const indexSha256 = Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256", index)))
  .map((b) => b.toString(16).padStart(2, "0")).join("");
await Deno.writeTextFile(
  join(outputRoot, `${runPrefix}-build.json`),
  JSON.stringify(
    {
      evidence: "actual-Web-with-synthetic-HTTP-NOT-real-Backend",
      buildRoot,
      indexSha256,
      capturedAt: new Date().toISOString(),
      visualVerdict: "UNREVIEWED — parent must ViewImage before pass",
    },
    null,
    2,
  ),
);
const port = await freePort();
for (const theme of ["light", "dark"] as const) {
  const scenario: Scenario = {
    schemaVersion: 1,
    id: `subject-permissions-before-${theme}`,
    title: "T-802 BEFORE — actual Subject detail / synthetic HTTP, NOT real Backend",
    baseUrl: `http://127.0.0.1:${port}`,
    colorScheme: theme,
    reducedMotion: "reduce",
    personas: [{
      id: "synthetic-owner",
      label: "Synthetic HTTP owner, NOT real Backend authentication",
      auth: { kind: "anonymous" },
    }],
    viewports: [
      { label: "desktop", width: 1440, height: 900 },
      { label: "narrow", width: 768, height: 900 },
      { label: "mobile", width: 390, height: 844 },
    ],
    routes: [{
      id: "subject-detail",
      label: "Existing Subject detail before permission UI",
      path:
        "/w/memory-review/memory/release-coordination-subject-with-a-deliberately-long-stable-identity-for-responsive-review",
      goal:
        "Find Subject identity, current state and ordinary routes before permission management changes.",
      dataState:
        "Active Subject with current Worker and representative long identifiers. All HTTP is synthetic; no real Backend evidence.",
      ready: { kind: "selector", selector: ".subject-overview" },
      capturePoints: [{ id: "initial", label: "BEFORE Subject detail" }],
    }],
    processes: [{
      id: "t-802-owned-synthetic-http",
      command: Deno.execPath(),
      cwd: join(repositoryRoot, "tools/web-ux"),
      args: [
        "run",
        "-A",
        join(repositoryRoot, "tools/web-ux/browser-tests/memory_document_fixture_server.ts"),
        String(port),
        buildRoot,
        "3",
        "permissions",
      ],
      readyUrl: `http://127.0.0.1:${port}/health`,
    }],
  };
  const scenarioPath = join(outputRoot, `${runPrefix}-${theme}-scenario.json`);
  await Deno.writeTextFile(scenarioPath, JSON.stringify(scenario, null, 2));
  const bundle = await capture({
    scenarioPath,
    outputDirectory: outputRoot,
    runId: `${runPrefix}-${theme}`,
  });
  console.log(JSON.stringify({ runId: bundle.runId, status: bundle.status }));
  if (bundle.status !== "completed") {
    throw new Error(
      `BEFORE capture was not clean: ${bundle.runId}; inspect review-context.json. No visual pass is implied.`,
    );
  }
}

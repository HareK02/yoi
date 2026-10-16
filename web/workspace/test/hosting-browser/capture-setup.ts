import { assert, assertEquals } from "@std/assert";
import { resolve } from "@std/path";
import type { ReviewContext } from "../../../../tools/web-ux/src/types.ts";
const root = resolve(import.meta.dirname!, "../../../..");
const output = `${root}/target/web-ux/hosting`;
const synthetic = `ui-projection-${crypto.randomUUID()}`;
// 合成値はこの子processのmemory/envに限定する。argv、URL、保存証拠へ出さない。
await Deno.mkdir(`${root}/target/web-ux/tmp`, { recursive: true });
const audit = [];
const includeExisting = Deno.args.includes("--existing");
const runLabel = Deno.args.find((arg) => arg.startsWith("--run-label="))?.slice(12) ??
  String(Date.now());
assert(/^[a-z0-9-]+$/.test(runLabel), "run label must be a safe path component");
for (const theme of ["light", "dark"]) {
  for (
    const kind of includeExisting ? ["setup", "settings"] : ["setup"]
  ) {
    const run = `${kind === "setup" ? "setup" : "after-setup"}-${runLabel}-${theme}`;
    const child = await new Deno.Command(Deno.execPath(), {
      cwd: root,
      env: { TMPDIR: `${root}/target/web-ux/tmp`, WEB_UX_SYNTHETIC_SETUP_INPUT: synthetic },
      args: [
        "run",
        "--config",
        "tools/web-ux/deno.json",
        "--allow-env",
        "--allow-net",
        "--allow-read",
        "--allow-write=target/web-ux",
        "--allow-run",
        "--allow-sys",
        "tools/web-ux/cli.ts",
        "capture",
        "--scenario",
        `tools/web-ux/scenarios/hosting-${kind}-${theme}.json`,
        "--output",
        "target/web-ux/hosting",
        "--run-id",
        run,
      ],
      stdout: "inherit",
      stderr: "inherit",
    }).output();
    assertEquals(child.code, 2); // 両scenarioとも意図した拒否・障害表示を含む。
    const context: ReviewContext = JSON.parse(
      await Deno.readTextFile(`${output}/${run}/review-context.json`),
    );
    assertEquals(context.diagnostics, []);
    assertEquals(context.status, "completed-with-errors");
    assertEquals(context.captures.length, kind === "setup" ? 18 : 54);
    let images = 0;
    for (const capture of context.captures) {
      assertEquals(capture.document.status, 200);
      assertEquals(capture.errorSummary.truncated, false);
      const rejected = capture.route.id === "setup-reject";
      const expected = rejected ||
        ["admin-load-error", "admin-save-error", "account-error"].includes(capture.route.id);
      if (expected) {
        assert(capture.errors.every((error) =>
          error.status === (rejected ? 403 : 503) ||
          (error.kind === "console" && error.message.includes(String(rejected ? 403 : 503))) ||
          (error.kind === "request" && error.message === "net::ERR_ABORTED")
        ));
        assert(capture.errors.some((error) => error.status === (rejected ? 403 : 503)));
      } else assertEquals(capture.errors, []);
      for (const image of capture.screenshots) {
        assertEquals(
          await sha(await Deno.readFile(`${output}/${run}/${image.bundlePath}`)),
          image.sha256,
        );
        images++;
      }
    }
    let scannedTextFiles = 0;
    for await (const path of files(`${output}/${run}`)) {
      if (/\.(json|md|html|log)$/.test(path)) {
        assert(
          !(await Deno.readTextFile(path)).includes(synthetic),
          "synthetic input must not appear in artifacts",
        );
        scannedTextFiles++;
      }
    }
    audit.push({
      run,
      source: context.source,
      captures: context.captures.length,
      images,
      unexpectedErrors: 0,
      allDocuments200: true,
      imageHashesVerified: true,
      syntheticInputAbsent: true,
      scannedTextFiles,
    });
  }
}
async function sha(bytes: Uint8Array<ArrayBuffer>) {
  return [...new Uint8Array(await crypto.subtle.digest("SHA-256", bytes))].map((value) =>
    value.toString(16).padStart(2, "0")
  ).join("");
}
async function* files(dir: string): AsyncGenerator<string> {
  for await (const entry of Deno.readDir(dir)) {
    const path = `${dir}/${entry.name}`;
    if (entry.isDirectory) yield* files(path);
    else if (entry.isFile) yield path;
  }
}
async function inventory(dir: string) {
  const result = [];
  for await (const path of files(dir)) {
    result.push({
      path: path.slice(root.length + 1),
      sha256: await sha(await Deno.readFile(path)),
    });
  }
  return result.sort((a, b) => a.path.localeCompare(b.path));
}
const diff = await new Deno.Command("git", {
  cwd: root,
  args: ["diff", "--binary", "HEAD", "--", "web/workspace/src", "web/workspace/deno.json"],
  stdout: "piped",
  stderr: "null",
}).output();
assert(diff.success);
await Deno.writeTextFile(
  `${output}/setup-evidence-audit.json`,
  JSON.stringify(
    {
      boundary:
        "Production UI modules/styles unchanged. Setup-only bootstrap replaces navigator.credentials create/get; synthetic API responses. Not native WebAuthn, real token proof, authorization or persistence evidence. No request header/body logging.",
      audit,
      currentTrackedWebDiffSha256: await sha(diff.stdout),
      currentSourceFiles: await inventory(`${root}/web/workspace/src`),
      currentBuildFiles: await inventory(`${root}/web/workspace/build`),
      fixtureFiles: await inventory(import.meta.dirname!),
    },
    null,
    2,
  ),
);
console.log("Capture diagnostics, image hashes and synthetic-input artifact scan PASS.");

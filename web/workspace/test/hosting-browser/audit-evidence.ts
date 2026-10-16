import { assert, assertEquals } from "@std/assert";
import { resolve } from "@std/path";
import type { ReviewContext } from "../../../../tools/web-ux/src/types.ts";
const root = resolve(import.meta.dirname!, "../../../..");
const output = `${root}/target/web-ux/hosting`;
async function sha(bytes: Uint8Array<ArrayBuffer>) {
  return [...new Uint8Array(await crypto.subtle.digest("SHA-256", bytes))].map((value) =>
    value.toString(16).padStart(2, "0")
  ).join("");
}
const results = [];
for (const theme of ["light", "dark"]) {
  for (const prefix of ["before", "after", "navigation-before", "navigation-after"]) {
    const run = `${prefix}-final-${theme}`;
    const context: ReviewContext = JSON.parse(
      await Deno.readTextFile(`${output}/${run}/review-context.json`),
    );
    assertEquals(context.diagnostics, []);
    assert(!context.diagnosticSummary.truncated);
    let screenshots = 0, expectedFailureCaptures = 0;
    for (const capture of context.captures) {
      assertEquals(capture.document.status, 200);
      assert(!capture.errorSummary.truncated);
      const expectedFailure = ["admin-load-error", "admin-save-error", "account-error"].includes(
        capture.route.id,
      );
      if (expectedFailure) {
        expectedFailureCaptures++;
        // Non-OK bodies are cancelled by production client; ERR_ABORTED is the
        // same deliberately failed response, not an unrelated network failure.
        assertEquals(capture.errors.length, 3);
        assertEquals(
          capture.errors.filter((error) => error.status === 503).length,
          1,
        );
        assertEquals(
          capture.errors.filter((error) =>
            error.kind === "console" && error.message.includes("503")
          ).length,
          1,
        );
        assertEquals(
          capture.errors.filter((error) =>
            error.kind === "request" && error.message === "net::ERR_ABORTED"
          ).length,
          1,
        );
      } else assertEquals(capture.errors, []);
      assert(capture.snapshot !== null);
      const snapshot = await Deno.readTextFile(`${output}/${run}/${capture.snapshot.bundlePath}`);
      if (["ordinary", "anonymous"].includes(capture.route.id)) {
        assert(!snapshot.includes('button "Edit registration policy"'));
        assert(!snapshot.includes('link "Hosting settings"'));
      }
      if (capture.route.id === "ordinary-account-off") {
        assert(snapshot.includes('button "Add passkey to alice"'));
        assert(!snapshot.includes('link "Hosting settings"'));
      }
      for (const screenshot of capture.screenshots) {
        assertEquals(
          await sha(await Deno.readFile(`${output}/${run}/${screenshot.bundlePath}`)),
          screenshot.sha256,
        );
        screenshots++;
      }
    }
    assertEquals(expectedFailureCaptures, prefix === "after" ? 9 : 0);
    results.push({
      run,
      captures: context.captures.length,
      screenshots,
      expectedFailureCaptures,
      allDocuments200: true,
      unexpectedErrors: 0,
      imageHashesVerified: true,
      diagnosticTruncation: false,
    });
  }
}
async function inventory(directory: string): Promise<{ path: string; sha256: string }[]> {
  const files = [];
  for await (const entry of Deno.readDir(directory)) {
    const path = `${directory}/${entry.name}`;
    if (entry.isDirectory) files.push(...await inventory(path));
    else if (entry.isFile) {
      files.push({
        path: path.slice(root.length + 1),
        sha256: await sha(await Deno.readFile(path)),
      });
    }
  }
  return files.sort((a, b) => a.path.localeCompare(b.path));
}
const baselineAssets = await inventory(`${root}/target/web-ux/baseline-source/web/workspace/build`);
const currentAssets = await inventory(`${root}/web/workspace/build`);
await Deno.writeTextFile(
  `${output}/evidence-audit.json`,
  JSON.stringify(
    {
      boundary:
        "Actual production static shells; synthetic HTTP only. No Backend authorization/persistence/WebAuthn evidence.",
      baselineSource: "git archive 7042ec7d50ca8b03b8034c33e9792f4b6ac9d48d web/workspace",
      currentSource:
        "Dirty shared worktree at same HEAD; exact served build bytes identified by inventory. Parent owns source/build provenance and final visual verdict.",
      results,
      baselineAssets,
      currentAssets,
    },
    null,
    2,
  ),
);
console.log(JSON.stringify(results, null, 2));

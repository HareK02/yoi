import { dirname, fromFileUrl, join } from "@std/path";

// Build once per suite invocation, before any browser reads the shared output.
// Each test still owns independent HTTP/browser state; only static assets are shared.
const workspace = join(dirname(fromFileUrl(import.meta.url)), "../../../web/workspace");
const started = performance.now();
const build = await new Deno.Command(Deno.execPath(), {
  args: ["task", "build"],
  cwd: workspace,
  stdout: "inherit",
  stderr: "inherit",
}).output();
console.error(`Browser production build: ${((performance.now() - started) / 1000).toFixed(2)}s`);
if (!build.success) Deno.exit(build.code);
const tests = await new Deno.Command(Deno.execPath(), {
  args: [
    "test",
    "--unstable-sloppy-imports",
    "--allow-env",
    "--allow-net",
    "--allow-read",
    "--allow-write",
    "--allow-run",
    "--allow-sys",
    ...Deno.args,
  ],
  env: { WEB_UX_BUILD_ROOT: join(workspace, "build") },
  stdout: "inherit",
  stderr: "inherit",
}).output();
Deno.exit(tests.code);

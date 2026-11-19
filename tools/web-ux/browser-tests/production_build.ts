import { join } from "@std/path";

// The suite entry builds once and supplies this existing explicit asset override.
// Standalone tests still build their own current source; no cross-run success cache.
const suppliedBuildRoot = Deno.env.get("WEB_UX_BUILD_ROOT");
const builds = new Map<string, Promise<string>>();

export function productionBuild(workspaceRoot: string): Promise<string> {
  let build = builds.get(workspaceRoot);
  if (!build) {
    build = (async () => {
      if (!suppliedBuildRoot) {
        const result = await new Deno.Command(Deno.execPath(), {
          args: ["task", "build"],
          cwd: workspaceRoot,
          stdout: "piped",
          stderr: "piped",
        }).output();
        if (!result.success) {
          throw new Error(
            `Workspace production build failed:\n${new TextDecoder().decode(result.stderr)}`,
          );
        }
      }
      const root = suppliedBuildRoot ?? join(workspaceRoot, "build");
      if (!(await Deno.stat(join(root, "index.html"))).isFile) {
        throw new Error(`Production build has no index.html: ${root}`);
      }
      return root;
    })();
    builds.set(workspaceRoot, build);
  }
  return build;
}

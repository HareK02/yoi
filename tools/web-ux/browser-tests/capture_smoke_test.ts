import { assertEquals, assertRejects } from "@std/assert";
import { join } from "@std/path";
import { writeAuthMetadata } from "../src/auth_state.ts";
import { capture } from "../src/capture.ts";

async function freePort(): Promise<number> {
  const listener = Deno.listen({ hostname: "127.0.0.1", port: 0 });
  const port = (listener.addr as Deno.NetAddr).port;
  listener.close();
  return port;
}

Deno.test("browser smoke captures distinct owner and non-owner evidence and cleans its server", async () => {
  const directory = await Deno.makeTempDir();
  const port = await freePort();
  const baseUrl = `http://127.0.0.1:${port}`;
  try {
    const authDirectory = join(directory, "auth");
    await Deno.mkdir(authDirectory);
    for (const persona of ["owner", "non-owner"]) {
      const storageState = join(authDirectory, `${persona}.json`);
      await Deno.writeTextFile(
        storageState,
        JSON.stringify({
          cookies: [{
            name: "persona",
            value: persona,
            domain: "127.0.0.1",
            path: "/",
            expires: -1,
            httpOnly: true,
            secure: false,
            sameSite: "Lax",
          }],
          origins: [],
        }),
      );
      await writeAuthMetadata(storageState, persona, baseUrl, 1);
    }
    const scenarioPath = join(directory, "scenario.json");
    await Deno.writeTextFile(
      scenarioPath,
      JSON.stringify({
        schemaVersion: 1,
        id: "browser-smoke",
        title: "Browser smoke",
        baseUrl,
        personas: [
          { id: "owner", label: "Owner", auth: { kind: "storage-state", path: "auth/owner.json" } },
          {
            id: "non-owner",
            label: "Non-owner",
            auth: { kind: "storage-state", path: "auth/non-owner.json" },
          },
        ],
        viewports: [{ label: "desktop", width: 1000, height: 700 }],
        routes: [{
          id: "repositories",
          label: "Repositories",
          path: "/screen",
          goal: "Verify permission-specific composition",
          dataState: "Deterministic fixture repository",
          ready: { kind: "selector", selector: "main" },
          capturePoints: [{ id: "initial", label: "Initial" }],
        }],
        processes: [{
          id: "fixture-server",
          command: Deno.execPath(),
          args: [
            "run",
            "--allow-net",
            join(Deno.cwd(), "browser-tests/fixture_server.ts"),
            String(port),
          ],
          readyUrl: `${baseUrl}/health`,
        }],
      }),
    );
    const manifest = await capture({
      scenarioPath,
      outputDirectory: join(directory, "artifacts"),
      runId: "multi-persona",
    });
    assertEquals(manifest.status, "completed");
    assertEquals(manifest.captures.map((item) => item.persona.id), ["owner", "non-owner"]);
    assertEquals(manifest.captures.every((item) => item.screenshots.length === 1), true);
    assertEquals(manifest.contactSheet.png, "contact-sheet.png");
    const runDirectory = join(directory, "artifacts", "multi-persona");
    const reviewContext = await Deno.readTextFile(join(runDirectory, "review-context.json"));
    assertEquals(reviewContext.includes('"cookies"'), false);
    if (Deno.build.os !== "windows") {
      const screenshot = join(runDirectory, manifest.captures[0].screenshots[0].path);
      assertEquals((await Deno.stat(screenshot)).mode! & 0o777, 0o600);
    }
    await assertRejects(
      () => fetch(`${baseUrl}/health`, { signal: AbortSignal.timeout(500) }),
      TypeError,
    );
  } finally {
    await Deno.remove(directory, { recursive: true });
  }
});

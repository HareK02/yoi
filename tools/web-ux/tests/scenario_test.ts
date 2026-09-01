import { assertEquals, assertRejects, assertThrows } from "@std/assert";
import { join } from "@std/path";
import { cleanup } from "../src/lifecycle.ts";
import { interpolateEnvironment, loadScenario, validateBaseUrl } from "../src/scenario.ts";

function minimalScenario(extra = ""): string {
  return `{
    "schemaVersion": 1,
    "id": "test-screen",
    "title": "Test screen",
    "baseUrl": "http://127.0.0.1:5173",
    "personas": [{"id":"anonymous","label":"Anonymous","auth":{"kind":"anonymous"}}],
    "viewports": [{"label":"desktop","width":1000,"height":800}],
    "routes": [{
      "id":"home","label":"Home","path":"/","goal":"Inspect home",
      "dataState":"Fixture data","ready":{"kind":"selector","selector":"main"},
      "capturePoints":[{"id":"initial","label":"Initial"}]
    }]${extra}
  }`;
}

Deno.test("scenario parser preserves explicit visual review context", async () => {
  const directory = await Deno.makeTempDir();
  try {
    const path = join(directory, "scenario.json");
    await Deno.writeTextFile(path, minimalScenario());
    const scenario = await loadScenario(path);
    assertEquals(scenario.personas[0].auth, { kind: "anonymous" });
    assertEquals(scenario.routes[0].goal, "Inspect home");
    assertEquals(scenario.routes[0].dataState, "Fixture data");
    assertEquals(scenario.routes[0].ready, {
      kind: "selector",
      selector: "main",
      timeoutMs: undefined,
    });
    assertEquals(scenario.reducedMotion, "reduce");
  } finally {
    await Deno.remove(directory, { recursive: true });
  }
});

Deno.test("scenario parser rejects duplicate persona identity", async () => {
  const directory = await Deno.makeTempDir();
  try {
    const path = join(directory, "scenario.json");
    await Deno.writeTextFile(
      path,
      minimalScenario().replace(
        '[{"id":"anonymous","label":"Anonymous","auth":{"kind":"anonymous"}}]',
        '[{"id":"same","label":"First","auth":{"kind":"anonymous"}},{"id":"same","label":"Second","auth":{"kind":"anonymous"}}]',
      ),
    );
    await assertRejects(() => loadScenario(path), Error, "duplicate id: same");
  } finally {
    await Deno.remove(directory, { recursive: true });
  }
});

Deno.test("base URL rejects embedded credentials and non-http schemes", () => {
  assertThrows(
    () => validateBaseUrl("https://user:secret@example.test"),
    Error,
    "must not contain credentials",
  );
  assertThrows(() => validateBaseUrl("file:///tmp/index.html"), Error, "must use http or https");
});

Deno.test("environment interpolation fails closed", () => {
  assertEquals(
    interpolateEnvironment("/w/${WORKSPACE_ID}", { WORKSPACE_ID: "W-test" }),
    "/w/W-test",
  );
  assertThrows(
    () => interpolateEnvironment("${MISSING}", {}),
    Error,
    "required environment variable is missing",
  );
});

Deno.test("cleanup removes only complete review bundles beyond retention", async () => {
  const directory = await Deno.makeTempDir();
  try {
    for (const name of ["one", "two", "three"]) {
      const run = join(directory, name);
      await Deno.mkdir(run);
      await Deno.writeTextFile(join(run, "review-context.json"), "{}");
      await new Promise((resolve) => setTimeout(resolve, 5));
    }
    const unrelated = join(directory, "auth");
    await Deno.mkdir(unrelated);
    await Deno.writeTextFile(join(unrelated, "state.json"), "secret");
    const removed = await cleanup({ outputDirectory: directory, keep: 1 });
    assertEquals(removed.length, 2);
    assertEquals(await Deno.readTextFile(join(unrelated, "state.json")), "secret");
  } finally {
    await Deno.remove(directory, { recursive: true });
  }
});

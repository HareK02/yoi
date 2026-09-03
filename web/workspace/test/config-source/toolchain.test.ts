declare const Deno: {
  test(name: string, fn: () => Promise<void> | void): void;
  readTextFile(path: URL): Promise<string>;
};

import {
  shouldStartCompletionAfterTyping,
  toCodeMirrorCompletion,
} from "../../src/lib/workspace/config-source/completion.ts";
import { jsonWorkerMessage } from "../../src/lib/workspace/config-source/toolchain-message.ts";

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

Deno.test("toolchain converts reactive-like proxies to plain Worker messages", async () => {
  const snapshot = new Proxy(
    {
      revision: 1,
      digest: "sha256:test",
      entries: {},
    },
    {},
  );
  const request = {
    id: 1,
    kind: "set_snapshot" as const,
    snapshot,
  };

  let cloneRejected = false;
  try {
    structuredClone(request);
  } catch (error) {
    cloneRejected = error instanceof DOMException &&
      error.name === "DataCloneError";
  }
  assert(
    cloneRejected,
    "fixture should reproduce the Worker postMessage clone failure",
  );

  const message = jsonWorkerMessage(request);
  assert(
    message.snapshot !== snapshot,
    "snapshot should be detached from the Proxy",
  );
  structuredClone(message);

  const source = await Deno.readTextFile(
    new URL(
      "../../src/lib/workspace/config-source/toolchain.ts",
      import.meta.url,
    ),
  );
  assert(
    source.includes("const message = jsonWorkerMessage({ ...request, id })") &&
      source.includes("this.#worker.postMessage(message)"),
    "ConfigSourceToolchain should normalize every command at the Worker boundary",
  );
  const workerSource = await Deno.readTextFile(
    new URL(
      "../../src/lib/workspace/config-source/toolchain.worker.ts",
      import.meta.url,
    ),
  );
  assert(
    workerSource.includes("set_schema_bundle(request.schemaBundle)"),
    "Config Source worker should install the WorkspaceConfigSchema global contract",
  );
});

Deno.test("completion starts only after non-whitespace typing", () => {
  assert(
    !shouldStartCompletionAfterTyping(" "),
    "Space should not start completion",
  );
  assert(
    !shouldStartCompletionAfterTyping("\n"),
    "Enter should not start completion",
  );
  assert(
    !shouldStartCompletionAfterTyping("\t"),
    "other whitespace should not start completion",
  );
  assert(
    shouldStartCompletionAfterTyping("p"),
    "non-whitespace typing should start completion",
  );
});

Deno.test("toolchain preserves WASM UTF-16 completion ranges for CodeMirror", () => {
  const result = toCodeMirrorCompletion({
    from: "let 名 = ".length,
    items: [
      {
        label: "true",
        kind: "constant",
        detail: "Bool",
        priority: 20,
      },
    ],
  });

  assert(result !== null, "WASM completion should produce a CodeMirror result");
  assert(
    result.from === "let 名 = ".length,
    "WASM UTF-16 offsets should be preserved for CodeMirror",
  );
  assert(
    result.options.length === 1,
    "WASM items should become CodeMirror options",
  );
  assert(
    result.options[0].label === "true",
    "completion label should be preserved",
  );
  assert(
    result.options[0].type === "constant",
    "completion kind should become the icon type",
  );
  assert(
    result.options[0].detail === "Bool",
    "completion detail should be preserved",
  );
  assert(
    result.options[0].boost === 20,
    "completion priority should become its boost",
  );
});

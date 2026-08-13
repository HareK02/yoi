/// <reference lib="deno.ns" />

import { assertEquals } from "jsr:@std/assert";
// The generated wasm-bindgen loader is JavaScript with an adjacent declaration file.
// @ts-expect-error Deno checks the generated JS implementation rather than its .d.ts.
import init, {
  analyze_snapshot,
  evaluate_snapshot,
} from "../../src/lib/workspace/config-source/generated/config_source_wasm.js";
import type { ConfigTreeSnapshot, ToolchainContract } from "../../src/lib/workspace/config-source/types.ts";

const bytes = await Deno.readFile(
  new URL("../../src/lib/workspace/config-source/generated/config_source_wasm_bg.wasm", import.meta.url),
);
await init({ module_or_path: bytes });

const snapshot: ConfigTreeSnapshot = {
  revision: 4,
  digest: "sha256:test-tree",
  entries: {
    "workspace.dcdl": {
      path: "workspace.dcdl",
      content_type: "decodal",
      content: 'import "./lib/value.dcdl"',
      content_digest: "sha256:root",
    },
    "lib/value.dcdl": {
      path: "lib/value.dcdl",
      content_type: "decodal",
      content: "{ answer = 42; }",
      content_digest: "sha256:value",
    },
  },
};

const contract: ToolchainContract = {
  contract_version: 1,
  decodal_version: "0.2.0",
  schema_version: 1,
  entrypoints: ["workspace.dcdl"],
  import_policy_version: 1,
  fingerprint: "sha256:test-contract",
};

Deno.test("generated WASM evaluates the same virtual import contract", () => {
  const result = evaluate_snapshot(snapshot, contract) as {
    projections: Array<{ data_json: { answer: number } }>;
  };
  assertEquals(result.projections[0].data_json, { answer: 42 });
});

Deno.test("generated WASM diagnostics carry snapshot provenance", () => {
  const diagnostics = analyze_snapshot(
    snapshot,
    "workspace.dcdl",
    "{ broken = ; }",
  ) as Array<{
    path: string;
    revision: number;
    tree_digest: string;
    kind: string;
  }>;
  assertEquals(diagnostics[0].path, "workspace.dcdl");
  assertEquals(diagnostics[0].revision, 4);
  assertEquals(diagnostics[0].tree_digest, "sha256:test-tree");
  assertEquals(diagnostics[0].kind, "syntax");
});

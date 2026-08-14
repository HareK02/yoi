/// <reference lib="deno.ns" />

import { assertEquals } from "jsr:@std/assert";
import init, {
  analyze_snapshot,
  compose_schema_bundle,
  evaluate_snapshot,
} from "../../src/lib/workspace/config-source/generated/config_source_wasm.js";
import type {
  ConfigTreeSnapshot,
  ToolchainContract,
  WorkspaceConfigSchemaBundle,
} from "../../src/lib/workspace/config-source/types.ts";

const bytes = await Deno.readFile(
  new URL(
    "../../src/lib/workspace/config-source/generated/config_source_wasm_bg.wasm",
    import.meta.url,
  ),
);
await init({ module_or_path: bytes });

async function digestText(text: string): Promise<string> {
  const bytes = new TextEncoder().encode(text);
  const digest = await crypto.subtle.digest("SHA-256", bytes);
  return `sha256:${
    Array.from(new Uint8Array(digest), (byte) =>
      byte.toString(16).padStart(2, "0")).join("")
  }`;
}

async function toolchainFingerprint(
  entrypoints: string[],
  schemaBundle: WorkspaceConfigSchemaBundle,
): Promise<string> {
  return await digestText(JSON.stringify([
    2,
    "0.4.0",
    1,
    entrypoints,
    1,
    schemaBundle.fingerprint,
  ]));
}

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

const emptySchemaBundle = compose_schema_bundle(
  [],
) as WorkspaceConfigSchemaBundle;
const contract: ToolchainContract = {
  contract_version: 2,
  decodal_version: "0.4.0",
  schema_version: 1,
  entrypoints: ["workspace.dcdl"],
  import_policy_version: 1,
  schema_bundle: emptySchemaBundle,
  fingerprint: await toolchainFingerprint(
    ["workspace.dcdl"],
    emptySchemaBundle,
  ),
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

const featuresSchema = "{ features = {...{ enabled = Bool; }}; }";
const webSchema = "{ web = { enabled = Bool; ...Unknown }; }";
const schemaBundle = compose_schema_bundle([
  {
    provider_id: "builtin:features",
    namespace: "features",
    version: "1",
    source: featuresSchema,
    source_digest: await digestText(featuresSchema),
  },
  {
    provider_id: "builtin:web",
    namespace: "web",
    version: "1",
    source: webSchema,
    source_digest: await digestText(webSchema),
  },
]) as WorkspaceConfigSchemaBundle;

function schemaSnapshot(source: string): ConfigTreeSnapshot {
  return {
    revision: 7,
    digest: "sha256:schema-tree",
    entries: {
      "main.dcdl": {
        path: "main.dcdl",
        content_type: "decodal",
        content: source,
        content_digest: "sha256:main",
      },
    },
  };
}

const schemaContract: ToolchainContract = {
  contract_version: 2,
  decodal_version: "0.4.0",
  schema_version: 1,
  entrypoints: ["main.dcdl"],
  import_policy_version: 1,
  schema_bundle: schemaBundle,
  fingerprint: await toolchainFingerprint(["main.dcdl"], schemaBundle),
};

Deno.test("generated WASM applies Decodal 0.4 typed maps and explicit object rest", () => {
  const result = evaluate_snapshot(
    schemaSnapshot(
      "{ features = { console = { enabled = true; }; }; web = { enabled = true; extension_value = 42; }; }",
    ),
    schemaContract,
  ) as { projections: Array<{ data_json: Record<string, unknown> }> };
  assertEquals(result.projections[0].data_json, {
    features: { console: { enabled: true } },
    web: { enabled: true, extension_value: 42 },
  });
});

type ProjectedDiagnostic = {
  path: string;
  kind: string;
  message: string;
  span: { start_byte: number; end_byte: number };
};

function evaluateFailure(source: string): ProjectedDiagnostic {
  try {
    evaluate_snapshot(schemaSnapshot(source), schemaContract);
  } catch (error) {
    const diagnostics = error as ProjectedDiagnostic[];
    assertEquals(Array.isArray(diagnostics), true);
    return diagnostics[0];
  }
  throw new Error("expected Decodal evaluation to fail");
}

Deno.test("generated WASM preserves native Decodal 0.4 diagnostic semantics", () => {
  for (
    const [source, expectedKind] of [
      ["{ features = {}; custom = 42; }", "constraintviolation"],
      [
        '{ features = { web = { enabled = "yes"; }; }; }',
        "constraintviolation",
      ],
      ["{ features = { web = {}; }; }", "materialize"],
      [
        "{ features = { web = { enabled = true; typo = 1; }; }; }",
        "constraintviolation",
      ],
    ] as const
  ) {
    const diagnostic = evaluateFailure(source);
    assertEquals(diagnostic.path, "main.dcdl");
    assertEquals(diagnostic.kind, expectedKind);
    assertEquals(diagnostic.message.length > 0, true);
    assertEquals(diagnostic.span.end_byte > diagnostic.span.start_byte, true);
  }
});

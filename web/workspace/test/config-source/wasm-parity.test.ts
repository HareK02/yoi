/// <reference lib="deno.ns" />

import { assertEquals } from "jsr:@std/assert";
import init, {
  analyze_snapshot,
  complete_current,
  compose_schema_bundle,
  evaluate_snapshot,
  set_schema_bundle,
  set_snapshot,
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

const mainEntrypointContract: ToolchainContract = {
  ...contract,
  entrypoints: ["main.dcdl"],
  fingerprint: await toolchainFingerprint(["main.dcdl"], emptySchemaBundle),
};

Deno.test("generated WASM evaluates the same virtual import contract", () => {
  const result = evaluate_snapshot(snapshot, contract) as {
    projections: Array<{ data_json: { answer: number } }>;
  };
  assertEquals(result.projections[0].data_json, { answer: 42 });
});

Deno.test("generated WASM accepts the mandatory main schema assertion", () => {
  const assertedSnapshot: ConfigTreeSnapshot = {
    revision: 1,
    digest: "sha256:asserted-main",
    entries: {
      "main.dcdl": {
        path: "main.dcdl",
        content_type: "decodal",
        content: "{ answer = 42; } as WorkspaceConfigSchema\n",
        content_digest: "sha256:asserted-main-entry",
      },
    },
  };
  const result = evaluate_snapshot(
    assertedSnapshot,
    mainEntrypointContract,
  ) as { projections: Array<{ data_json: { answer: number } }> };

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

const markdownSnapshot: ConfigTreeSnapshot = {
  revision: 8,
  digest: "sha256:markdown-tree",
  entries: {
    "main.dcdl": {
      path: "main.dcdl",
      content_type: "decodal",
      content:
        `{ skill = import "./skills/debug-rust/SKILL.md" as { frontmatter = { name = String; description = String; ...Unknown }; content = String; }; }`,
      content_digest: "sha256:markdown-main",
    },
    "skills/debug-rust/SKILL.md": {
      path: "skills/debug-rust/SKILL.md",
      content_type: "text",
      content:
        "---\nname: debug-rust\ndescription: Debug Rust\ncustom-authority: no\n---\n# Debug Rust\n",
      content_digest: "sha256:markdown-skill",
    },
  },
};
const markdownContract: ToolchainContract = {
  ...contract,
  entrypoints: ["main.dcdl"],
  fingerprint: await toolchainFingerprint(["main.dcdl"], emptySchemaBundle),
};

Deno.test("generated WASM evaluates Markdown with the shared Skill projection", () => {
  const result = evaluate_snapshot(markdownSnapshot, markdownContract) as {
    projections: Array<{ data_json: Record<string, unknown> }>;
  };
  assertEquals(result.projections[0].data_json, {
    skill: {
      frontmatter: {
        "custom-authority": "no",
        description: "Debug Rust",
        name: "debug-rust",
      },
      content: "# Debug Rust\n",
    },
  });
});

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

Deno.test("generated WASM returns completion items for the editor adapter", () => {
  const source = 'import "./"';
  set_snapshot({
    ...snapshot,
    entries: {
      ...snapshot.entries,
      "workspace.dcdl": {
        ...snapshot.entries["workspace.dcdl"],
        content: source,
      },
    },
  });
  const result = complete_current(
    "workspace.dcdl",
    source,
    source.length - 1,
    true,
  ) as {
    from: number;
    items: Array<{ label: string; kind: string }>;
  };

  assertEquals(result.from, 8);
  assertEquals(result.items[0].label, "./lib/value.dcdl");
  assertEquals(result.items[0].kind, "file");
});

Deno.test("generated WASM completes blank nested schema positions after Unicode", () => {
  const source =
    '{ description = "日本語"; profile = {  }\n} as WorkspaceConfigSchema';
  const cursor = source.indexOf("{  }") + 2;
  set_snapshot({
    ...snapshot,
    entries: {
      ...snapshot.entries,
      "workspace.dcdl": {
        ...snapshot.entries["workspace.dcdl"],
        content: source,
      },
    },
  });
  set_schema_bundle({
    contributions: [],
    source: "{ profile = { default_profile = String; }; prompts = {}; }",
    fingerprint: "sha256:test-schema",
  });

  const result = complete_current(
    "workspace.dcdl",
    source,
    cursor,
    true,
  ) as {
    from: number;
    items: Array<{ label: string; kind: string }>;
  };

  assertEquals(result.from, cursor);
  assertEquals(
    result.items.some((item) => item.label === "default_profile"),
    true,
  );
  assertEquals(result.items.some((item) => item.label === "profile"), false);
  assertEquals(result.items.some((item) => item.label === "prompts"), false);
});

Deno.test("generated WASM completes asserted WorkspaceConfigSchema keys", () => {
  const bareSource = "{ pro }";
  const source = "{ pro } as WorkspaceConfigSchema";
  const cursor = source.indexOf("pro") + 3;
  set_snapshot({
    ...snapshot,
    entries: {
      ...snapshot.entries,
      "workspace.dcdl": {
        ...snapshot.entries["workspace.dcdl"],
        content: source,
      },
    },
  });
  set_schema_bundle({
    contributions: [],
    source: "{ profile = { default_profile = String; }; prompts = {}; }",
    fingerprint: "sha256:test-schema",
  });
  const bare = complete_current(
    "workspace.dcdl",
    bareSource,
    bareSource.indexOf("pro") + 3,
    true,
  ) as { items: Array<{ label: string }> } | null;
  assertEquals(
    bare?.items.some((item) => item.label === "profile") ?? false,
    false,
  );

  const result = complete_current(
    "workspace.dcdl",
    source,
    cursor,
    true,
  ) as {
    from: number;
    items: Array<{ label: string; kind: string }>;
  };

  assertEquals(result.from, 2);
  assertEquals(result.items.some((item) => item.label === "profile"), true);
});

Deno.test("generated WASM authors value-based Profiles using the Backend schema without materializing authoring fields", async () => {
  const source = await Deno.readTextFile(
    new URL(
      "../../../../resources/config-schema/profile.dcdl",
      import.meta.url,
    ),
  );
  const authoring_source = await Deno.readTextFile(
    new URL(
      "../../../../resources/config-schema/profile-authoring.dcdl",
      import.meta.url,
    ),
  );
  const schema = compose_schema_bundle([{
    provider_id: "builtin:profile",
    namespace: "profile",
    version: "2",
    source,
    source_digest: await digestText(source),
    authoring_source,
  }]) as WorkspaceConfigSchemaBundle;
  const content =
    '{ profile = { entries = [{ selector = "project:alpha"; profile = {}; }]; }; } as WorkspaceConfigSchema';
  const tree: ConfigTreeSnapshot = {
    revision: 8,
    digest: "sha256:value-profile-tree",
    entries: {
      "main.dcdl": {
        path: "main.dcdl",
        content_type: "decodal",
        content,
        content_digest: await digestText(content),
      },
    },
  };
  const valueContract = {
    ...contract,
    entrypoints: ["main.dcdl"],
    schema_bundle: schema,
    fingerprint: await toolchainFingerprint(["main.dcdl"], schema),
  };
  set_snapshot(tree);
  set_schema_bundle(schema);
  const evaluated = evaluate_snapshot(tree, valueContract) as {
    projections: Array<
      { data_json: { profile: { entries: Array<{ profile: unknown }> } } }
    >;
  };
  assertEquals(
    evaluated.projections[0].data_json.profile.entries[0].profile,
    {},
  );
  for (
    const [body, token, label] of [
      [
        "{ profile = { entries = [{ sel }] } } as WorkspaceConfigSchema",
        "sel",
        "selector",
      ],
      [
        "{ profile = { entries = [{ pro }] } } as WorkspaceConfigSchema",
        "pro",
        "profile",
      ],
      [
        "{ profile = { entries = [{ profile = { wor } }] } } as WorkspaceConfigSchema",
        "wor",
        "worker",
      ],
      [
        "{ profile = { entries = [{ profile = { worker = { mo } } }] } } as WorkspaceConfigSchema",
        "mo",
        "mode",
      ],
      [
        "{ profile = { entries = [{ profile = { feature = { ta } } }] } } as WorkspaceConfigSchema",
        "ta",
        "task",
      ],
    ]
  ) {
    const cursor = body.lastIndexOf(token) + token.length;
    const result = complete_current("main.dcdl", body, cursor, true) as {
      from: number;
      items: Array<{ label: string }>;
    };
    assertEquals(
      result.items.some((item) => item.label === label),
      true,
      label,
    );
    assertEquals(result.from, cursor - token.length);
  }
  for (
    const value of [
      "{}",
      "{ worker = {}; }",
      '{ worker = { mode = "wip"; }; }',
      "{ feature = { task = {}; }; }",
    ]
  ) {
    const partialContent = content.replace(
      "profile = {};",
      `profile = ${value};`,
    );
    assertEquals(
      analyze_snapshot(tree, "main.dcdl", partialContent),
      [],
      value,
    );
  }
  const recipe = "{ worker = { mode = 42; }; }";
  const analysisTree = {
    ...tree,
    entries: {
      ...tree.entries,
      "recipe.dcdl": {
        path: "recipe.dcdl",
        content_type: "decodal",
        content: recipe,
        content_digest: await digestText(recipe),
      },
    },
  };
  for (
    const [value, field, expectedPath] of [
      ["42", "profile", "main.dcdl"],
      ["{ worker = 42; }", "worker", "main.dcdl"],
      ["{ worker = { mode = 42; }; }", "mode", "main.dcdl"],
      ["{ feature = { task = { enabled = 42; }; }; }", "enabled", "main.dcdl"],
      ['{ worker = { typo = "wip"; }; }', "typo", "main.dcdl"],
      ['import "./recipe.dcdl"', "mode", "recipe.dcdl"],
      [
        '(import "./recipe.dcdl") // { worker = { mode = 42; }; }',
        "mode",
        "main.dcdl",
      ],
    ]
  ) {
    const invalidContent = content.replace(
      "profile = {};",
      `profile = ${value};`,
    );
    const diagnostics = analyze_snapshot(
      analysisTree,
      "main.dcdl",
      invalidContent,
    ) as Array<{
      path: string;
      revision: number;
      kind: string;
      span: { start_byte: number; end_byte: number };
      message: string;
      labels: Array<
        { span: { start_byte: number; end_byte: number }; message: string }
      >;
    }>;
    assertEquals(diagnostics.length > 0, true, value);
    const diagnostic = diagnostics[0];
    assertEquals(diagnostic.path, expectedPath);
    assertEquals(diagnostic.revision, tree.revision);
    assertEquals(
      ["constraint_violation", "type_mismatch"].includes(diagnostic.kind),
      true,
    );
    assertEquals(
      diagnostic.message.includes(field) ||
        diagnostic.labels.some((label) => label.message.includes(field)),
      true,
    );
    // New authoring failures must be anchored in the supplied value, not the
    // much longer schema source, including relative imported recipes.
    if (value !== "42") {
      const length = new TextEncoder().encode(
        expectedPath === "main.dcdl" ? invalidContent : recipe,
      ).length;
      assertEquals(
        diagnostic.span.end_byte > diagnostic.span.start_byte &&
          diagnostic.span.end_byte <= length,
        true,
      );
      assertEquals(
        diagnostic.labels.every((label) => label.span.end_byte <= length),
        true,
      );
    }
  }
  // Neither completion nor diagnostic shape checking changes the saved value.
  assertEquals(evaluate_snapshot(tree, valueContract), evaluated);
});

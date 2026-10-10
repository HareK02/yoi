/// <reference lib="deno.ns" />

import { createHash } from "node:crypto";
import { assertEquals, assertThrows } from "jsr:@std/assert";
import init, {
  analyze_snapshot,
  apply_changes,
  changes_between,
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


function fixtureSnapshot(tree: Pick<ConfigTreeSnapshot, "entries">): ConfigTreeSnapshot {
  const hash = createHash("sha256").update("yoi-config-tree-v1\0");
  const entries: ConfigTreeSnapshot["entries"] = {};
  for (const path of Object.keys(tree.entries).toSorted()) {
    const entry = tree.entries[path];
    entries[path] = {
      ...entry,
      content_digest: "sha256:" + createHash("sha256").update(entry.content).digest("hex"),
    };
    hash.update(path).update("\0")
      .update(entry.content_type === "decodal" ? "text/x-decodal" : "text/plain")
      .update("\0").update(entry.content).update("\0");
  }
  return { digest: "sha256:" + hash.digest("hex"), entries };
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
  const result = evaluate_snapshot(fixtureSnapshot(snapshot), contract) as {
    projections: Array<{ data_json: { answer: number } }>;
  };
  assertEquals(result.projections[0].data_json, { answer: 42 });
});

Deno.test("generated WASM keeps literal paths matching old cache identities as relative import bases", async () => {
  const source = "{}";
  const literalPath = `a.dcdl@text/x-decodal@${await digestText(source)}`;
  // Match the native fixture: even an unreferenced source must resolve its
  // relative import from the literal path rather than another entry's cache ID.
  const entries: ConfigTreeSnapshot["entries"] = {};
  for (const [path, content] of [
    ["main.dcdl", "{}"],
    ["a.dcdl", source],
    [literalPath, 'import "./shared.dcdl"'],
    ["a.dcdl@text/shared.dcdl", "{ answer = 42; }"],
  ]) {
    entries[path] = {
      path,
      content_type: "decodal",
      content,
      content_digest: await digestText(content),
    };
  }
  const result = evaluate_snapshot(
    fixtureSnapshot({ entries }),
    mainEntrypointContract,
  ) as { projections: Array<{ data_json: unknown }> };
  assertEquals(result.projections[0].data_json, {});
});

Deno.test("generated WASM accepts the mandatory main schema assertion", () => {
  const assertedSnapshot: ConfigTreeSnapshot = {
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
  const result = evaluate_snapshot(fixtureSnapshot(assertedSnapshot),
    mainEntrypointContract,
  ) as { projections: Array<{ data_json: { answer: number } }> };

  assertEquals(result.projections[0].data_json, { answer: 42 });
});

Deno.test("generated WASM diagnostics carry snapshot provenance", () => {
  const diagnostics = analyze_snapshot(fixtureSnapshot(snapshot),
    "workspace.dcdl",
    "{ broken = ; }",
  ) as Array<{
    path: string;
    tree_digest: string;
    kind: string;
  }>;
  assertEquals(diagnostics[0].path, "workspace.dcdl");
  assertEquals(diagnostics[0].tree_digest, fixtureSnapshot(snapshot).digest);
  assertEquals("revision" in diagnostics[0], false);
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
  const result = evaluate_snapshot(fixtureSnapshot(markdownSnapshot), markdownContract) as {
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
  const result = evaluate_snapshot(fixtureSnapshot(schemaSnapshot(
      "{ features = { console = { enabled = true; }; }; web = { enabled = true; extension_value = 42; }; }",
    )),
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
    evaluate_snapshot(fixtureSnapshot(schemaSnapshot(source)), schemaContract);
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
  set_snapshot(fixtureSnapshot({
    ...snapshot,
    entries: {
      ...snapshot.entries,
      "workspace.dcdl": {
        ...snapshot.entries["workspace.dcdl"],
        content: source,
      },
    },
  }));
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

Deno.test("generated WASM exposes read-only builtin completions without editable builtin entries", () => {
  const tree = schemaSnapshot("{}");
  set_snapshot(fixtureSnapshot(tree));
  set_schema_bundle(emptySchemaBundle);
  const source = 'let 名 = 1; import "$builtin/profiles/comp"';
  const result = complete_current(
    "main.dcdl",
    source,
    source.length - 1,
    true,
  ) as {
    from: number;
    items: Array<{ label: string; kind: string; detail: string }>;
  };
  const companion = result.items.find((item) =>
    item.label === "$builtin/profiles/companion.dcdl"
  );
  assertEquals(result.from, 'let 名 = 1; import "'.length);
  assertEquals(companion?.kind, "file");
  assertEquals(companion?.detail, "read-only builtin Decodal source");
  assertEquals(Object.keys(tree.entries), ["main.dcdl"]);
  const evaluated = evaluate_snapshot(fixtureSnapshot(schemaSnapshot('import "$builtin/profiles/companion.dcdl"')),
    mainEntrypointContract,
  ) as {
    projections: Array<{ data_json: { slug: string } }>;
  };
  assertEquals(evaluated.projections[0].data_json.slug, "companion");
});

Deno.test("generated WASM rejects unknown builtin imports without same-suffix workspace fallback", () => {
  const tree = schemaSnapshot('import "$builtin/profiles/missing.dcdl"');
  tree.entries["profiles/missing.dcdl"] = {
    path: "profiles/missing.dcdl",
    content_type: "decodal",
    content: "{}",
    content_digest: "sha256:workspace-fallback",
  };
  set_schema_bundle(emptySchemaBundle);
  const diagnostics = analyze_snapshot(fixtureSnapshot(tree), "main.dcdl", undefined) as Array<{
    path: string;
    tree_digest: string;
    message: string;
  }>;
  assertEquals(diagnostics.length > 0, true);
  assertEquals(diagnostics[0].path, "main.dcdl");
  assertEquals(diagnostics[0].tree_digest, fixtureSnapshot(tree).digest);
  assertEquals(
    diagnostics[0].message.includes(
      "unknown or non-public read-only builtin source",
    ),
    true,
  );
  assertEquals(
    diagnostics[0].message.includes("$builtin/profiles/missing.dcdl"),
    true,
  );
  let failed = false;
  try {
    evaluate_snapshot(fixtureSnapshot(tree), mainEntrypointContract);
  } catch (error) {
    failed = true;
    assertEquals(
      (error as Array<{ message: string }>)[0].message.includes(
        "unknown or non-public read-only builtin source",
      ),
      true,
    );
  }
  assertEquals(failed, true);
  assertEquals(
    analyze_snapshot(fixtureSnapshot(tree), "main.dcdl", 'import "./profiles/missing.dcdl"'),
    [],
  );
});

function importFailure(tree: ConfigTreeSnapshot): ProjectedDiagnostic[] {
  try {
    evaluate_snapshot(fixtureSnapshot(tree), mainEntrypointContract);
  } catch (error) {
    // A WASM trap/stack overflow must not count as a structured import failure.
    assertEquals(Array.isArray(error), true);
    const diagnostics = error as Array<
      ProjectedDiagnostic & {
        tree_digest: string;
      }
    >;
    assertEquals(diagnostics.length > 0, true);
    assertEquals(diagnostics[0].tree_digest, fixtureSnapshot(tree).digest);
    return diagnostics;
  }
  throw new Error("expected a structured import diagnostic");
}

Deno.test("generated WASM rejects nested lazy-object import cycles without a stack overflow", () => {
  const tree = schemaSnapshot('{ nested = import "./b.dcdl"; }');
  tree.entries["b.dcdl"] = {
    path: "b.dcdl",
    content_type: "decodal",
    content: '{ nested = import "./main.dcdl"; }',
    content_digest: "sha256:nested-cycle-b",
  };
  const diagnostics = importFailure(tree);
  assertEquals(diagnostics[0].kind, "cycle");
  assertEquals(diagnostics[0].message.includes("import cycle"), true);
});

Deno.test("generated WASM enforces the import depth budget on a real 33-deep chain", () => {
  function chain(depth: number): ConfigTreeSnapshot {
    const tree = schemaSnapshot('import "./chain/1.dcdl"');
    for (let index = 1; index <= depth; index++) {
      const path = `chain/${index}.dcdl`;
      tree.entries[path] = {
        path,
        content_type: "decodal",
        content: index === depth
          ? "{ answer = 42; }"
          : `import "./${index + 1}.dcdl"`,
        content_digest: `sha256:chain-${depth}-${index}`,
      };
    }
    return tree;
  }
  // The synthetic evaluation-wrapper import consumes one of the 32 edges.
  const accepted = evaluate_snapshot(fixtureSnapshot(chain(31)), mainEntrypointContract) as {
    projections: Array<{ data_json: { answer: number } }>;
  };
  assertEquals(accepted.projections[0].data_json, { answer: 42 });
  for (const depth of [32, 33]) {
    const diagnostics = importFailure(chain(depth));
    assertEquals(diagnostics[0].kind, "import");
    assertEquals(
      diagnostics.some((diagnostic) =>
        diagnostic.message.includes("import depth")
      ),
      true,
    );
  }
});

Deno.test("generated WASM completes blank nested schema positions after Unicode", () => {
  const source =
    '{ description = "日本語"; profile = {  }\n} as WorkspaceConfigSchema';
  const cursor = source.indexOf("{  }") + 2;
  set_snapshot(fixtureSnapshot({
    ...snapshot,
    entries: {
      ...snapshot.entries,
      "workspace.dcdl": {
        ...snapshot.entries["workspace.dcdl"],
        content: source,
      },
    },
  }));
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
  set_snapshot(fixtureSnapshot({
    ...snapshot,
    entries: {
      ...snapshot.entries,
      "workspace.dcdl": {
        ...snapshot.entries["workspace.dcdl"],
        content: source,
      },
    },
  }));
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

function fixtureDecodal(value: unknown): string {
  if (Array.isArray(value)) return `[${value.map(fixtureDecodal).join(", ")}]`;
  if (value !== null && typeof value === "object") {
    return `{ ${
      Object.entries(value).map(([key, value]) =>
        `${key} = ${fixtureDecodal(value)};`
      ).join(" ")
    } }`;
  }
  if (
    typeof value === "string" || typeof value === "number" ||
    typeof value === "boolean"
  ) return JSON.stringify(value);
  throw new Error("fixture omission must not be represented as null/undefined");
}

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
  set_snapshot(fixtureSnapshot(tree));
  set_schema_bundle(schema);
  const evaluated = evaluate_snapshot(fixtureSnapshot(tree), valueContract) as {
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
      analyze_snapshot(fixtureSnapshot(tree), "main.dcdl", partialContent),
      [],
      value,
    );
  }
  const recipe = "{ worker = { mode = 42; }; }";
  const analysisTree: ConfigTreeSnapshot = {
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
    const diagnostics = analyze_snapshot(fixtureSnapshot(analysisTree),
      "main.dcdl",
      invalidContent,
    ) as Array<{
      path: string;
      tree_digest: string;
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
    assertEquals(diagnostic.tree_digest, fixtureSnapshot(analysisTree).digest);
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
  for (const field of ["scope", "delegation_scope"]) {
    for (
      const [prefix, label] of [["int", "intent"], ["deny", "deny_write"], [
        "sym",
        "symlink_policy",
      ]]
    ) {
      const body =
        `{ profile.entries = [{ profile.${field} = { ${prefix} } }]; } as WorkspaceConfigSchema`;
      const cursor = body.lastIndexOf(prefix) + prefix.length;
      const result = complete_current("main.dcdl", body, cursor, true) as {
        items: Array<{ label: string }>;
      };
      assertEquals(
        result.items.some((item) => item.label === label),
        true,
        `${field}.${label}`,
      );
    }
  }
  for (
    const [recipe, expected] of [
      ['{ scope = "workspace_read"; delegation_scope = "workspace_write"; }', {
        scope: "workspace_read",
        delegation_scope: "workspace_write",
      }],
      ['{ scope = "workspace_write"; delegation_scope = "workspace_read"; }', {
        scope: "workspace_write",
        delegation_scope: "workspace_read",
      }],
      [
        '{ scope = { intent = "workspace_read"; }; delegation_scope = { intent = "workspace_write"; deny_write = ["private"]; symlink_policy = "resolved"; }; }',
        {
          scope: { intent: "workspace_read" },
          delegation_scope: {
            intent: "workspace_write",
            deny_write: ["private"],
            symlink_policy: "resolved",
          },
        },
      ],
    ] as const
  ) {
    for (
      const form of [
        recipe,
        'import "./scoped.dcdl"',
        '(import "./scoped.dcdl") // { description = "patched"; }',
      ]
    ) {
      const scopedContent = content.replace(
        "profile = {};",
        `profile = ${form};`,
      );
      const scopedTree: ConfigTreeSnapshot = {
        ...tree,
        entries: {
          "main.dcdl": {
            ...tree.entries["main.dcdl"],
            content: scopedContent,
            content_digest: await digestText(scopedContent),
          },
          "scoped.dcdl": {
            path: "scoped.dcdl",
            content_type: "decodal",
            content: recipe,
            content_digest: await digestText(recipe),
          },
        },
      };
      assertEquals(
        analyze_snapshot(fixtureSnapshot(scopedTree), "main.dcdl", undefined),
        [],
        form,
      );
      const result = evaluate_snapshot(fixtureSnapshot(scopedTree),
        valueContract,
      ) as typeof evaluated;
      assertEquals(
        result.projections[0].data_json.profile.entries[0].profile,
        form.includes("patched")
          ? { ...expected, description: "patched" }
          : expected,
      );
    }
  }
  const cases = JSON.parse(
    await Deno.readTextFile(
      new URL(
        "../../../../resources/config-schema/profile-authoring-values.json",
        import.meta.url,
      ),
    ),
  ) as Array<{ name: string; profile: Record<string, unknown> }>;
  for (const fixture of cases) {
    const recipe = fixtureDecodal(fixture.profile);
    for (
      const [form, patched] of [[recipe, false], [
        'import "./recipe.dcdl"',
        false,
      ], [
        '(import "./recipe.dcdl") // { description = "patched"; }',
        true,
      ]] as const
    ) {
      const fixtureContent = content.replace(
        "profile = {};",
        `profile = ${form};`,
      );
      const fixtureTree: ConfigTreeSnapshot = {
        ...tree,
        entries: {
          "main.dcdl": {
            ...tree.entries["main.dcdl"],
            content: fixtureContent,
            content_digest: await digestText(fixtureContent),
          },
          "recipe.dcdl": {
            path: "recipe.dcdl",
            content_type: "decodal",
            content: recipe,
            content_digest: await digestText(recipe),
          },
        },
      };
      assertEquals(
        analyze_snapshot(fixtureSnapshot(fixtureTree), "main.dcdl", undefined),
        [],
        `${fixture.name}: ${form}`,
      );
      const result = evaluate_snapshot(fixtureSnapshot(fixtureTree),
        valueContract,
      ) as typeof evaluated;
      assertEquals(
        result.projections[0].data_json.profile.entries[0].profile,
        patched
          ? { ...fixture.profile, description: "patched" }
          : fixture.profile,
      );
    }
  }
  for (
    const [path, prefix, label] of [
      ["compaction", "reta", "retained_tokens"],
      ["compaction", "prune_min", "prune_min_savings"],
      ["compaction", "req", "request"],
      ["compaction", "wor", "worker"],
      ["compaction", "compact_ret", "compact_retained_tokens"],
      ["feature.memory.extraction", "reas", "reasoning"],
      ["feature.subjektiv.extraction", "reas", "reasoning"],
    ]
  ) {
    const body =
      `{ profile.entries = [{ profile.${path} = { ${prefix} } }]; } as WorkspaceConfigSchema`;
    const cursor = body.lastIndexOf(prefix) + prefix.length;
    const result = complete_current("main.dcdl", body, cursor, true) as {
      items: Array<{ label: string }>;
    };
    assertEquals(
      result.items.some((item) => item.label === label),
      true,
      `${path}.${label}`,
    );
  }
  // Neither completion nor diagnostic shape checking changes the saved value.
  assertEquals(evaluate_snapshot(fixtureSnapshot(tree), valueContract), evaluated);
});

Deno.test("generated WASM returns to the same content digest and enforces entry digest CAS", () => {
  const base = fixtureSnapshot(schemaSnapshot("{ answer = 1; }"));
  set_schema_bundle(emptySchemaBundle);
  set_snapshot(base);
  const changed = apply_changes([{
    kind: "update", path: "main.dcdl",
    expected_digest: base.entries["main.dcdl"].content_digest,
    content: "{ broken = ; }",
  }]) as ConfigTreeSnapshot;
  assertEquals(changed.digest === base.digest, false);
  assertEquals("revision" in changed, false);
  const diagnostics = analyze_snapshot(changed, "main.dcdl", undefined) as Array<{ tree_digest: string }>;
  assertEquals(diagnostics.length > 0, true);
  assertEquals(diagnostics.every((diagnostic) => diagnostic.tree_digest === changed.digest), true);
  assertThrows(() => apply_changes([{
    kind: "update", path: "main.dcdl",
    expected_digest: base.entries["main.dcdl"].content_digest,
    content: "{ answer = 2; }",
  }]));
  const restored = apply_changes(changes_between(changed, base)) as ConfigTreeSnapshot;
  assertEquals(restored, base);
  assertEquals(analyze_snapshot(restored, "main.dcdl", undefined), []);
});

Deno.test("generated WASM rejects cache URI paths in snapshot JSON despite valid content digests", async () => {
  const source = "{}";
  const cacheUri = `config-source://a.dcdl@text/x-decodal@${await digestText(source)}`;
  // Both entry and tree digests are correct, so rejection must come from the
  // untrusted VirtualPath boundary rather than a content-identity mismatch.
  const snapshot = fixtureSnapshot({
    entries: {
      "a.dcdl": {
        path: "a.dcdl",
        content_type: "decodal",
        content: source,
        content_digest: await digestText(source),
      },
      [cacheUri]: {
        path: cacheUri,
        content_type: "decodal",
        content: 'import "./shared.dcdl"',
        content_digest: await digestText('import "./shared.dcdl"'),
      },
    },
  });
  const json = JSON.stringify(snapshot);
  const rejection = assertThrows(() => set_snapshot(JSON.parse(json)));
  assertEquals(String(rejection).includes("config-source://"), true);
});

Deno.test("generated WASM rejects forged content identity at the snapshot boundary", () => {
  const base = fixtureSnapshot(schemaSnapshot("{}"));
  for (const forged of [
    { ...base, digest: "sha256:forged" },
    { ...base, entries: { "main.dcdl": { ...base.entries["main.dcdl"], content: "{ changed = true; }" } } },
  ]) {
    assertThrows(() => set_snapshot(forged));
  }
});

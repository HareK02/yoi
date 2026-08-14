declare const Deno: {
  test(name: string, fn: () => Promise<void> | void): void;
  readTextFile(path: URL): Promise<string>;
};

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

Deno.test("config editor snapshots Svelte proxies before cloning baselines", async () => {
  const source = await Deno.readTextFile(
    new URL(
      "../../src/lib/workspace/config-source/ConfigSourceEditor.svelte",
      import.meta.url,
    ),
  );

  assert(
    source.includes("$state.raw<WorkspaceConfigTreeResponse"),
    "the immutable config baseline should not be wrapped in another deep proxy",
  );
  assert(
    source.includes("$state.snapshot(treeState.snapshot)"),
    "reactive tree snapshots should be converted to plain values before reuse",
  );
  assert(
    !source.includes("structuredClone(treeState.snapshot)"),
    "structuredClone must never receive a Svelte state proxy",
  );
  assert(
    source.includes("toolchain?.setSnapshot(") &&
      source.includes("treeState.contract.schema_bundle") &&
      source.includes("remote.contract.schema_bundle"),
    "the authoritative schema bundle should be installed with every snapshot",
  );
});

Deno.test("Decodal editor follows readonly prop changes after mount", async () => {
  const source = await Deno.readTextFile(
    new URL(
      "../../src/lib/workspace/settings/DecodalSourceEditor.svelte",
      import.meta.url,
    ),
  );

  assert(
    source.includes("let view = $state.raw<EditorView | null>(null)") &&
      source.includes("if (!host || untrack(() => view)) return"),
    "imperative EditorView should avoid deep proxying and remain outside the mount effect dependency graph",
  );
  assert(
    source.includes("new Compartment()") &&
      source.includes("readonlyCompartment.reconfigure") &&
      source.includes("EditorView.editable.of(!nextReadonly)"),
    "readonly and editable facets should be reconfigured when the prop changes",
  );
  assert(
    source.includes("syntaxHighlighting(syntaxTheme)") &&
      source.includes("tags.keyword") &&
      source.includes("tags.string"),
    "Decodal tokens should use an explicit syntax theme",
  );
  assert(
    source.includes(".cm-cursor, .cm-dropCursor") &&
      source.includes(".cm-selectionBackground") &&
      source.includes("var(--interactive-selected)"),
    "cursor and selection should be visible against the workspace theme",
  );
  assert(
    !source.includes("--surface-2") &&
      !source.includes("--text-primary") &&
      !source.includes("--border-subtle"),
    "CodeMirror theme must use workspace tokens that actually exist",
  );
  assert(
    source.includes("fixedSchemaWrapperCompartment.reconfigure") &&
      source.includes("fixedSchemaWrapperExtension()") &&
      source.includes("filter: false"),
    "fixed schema wrapper protection should react to the main source and permit authoritative synchronization",
  );
});

Deno.test("main entrypoint always enables the fixed schema wrapper", async () => {
  const source = await Deno.readTextFile(
    new URL(
      "../../src/lib/workspace/config-source/ConfigSourceEditor.svelte",
      import.meta.url,
    ),
  );

  assert(
    source.includes("fixedSchemaWrapper={mainSelected}"),
    "main.dcdl should always enable fixed wrapper behavior",
  );
  assert(
    !source.includes("Wrap with WorkspaceConfigSchema") &&
      !source.includes("wrapMainWithWorkspaceSchema"),
    "the authoritative main source should not require an optional client-side conversion",
  );
});

import { CODEMIRROR_VITE_DEDUPE } from "../../src/lib/workspace/config-source/vite-dedupe.ts";

declare const Deno: {
  test(name: string, fn: () => Promise<void> | void): void;
  readTextFile(path: URL): Promise<string>;
};

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

Deno.test("Vite deduplicates CodeMirror stateful packages", () => {
  for (
    const packageName of [
      "@codemirror/autocomplete",
      "@codemirror/language",
      "@codemirror/state",
      "@codemirror/view",
      "@lezer/common",
    ]
  ) {
    assert(
      CODEMIRROR_VITE_DEDUPE.includes(packageName),
      `Vite must deduplicate ${packageName}`,
    );
  }
});

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
    source.includes("keymap.of(completionKeymap)") &&
      source.includes("startCompletion(editor)") &&
      source.includes("update.selectionSet && !update.docChanged") &&
      source.includes("completionStatus(editor.state) === null"),
    "completion should be explicitly available and start when an editable cursor moves into an empty schema position",
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

Deno.test("commit formats every working Decodal source without a preview roundtrip", async () => {
  const source = await Deno.readTextFile(
    new URL(
      "../../src/lib/workspace/config-source/ConfigSourceEditor.svelte",
      import.meta.url,
    ),
  );

  assert(
    source.includes("async function formatWorkingSources()") &&
      source.includes('change.kind === "create" || change.kind === "update"') &&
      source.includes('change.kind === "rename"') &&
      source.includes('entry.content_type !== "decodal"') &&
      source.includes("await toolchain.format(entry.content)"),
    "all changed Decodal entries should be formatted rather than only the selected source",
  );
  assert(
    source.includes("await formatWorkingSources();") &&
      source.includes("entrypoints: entrypoints()") &&
      source.includes("Committed formatted revision"),
    "commit should format first and send the working changes directly to Backend authority",
  );
  assert(
    !source.includes("previewConfigTree") &&
      !source.includes("requestCandidatePreview") &&
      !source.includes("toolchain_fingerprint"),
    "normal commit must not depend on a separate preview or client-echoed toolchain fingerprint",
  );
});

Deno.test("config diagnostics analyze continuously with debounce and generation fencing", async () => {
  const source = await Deno.readTextFile(
    new URL(
      "../../src/lib/workspace/config-source/ConfigSourceEditor.svelte",
      import.meta.url,
    ),
  );

  assert(
    source.includes("setTimeout(() =>") &&
      source.includes("}, 250)") &&
      source.includes("analyzer.analyze(path, value)") &&
      source.includes("generation === analysisGeneration") &&
      source.includes("analysisReady"),
    "source changes should trigger only the latest debounced analysis after snapshot initialization",
  );
  assert(
    !source.includes("onclick={analyze}") &&
      !source.includes(">Analyze</button>") &&
      !source.includes(">Preview</button>"),
    "manual Analyze and Preview controls should be removed",
  );
});

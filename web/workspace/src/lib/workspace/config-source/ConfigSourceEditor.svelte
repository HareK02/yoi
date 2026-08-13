<script lang="ts">
  import { onMount } from "svelte";
  import DecodalSourceEditor from "$lib/workspace/settings/DecodalSourceEditor.svelte";
  import {
    commitConfigTree,
    fetchConfigTree,
    previewConfigTree,
  } from "./api.ts";
  import { ConfigSourceToolchain } from "./toolchain.ts";
  import type {
    ConfigDiagnostic,
    ConfigTreeChange,
    WorkspaceConfigTreeResponse,
  } from "./types.ts";

  let { workspaceId }: { workspaceId: string } = $props();
  let treeState = $state<WorkspaceConfigTreeResponse | null>(null);
  let selectedPath = $state("");
  let source = $state("");
  let newPath = $state("workspace.dcdl");
  let diagnostics = $state<ConfigDiagnostic[]>([]);
  let status = $state("Loading source tree…");
  let busy = $state(false);
  let draftChanges = $state<ConfigTreeChange[]>([]);
  let baseRevision = $state(0);
  let baseDigest = $state("");
  let renamePath = $state("");
  let baseSnapshot = $state<WorkspaceConfigTreeResponse["snapshot"] | null>(null);
  let preflightDigest = $state("");
  let toolchain: ConfigSourceToolchain | null = null;

  const paths = $derived(
    treeState ? Object.keys(treeState.snapshot.entries).toSorted() : [],
  );
  const selected = $derived(
    treeState && selectedPath ? treeState.snapshot.entries[selectedPath] : undefined,
  );
  const dirty = $derived(draftChanges.length > 0 || (selected ? source !== selected.content : source.length > 0));
  const commitReady = $derived(dirty && preflightDigest === treeState?.snapshot.digest);

  onMount(() => {
    toolchain = new ConfigSourceToolchain();
    void reload();
    return () => toolchain?.close();
  });

  async function reload() {
    try {
      treeState = await fetchConfigTree(workspaceId);
      if (!selectedPath || !treeState.snapshot.entries[selectedPath]) {
        selectedPath = Object.keys(treeState.snapshot.entries).toSorted()[0] ?? "";
      }
      source = selectedPath ? treeState.snapshot.entries[selectedPath].content : "";
      baseSnapshot = structuredClone(treeState.snapshot);
      baseRevision = treeState.snapshot.revision;
      baseDigest = treeState.snapshot.digest;
      await toolchain?.setSnapshot(treeState.snapshot);
      draftChanges = [];
      renamePath = selectedPath;
      diagnostics = [];
      status = treeState.snapshot.revision === 0
        ? "No committed sources yet. Create workspace.dcdl to begin."
        : `Revision ${treeState.snapshot.revision} · ${treeState.snapshot.digest.slice(0, 20)}…`;
    } catch (error) {
      status = String(error);
    }
  }

  async function stageCurrent() {
    const change = currentChange();
    if (!change || !toolchain) return;
    const candidate = await toolchain.applyChanges([change]);
    if (treeState) treeState = { ...treeState, snapshot: candidate };
    if (baseSnapshot) draftChanges = await toolchain.changesBetween(baseSnapshot, candidate);
    preflightDigest = "";
  }

  async function select(path: string) {
    await stageCurrent();
    selectedPath = path;
    source = treeState?.snapshot.entries[path]?.content ?? "";
    renamePath = path;
    diagnostics = [];
  }

  function currentChange(): ConfigTreeChange | null {
    if (!treeState || !selectedPath) return null;
    const entry = treeState.snapshot.entries[selectedPath];
    if (!entry) {
      return {
        kind: "create",
        path: selectedPath,
        content_type: "decodal",
        content: source,
      };
    }
    if (source === entry.content) return null;
    return {
      kind: "update",
      path: selectedPath,
      expected_digest: entry.content_digest,
      content: source,
    };
  }

  function entrypoints(): string[] {
    if (!treeState) return [];
    const known = new Set(Object.keys(treeState.snapshot.entries));
    const configured = treeState.contract.entrypoints.filter((path) => known.has(path));
    if (configured.length > 0) return configured;
    if (treeState.snapshot.entries["workspace.dcdl"] || selectedPath === "workspace.dcdl") {
      return ["workspace.dcdl"];
    }
    return selectedPath ? [selectedPath] : [];
  }

  async function analyze() {
    if (!toolchain || !treeState || !selectedPath) return;
    diagnostics = await toolchain.analyze(selectedPath, source);
    status = diagnostics.length === 0 ? "No diagnostics." : `${diagnostics.length} diagnostic(s).`;
  }

  async function format() {
    if (!toolchain) return;
    try {
      source = await toolchain.format(source);
      await analyze();
    } catch (error) {
      status = String(error);
    }
  }

  async function preview() {
    if (!treeState) return;
    await stageCurrent();
    if (draftChanges.length === 0) {
      status = "No draft changes to preview.";
      return;
    }
    busy = true;
    try {
      const candidate = await previewConfigTree(workspaceId, {
        changes: draftChanges,
        entrypoints: entrypoints(),
        toolchain_fingerprint: treeState.contract.fingerprint,
      });
      diagnostics = [];
      preflightDigest = candidate.snapshot.digest;
      status = `Preview valid · projection ${candidate.evaluation.projection_digest.slice(0, 20)}…`;
    } catch (error) {
      status = String(error);
    } finally {
      busy = false;
    }
  }

  async function commit() {
    if (!treeState) return;
    await stageCurrent();
    if (draftChanges.length === 0) {
      status = "No draft changes to commit.";
      return;
    }
    if (preflightDigest !== treeState.snapshot.digest) {
      status = "Preview the complete candidate successfully before Commit.";
      return;
    }
    busy = true;
    try {
      treeState = await commitConfigTree(workspaceId, {
        base_revision: baseRevision,
        base_digest: baseDigest,
        changes: draftChanges,
        entrypoints: entrypoints(),
        toolchain_fingerprint: treeState.contract.fingerprint,
      });
      draftChanges = [];
      preflightDigest = "";
      baseSnapshot = structuredClone(treeState.snapshot);
      baseRevision = treeState.snapshot.revision;
      baseDigest = treeState.snapshot.digest;
      await toolchain?.setSnapshot(treeState.snapshot);
      source = treeState.snapshot.entries[selectedPath]?.content ?? "";
      diagnostics = [];
      status = `Committed revision ${treeState.snapshot.revision}.`;
    } catch (error) {
      status = String(error);
    } finally {
      busy = false;
    }
  }

  function createEntry() {
    const path = newPath.trim();
    if (!path || treeState?.snapshot.entries[path]) return;
    selectedPath = path;
    source = "{}\n";
    diagnostics = [];
    status = `Drafting new source ${path}. It is not persisted until Commit succeeds.`;
  }

  async function deleteEntry() {
    if (!treeState || !selected || !toolchain) return;
    const change: ConfigTreeChange = {
      kind: "delete",
      path: selectedPath,
      expected_digest: selected.content_digest,
    };
    const candidate = await toolchain.applyChanges([change]);
    treeState = { ...treeState, snapshot: candidate };
    if (baseSnapshot) draftChanges = await toolchain.changesBetween(baseSnapshot, candidate);
    preflightDigest = "";
    selectedPath = Object.keys(candidate.entries).toSorted()[0] ?? "";
    source = selectedPath ? candidate.entries[selectedPath].content : "";
    renamePath = selectedPath;
    status = "Delete staged. Preview and Commit to persist the candidate tree.";
  }

  async function renameEntry() {
    if (!treeState || !selected || !toolchain) return;
    const to = renamePath.trim();
    if (!to || to === selectedPath) return;
    await stageCurrent();
    const change: ConfigTreeChange = {
      kind: "rename",
      from: selectedPath,
      to,
      expected_digest: selected.content_digest,
    };
    const candidate = await toolchain.applyChanges([change]);
    treeState = { ...treeState, snapshot: candidate };
    if (baseSnapshot) draftChanges = await toolchain.changesBetween(baseSnapshot, candidate);
    preflightDigest = "";
    selectedPath = to;
    source = candidate.entries[to]?.content ?? "";
    status = `Rename to ${to} staged. Preview and Commit to persist.`;
  }
</script>

<section class="config-source-shell" aria-label="Workspace configuration source tree">
  <aside class="config-source-tree">
    <div class="config-source-tree__header">
      <strong>Source tree</strong>
      <span>{paths.length}</span>
    </div>
    <nav aria-label="Virtual configuration paths">
      {#each paths as path}
        <button
          type="button"
          class:active={path === selectedPath}
          onclick={() => select(path)}
        >{path}</button>
      {/each}
    </nav>
    <form class="config-source-create" onsubmit={(event) => { event.preventDefault(); createEntry(); }}>
      <label for="new-config-path">New path</label>
      <input id="new-config-path" bind:value={newPath} placeholder="workspace.dcdl" />
      <button type="submit">Create draft</button>
    </form>
  </aside>

  <div class="config-source-workbench">
    <header class="config-source-workbench__header">
      <div>
        <span>Virtual path</span>
        <strong>{selectedPath || "Select or create a source"}</strong>
      </div>
      <div class="config-source-actions">
        <input aria-label="Rename path" bind:value={renamePath} disabled={!selected || busy} />
        <button type="button" onclick={renameEntry} disabled={!selected || renamePath === selectedPath || busy}>Rename</button>
        <button type="button" onclick={format} disabled={!selectedPath || busy}>Format</button>
        <button type="button" onclick={analyze} disabled={!selectedPath || busy}>Analyze</button>
        <button type="button" onclick={preview} disabled={!dirty || busy}>Preview</button>
        <button class="primary" type="button" onclick={commit} disabled={!commitReady || busy}>Commit</button>
        <button class="danger" type="button" onclick={deleteEntry} disabled={!selected || busy}>Delete</button>
      </div>
    </header>
    <DecodalSourceEditor
      value={source}
      readonly={!selectedPath || busy}
      onChange={(value) => source = value}
      onComplete={(value, offset, explicit) => toolchain?.complete(selectedPath, value, offset, explicit) ?? Promise.resolve(null)}
    />
    <p class="config-source-status" aria-live="polite">{status}</p>
    {#if diagnostics.length > 0}
      <ol class="config-source-diagnostics">
        {#each diagnostics as diagnostic}
          <li>
            <strong>{diagnostic.kind}</strong>
            <span>{diagnostic.message}</span>
            <small>bytes {diagnostic.span.start_byte}–{diagnostic.span.end_byte}</small>
          </li>
        {/each}
      </ol>
    {/if}
  </div>
</section>

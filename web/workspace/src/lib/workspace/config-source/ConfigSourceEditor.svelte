<script lang="ts">
  import { onMount } from "svelte";
  import DecodalSourceEditor from "$lib/workspace/settings/DecodalSourceEditor.svelte";
  import {
    commitConfigTree,
    fetchConfigTree,
  } from "./api.ts";
  import { ConfigSourceToolchain } from "./toolchain.ts";
  import type {
    ConfigDiagnostic,
    ConfigTreeChange,
    WorkspaceConfigTreeResponse,
  } from "./types.ts";

  const MAIN_ENTRYPOINT = "main.dcdl";

  let { workspaceId }: { workspaceId: string } = $props();
  let treeState = $state<WorkspaceConfigTreeResponse | null>(null);
  let selectedPath = $state("");
  let source = $state("");
  let newPath = $state("module.dcdl");
  let diagnostics = $state<ConfigDiagnostic[]>([]);
  let status = $state("Loading source tree…");
  let busy = $state(false);
  let workingChanges = $state<ConfigTreeChange[]>([]);
  let baseRevision = $state(0);
  let baseDigest = $state("");
  let renamePath = $state("");
  let baseSnapshot = $state.raw<WorkspaceConfigTreeResponse["snapshot"] | null>(null);
  let conflict = $state(false);
  let toolchain = $state.raw<ConfigSourceToolchain | null>(null);
  let analysisReady = $state(false);
  let analysisGeneration = 0;

  const paths = $derived(
    treeState ? Object.keys(treeState.snapshot.entries).toSorted() : [],
  );
  const selected = $derived(
    treeState && selectedPath ? treeState.snapshot.entries[selectedPath] : undefined,
  );
  const mainSelected = $derived(selectedPath === MAIN_ENTRYPOINT);
  const dirty = $derived(workingChanges.length > 0 || (selected ? source !== selected.content : source.length > 0));

  onMount(() => {
    toolchain = new ConfigSourceToolchain();
    void reload();
    return () => toolchain?.close();
  });

  $effect(() => {
    const analyzer = toolchain;
    const path = selectedPath;
    const value = source;
    const ready = analysisReady;
    const generation = ++analysisGeneration;
    diagnostics = [];
    if (!analyzer || !path || !ready) return;

    const timer = setTimeout(() => {
      void analyzer.analyze(path, value).then((result) => {
        if (generation === analysisGeneration) diagnostics = result;
      }).catch((error) => {
        if (generation === analysisGeneration) status = `Analyze failed: ${String(error)}`;
      });
    }, 250);
    return () => {
      clearTimeout(timer);
      if (generation === analysisGeneration) analysisGeneration += 1;
    };
  });

  async function reload() {
    analysisReady = false;
    try {
      treeState = await fetchConfigTree(workspaceId);
      if (!selectedPath || !treeState.snapshot.entries[selectedPath]) {
        selectedPath = Object.keys(treeState.snapshot.entries).toSorted()[0] ?? "";
      }
      source = selectedPath ? treeState.snapshot.entries[selectedPath].content : "";
      baseSnapshot = $state.snapshot(treeState.snapshot);
      baseRevision = treeState.snapshot.revision;
      baseDigest = treeState.snapshot.digest;
      await toolchain?.setSnapshot(treeState.snapshot, treeState.contract.schema_bundle);
      analysisReady = true;
      workingChanges = [];
      renamePath = selectedPath;
      diagnostics = [];
      conflict = false;
      status = `Revision ${treeState.snapshot.revision} · ${treeState.snapshot.digest.slice(0, 20)}…`;
    } catch (error) {
      status = String(error);
    }
  }

  async function stageCurrent() {
    const change = currentChange();
    if (!change || !toolchain) return;
    const candidate = await toolchain.applyChanges([change]);
    if (treeState) treeState = { ...treeState, snapshot: candidate };
    if (baseSnapshot) workingChanges = await toolchain.changesBetween(baseSnapshot, candidate);
    conflict = false;
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
    return [MAIN_ENTRYPOINT];
  }

  async function format() {
    if (!toolchain) return;
    try {
      source = await toolchain.format(source);
      status = "Formatted source. Changes remain local until Commit succeeds.";
    } catch (error) {
      status = String(error);
    }
  }

  async function formatWorkingSources() {
    if (!toolchain || !treeState || !baseSnapshot) return;
    await stageCurrent();
    const paths = new Set<string>();
    for (const change of workingChanges) {
      if (change.kind === "create" || change.kind === "update") paths.add(change.path);
      if (change.kind === "rename") paths.add(change.to);
    }

    let candidate = treeState.snapshot;
    let formattedAny = false;
    for (const path of paths) {
      const entry = candidate.entries[path];
      if (!entry || entry.content_type !== "decodal") continue;
      const formatted = await toolchain.format(entry.content);
      if (formatted === entry.content) continue;
      candidate = await toolchain.applyChanges([{
        kind: "update",
        path,
        expected_digest: entry.content_digest,
        content: formatted,
      }]);
      formattedAny = true;
    }
    if (!formattedAny) return;

    treeState = { ...treeState, snapshot: candidate };
    workingChanges = await toolchain.changesBetween(baseSnapshot, candidate);
    source = candidate.entries[selectedPath]?.content ?? source;
    conflict = false;
  }

  function recordCommitError(error: unknown) {
    const message = String(error);
    conflict = message.includes("conflict") || message.includes("base revision/digest mismatch");
    status = conflict ? `${message} Reload the authoritative tree before editing again.` : message;
  }

  async function commit() {
    if (!treeState) return;
    busy = true;
    try {
      await formatWorkingSources();
      if (workingChanges.length === 0) {
        status = "No working changes to commit.";
        return;
      }
      treeState = await commitConfigTree(workspaceId, {
        base_revision: baseRevision,
        base_digest: baseDigest,
        changes: workingChanges,
        entrypoints: entrypoints(),
      });
      workingChanges = [];
      conflict = false;
      baseSnapshot = $state.snapshot(treeState.snapshot);
      baseRevision = treeState.snapshot.revision;
      baseDigest = treeState.snapshot.digest;
      await toolchain?.setSnapshot(treeState.snapshot, treeState.contract.schema_bundle);
      source = treeState.snapshot.entries[selectedPath]?.content ?? "";
      diagnostics = [];
      status = `Committed formatted revision ${treeState.snapshot.revision}.`;
    } catch (error) {
      recordCommitError(error);
    } finally {
      busy = false;
    }
  }

  async function discardAndReload() {
    workingChanges = [];
    source = "";
    await reload();
  }

  async function reloadAndReapply() {
    if (!toolchain || !treeState) return;
    const localChanges = [...workingChanges];
    const remote = await fetchConfigTree(workspaceId);
    baseSnapshot = structuredClone(remote.snapshot);
    baseRevision = remote.snapshot.revision;
    baseDigest = remote.snapshot.digest;
    await toolchain.setSnapshot(remote.snapshot, remote.contract.schema_bundle);
    try {
      const candidate = await toolchain.applyChanges(localChanges);
      workingChanges = localChanges;
      treeState = { ...remote, snapshot: candidate };
      selectedPath = candidate.entries[selectedPath] ? selectedPath : Object.keys(candidate.entries).toSorted()[0] ?? "";
      source = selectedPath ? candidate.entries[selectedPath].content : "";
      conflict = false;
      status = "Local changes reapplied to the latest revision. Commit to persist them.";
    } catch (error) {
      conflict = true;
      status = `Local changes conflict with the latest revision: ${String(error)}. Discard local changes or resolve against a fresh reload.`;
    }
  }

  function createEntry() {
    const path = newPath.trim();
    if (!path || treeState?.snapshot.entries[path]) return;
    selectedPath = path;
    source = "{}\n";
    diagnostics = [];
    status = `Creating local source ${path}. It is not persisted until Commit succeeds.`;
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
    if (baseSnapshot) workingChanges = await toolchain.changesBetween(baseSnapshot, candidate);
    conflict = false;
    selectedPath = Object.keys(candidate.entries).toSorted()[0] ?? "";
    source = selectedPath ? candidate.entries[selectedPath].content : "";
    renamePath = selectedPath;
    status = "Delete staged locally. Commit to persist the working tree.";
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
    if (baseSnapshot) workingChanges = await toolchain.changesBetween(baseSnapshot, candidate);
    conflict = false;
    selectedPath = to;
    source = candidate.entries[to]?.content ?? "";
    status = `Rename to ${to} staged locally. Commit to persist it.`;
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
        >
          <span>{path}</span>
          {#if path === MAIN_ENTRYPOINT}<small>entrypoint</small>{/if}
        </button>
      {/each}
    </nav>
    <form class="config-source-create" onsubmit={(event) => { event.preventDefault(); createEntry(); }}>
      <label for="new-config-path">New path</label>
      <input id="new-config-path" bind:value={newPath} placeholder="module.dcdl" />
      <button type="submit">Create local source</button>
    </form>
  </aside>

  <div class="config-source-workbench">
    <header class="config-source-workbench__header">
      <div>
        <span>Virtual path</span>
        <strong>{selectedPath || "Select or create a source"}</strong>
      </div>
      <div class="config-source-actions">
        <input aria-label="Rename path" bind:value={renamePath} disabled={!selected || mainSelected || busy} />
        <button type="button" onclick={renameEntry} disabled={!selected || mainSelected || renamePath === selectedPath || busy}>Rename</button>
        <button type="button" onclick={format} disabled={!selectedPath || busy}>Format</button>
        <button class="primary" type="button" onclick={commit} disabled={!dirty || busy}>Commit</button>
        <button class="danger" type="button" onclick={deleteEntry} disabled={!selected || mainSelected || busy}>Delete</button>
      </div>
    </header>
    <DecodalSourceEditor
      value={source}
      readonly={!selectedPath || busy || !analysisReady}
      fixedSchemaWrapper={mainSelected}
      onChange={(value) => source = value}
      onComplete={(value, offset, explicit) => analysisReady && toolchain
        ? toolchain.complete(selectedPath, value, offset, explicit)
        : Promise.resolve(null)}
    />
    <p class="config-source-status" aria-live="polite">{status}</p>
    {#if conflict}
      <div class="config-source-conflict" role="alert">
        <button type="button" onclick={discardAndReload}>Discard local candidate and reload</button>
        <button type="button" onclick={reloadAndReapply}>Reload and reapply local candidate</button>
      </div>
    {/if}
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

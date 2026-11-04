<script lang="ts">
  // 内容digestとは別に、GETで観測した履歴headと移行条件を編集元として保持する。
  // 応答喪失時は依頼全体を固定し、照合で採用を確認するまで別IDへ切り替えない。
  // 診断は非abortかつ解析対象tree digest一致の場合だけ表示する。content条件と
  // 非同期requestの有効性は別に検査し、遅い解析を現在のsourceの診断として出さない。
  import { onMount } from "svelte";
  import DecodalSourceEditor from "#lib/workspace/settings/DecodalSourceEditor.svelte";
  import {
    commitConfigTree,
    ConfigSourceApiError,
    fetchConfigTree,
    previewConfigTree,
    reconcileConfigTree,
  } from "./api.ts";
  import type { ConfigurationEditingState } from "./api.ts";
  import type {
    ConfigurationAdoptionReceipt,
    ConfigurationAdoptionRequest,
  } from "#lib/generated/configuration-adoption.generated.ts";
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
  let loadState = $state<"loading" | "error" | "ready">("loading");
  let mounted = false;
  let busy = $state(false);
  let workingChanges = $state<ConfigTreeChange[]>([]);
  let baseDigest = $state("");
  let editingMetadata = $state.raw<ConfigurationEditingState | null>(null);
  let editingWorkspaceId = $state("");
  let pendingAdoption = $state.raw<{
    workspaceId: string;
    request: ConfigurationAdoptionRequest;
    draftSignature: string;
  } | null>(null);
  let renamePath = $state("");
  let baseSnapshot = $state.raw<WorkspaceConfigTreeResponse["snapshot"] | null>(null);
  let conflict = $state(false);
  let toolchain = $state.raw<ConfigSourceToolchain | null>(null);
  let analysisReady = $state(false);
  let analysisRequest: AbortController | null = null;

  const paths = $derived(
    treeState ? Object.keys(treeState.snapshot.entries).toSorted() : [],
  );
  const selected = $derived(
    treeState && selectedPath ? treeState.snapshot.entries[selectedPath] : undefined,
  );
  const mainSelected = $derived(selectedPath === MAIN_ENTRYPOINT);
  const dirty = $derived(workingChanges.length > 0 || (selected ? source !== selected.content : source.length > 0));

  onMount(() => {
    mounted = true;
    void reload();
    return () => {
      mounted = false;
      analysisRequest?.abort();
      toolchain?.close();
    };
  });

  $effect(() => {
    const analyzer = toolchain;
    const path = selectedPath;
    const value = source;
    const ready = analysisReady;
    const digest = treeState?.snapshot.digest;
    analysisRequest?.abort();
    const request = new AbortController();
    analysisRequest = request;
    diagnostics = [];
    if (!analyzer || !path || !ready) return;

    const timer = setTimeout(() => {
      void analyzer.analyze(path, value).then((result) => {
        if (!request.signal.aborted) diagnostics = result.filter((diagnostic) => diagnostic.tree_digest === digest);
      }).catch((error) => {
        if (!request.signal.aborted && !pendingAdoption && !busy) status = `Analyze failed: ${String(error)}`;
      });
    }, 250);
    return () => {
      clearTimeout(timer);
      request.abort();
    };
  });

  async function reload() {
    if (pendingAdoption) {
      status = "Adoption outcome unknown. Reconcile the retained request before reloading.";
      return;
    }
    loadState = "loading";
    status = "Loading source tree…";
    analysisReady = false;
    analysisRequest?.abort();
    const requestedWorkspace = workspaceId;
    try {
      const observed = await fetchConfigTree(requestedWorkspace);
      const remote = observed.state;
      if (!mounted || workspaceId !== requestedWorkspace) return;
      toolchain?.close();
      toolchain = null;
      toolchain = new ConfigSourceToolchain();
      await toolchain.setSnapshot(remote.snapshot, remote.contract.schema_bundle);
      if (!mounted || workspaceId !== requestedWorkspace) return;
      // Publish the new baseline only after both fetch and toolchain setup succeed.
      treeState = remote;
      editingMetadata = observed;
      editingWorkspaceId = requestedWorkspace;
      if (!selectedPath || !treeState.snapshot.entries[selectedPath]) {
        selectedPath = Object.keys(treeState.snapshot.entries).toSorted()[0] ?? "";
      }
      source = selectedPath ? treeState.snapshot.entries[selectedPath].content : "";
      baseSnapshot = $state.snapshot(treeState.snapshot);
      baseDigest = treeState.snapshot.digest;
      analysisReady = true;
      workingChanges = [];
      renamePath = selectedPath;
      diagnostics = [];
      conflict = false;
      status = `Tree digest ${treeState.snapshot.digest.slice(0, 20)}…`;
      loadState = "ready";
    } catch (error) {
      if (!mounted) return;
      toolchain?.close();
      toolchain = null;
      status = String(error);
      loadState = "error";
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
    conflict = error instanceof ConfigSourceApiError && error.status === 409;
    status = conflict ? `${message} Reload the authoritative tree before editing again.` : message;
  }

  function draftSignature(): string {
    const entries = new Map(Object.entries(treeState?.snapshot.entries ?? {}).map(
      ([path, entry]) => [path, [entry.content_type, entry.content]],
    ));
    if (selectedPath) entries.set(selectedPath, [selected?.content_type ?? "decodal", source]);
    return JSON.stringify([...entries].sort(([left], [right]) => left.localeCompare(right)));
  }

  async function applyReceipt(receipt: ConfigurationAdoptionReceipt) {
    const edited = pendingAdoption !== null && draftSignature() !== pendingAdoption.draftSignature;
    const local = edited && treeState ? $state.snapshot(treeState.snapshot) : null;
    const localSource = source;
    // receipt確定後のtoolchain障害は採用失敗ではない。headと確定状態を先に更新する。
    treeState = receipt.state;
    editingMetadata = {
      state: receipt.state,
      history_head: receipt.history_head,
      catalog_digest: editingMetadata!.catalog_digest,
      expected_legacy_export: null,
      source_stored: true,
    };
    baseSnapshot = $state.snapshot(receipt.state.snapshot);
    baseDigest = receipt.state.snapshot.digest;
    pendingAdoption = null;
    workingChanges = [];
    conflict = false;
    analysisReady = false;
    diagnostics = [];
    if (local) treeState = { ...receipt.state, snapshot: local };
    if (!edited && !treeState.snapshot.entries[selectedPath]) {
      selectedPath = Object.keys(treeState.snapshot.entries).toSorted()[0] ?? "";
    }
    source = edited ? localSource : treeState.snapshot.entries[selectedPath]?.content ?? "";
    try {
      if (local && toolchain) workingChanges = await toolchain.changesBetween(baseSnapshot, local);
      await toolchain?.setSnapshot(treeState.snapshot, treeState.contract.schema_bundle);
      analysisReady = true;
      status = edited
        ? "Adoption confirmed. Later local edits remain unsaved; Commit to adopt them separately."
        : `Committed formatted source tree · ${receipt.state.snapshot.digest.slice(0, 20)}…`;
    } catch (error) {
      status = `Adoption confirmed, but local editor setup failed: ${String(error)}`;
    }
  }

  async function reconcilePending(): Promise<"adopted" | "unresolved" | "unavailable"> {
    const pending = pendingAdoption;
    if (!pending) return "adopted";
    try {
      const result = await reconcileConfigTree(pending.workspaceId, pending.request);
      if (result.status === "adopted") {
        await applyReceipt(result.receipt);
        return "adopted";
      }
      status = result.status === "pending_transfer"
        ? "Adoption outcome unknown: transfer pending. Commit resumes the same request; reconcile before adopting edits."
        : "Adoption outcome unknown: not observed does not mean failed. Commit resumes the same request; reconcile before adopting edits.";
      return "unresolved";
    } catch (error) {
      status = `Adoption outcome unknown. Retained request must be reconciled: ${String(error)}`;
      return "unavailable";
    }
  }

  async function submitPending(wasUnknown: boolean) {
    const pending = pendingAdoption;
    if (!pending) return;
    let receipt: ConfigurationAdoptionReceipt;
    try {
      receipt = await commitConfigTree(pending.workspaceId, pending.request);
    } catch (error) {
      // 明確な初回拒否と、既に結果不明の依頼に対する拒否は区別する。
      if (!wasUnknown && error instanceof ConfigSourceApiError &&
        [400, 401, 403, 404, 409, 422].includes(error.status)) {
        pendingAdoption = null;
        recordCommitError(error);
      } else {
        status = `Adoption outcome unknown: ${String(error)}`;
        await reconcilePending();
      }
      return;
    }
    await applyReceipt(receipt);
  }

  async function reconcile() {
    if (busy) return;
    busy = true;
    try {
      await reconcilePending();
    } finally {
      busy = false;
    }
  }

  async function commit() {
    if (!treeState || !editingMetadata || busy || !analysisReady) return;
    busy = true;
    try {
      if (pendingAdoption) {
        const edited = draftSignature() !== pendingAdoption.draftSignature;
        const outcome = await reconcilePending();
        if (outcome !== "unresolved") return;
        if (edited) {
          status = "Adoption outcome unknown. Draft changed; reconcile the original request before adopting edits.";
          return;
        }
        // 明示的な次のCommitだけが、保存済みの同一依頼を再送できる。
        await submitPending(true);
        return;
      }
      if (editingWorkspaceId !== workspaceId) {
        status = "Workspace changed. Reload the authoritative source tree before adopting edits.";
        return;
      }
      const adoptionWorkspaceId = editingWorkspaceId;
      await formatWorkingSources();
      if (workingChanges.length === 0 && editingMetadata.source_stored) {
        status = "No working changes to commit.";
        return;
      }
      const request: ConfigurationAdoptionRequest = {
        operation_id: crypto.randomUUID(),
        expected_head: editingMetadata.history_head,
        expected_catalog_digest: editingMetadata.catalog_digest,
        expected_legacy_export: editingMetadata.expected_legacy_export,
        source: {
          base_digest: baseDigest,
          changes: $state.snapshot(workingChanges),
          entrypoints: entrypoints(),
        },
      };
      const signature = draftSignature();
      await previewConfigTree(adoptionWorkspaceId, request);
      pendingAdoption = { workspaceId: adoptionWorkspaceId, request, draftSignature: signature };
      await submitPending(false);
    } catch (error) {
      if (pendingAdoption) status = `Adoption outcome unknown: ${String(error)}`;
      else recordCommitError(error);
    } finally {
      busy = false;
    }
  }

  async function discardAndReload() {
    if (busy) return;
    await reload();
  }

  async function reloadAndReapply() {
    if (!toolchain || !treeState || busy || pendingAdoption) return;
    busy = true;
    const requestedWorkspace = workspaceId;
    try {
      await stageCurrent();
      const localChanges = $state.snapshot(workingChanges);
      const observed = await fetchConfigTree(requestedWorkspace);
      const remote = observed.state;
      await toolchain.setSnapshot(remote.snapshot, remote.contract.schema_bundle);
      const candidate = await toolchain.applyChanges(localChanges);
      baseSnapshot = structuredClone(remote.snapshot);
      baseDigest = remote.snapshot.digest;
      editingMetadata = observed;
      editingWorkspaceId = requestedWorkspace;
      workingChanges = localChanges;
      treeState = { ...remote, snapshot: candidate };
      selectedPath = candidate.entries[selectedPath] ? selectedPath : Object.keys(candidate.entries).toSorted()[0] ?? "";
      source = selectedPath ? candidate.entries[selectedPath].content : "";
      conflict = false;
      status = "Local changes reapplied to the latest source tree. Commit to persist them.";
    } catch (error) {
      conflict = true;
      try {
        await toolchain.setSnapshot(treeState.snapshot, treeState.contract.schema_bundle);
      } catch {
        analysisReady = false;
      }
      status = `Local changes conflict with the latest source tree: ${String(error)}. Discard local changes or resolve against a fresh reload.`;
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

<section class="config-source-shell" aria-label="Workspace configuration source tree" aria-busy={loadState === "loading"}>
  {#if loadState === "loading"}
    <div class="config-source-load" role="status">Loading source tree…</div>
  {:else if loadState === "error"}
    <div class="config-source-load">
      <div role="alert">
        <strong>Unable to load workspace configuration</strong>
        <p>{status}</p>
      </div>
      <button type="button" onclick={reload}>Reload source tree</button>
    </div>
  {:else}
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
          disabled={busy}
          onclick={() => select(path)}
        >
          <span>{path}</span>
          {#if path === MAIN_ENTRYPOINT}<small>entrypoint</small>{/if}
        </button>
      {/each}
    </nav>
    <form class="config-source-create" onsubmit={(event) => { event.preventDefault(); createEntry(); }}>
      <label for="new-config-path">New path</label>
      <input id="new-config-path" bind:value={newPath} placeholder="module.dcdl" disabled={busy} />
      <button type="submit" disabled={busy}>Create local source</button>
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
        <button class="primary" type="button" onclick={commit} disabled={(!dirty && !pendingAdoption && editingMetadata?.source_stored !== false) || busy || !analysisReady}>Commit</button>
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
    {#if pendingAdoption}
      <div class="config-source-conflict" role="status">
        <span>Adoption outcome unknown. Commit resumes the unchanged request; edited drafts require reconciliation first.</span>
        <button type="button" onclick={reconcile} disabled={busy}>Reconcile adoption</button>
      </div>
    {/if}
    {#if conflict && !pendingAdoption}
      <div class="config-source-conflict" role="alert">
        <button type="button" onclick={discardAndReload} disabled={busy}>Discard local candidate and reload</button>
        <button type="button" onclick={reloadAndReapply} disabled={busy}>Reload and reapply local candidate</button>
      </div>
    {/if}
    {#if diagnostics.length > 0}
      <ol class="config-source-diagnostics">
        {#each diagnostics as diagnostic}
          <li>
            <strong>{diagnostic.kind}</strong>
            <span>{diagnostic.message}</span>
            <small>{diagnostic.path}{diagnostic.path.startsWith("$builtin/") ? " (read-only builtin source)" : ""} · bytes {diagnostic.span.start_byte}–{diagnostic.span.end_byte}</small>
          </li>
        {/each}
      </ol>
    {/if}
  </div>
  {/if}
</section>

<style>
  .config-source-load {
    grid-column: 1 / -1;
    padding: 1rem;
  }
</style>

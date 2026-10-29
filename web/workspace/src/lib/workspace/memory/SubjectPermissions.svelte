<script lang="ts">
  import { onMount } from 'svelte';
  import { attemptKey, fieldLabel, makeResource, PermissionHttpError, permissionKey, policyDiff, readAttempt, readPermissions, resourceFields, resourceLabel, writePermissions } from './permissions.ts';
  import type { Interruption, Lane, PermissionResource, PermissionView, PolicyUpdate, ScopedPermission } from './permissions.ts';
  let { workspaceId, subjectId }: { workspaceId: string; subjectId: string } = $props();
  let view = $state<PermissionView | null>(null);
  let execution = $state<ScopedPermission[]>([]);
  let delegation = $state<ScopedPermission[]>([]);
  let pending = $state<PolicyUpdate | null>(null);
  let storageBlocked = $state(false);
  let busy = $state(false);
  let error = $state('');
  let notice = $state('');
  let confirming = $state(false);
  let interruptions = $state<Interruption[]>([]);
  let lane = $state<Lane>('execution');
  let operationName = $state('');
  let resourceKind = $state<PermissionResource['kind']>('workspace');
  let inputs = $state<Record<string, string>>({});
  let recursive = $state(false);
  const selected = $derived(view?.operations.find((o) => o.operation === operationName));
  const changes = $derived({ execution: policyDiff(view?.permissions?.execution ?? [], execution), delegation: policyDiff(view?.permissions?.delegation ?? [], delegation) });
  const dirty = $derived(Object.values(changes).some((d) => d.added.length || d.removed.length));
  const editable = $derived(view?.can_manage && !busy && !pending && !storageBlocked);
  const label = (p: ScopedPermission) => view?.operations.find((o) => o.operation === p.operation)?.label ?? 'Preserved unsupported permission';
  const supported = (p: ScopedPermission) => view?.operations.some((o) => o.operation === p.operation && o.available && o.resource_kinds.includes(p.resource.kind));
  function accept(v: PermissionView) {
    view = v;
    execution = structuredClone(v.permissions?.execution ?? []);
    delegation = structuredClone(v.permissions?.delegation ?? []);
    confirming = false;
  }
  function describe(error: unknown) {
    if (error instanceof PermissionHttpError) {
      if (error.status === 403 || error.status === 401) return 'Read-only / permission unavailable. This account cannot manage this Subject.';
      if (error.status === 404) return 'Target unavailable. This Subject may have been deleted; an old screen cannot restore it.';
      if (error.status === 409) return 'Conflict: another save changed the policy. Your edits were not applied. Reload current permissions and review again.';
      if (error.status === 400) return 'The operation or resource was rejected. Check the selected target; no new permissions were saved.';
    }
    return 'The request could not be confirmed. Do not assume success or create another save.';
  }
  async function reload() {
    if (busy) return;
    busy = true; error = ''; notice = '';
    try { accept(await readPermissions(fetch, workspaceId, subjectId)); }
    catch (e) { error = describe(e); }
    finally { busy = false; }
  }
  onMount(() => {
    try { pending = readAttempt(sessionStorage, workspaceId, subjectId); }
    catch { storageBlocked = true; error = 'The saved request cannot be read. Saving is blocked; do not start another request.'; }
    if (!storageBlocked) void reload();
  });
  function chooseOperation() {
    resourceKind = selected?.resource_kinds[0] ?? 'workspace'; inputs = {}; recursive = false; confirming = false;
  }
  function add() {
    if (!editable || !selected?.available || !selected.resource_kinds.includes(resourceKind)) return;
    try {
      const p = { operation: selected.operation, resource: makeResource(resourceKind, workspaceId, subjectId, inputs, recursive) };
      const current = lane === 'execution' ? execution : delegation;
      if (!current.some((x) => permissionKey(x) === permissionKey(p))) {
        if (lane === 'execution') execution = [...execution, p]; else delegation = [...delegation, p];
      }
      confirming = false; error = ''; notice = '';
    } catch (e) { error = e instanceof Error ? e.message : 'Invalid resource'; }
  }
  function remove(which: Lane, index: number) {
    if (!editable) return;
    if (which === 'execution') execution = execution.filter((_, i) => i !== index);
    else delegation = delegation.filter((_, i) => i !== index);
    confirming = false; notice = '';
  }
  async function save(request?: PolicyUpdate) {
    if (busy || storageBlocked || !view?.can_manage) return;
    const attempt = request ?? { operation_id: crypto.randomUUID(), expected_change_id: view.permissions?.change_id ?? null, execution: $state.snapshot(execution), delegation: $state.snapshot(delegation) };
    busy = true; error = ''; notice = '';
    try {
      // 永続入力を保持できなければPOSTしない。再接続で新しいoperationを作らない。
      sessionStorage.setItem(attemptKey(workspaceId, subjectId), JSON.stringify(attempt));
      pending = attempt;
    } catch { error = 'Cannot retain the save request in this tab. Nothing was sent.'; busy = false; return; }
    let receiptAccepted = false;
    try {
      const receipt = await writePermissions(fetch, workspaceId, subjectId, attempt);
      receiptAccepted = true;
      const current = await readPermissions(fetch, workspaceId, subjectId);
      accept(current); interruptions = receipt.interruptions;
      sessionStorage.removeItem(attemptKey(workspaceId, subjectId)); pending = null;
      notice = current.permissions?.change_id === receipt.permissions.change_id
        ? 'Saved and re-read from the current policy.'
        : 'The original save was confirmed, but a later change is now current. The current policy is shown; the receipt did not restore older permissions.';
    } catch (e) {
      const definitive = !receiptAccepted && e instanceof PermissionHttpError && [400, 401, 403, 404, 409].includes(e.status);
      if (definitive) {
        sessionStorage.removeItem(attemptKey(workspaceId, subjectId)); pending = null;
        // 競合後の新しい観測を元の下書きのCASへ黙って差し替えない。
        view = view ? { ...view, can_manage: false } : null;
      }
      error = receiptAccepted ? 'The save receipt was accepted, but current policy could not be re-read. Reconcile the same request; do not treat its receipt as current authority.' : describe(e);
    } finally { busy = false; }
  }
</script>

<section class="permissions" aria-labelledby="permissions-heading" aria-busy={busy} data-permission-state={pending ? 'unknown' : error ? 'error' : dirty ? 'unsaved' : notice ? 'saved' : view ? 'ready' : 'loading'}>
  <header><h2 id="permissions-heading">Operation permissions</h2><span class="state">{pending ? 'Outcome unknown' : dirty ? 'Unsaved changes' : notice ? 'Saved / current policy re-read' : view?.permissions ? 'Saved policy' : view ? 'Not configured · no permissions' : 'Loading…'}</span></header>
  <p>Direct operations and the background Worker delegation ceiling are independent. Profile and Feature selection never grant permissions. Workdir/provider grants, target existence and assignment conditions still apply.</p>
  {#if error}<p class="attention" role="alert">{error}</p>{/if}
  {#if notice}<p role="status">{notice}</p>{/if}
  {#if pending}
    <div class="attention"><strong>Save outcome not yet confirmed.</strong><p>This tab retained the exact request. Reconcile it using the same operation and then re-read the current policy. Do not start another save.</p><button disabled={busy || !view?.can_manage} onclick={() => save(pending!)}>Reconcile the same save</button></div>
  {/if}
  {#if !view}
    <button disabled={busy || storageBlocked} onclick={reload}>Read current permissions</button>
  {:else}
    {#if !view.can_manage}<p role="status">Read-only. The Backend must authorize permission management; hiding controls is not authorization.</p>{/if}
    <div class="lanes">
      {#each ['execution', 'delegation'] as which}
        {@const permissions = which === 'execution' ? execution : delegation}
        <section class="lane" aria-label={which === 'execution' ? 'Direct operations' : 'Background Worker delegation ceiling'}>
          <h3>{which === 'execution' ? 'Direct operations' : 'Background Worker delegation ceiling'}</h3>
          <p>{which === 'execution' ? 'Operations the Subject body may use, if its tools and current connections support them.' : 'Maximum operations and resources that may be explicitly passed to background Workers. This does not give the body work tools or start a Worker.'}</p>
          {#if permissions.length === 0}<p class="empty">No permissions in this scope.</p>{/if}
          <ul>{#each permissions as p, index}
            <li><div><strong>{label(p)}</strong><span class="resource">{resourceLabel(p.resource)}</span>
              {#if !supported(p)}<span class="legacy">Preserved, not editable here. No conversion or expansion is implied.</span><details><summary>Saved operation identifier</summary><code>{p.operation}</code></details>{/if}
            </div>{#if editable}<button aria-label={`Remove ${label(p)} from ${which === 'execution' ? 'direct operations' : 'delegation ceiling'}`} onclick={() => remove(which as Lane, index)}>Remove</button>{/if}</li>
          {/each}</ul>
        </section>
      {/each}
    </div>
    {#if view.can_manage}
      <form onsubmit={(event) => { event.preventDefault(); add(); }} class="permission-form">
        <h3>Add an explicit operation</h3>
        <fieldset disabled={!editable}>
          <label>Permission scope<select value={lane} onchange={(event) => { lane = event.currentTarget.value as Lane; }}><option value="execution">Direct operations</option><option value="delegation">Background Worker delegation ceiling</option></select></label>
          <label>Operation<select value={operationName} onchange={(event) => { operationName = event.currentTarget.value; chooseOperation(); }}><option value="">Choose an operation</option>{#each view.operations as o}<option value={o.operation}>{o.label}{o.available ? '' : ' — unavailable'}</option>{/each}</select></label>
          {#if selected}
            <p>{selected.description}</p>
            {#if !selected.available}<p class="attention">Target unavailable: {selected.unavailable_reason} No permission will be added.</p>
            {:else}
              <label>Resource type<select value={resourceKind} onchange={(event) => { resourceKind = event.currentTarget.value as PermissionResource['kind']; inputs = {}; recursive = false; }}>{#each selected.resource_kinds as kind}<option value={kind}>{kind.replaceAll('_', ' ')}</option>{/each}</select></label>
              {#if resourceKind === 'workspace'}<p>Collection operations in this Workspace only; this does not cover individual resources.</p>{/if}
              {#if resourceKind === 'feature'}<p>MCP is a temporary coarse activation exception. This is not per-server or per-tool authorization and does not grant ordinary Feature operations.</p>{/if}
              {#each resourceFields(resourceKind) as field}<label>{fieldLabel[field]}<input bind:value={inputs[field]} placeholder={field === 'subject_id' ? subjectId : field === 'path' ? '.' : ''} required={field !== 'subject_id' && field !== 'path'} /></label>{/each}
              {#if resourceKind === 'workdir' || resourceKind === 'workspace_config'}<label class="check"><input type="checkbox" bind:checked={recursive} />Include descendant paths</label>{/if}
              <button type="submit">Add to unsaved changes</button>
            {/if}
          {/if}
        </fieldset>
      </form>
    {/if}
    <details class="help"><summary>Examples and enforcement boundaries</summary><p>For read-only work, add a read operation and its exact resource to the delegation ceiling. For limited editing, separately add the write operation for only the intended Workdir and relative path. Nothing else is implied; no wildcard or fixed all-powerful role is added.</p><p>Worker operations require both permission to use the operation and a directional Backend control grant for the actual target. Idle/CAS and lifecycle rules remain separate. This editor never asks for per-Worker notification/stop permissions or copies creation grants into the Subject policy. The T-803 connection is not yet available. Subject Ticket assignment migration (T-778) is not enabled by this screen.</p><p>Saving fences later operations using the current policy. Running work may receive cancellation requests; acceptance is not a confirmed stop. Already completed effects are not undone. Existing child grants are invalidated rather than silently refreshed.</p></details>
    {#if dirty && !pending}
      <section class="diff" aria-label="Unsaved permission differences"><h3>Review before saving</h3>
        {#each ['execution', 'delegation'] as which}<h4>{which === 'execution' ? 'Direct operations' : 'Delegation ceiling'}</h4>
          <ul>{#each changes[which as Lane].added as p}<li>+ Add: {label(p)} — {resourceLabel(p.resource)}</li>{/each}{#each changes[which as Lane].removed as p}<li>− Remove: {label(p)} — {resourceLabel(p.resource)}</li>{/each}</ul>
        {/each}
        {#if editable}<label class="check"><input type="checkbox" bind:checked={confirming} />I reviewed the operation, resource and delegation differences.</label><button class="save" disabled={!confirming || busy} onclick={() => save()}>Save permissions</button>{/if}
      </section>
    {/if}
    <button class="reload" disabled={busy || !!pending || storageBlocked} onclick={reload}>{dirty ? 'Discard unsaved changes and reload current policy' : 'Reload current policy'}</button>
    {#if interruptions.length}<section aria-label="Cancellation evidence"><h3>Cancellation requests, not stop confirmation</h3><ul>{#each interruptions as i}<li>Worker {i.worker_id}: cancellation {i.cancellation_accepted ? 'accepted' : 'not confirmed'}; Backend command cleanup {i.backend_command_sessions_closed ? 'confirmed' : 'not confirmed'}. Runtime stop remains unconfirmed.</li>{/each}</ul></section>{/if}
  {/if}
</section>

<style>
  .permissions { max-width: 72rem; min-width: 0; }
  header { display: flex; flex-wrap: wrap; align-items: baseline; gap: .75rem; }
  h2 { margin: 0; font-size: var(--font-size-body); } h3 { font-size: var(--font-size-body); margin: 0 0 .5rem; } h4 { margin: 1rem 0 .4rem; }
  p { line-height: 1.55; color: var(--text-muted); } .state { font-size: var(--font-size-compact); }
  .lanes { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 1.5rem; margin: 1.5rem 0; }
  .lane { min-width: 0; border-top: 1px solid var(--line); padding-top: 1rem; }
  ul { list-style: none; padding: 0; margin: .75rem 0; } li { display: flex; align-items: start; gap: .75rem; justify-content: space-between; padding: .75rem 0; border-bottom: 1px solid var(--line); overflow-wrap: anywhere; }
  li > div { min-width: 0; } li button { flex-shrink: 0; white-space: nowrap; } .resource, .legacy { display: block; margin-top: .35rem; font-size: var(--font-size-body); } .legacy { color: var(--text-muted); }
  .permission-form, .diff { border-top: 1px solid var(--line); margin-top: 1.5rem; padding-top: 1rem; }
  fieldset { border: 0; padding: 0; min-width: 0; display: grid; gap: .75rem; max-width: 38rem; }
  label { display: grid; gap: .35rem; font-size: var(--font-size-body); } input, select { width: 100%; min-width: 0; padding: .55rem; border: 1px solid var(--line); border-radius: 3px; background: var(--bg); color: var(--text); font: inherit; }
  button { background: transparent; color: var(--text); border: 1px solid var(--line); padding: .45rem .7rem; border-radius: 3px; cursor: pointer; font: inherit; font-size: var(--font-size-body); } button:disabled { opacity: .5; cursor: default; } button.save { background: var(--text); color: var(--bg); margin-top: .75rem; }
  .check { display: flex; align-items: start; gap: .5rem; } .check input { width: auto; margin-top: .2rem; } .reload { margin: 1rem 0; }
  .attention { border-left: 3px solid var(--text-muted); padding: .7rem 1rem; overflow-wrap: anywhere; } .help { margin: 1.5rem 0; } summary { cursor: pointer; } code { overflow-wrap: anywhere; }
  @media (max-width: 900px) { .lanes { grid-template-columns: 1fr; gap: 1rem; } }
</style>

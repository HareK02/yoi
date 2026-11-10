<script lang="ts">
  import { onMount } from 'svelte';
  import BevelLine from '#lib/workspace/ui/BevelLine.svelte';
  import { PermissionHttpError, readPermissions, referenceLabel } from './permissions.ts';
  import type { Lane, PermissionView } from './permissions.ts';
  let { workspaceId, subjectId }: { workspaceId: string; subjectId: string } = $props();
  let view = $state<PermissionView | null>(null);
  let busy = $state(false);
  let error = $state('');
  const lanes: { key: Lane; label: string }[] = [
    { key: 'execution', label: 'Direct operations' },
    { key: 'delegation', label: 'Delegation ceiling' },
  ];
  async function reload() {
    if (busy) return;
    busy = true; error = '';
    // Old observations are not current authorization after a failed refresh.
    view = null;
    try { view = await readPermissions(fetch, workspaceId, subjectId); }
    catch (e) {
      error = e instanceof PermissionHttpError && e.status === 401
        ? 'Sign in to read the current Subject policy.'
        : e instanceof PermissionHttpError && e.status === 403
        ? 'You do not have permission to read the current Subject policy.'
        : e instanceof PermissionHttpError && e.status === 404
        ? 'Current Subject policy unavailable. The Subject may have been deleted.'
        : 'Current Subject policy could not be read. No effective permission is inferred. Reload to try again.';
    } finally { busy = false; }
  }
  onMount(() => { void reload(); });
</script>

<section class="permissions" aria-labelledby="permissions-heading" aria-busy={busy} data-permission-state={busy ? 'loading' : error ? 'error' : 'ready'}>
  <header>
    <h2 id="permissions-heading">Current Subject policy</h2>
    <button type="button" class="workspace-secondary-button" disabled={busy} onclick={reload}>Reload permissions</button>
  </header>
  {#if error}<p role="alert">{error}</p>{/if}
  {#if busy}<p role="status">Reading current Subject policy…</p>{/if}
  {#if view}
    {#if view.readOnly}<p class="muted">Read-only policy access.</p>{/if}
    {#if !view.policy}<p>Subject policy is unset. No default assignment or effective permission is inferred.</p>{/if}
    <p class="context">Resolved operation keys alone do not authorize an operation. Target, assignment, domain and provider restrictions still apply. Delegation is a ceiling, not a grant to a child or Job. Policy changes apply at the next current-policy check, not as proof that running work stopped.</p>
    <div class="lanes">
      {#each lanes as lane}
        {@const assignment = view.policy?.[lane.key]}
        {@const resolution = view[lane.key === 'execution' ? 'execution_resolution' : 'delegation_resolution']}
        <section class="lane" aria-label={lane.label}>
          <BevelLine />
          <h3>{lane.label}</h3>
          <p class="assignment-state">{!assignment ? 'Unset assignment' : assignment.revoked ? 'Revoked assignment' : assignment.grants.length ? 'Current assignment' : 'Explicitly empty assignment'}</p>
          {#if assignment}
            <h4>{assignment.revoked ? 'Stored grant references · revoked' : 'Grant references'}</h4>
            {#if assignment.grants.length}<ul>{#each assignment.grants as ref}<li><code>{referenceLabel(ref)}</code></li>{/each}</ul>{:else}<p class="muted">No grants.</p>{/if}
            {#if assignment.exclusions.length}<h4>Exclusions</h4><ul>{#each assignment.exclusions as ref}<li><code>{referenceLabel(ref)}</code></li>{/each}</ul>{/if}
          {/if}
          <h4>Resolved operation keys</h4>
          {#if Object.keys(resolution.allowed).length === 0}<p class="muted">No allowed operation keys.</p>{/if}
          <ul class="resolved">{#each Object.entries(resolution.allowed) as [key, paths]}<li>
            <code>{key}</code>
            <details><summary>Permission origin</summary><ul>{#each paths as path}<li>{#each path as step, index}{#if index}{' → '}{/if}<span>{step.kind} · <code>{step.value}</code></span>{/each}</li>{/each}</ul></details>
          </li>{/each}</ul>
          {#if resolution.diagnostics.length}<h4>Resolution diagnostics</h4><ul>{#each resolution.diagnostics as diagnostic}<li>{diagnostic.kind}{#if 'value' in diagnostic} · <code>{diagnostic.value}</code>{/if}</li>{/each}</ul>{/if}
        </section>
      {/each}
    </div>
    {#if view.policy?.legacy_constraints}
      <details class="legacy"><summary>Retained domain restrictions</summary>
        <p>These restrictions remain additional limits on operations. Individual targets and provider limits are not editable data on permission keys.</p>
        <pre>{JSON.stringify(view.policy.legacy_constraints, null, 2)}</pre>
      </details>
    {/if}
    <details class="catalog"><summary>Feature declaration catalog</summary>
      <p>Only explicit children are included. A shared dot prefix does not imply permission.</p>
      {#if !view.catalog.namespaces.length}<p>No Feature declarations.</p>{/if}
      {#each view.catalog.namespaces as namespace}
        <section class="namespace">
          <h3>Namespace · <code>{namespace.namespace}</code></h3>
          <p>Feature · <code>{namespace.feature}</code> · {namespace.enabled ? 'Enabled' : 'Disabled'}</p>
          <ul>{#each namespace.declaration.permissions as permission}<li><code>{permission.key}</code> — {permission.description}{#if permission.children.length}<div>Explicit children: <code>{permission.children.join(', ')}</code></div>{/if}</li>{/each}</ul>
          {#if namespace.declaration.sets.length}<h4>Set contributions</h4><ul>{#each namespace.declaration.sets as set}<li>Set · <code>{set.set}</code><ul>{#each set.children as ref}<li><code>{referenceLabel(ref)}</code></li>{/each}</ul></li>{/each}</ul>{/if}
        </section>
      {/each}
    </details>
    <details class="policy-change"><summary>Policy change</summary>
      <dl><dt>{view.policy ? 'Current policy change' : view.change_id !== null ? 'Removal change' : 'Policy change'}</dt><dd><code>{view.change_id ?? 'No policy change recorded'}</code></dd></dl>
      {#if !view.policy && view.change_id !== null}<p>This change identifies a removal tombstone, not a current assignment or grant.</p>{/if}
    </details>
  {/if}
</section>
<style>
  .permissions { min-width: 0; }
  header { display: flex; flex-wrap: wrap; justify-content: space-between; align-items: center; gap: 0.75rem; margin-bottom: 1rem; }
  h2 { font-size: var(--font-size-body); line-height: var(--line-height-body); margin: 0; }
  h3 { font-size: var(--font-size-body); margin: 0 0 0.5rem; }
  h4 { font-size: var(--font-size-body); margin: 1rem 0 0.5rem; font-weight: 500; }
  p { margin: 0.75rem 0; }
  .context, .muted { color: var(--text-muted); }
  [role='alert'] { color: var(--warning); }
  .lanes { display: grid; grid-template-columns: minmax(0, 1fr) minmax(0, 1fr); gap: 1.5rem; margin-top: 1.5rem; }
  .lane { min-width: 0; }
  .lane h3 { margin-top: 1rem; }
  ul { padding-left: 1.25rem; }
  li { margin: 0.5rem 0; }
  code, li, dd, summary { overflow-wrap: anywhere; }
  details { margin-top: 1rem; }
  .resolved details { margin: 0.35rem 0; color: var(--text-muted); }
  summary { cursor: pointer; }
  .legacy, .catalog, .policy-change { margin-top: 1.5rem; }
  button { font-size: var(--font-size-body); line-height: var(--line-height-body); border-radius: var(--radius-soft); }
  button:hover:not(:disabled) { background: var(--interactive-hover); }
  button:active:not(:disabled) { background: var(--interactive-selected); }
  button:focus-visible, summary:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  .namespace { padding: 1rem 0; }
  pre { white-space: pre-wrap; overflow-wrap: anywhere; font-size: var(--font-size-compact); }
  dd { margin: 0.25rem 0 0.75rem; }
  @media (max-width: 900px) { .lanes { grid-template-columns: minmax(0, 1fr); } }
</style>

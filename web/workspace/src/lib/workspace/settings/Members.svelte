<script lang="ts">
  import { onMount, tick, untrack } from 'svelte';
  import { addMember, loadMembers, removeMember, type WorkspaceMember, type WorkspaceMemberListResponse } from '#lib/workspace/api/members.ts';
  import { SESSION_CHANGED_EVENT } from '#lib/workspace/auth/hosting.ts';
  import { workspaceRoute } from '#lib/workspace/api/http.ts';
  let { workspaceId }: { workspaceId: string } = $props();
  let snapshot = $state<WorkspaceMemberListResponse | null>(null);
  let loadState = $state<'loading' | 'ready' | 'denied' | 'error'>('loading');
  let error = $state('');
  let status = $state('');
  let showAdd = $state(false);
  let handle = $state('');
  let busy = $state(false);
  let blocked = $state(false);
  let input = $state<HTMLInputElement>();
  let addButton = $state<HTMLButtonElement>();
  let reloadButton = $state<HTMLButtonElement>();
  let generation = 0;
  let controller: AbortController | undefined;

  async function load(append = false) {
    if (append && (!snapshot?.next_after_account_id || busy)) return;
    const id = workspaceId;
    const after = append ? snapshot!.next_after_account_id : null;
    const prior = append ? snapshot : null;
    const current = ++generation;
    controller?.abort(); controller = new AbortController();
    if (append) busy = true;
    else { snapshot = null; loadState = 'loading'; showAdd = false; handle = ''; busy = false; }
    error = ''; blocked = false;
    const result = await loadMembers(fetch, id, after, controller.signal);
    if (current !== generation) return;
    busy = false;
    if (!result.data) {
      if (result.status === 401 || result.status === 403 || result.status === 404) {
        snapshot = null; showAdd = false; loadState = 'denied';
      } else { loadState = prior ? 'ready' : 'error'; blocked = true; error = 'Members could not be loaded. Reload before making changes.'; }
      return;
    }
    if (prior && prior.membership_change_key !== result.data.membership_change_key) {
      blocked = true; error = 'Membership changed while loading. Reload members before making changes.'; return;
    }
    snapshot = prior ? { ...result.data, members: [...prior.members, ...result.data.members] } : result.data;
    loadState = 'ready';
  }
  $effect(() => {
    workspaceId;
    untrack(() => { status = ''; void load(); });
    return () => { ++generation; controller?.abort(); };
  });
  onMount(() => {
    const refresh = () => { status = ''; void load(); };
    window.addEventListener(SESSION_CHANGED_EVENT, refresh);
    return () => window.removeEventListener(SESSION_CHANGED_EVENT, refresh);
  });
  async function openAdd() {
    showAdd = true; status = ''; error = '';
    await tick(); input?.focus();
  }
  async function cancel() { showAdd = false; handle = ''; await tick(); addButton?.focus(); }
  async function mutate(member?: WorkspaceMember) {
    if (busy || blocked || loadState !== 'ready' || !snapshot) return;
    if (member?.is_owner || (member && !member.membership_key) || (!member && !handle.trim())) return;
    const current = generation;
    const id = workspaceId;
    const operation_key = crypto.randomUUID();
    busy = true; error = ''; status = '';
    const result = member
      ? await removeMember(fetch, id, member.account_id, { operation_key, expected_membership_key: member.membership_key! }, controller?.signal)
      : await addMember(fetch, id, { operation_key, account_handle: handle.trim(), expected_membership_change_key: snapshot.membership_change_key }, controller?.signal);
    if (current !== generation) return;
    busy = false;
    if (!result.data) {
      if (result.status === 401 || result.status === 403) {
        snapshot = null; showAdd = false; loadState = 'denied'; return;
      }
      blocked = true;
      error = result.status === 409 ? 'Membership changed. Reload members and review the current list before trying again.'
        : result.status === 404 ? 'No active account with that ID was found. Reload members and check the Account ID.'
        : 'The change could not be confirmed. Reload members to check the result before making another change.';
      return;
    }
    status = 'Change recorded. Current membership is shown below.';
    await load();
    await tick();
    if (workspaceId === id) (member ? reloadButton : addButton)?.focus();
  }
</script>

<section class="members" aria-labelledby="members-heading">
  <header>
    <h1 id="members-heading">Members</h1>
    {#if loadState === 'ready'}
      <div class="actions">
        <button bind:this={reloadButton} type="button" disabled={busy} onclick={() => load()}>Reload members</button>
        {#if !showAdd}<button bind:this={addButton} type="button" disabled={busy || blocked} onclick={openAdd}>Add member</button>{/if}
      </div>
    {/if}
  </header>
  {#if loadState === 'loading'}
    <p role="status">Loading members…</p>
  {:else if loadState === 'denied'}
    <p role="alert">Only the Workspace owner can manage members. Your account may no longer have access.</p>
    <a href="/workspaces">Choose a Workspace</a>
  {:else if loadState === 'error'}
    <p role="alert" class="error">{error}</p>
    <button type="button" onclick={() => load()}>Reload members</button>
  {:else if snapshot}
    {#if showAdd}
      <form onsubmit={(event) => { event.preventDefault(); void mutate(); }}>
        <label for="member-handle">Account ID (handle)</label>
        <input bind:this={input} id="member-handle" bind:value={handle} required maxlength="128" autocomplete="off" autocapitalize="off" spellcheck="false" disabled={busy || blocked} aria-describedby="member-help" />
        <p id="member-help" class="muted">Use an existing account’s ID, not its display name. Adding grants immediate Workspace access; no invitation acceptance is required.</p>
        <div class="actions">
          <button type="submit" class="primary" disabled={busy || blocked || !handle.trim()}>{busy ? 'Saving…' : 'Add member'}</button>
          <button type="button" disabled={busy} onclick={cancel}>Cancel</button>
        </div>
      </form>
    {/if}
    {#if error}<p role="alert" class="error">{error}</p>{/if}
    {#if status}<p role="status">{status}</p>{/if}
    <p class="muted">Members can view and use this Workspace. Only the owner can change settings and membership.</p>
    <ul aria-label="Workspace members">
      {#each snapshot.members as member (member.account_id)}
        <li>
          <div class="identity"><strong>{member.display_name}</strong><code>@{member.handle}</code></div>
          <span>{member.is_owner ? 'Owner' : 'Member'}</span>
          {#if !member.is_owner}
            <button type="button" disabled={busy || blocked} aria-label={`Remove @${member.handle}`} onclick={() => mutate(member)}>Remove</button>
          {/if}
        </li>
      {/each}
    </ul>
    {#if snapshot.next_after_account_id}
      <button type="button" disabled={busy || blocked} onclick={() => load(true)}>{busy ? 'Loading…' : 'Load more members'}</button>
    {/if}
    <p class="muted">Removing a member revokes their Workspace access, including active subscriptions. Their account is not deleted.</p>
    <a href={workspaceRoute(workspaceId)}>Back to Workspace</a>
  {/if}
</section>

<style>
  .members { display: grid; align-content: start; gap: var(--space-4); max-width: 56rem; min-width: 0; }
  header, .actions { display: flex; flex-wrap: wrap; align-items: center; gap: var(--space-2); }
  header { justify-content: space-between; gap: var(--space-4); }
  h1, p, ul { margin: 0; }
  h1 { font-size: var(--font-size-title); line-height: var(--line-height-title); }
  form { display: grid; gap: var(--space-2); max-width: 36rem; }
  .muted, code { color: var(--text-muted); }
  .error { color: var(--danger); }
  a { color: var(--text-strong); }
  ul { padding: 0; list-style: none; }
  li { display: grid; grid-template-columns: minmax(0, 1fr) auto 6rem; gap: var(--space-3); align-items: center; border-bottom: 1px solid var(--line); padding-block: var(--space-3); }
  .identity { display: grid; min-width: 0; gap: var(--space-1); overflow-wrap: anywhere; }
  input { box-sizing: border-box; width: 100%; min-width: 0; padding: var(--space-2); background: var(--bg); color: var(--text-strong); border: 1px solid var(--line-strong); border-radius: var(--radius-soft); font: inherit; }
  button { justify-self: start; min-height: 2.25rem; padding: var(--space-2) var(--space-3); border: 1px solid var(--line-strong); border-radius: var(--radius-soft); background: transparent; color: var(--text-strong); font: inherit; cursor: pointer; }
  button.primary { background: var(--text-strong); color: var(--bg); }
  button:hover:not(:disabled), button:active:not(:disabled) { background: var(--interactive-hover); color: var(--text-strong); }
  button:disabled { opacity: .55; cursor: not-allowed; }
  button:focus-visible, input:focus-visible, a:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  @media (max-width: 480px) { li { grid-template-columns: minmax(0, 1fr) auto; } li button { grid-column: 2; } }
</style>

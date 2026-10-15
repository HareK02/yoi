<script lang="ts">
  import { onMount, tick } from 'svelte';
  import type { HostingSettings } from '#lib/generated/auth-api.ts';
  import { loadWhoami } from '#lib/workspace/auth/api.ts';
  import {
    HostingRequestError,
    loadHostingAccess,
    loadHostingSettings,
    saveHostingSettings,
    SESSION_CHANGED_EVENT,
  } from '#lib/workspace/auth/hosting.ts';

  let viewState = $state<'loading' | 'anonymous' | 'denied' | 'error' | 'ready'>('loading');
  let settings = $state<HostingSettings | null>(null);
  let draft = $state(false);
  let editing = $state(false);
  let saving = $state(false);
  let saveBlocked = $state(false);
  let error = $state('');
  let status = $state('');
  let request = 0;
  let editButton = $state<HTMLButtonElement>();
  let checkbox = $state<HTMLInputElement>();

  function permissionFailure(err: unknown): boolean {
    if (!(err instanceof HostingRequestError)) return false;
    if (err.status !== 401 && err.status !== 403) return false;
    settings = null;
    editing = false;
    viewState = err.status === 401 ? 'anonymous' : 'denied';
    return true;
  }

  async function load() {
    const current = ++request;
    viewState = 'loading';
    settings = null;
    editing = false;
    error = '';
    status = '';
    saveBlocked = false;
    try {
      const whoami = await loadWhoami();
      if (current !== request) return;
      if (!whoami.actor) { viewState = 'anonymous'; return; }
      const access = await loadHostingAccess();
      if (current !== request) return;
      if (!access.is_admin) { viewState = 'denied'; return; }
      const loaded = await loadHostingSettings();
      if (current !== request) return;
      settings = loaded;
      draft = loaded.self_registration_enabled;
      viewState = 'ready';
    } catch (err) {
      if (current !== request) return;
      if (!permissionFailure(err)) {
        viewState = 'error';
        error = 'Hosting settings could not be loaded.';
      }
    }
  }

  async function edit() {
    if (!settings) return;
    draft = settings.self_registration_enabled;
    editing = true;
    status = '';
    await tick();
    checkbox?.focus();
  }

  async function cancel() {
    editing = false;
    await tick();
    editButton?.focus();
  }

  async function save() {
    if (saving || saveBlocked || viewState !== 'ready' || !settings) return;
    const current = request;
    saving = true;
    error = '';
    status = '';
    try {
      const saved = await saveHostingSettings({ self_registration_enabled: draft });
      if (current !== request) return;
      settings = saved;
      draft = saved.self_registration_enabled;
      editing = false;
      status = 'Hosting settings saved.';
      await tick();
      editButton?.focus();
    } catch (err) {
      if (current !== request) return;
      if (!permissionFailure(err)) {
        // 応答喪失時に未保存と断定しない。再読込後に次の保存を判断する。
        saveBlocked = true;
        error = 'Save could not be confirmed. Reload settings before making another change.';
      }
    } finally {
      saving = false;
    }
  }

  onMount(() => {
    void load();
    const refresh = () => { void load(); };
    window.addEventListener(SESSION_CHANGED_EVENT, refresh);
    return () => {
      ++request;
      window.removeEventListener(SESSION_CHANGED_EVENT, refresh);
    };
  });
</script>

<svelte:head><title>Hosting settings · Yoi</title></svelte:head>

<div class="hosting-settings">
  <h1>Hosting settings</h1>
  {#if viewState === 'loading'}
    <p role="status">Loading hosting settings…</p>
  {:else if viewState === 'anonymous'}
    <p>Sign in with a hosting administrator account to view these settings.</p>
    <a href="/account">Open Account</a>
  {:else if viewState === 'denied'}
    <p>Hosting administrator access is required.</p>
  {:else if viewState === 'error'}
    <p class="error" role="alert">{error}</p>
    <button type="button" onclick={load}>Retry loading settings</button>
  {:else if settings}
    <a href="/hosting/accounts">Create accounts or reissue activation codes</a>
    <section aria-labelledby="registration-heading">
      <h2 id="registration-heading">Self-registration</h2>
      <p class="muted">Applies to this hosting, across all Workspaces.</p>
      <p>Last loaded policy: <strong>{settings.self_registration_enabled ? 'Enabled' : 'Disabled'}</strong></p>
      {#if editing}
        <form onsubmit={(event) => { event.preventDefault(); void save(); }}>
          <label class="checkbox-row">
            <input bind:this={checkbox} type="checkbox" bind:checked={draft} disabled={saving || saveBlocked} aria-describedby="registration-impact" />
            Allow new account registration
          </label>
          <p id="registration-impact" class="muted">Disabling registration does not prevent existing users from logging in or adding a passkey to their own account.</p>
          <div class="actions">
            <button class="primary" type="submit" disabled={saving || saveBlocked || draft === settings.self_registration_enabled}>{saving ? 'Saving…' : 'Save settings'}</button>
            <button type="button" disabled={saving} onclick={cancel}>Cancel</button>
          </div>
        </form>
      {:else}
        <button bind:this={editButton} type="button" disabled={saveBlocked} onclick={edit}>Edit registration policy</button>
      {/if}
      {#if error}
        <p class="error" role="alert">{error}</p>
        <button type="button" onclick={load}>Reload settings</button>
      {/if}
      {#if status}<p role="status">{status}</p>{/if}
    </section>
  {/if}
</div>

<style>
  .hosting-settings, section, form { display: grid; align-content: start; gap: var(--space-4); min-width: 0; }
  .hosting-settings { max-width: 48rem; }
  h1, h2, p { margin: 0; }
  h1 { font-size: var(--font-size-title); line-height: var(--line-height-title); }
  h2 { font-size: var(--font-size-body); line-height: var(--line-height-body); }
  .muted { color: var(--text-muted); }
  .error { color: var(--danger); }
  .actions { display: flex; flex-wrap: wrap; gap: var(--space-2); }
  .checkbox-row { display: flex; align-items: center; gap: var(--space-2); }
  input { flex: 0 0 auto; width: 1rem; height: 1rem; }
  button { justify-self: start; min-height: 2.25rem; padding: var(--space-2) var(--space-3); border: 1px solid var(--line-strong); border-radius: var(--radius-soft); background: transparent; color: var(--text-strong); font: inherit; cursor: pointer; }
  button.primary { background: var(--text-strong); color: var(--bg); }
  button:hover:not(:disabled) { background: var(--interactive-hover); color: var(--text-strong); }
  button:disabled { opacity: .55; cursor: not-allowed; }
  button:focus-visible, input:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
</style>

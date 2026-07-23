<script lang="ts">
  import { onMount } from 'svelte';
  import { page } from '$app/state';
  import {
    approveDeviceLogin,
    loadWhoami,
    loginWithPasskey,
  } from '$lib/workspace/auth/api';
  import type { RequestActor } from '$lib/workspace/auth/model';

  let actor = $state<RequestActor | null>(null);
  let handle = $state('local');
  let userCode = $state('');
  let loading = $state(true);
  let busy = $state(false);
  let status = $state('');
  let error = $state('');

  async function refreshWhoami() {
    loading = true;
    error = '';
    try {
      const whoami = await loadWhoami();
      actor = whoami.actor;
      if (actor?.handle) handle = actor.handle;
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
    } finally {
      loading = false;
    }
  }

  async function login() {
    busy = true;
    status = 'Waiting for passkey login…';
    error = '';
    try {
      await loginWithPasskey(handle.trim());
      status = 'Logged in. You can approve the CLI login now.';
      await refreshWhoami();
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
      status = '';
    } finally {
      busy = false;
    }
  }

  async function approve() {
    busy = true;
    status = 'Approving device login…';
    error = '';
    try {
      const result = await approveDeviceLogin(userCode.trim().toUpperCase());
      status = result.status === 'approved'
        ? 'Device login approved. You can return to the CLI.'
        : `Device login status: ${result.status}`;
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
      status = '';
    } finally {
      busy = false;
    }
  }

  onMount(() => {
    userCode = page.url.searchParams.get('user_code') ?? page.url.searchParams.get('code') ?? '';
    void refreshWhoami();
  });
</script>

<svelte:head>
  <title>Device Login · Yoi Workspace</title>
</svelte:head>

<main class="settings-page">
  <header class="settings-header">
    <p class="eyebrow">Device Login</p>
    <h1>Approve CLI login</h1>
    <p>Use your browser session to approve a pending CLI/TUI login request.</p>
  </header>

  <section class="settings-panel">
    <h2>Browser session</h2>
    {#if loading}
      <p class="muted">Loading session…</p>
    {:else if actor}
      <p>Logged in as <strong>{actor.display_name}</strong> <span class="muted">@{actor.handle}</span>.</p>
    {:else}
      <p class="muted">You need to log in with a passkey before approving a device login.</p>
      <form class="settings-form" onsubmit={(event) => { event.preventDefault(); void login(); }}>
        <label>
          User handle
          <input bind:value={handle} autocomplete="username webauthn" placeholder="local" />
        </label>
        <button type="submit" disabled={busy || !handle.trim()}>Log in with passkey</button>
      </form>
    {/if}
  </section>

  <section class="settings-panel">
    <h2>Approval code</h2>
    <form class="settings-form" onsubmit={(event) => { event.preventDefault(); void approve(); }}>
      <label>
        User code
        <input bind:value={userCode} placeholder="ABCD-EFGH" autocapitalize="characters" />
      </label>
      <button type="submit" disabled={busy || !actor || !userCode.trim()}>Approve device login</button>
    </form>
  </section>

  {#if status}
    <p class="status-message">{status}</p>
  {/if}
  {#if error}
    <p class="error-message">{error}</p>
  {/if}
</main>

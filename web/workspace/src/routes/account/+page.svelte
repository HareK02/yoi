<script lang="ts">
  import { onMount } from 'svelte';
  import {
    loadWhoami,
    loginWithPasskey,
    logout,
    registerPasskey,
  } from '$lib/workspace/auth/api';
  import type { RequestActor } from '$lib/workspace/auth/model';

  let actor = $state<RequestActor | null>(null);
  let handle = $state('local');
  let displayName = $state('Local User');
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
      if (actor?.user.handle) {
        handle = actor.user.handle;
        displayName = actor.user.display_name;
      }
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
    } finally {
      loading = false;
    }
  }

  async function register() {
    busy = true;
    status = 'Waiting for passkey registration…';
    error = '';
    try {
      await registerPasskey(handle.trim(), displayName.trim() || handle.trim());
      status = 'Passkey registered.';
      await refreshWhoami();
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
      status = '';
    } finally {
      busy = false;
    }
  }

  async function login() {
    busy = true;
    status = 'Waiting for passkey login…';
    error = '';
    try {
      await loginWithPasskey(handle.trim());
      status = 'Logged in.';
      await refreshWhoami();
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
      status = '';
    } finally {
      busy = false;
    }
  }

  async function signOut() {
    busy = true;
    error = '';
    try {
      await logout();
      actor = null;
      status = 'Logged out.';
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
    } finally {
      busy = false;
    }
  }

  onMount(() => {
    void refreshWhoami();
  });
</script>

<svelte:head>
  <title>Account · Yoi Workspace</title>
</svelte:head>

<main class="settings-page">
  <header class="settings-header">
    <p class="eyebrow">Account</p>
    <h1>Account</h1>
    <p>Register a passkey, sign in, sign out, and inspect the current browser session.</p>
  </header>

  <section class="settings-panel account-panel">
    <div>
      <h2>Current user</h2>
      {#if loading}
        <p class="muted">Loading session…</p>
      {:else if actor}
        <dl class="account-details">
          <div>
            <dt>Handle</dt>
            <dd>{actor.user.handle}</dd>
          </div>
          <div>
            <dt>Display name</dt>
            <dd>{actor.user.display_name}</dd>
          </div>
          <div>
            <dt>User ID</dt>
            <dd><code>{actor.user.user_id}</code></dd>
          </div>
          <div>
            <dt>Account ID</dt>
            <dd><code>{actor.user.account_id}</code></dd>
          </div>
          <div>
            <dt>Auth kind</dt>
            <dd>{actor.auth_kind}</dd>
          </div>
        </dl>
        <div class="settings-action-row">
          <button type="button" disabled={busy} onclick={signOut}>Log out</button>
          <button class="secondary-button" type="button" disabled={busy} onclick={refreshWhoami}>Refresh</button>
        </div>
      {:else}
        <p class="muted">No browser session is active.</p>
      {/if}
    </div>
  </section>

  <section class="settings-panel">
    <h2>Passkey</h2>
    <form class="settings-form" onsubmit={(event) => { event.preventDefault(); }}>
      <label>
        User handle
        <input bind:value={handle} autocomplete="username webauthn" placeholder="local" />
      </label>
      <label>
        Display name
        <input bind:value={displayName} autocomplete="name" placeholder="Local User" />
      </label>
      <div class="settings-action-row">
        <button type="button" disabled={busy || !handle.trim()} onclick={register}>Register passkey</button>
        <button type="button" disabled={busy || !handle.trim()} onclick={login}>Log in with passkey</button>
      </div>
    </form>
  </section>

  {#if status}
    <p class="status-message">{status}</p>
  {/if}
  {#if error}
    <p class="error-message">{error}</p>
  {/if}
</main>

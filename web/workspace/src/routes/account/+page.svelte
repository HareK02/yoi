<script lang="ts">
  import { onMount } from 'svelte';
  import {
    addPasskey,
    loadWhoami,
    loginWithPasskey,
    logout,
    registerPasskey,
  } from '#lib/workspace/auth/api.ts';
  import '#lib/workspace/styles/settings.css';
  import type { RequestActor } from '#lib/workspace/auth/model.ts';

  let actor = $state<RequestActor | null>(null);
  let handle = $state('local');
  let displayName = $state('Local User');
  let loading = $state(true);
  let sessionReady = $state(false);
  let busy = $state(false);
  let status = $state('');
  let error = $state('');

  async function refreshWhoami() {
    loading = true;
    sessionReady = false;
    error = '';
    try {
      const whoami = await loadWhoami();
      actor = whoami.actor;
      sessionReady = true;
      if (actor?.handle) {
        handle = actor.handle;
        displayName = actor.display_name;
      }
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
    } finally {
      loading = false;
    }
  }

  async function register() {
    if (busy || !sessionReady) return;
    const adding = actor !== null;
    busy = true;
    status = adding ? 'Waiting for passkey addition…' : 'Waiting for new account registration…';
    error = '';
    try {
      if (adding) {
        await addPasskey();
      } else {
        await registerPasskey(handle.trim(), displayName.trim() || handle.trim());
      }
      status = adding ? 'Passkey added to your account.' : 'New account registered.';
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
            <dd>{actor.handle}</dd>
          </div>
          <div>
            <dt>Display name</dt>
            <dd>{actor.display_name}</dd>
          </div>
          <div>
            <dt>User ID</dt>
            <dd><code>{actor.user_id}</code></dd>
          </div>
          <div>
            <dt>Account ID</dt>
            <dd><code>{actor.account_id}</code></dd>
          </div>
          <div>
            <dt>Auth kind</dt>
            <dd>{actor.auth_method}</dd>
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
        {actor ? 'Current user handle' : 'User handle (new registration or login)'}
        <input bind:value={handle} disabled={busy || loading || !sessionReady || actor !== null} autocomplete="username webauthn" placeholder="local" />
      </label>
      <label>
        Display name (new account only)
        <input bind:value={displayName} disabled={busy || loading || !sessionReady || actor !== null} autocomplete="name" placeholder="Local User" />
      </label>
      <div class="settings-action-row">
        <button type="button" disabled={busy || loading || !sessionReady || (!actor && !handle.trim())} onclick={register}>{actor ? `Add passkey to ${actor.handle}` : 'Register new account'}</button>
        <button type="button" disabled={busy || loading || !sessionReady || actor !== null || !handle.trim()} onclick={login}>Log in with passkey</button>
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

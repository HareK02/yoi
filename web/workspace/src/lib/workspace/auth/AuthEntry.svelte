<script lang="ts">
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import { page } from '$app/state';
  import { loginWithLegacyPasskey, loginWithPasskey, registerPasskey, logout } from './api.ts';
  import { CLEAR_RETURN_EVENT } from './session-state.ts';
  import { BrowserAuth } from './browser-session.svelte.ts';
  import { loadRegistrationPolicy } from './hosting.ts';
  import { authEntryUrl, safeReturnTo, AUTHENTICATED_HOME } from './routes.ts';
  import RegistrationLink from './RegistrationLink.svelte';
  import './auth-page.css';

  let { mode }: { mode: 'login' | 'register' } = $props();
  const auth = new BrowserAuth();
  let handle = $state('');
  let displayName = $state('');
  let legacyOpen = $state(false);
  let policy = $state<boolean | null>(null);
  let policyLoading = $state(true);
  let policyError = $state('');
  let policyRequest = 0;
  const destination = $derived(safeReturnTo(page.url.searchParams.get('returnTo')));

  async function refreshPolicy() {
    if (mode !== 'register') return;
    const request = ++policyRequest;
    policyLoading = true; policy = null; policyError = '';
    try {
      const result = await loadRegistrationPolicy();
      if (request === policyRequest) policy = result.self_registration_enabled;
    } catch {
      if (request === policyRequest) policyError = 'Registration availability could not be loaded.';
    } finally {
      if (request === policyRequest) policyLoading = false;
    }
  }

  async function submit(legacy = false) {
    const registering = mode === 'register';
    if (auth.busy || auth.session.kind !== 'ready' || auth.actor ||
        ((registering || legacy) && !handle.trim()) ||
        (registering && (policy !== true || !displayName.trim()))) return;
    const id = handle.trim();
    const name = displayName.trim();
    const confirmed = await auth.run(
      registering ? 'Waiting for new account registration…' : 'Waiting for passkey login…',
      registering ? 'New account registered.' : 'Logged in.',
      (signal) => registering ? registerPasskey(id, name, signal)
        : legacy ? loginWithLegacyPasskey(id, signal) : loginWithPasskey(signal),
    );
    if (confirmed && auth.actor) {
      await goto(destination, { replaceState: true });
    } else {
      if (confirmed) {
        auth.status = '';
        auth.error = 'A normal browser session was not confirmed. Log in again.';
      }
      if (registering) void refreshPolicy();
    }
  }

  async function forget() {
    await auth.run('Logging out…', 'Logged out.', logout, 'logout');
  }
  onMount(() => {
    const clearReturn = () => { void goto(page.url.pathname, { replaceState: true }); };
    window.addEventListener(CLEAR_RETURN_EVENT, clearReturn);
    void auth.refresh();
    void refreshPolicy();
    window.addEventListener('focus', refreshPolicy);
    return () => { ++policyRequest; auth.dispose(); window.removeEventListener('focus', refreshPolicy); window.removeEventListener(CLEAR_RETURN_EVENT, clearReturn); };
  });
</script>

<svelte:head><title>{mode === 'login' ? 'Log in' : 'Create account'} · Yoi</title></svelte:head>
<div class="auth-page" data-sveltekit-preload-data="off">
  <h1>{mode === 'login' ? 'Log in' : 'Create account'}</h1>
  {#if auth.session.kind === 'loading'}
    <p role="status" class="muted">Loading session…</p>
  {:else if auth.session.kind === 'error'}
    <p role="alert" class="error">Your session could not be loaded. Login status is unknown.</p>
    <button type="button" disabled={auth.busy} onclick={() => auth.refresh()}>Retry session</button>
    <button type="button" disabled={auth.busy} onclick={forget}>Forget this session</button>
  {:else if auth.actor}
    <p>Logged in as <strong>{auth.actor.display_name}</strong> <span class="muted">@{auth.actor.handle}</span>.</p>
    <a href={destination}>Continue to {destination === AUTHENTICATED_HOME ? 'dashboard / Workspaces' : 'requested page'}</a>
    <a href="/account">Open Account</a>
  {:else}
    {#if auth.session.session.kind === 'expired'}
      <p role="status">Your browser session expired. Log in to return to the requested page.</p>
      <button type="button" disabled={auth.busy} onclick={forget}>Forget this session</button>
    {/if}
    {#if mode === 'register'}
      {#if policyLoading}
        <p role="status" class="muted">Checking registration availability…</p>
      {:else if policyError}
        <p role="alert" class="error">{policyError}</p>
        <button type="button" onclick={refreshPolicy}>Retry registration availability</button>
      {:else if policy === false}
        <p>New account registration is disabled. Existing users can still log in.</p>
      {/if}
    {/if}
    {#if mode === 'login'}
      <form onsubmit={(event) => { event.preventDefault(); void submit(); }}>
        <button class="primary" type="submit" disabled={auth.busy}>Log in with passkey</button>
        <p class="muted">Choose a passkey and verify with your device. No account ID is needed.</p>
      </form>
      <details class="legacy-login" bind:open={legacyOpen}>
        <summary>Use an older key with account ID</summary>
        <form onsubmit={(event) => { event.preventDefault(); void submit(true); }}>
          <p class="muted">If an older key is not listed or cannot sign in above, use compatibility login. After login, open <a href="/account">Account</a> to inspect keys and verify migration. Adding with the same older-key authenticator may be excluded; use a different saved-key provider or authenticator. Keep older keys until normal ID-free login to the intended account is tested.</p>
          <label>Account ID (handle)
            <input bind:value={handle} required maxlength="128" autocomplete="username" autocapitalize="off" spellcheck="false" disabled={auth.busy} aria-describedby="legacy-id-help" />
          </label>
          <p id="legacy-id-help" class="muted">Use your account ID, not your display name. This only signs in with an existing key; it does not register one.</p>
          <button type="submit" disabled={auth.busy || !handle.trim()}>Log in with older key</button>
        </form>
      </details>
    {:else if policy === true}
      <form onsubmit={(event) => { event.preventDefault(); void submit(); }}>
        <label>Account ID (handle)
          <input bind:value={handle} required maxlength="128" autocomplete="username" autocapitalize="off" spellcheck="false" disabled={auth.busy} aria-describedby="id-help" />
        </label>
        <p id="id-help" class="muted">Your unique account ID, separate from your display name. It is not needed for normal passkey login.</p>
        <label>Display name
          <input bind:value={displayName} required maxlength="128" autocomplete="name" disabled={auth.busy} />
        </label>
        <p class="muted">Requires a device or security key that can save a discoverable passkey and verify you with a PIN or biometrics.</p>
        <button class="primary" type="submit" disabled={auth.busy || !handle.trim() || !displayName.trim()}>Register new account</button>
      </form>
    {/if}
  {/if}
  {#if auth.status}<p role="status">{auth.status}</p>{/if}
  {#if auth.busy}<button type="button" onclick={() => auth.cancel()}>Stop waiting</button>{/if}
  {#if auth.error}<p role="alert" class="error">{auth.error}</p>{/if}
  <nav aria-label="Authentication">
    {#if mode === 'login' && auth.session.kind === 'ready' && !auth.actor}
      <RegistrationLink returnTo={destination} />
    {:else if mode === 'register'}
      <a href={authEntryUrl('/login', destination)}>Log in with an existing account</a>
    {/if}
    <a href="/activate">Activate an account with a code</a>
    <a href={AUTHENTICATED_HOME}>Dashboard / Workspaces</a>
    <a href="/">Yoi home</a>
  </nav>
</div>

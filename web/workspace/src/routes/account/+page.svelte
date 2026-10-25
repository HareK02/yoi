<script lang="ts">
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import type { PasskeyListResponse } from '#lib/generated/auth-api.ts';
  import { addPasskey, logout, loadPasskeys, loadWhoami } from '#lib/workspace/auth/api.ts';
  import { BrowserAuth } from '#lib/workspace/auth/browser-session.svelte.ts';
  import { loginUrl } from '#lib/workspace/auth/routes.ts';
  import '#lib/workspace/auth/auth-page.css';

  const auth = new BrowserAuth();
  let keys = $state<PasskeyListResponse | null>(null);
  let keysError = $state('');
  let keysLoading = $state(false);
  let keyRead = 0;
  let active = true;
  let keyController: AbortController | null = null;
  function time(value: string | null) { return value ? new Date(value).toLocaleString() : 'Not recorded'; }
  async function refreshKeys() {
    const actor = auth.actor;
    if (!actor || !active) return;
    keyController?.abort();
    const controller = new AbortController();
    keyController = controller;
    const read = ++keyRead;
    keysLoading = true; keysError = '';
    try {
      const before = await loadWhoami(controller.signal);
      controller.signal.throwIfAborted();
      if (!active || read !== keyRead) return;
      if (before.actor?.user_id !== actor.user_id || before.actor.account_id !== actor.account_id) throw new Error('Account changed');
      const result = await loadPasskeys(controller.signal);
      controller.signal.throwIfAborted();
      if (!active || read !== keyRead) return;
      const after = await loadWhoami(controller.signal);
      controller.signal.throwIfAborted();
      if (read !== keyRead) return;
      if (result.user.user_id !== actor.user_id || result.user.account_id !== actor.account_id ||
          after.actor?.user_id !== actor.user_id || after.actor.account_id !== actor.account_id ||
          before.authentication_reference !== after.authentication_reference) throw new Error('Session changed');
      keys = result;
    } catch {
      if (active && read === keyRead && !controller.signal.aborted) { keys = null; keysError = 'Registered keys could not be confirmed for this account. Refresh your session before trying again.'; }
    } finally { if (read === keyRead) keysLoading = false; }
  }
  async function refresh() { await auth.refresh(); await refreshKeys(); }
  async function add() {
    if (!auth.actor || auth.additionAttempt && !auth.additionSaved) return;
    await auth.run('Waiting for passkey addition…', 'Passkey added to your account; the expected session was confirmed.', addPasskey);
    await refreshKeys();
  }
  async function checkAddition() { await auth.checkAddition(); await refreshKeys(); }
  async function signOut() {
    if (!auth.actor) return;
    const confirmed = await auth.run('Logging out…', 'Logged out.', logout, 'logout');
    if (confirmed && !auth.actor) await goto('/', { replaceState: true });
  }
  onMount(() => { void auth.ensure().then(refreshKeys); return () => { active = false; ++keyRead; keyController?.abort(); auth.dispose(); }; });
</script>

<svelte:head><title>Account · Yoi</title></svelte:head>
<div class="auth-page" data-sveltekit-preload-data="off">
  <h1>Account</h1>
  {#if auth.status}<p role="status">{auth.status}</p>{/if}
  {#if auth.busy}<button type="button" onclick={() => auth.cancel()}>Stop waiting</button>{/if}
  {#if auth.error}<p role="alert" class="error">{auth.error}</p>{/if}
  {#if auth.session.kind === 'loading'}
    <p class="muted" role="status">Loading session…</p>
  {:else if auth.session.kind === 'error'}
    <p role="alert" class="error">Your session could not be loaded. Login status is unknown.</p>
    <button type="button" disabled={auth.busy} onclick={refresh}>Retry session</button>
  {:else if auth.actor}
    <dl>
      <div><dt>Account ID (handle)</dt><dd>{auth.actor.handle}</dd></div>
      <div><dt>Display name</dt><dd>{auth.actor.display_name}</dd></div>
    </dl>
    <section aria-label="Your Passkeys">
      <h2>Passkeys</h2>
      <p class="muted">Add a passkey for login without an account ID. Use a device or key that saves passkeys and verifies you with a PIN or biometrics. Existing keys are preserved.</p>
      <p>If an older key is in the same authenticator or saved-key provider, duplicate exclusion can block addition there. Choose a different device, security key, or passkey provider. A different provider on the same device may work only if it does not contain an excluded key. Do not delete an older key to bypass this check.</p>
      <button type="button" disabled={auth.busy || !!auth.additionAttempt && !auth.additionSaved} onclick={add}>Add passkey to {auth.actor.handle}</button>
      {#if auth.additionAttempt}
        <button type="button" disabled={auth.busy} onclick={checkAddition}>Check addition result</button>
        <p class="muted">A saved result confirms this addition's server record, not that ID-free login works. If the session changed, result access may be unavailable; do not restore an old session or add blindly.</p>
      {/if}
      {#if keysLoading}<p role="status">Checking registered keys…</p>{/if}
      {#if keysError}<p role="alert" class="error">{keysError}</p>{/if}
      {#if keys && keys.user.user_id === auth.actor.user_id && keys.user.account_id === auth.actor.account_id}
        <ul class="passkey-list" aria-label="Registered keys">
          {#each keys.keys as key (key.key_reference)}
            <li>
              <dl>
                <div><dt>Created</dt><dd>{time(key.created_at)}</dd></div>
                <div><dt>Last used</dt><dd>{time(key.last_used_at)}</dd></div>
                <div><dt>ID-free login</dt><dd>{key.normal_login_verified_at ? `Verified ${time(key.normal_login_verified_at)}` : 'Not yet verified — storage alone does not prove support'}</dd></div>
              </dl>
              <details><summary>Key reference</summary><code>{key.key_reference}</code></details>
            </li>
          {/each}
        </ul>
        {#if keys.keys.length === 0}<p>No registered keys were returned for this account.</p>{/if}
      {/if}
      <button type="button" disabled={auth.busy || keysLoading} onclick={refreshKeys}>Refresh registered keys</button>
      <details>
        <summary>Verify migration before retiring older-key login</summary>
        <ol>
          <li>Keep this working session and every working older key. If addition is excluded, use a different saved-key provider or authenticator; removing your last key first can lock you out.</li>
          <li>In a separate browser profile or private window, open <a href="/login" target="_blank" rel="noopener">normal passkey login</a>. Do not open the older-key/ID form. Select the newly saved key and complete verification.</li>
          <li>Confirm the intended account ID in that window's Account page. Refresh the key list here and check that the chosen key now records verified ID-free login. Repeat for every key and device you intend to rely on; a stored key or resident-key hint alone is not proof.</li>
          <li>Retain a tested independent way back in before ceasing to rely on an older key. If any required device cannot complete normal login, keep the older-key path available. Do not delete old keys as a prerequisite.</li>
        </ol>
        <p>Removing the older-key entry for the service is not safe based on one account's success. The operator must first confirm migration and a working alternative for every affected account/device, keep a rollback path to the older-key entry, and verify normal login before retiring that entry. This page does not delete keys or disable compatibility login.</p>
      </details>
    </section>
    <div class="actions">
      <button type="button" disabled={auth.busy} onclick={signOut}>Log out</button>
      <button type="button" disabled={auth.busy} onclick={refresh}>Refresh session</button>
    </div>
  {:else}
    <p>No browser session is active.</p>
    <a href={loginUrl('/account')}>Log in to your account</a>
  {/if}
  <a href="/dashboard">Dashboard / Workspaces</a>
</div>
<style>
  .passkey-list { padding-inline-start: var(--space-5); display: grid; gap: var(--space-4); }
  .passkey-list li { min-width: 0; }
  code { overflow-wrap: anywhere; }
  ol { padding-inline-start: var(--space-5); }
  li + li { margin-block-start: var(--space-2); }
</style>

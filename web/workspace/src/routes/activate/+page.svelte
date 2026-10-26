<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import { activateWithPasskey } from '#lib/workspace/auth/activation.ts';
  import { normalizeActivationHandle } from '#lib/workspace/auth/hosting-accounts.ts';
  import { notifySessionChanged } from '#lib/workspace/auth/hosting.ts';
  import { browserSession } from '#lib/workspace/auth/session-state.ts';
  import type { AuthUser } from '#lib/workspace/auth/model.ts';

  let handle = $state('');
  let code = $state('');
  let busy = $state(false);
  let error = $state('');
  let user = $state<AuthUser | null>(null);
  let controller: AbortController | null = null;
  let generation = 0;
  // 本人はID+codeから開始し、WebAuthnとcomplete応答が揃うまで通常利用へ進ませない。
  // cancel/reloadはsecretを破棄して再入力。通信断ではactiveを推定しない。
  async function activate() {
    if (busy || user || !code) return;
    try { handle = normalizeActivationHandle(handle); }
    catch { error = 'Check the account ID supplied by your administrator.'; return; }
    const current = ++generation;
    const authGeneration = browserSession.authenticationGeneration;
    const authStamp = browserSession.authenticationStamp;
    controller = new AbortController(); busy = true; error = '';
    const pending = activateWithPasskey(handle, code, controller.signal);
    code = '';
    try {
      const confirmed = await pending;
      if (current !== generation) return;
      if (authGeneration !== browserSession.authenticationGeneration || authStamp !== browserSession.authenticationStamp) throw new Error('Activation superseded');
      user = confirmed;
      notifySessionChanged();
    } catch {
      if (current !== generation) return;
      error = 'Registration completion could not be confirmed. If you completed the Passkey prompt, check the Passkey login page first. Otherwise re-enter the ID and code to restart. Ask your administrator to reissue if the code expired, was locked or was replaced.';
    } finally { if (current === generation) { code = ''; busy = false; controller = null; } }
  }
  function cancel() {
    ++generation; controller?.abort(); controller = null; code = ''; busy = false;
    error = 'Registration stopped; completion is not confirmed. If you completed the Passkey prompt, check the Passkey login page first. Otherwise re-enter the ID and code to restart.';
  }
  onMount(() => {
    const clear = () => { ++generation; controller?.abort(); code = ''; busy = false; };
    window.addEventListener('pagehide', clear);
    return () => window.removeEventListener('pagehide', clear);
  });
  onDestroy(() => { ++generation; controller?.abort(); code = ''; });
</script>

<svelte:head><title>Activate account · Yoi</title></svelte:head>
<div class="activation" data-sveltekit-preload-data="off">
  <h1>Activate account</h1>
  {#if user}
    <p role="status">Passkey registration and activation confirmed for {user.handle}. You are signed in.</p>
    <p>Choose your saved Passkey for future logins; no account ID is needed. The activation code can no longer be used.</p>
    <a href="/account">Open Account</a>
  {:else}
    <p>Enter the ID and one-time code provided by your hosting administrator. You must register a discoverable Passkey before this account can be used. Use a device or key that can save passkeys and verify you with a PIN or biometrics. If unsupported or cancelled, the account stays pending; retry with a supported device.</p>
    <form onsubmit={(event) => { event.preventDefault(); void activate(); }} autocomplete="off">
      <label for="activation-handle">Account ID (handle)</label>
      <input id="activation-handle" bind:value={handle} required maxlength="128" autocomplete="off" autocapitalize="off" spellcheck="false" disabled={busy} />
      <label for="activation-code">Activation code</label>
      <input id="activation-code" type="password" bind:value={code} required maxlength="4096" autocomplete="off" autocapitalize="off" spellcheck="false" disabled={busy} aria-describedby="code-help" />
      <p id="code-help" class="muted">Use only this Server’s trusted HTTPS origin (or localhost). The code expires after 15 minutes or five incorrect attempts. It is not a password or an administrator setup token.</p>
      <div class="actions">
        <button class="primary" type="submit" disabled={busy || !handle.trim() || !code}>{busy ? 'Registering Passkey…' : 'Register Passkey and activate'}</button>
        {#if busy}<button type="button" onclick={cancel}>Cancel registration</button>{/if}
      </div>
    </form>
    {#if busy}<p role="status">Passkey registration pending. Account activation has not been confirmed.</p>{/if}
    {#if error}<p role="alert" class="error">{error}</p>{/if}
    <a href="/login">Already registered or completion uncertain? Log in with a Passkey</a>
    <details><summary>Restart or get a new code</summary><p>Allow the browser’s Passkey prompt and save a Passkey. Only a confirmed completion signs you in; checking the code alone gives no normal session, Workspace access or device approval. After cancellation or reload, enter the ID and code again. A correct code is not consumed by starting the prompt. If a request lost its response, try Passkey login before restarting. If no Passkey works, ask your administrator to reissue for the pending ID; reissue invalidates the old code and prompt. This works even when self-registration is disabled.</p></details>
  {/if}
</div>

<style>
  .activation, form, details[open] { display: grid; align-content: start; gap: var(--space-4); min-width: 0; }
  .activation { max-width: 42rem; }
  h1, p { margin: 0; }
  h1 { font-size: var(--font-size-title); line-height: var(--line-height-title); }
  .muted { color: var(--text-muted); }
  .error { color: var(--danger); }
  .actions { display: flex; flex-wrap: wrap; gap: var(--space-2); }
  input { width: 100%; min-width: 0; padding: var(--space-2); border: 1px solid var(--line-strong); border-radius: var(--radius-soft); background: var(--bg); color: var(--text-strong); font: inherit; }
  button { justify-self: start; min-height: 2.25rem; padding: var(--space-2) var(--space-3); border: 1px solid var(--line-strong); border-radius: var(--radius-soft); background: transparent; color: var(--text-strong); font: inherit; cursor: pointer; }
  button.primary { background: var(--text-strong); color: var(--bg); }
  button:hover:not(:disabled) { background: var(--interactive-hover); color: var(--text-strong); }
  button:disabled { opacity: .55; cursor: not-allowed; }
  a { color: var(--text-strong); text-decoration: underline; text-underline-offset: .15em; }
  a:hover, a:active { color: var(--text); }
  a:focus-visible, button:focus-visible, input:focus-visible, summary:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
</style>

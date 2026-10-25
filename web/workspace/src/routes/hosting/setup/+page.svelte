<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import { browserSession, observeBrowserSession, sessionMutation } from "#lib/workspace/auth/session-state.ts";
  import { loginWithPasskey, loadWhoami } from '#lib/workspace/auth/api.ts';
  import { observeAuthOperation } from '#lib/workspace/auth/operation.ts';
  import { enrollHostingPasskey } from '#lib/workspace/auth/hosting-enrollment.ts';
  import { loadHostingAccess, notifySessionChanged } from '#lib/workspace/auth/hosting.ts';
  import type { AuthUser } from '#lib/workspace/auth/model.ts';

  let token = $state('');
  let user = $state<AuthUser | null>(null);
  let busy = $state(false);
  let verifiedLogin = $state(false);
  let error = $state('');
  const controller = new AbortController();
  let loginController: AbortController | null = null;
  let active = true;

  let verifiedGeneration: number | null = null;
  let verifiedStamp: string | null = null;
  const isCurrent = (generation: number, stamp: string | null) => active && generation === browserSession.authenticationGeneration && stamp === browserSession.authenticationStamp;
  // 初期Bearerでの鍵追加はcookieを使わない。明示Passkey loginのsessionを検証する。
  function matchesSession(expected: AuthUser): boolean {
    const snapshot = browserSession.snapshot;
    if (snapshot.kind !== 'ready' || snapshot.value.session.kind !== 'authenticated') return false;
    const actor = snapshot.value.actor;
    return actor?.user_id === expected.user_id && actor.account_id === expected.account_id && actor.auth_method === 'browser_session';
  }
  onMount(() => {
    observeBrowserSession();
    return browserSession.subscribe(() => {
      // 公開pageにはprivate SessionBoundaryの取消がない。成功表示も確認した世代に限る。
      // 後続の再確認/失効/主体変更では明示loginとaccess確認からやり直す。
      if (!user || verifiedGeneration !== browserSession.authenticationGeneration || verifiedStamp !== browserSession.authenticationStamp || !matchesSession(user)) {
        verifiedLogin = false;
        verifiedGeneration = null;
        verifiedStamp = null;
      }
    });
  });

  async function enroll() {
    if (busy || user || !token.trim()) return;
    busy = true;
    error = '';
    const pending = enrollHostingPasskey(token.trim(), controller.signal);
    // 非同期操作に渡した後、form側にはsecretを残さない。
    token = '';
    try {
      const enrolled = await pending;
      if (!active) return;
      user = enrolled;
    } catch {
      if (active) error = 'Passkey enrollment could not be confirmed. The token was cleared. If you completed the passkey prompt, try signing in from Account before enrolling again. Otherwise check the token and its expiry with the host operator.';
    } finally {
      token = '';
      if (active) busy = false;
    }
  }

  async function login() {
    if (busy || !user) return;
    const expected = user;
    busy = true;
    error = '';
    sessionMutation();
    let generation = browserSession.authenticationGeneration;
    let stamp = browserSession.authenticationStamp;
    const operation = new AbortController();
    loginController = operation;
    let promptGeneration: number | null = generation;
    const unsubscribe = browserSession.subscribe(() => {
      if (promptGeneration !== null && !isCurrent(promptGeneration, stamp)) operation.abort();
    });
    try {
      const initial = await loadWhoami(operation.signal);
      if (!isCurrent(generation, stamp)) throw new Error('Login superseded');
      observeAuthOperation(operation.signal, {
        expected: initial, phase: () => {}, additionAttempt: () => {},
        verifySession: async () => {
          if (!isCurrent(generation, stamp)) operation.abort();
          operation.signal.throwIfAborted();
          const current = await loadWhoami(operation.signal);
          if (!isCurrent(generation, stamp) || initial.actor?.user_id !== current.actor?.user_id || initial.actor?.account_id !== current.actor?.account_id || initial.authentication_reference !== current.authentication_reference) throw new Error('Session changed');
        },
      });
      const signedIn = await loginWithPasskey(operation.signal);
      if (!isCurrent(generation, stamp)) throw new Error('Login superseded');
      if (signedIn.user.user_id !== expected.user_id || signedIn.user.account_id !== expected.account_id) {
        throw new Error('Unexpected login actor');
      }
      // Set-Cookie後の確認を共有状態へ発行し、操作前/操作中のreadを再利用しない。
      promptGeneration = null;
      sessionMutation();
      generation = browserSession.authenticationGeneration;
      stamp = browserSession.authenticationStamp;
      await browserSession.refresh();
      if (!isCurrent(generation, stamp) || !matchesSession(expected) || browserSession.snapshot.kind !== 'ready' || browserSession.snapshot.value.authentication_reference !== signedIn.authentication_reference) throw new Error('Session not verified');
      const access = await loadHostingAccess();
      if (!isCurrent(generation, stamp) || !access.is_admin) throw new Error('Access not verified');
      // accessのawait中にもcookie/actorは変わり得る。success公開前の最終確認を行う。
      await browserSession.refresh();
      if (!isCurrent(generation, stamp) || !matchesSession(expected) || browserSession.snapshot.kind !== 'ready' || browserSession.snapshot.value.authentication_reference !== signedIn.authentication_reference) throw new Error('Session superseded');
      verifiedGeneration = generation;
      verifiedStamp = stamp;
      verifiedLogin = true;
      notifySessionChanged();
    } catch {
      if (active) error = 'Passkey login or hosting administrator access could not be verified. Try signing in again, or ask the host operator to check this account.';
      // 応答不明時の再確認は自身がまだ現世代の時だけ。古いfinallyで新logout/loginを
      // invalidateしたり、その確定結果を再取得のloadingへ戻したりしない。
      if (isCurrent(generation, stamp)) {
        sessionMutation();
        await browserSession.refresh();
      }
    } finally {
      promptGeneration = null; unsubscribe(); loginController = null;
      if (active) busy = false;
    }
  }

  onDestroy(() => {
    active = false;
    token = '';
    controller.abort();
    loginController?.abort();
  });
</script>

<svelte:head><title>Hosting setup · Yoi</title></svelte:head>

<div class="hosting-setup">
  <h1>Hosting setup</h1>
  {#if !user}
    <p>Use a host-issued setup token to add the first passkey to the administrator account selected by the host. Opening this page does not create an account or grant administrator access.</p>
    <details>
      <summary>Get a token on the trusted host</summary>
      <p>The host operator runs this command against the Server database, using a new handle:</p>
      <pre><code>yoi-server hosting-admin --database PATH --handle NEW --create-user</code></pre>
      <p>Use the access_token printed once by that command within 15 minutes. Keep it private; do not put it in a URL, screenshot, shared terminal log, or support message. This is not public registration or an activation code.</p>
    </details>
    <form onsubmit={(event) => { event.preventDefault(); void enroll(); }} autocomplete="off">
      <label for="host-setup-token">Host setup token</label>
      <input id="host-setup-token" type="password" bind:value={token} autocomplete="off" spellcheck="false" autocapitalize="off" maxlength="4096" disabled={busy} required aria-describedby="setup-token-help" />
      <p id="setup-token-help" class="muted">Only enter a token from your trusted host operator on this Server’s trusted HTTPS origin (or localhost). It is kept only in this page’s memory and cleared after an attempt or when leaving.</p>
      <button class="primary" type="submit" disabled={busy || !token.trim()}>{busy ? 'Adding passkey…' : 'Add administrator passkey'}</button>
    </form>
    <a href="/account">Already have a passkey? Open Account</a>
  {:else if !verifiedLogin}
    <p role="status">Passkey added for {user.handle}. Enrollment is not a passkey login.</p>
    <p>Choose the new passkey for {user.handle} and verify with your device. No account ID is needed; only this administrator account will pass the access check.</p>
    <button class="primary" type="button" disabled={busy} onclick={login}>{busy ? 'Signing in…' : 'Sign in with new passkey'}</button>
  {:else}
    <p role="status">Passkey login and hosting administrator access verified.</p>
    <a href="/hosting/settings">Open hosting settings</a>
  {/if}
  {#if error}<p class="error" role="alert">{error}</p>{/if}
</div>

<style>
  .hosting-setup, form, details[open] { display: grid; align-content: start; gap: var(--space-4); min-width: 0; }
  .hosting-setup { max-width: 42rem; }
  h1, p, pre { margin: 0; }
  h1 { font-size: var(--font-size-title); line-height: var(--line-height-title); }
  .muted { color: var(--text-muted); }
  .error { color: var(--danger); }
  pre { white-space: pre-wrap; overflow-wrap: anywhere; font-family: var(--font-mono); }
  summary { cursor: pointer; }
  input { width: 100%; min-width: 0; padding: var(--space-2); border: 1px solid var(--line-strong); border-radius: var(--radius-soft); background: var(--bg); color: var(--text-strong); font: inherit; }
  button { justify-self: start; min-height: 2.25rem; padding: var(--space-2) var(--space-3); border: 1px solid var(--line-strong); border-radius: var(--radius-soft); font: inherit; cursor: pointer; }
  button.primary { background: var(--text-strong); color: var(--bg); }
  button:hover:not(:disabled) { background: var(--interactive-hover); color: var(--text-strong); }
  button:disabled { opacity: .55; cursor: not-allowed; }
  button:focus-visible, input:focus-visible, summary:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
</style>

<script lang="ts">
  import { onMount, type Snippet } from 'svelte';
  import { goto, invalidate } from '$app/navigation';
  import { page } from '$app/state';
  import { browserSession, observeBrowserSession, type SessionSnapshot } from './session-state.ts';
  import { isPublicPage, loginUrl } from './routes.ts';
  import './auth-page.css';

  let { children }: { children: Snippet } = $props();
  let session = $state<SessionSnapshot>(browserSession.snapshot);
  let admitted = $state(browserSession.snapshot.kind === 'ready' && browserSession.snapshot.value.session.kind === 'authenticated');
  const actorKey = $derived(session.kind === 'ready' && session.value.actor
    ? `${session.value.actor.user_id}:${session.value.actor.account_id}` : null);
  let admittedActor = $state<string | null>(browserSession.snapshot.kind === 'ready' && browserSession.snapshot.value.actor
    ? `${browserSession.snapshot.value.actor.user_id}:${browserSession.snapshot.value.actor.account_id}` : null);
  const sameActor = $derived(admittedActor === null || admittedActor === actorKey);
  const protectedPage = $derived(!isPublicPage(page.url.pathname));
  let navigating = false;

  onMount(() => {
    observeBrowserSession();
    return browserSession.subscribe((snapshot) => {
      // 子onMountの再確認より前に入場を記録する。effect後回しだとloadingの度に
      // childを再mountし、onMount→whoami→unmountの反復を作ってしまう。
      if (snapshot.kind === 'ready' && snapshot.value.session.kind === 'authenticated') admitted = true;
      session = snapshot;
    });
  });
  $effect(() => {
    if (!protectedPage || session.kind !== 'ready') return;
    const state = session.value.session.kind;
    if (state === 'authenticated') {
      if (!sameActor) {
        // 別tabで主体が変わった場合、旧主体で取得したloader/subscriptionを再利用しない。
        if (!navigating) { navigating = true; location.reload(); }
        return;
      }
      admittedActor = actorKey;
      admitted = true;
      return;
    }
    admitted = false;
    if (navigating) return;
    navigating = true;
    const target = state === 'expired'
      ? loginUrl(location.pathname + location.search + location.hash) : '/';
    void goto(target, { replaceState: true }).finally(() => { navigating = false; });
  });
  async function retry() {
    await browserSession.refresh();
    await invalidate('auth:session');
  }
</script>

{#if !protectedPage}
  {@render children()}
{:else}
  {#if session.kind === 'loading'}
    <div class="auth-page"><p role="status">Loading session…</p></div>
  {:else if session.kind === 'error'}
    <div class="auth-page">
      <p class="error" role="alert">Your session could not be loaded. Login status is unknown.</p>
      <button type="button" onclick={retry}>Retry session</button>
      <a href="/">Yoi home</a>
    </div>
  {/if}
  {#if session.kind === 'ready' ? session.value.session.kind === 'authenticated' && sameActor : admitted}
    <!-- 再確認中は表示/操作を停止し、進行中のlogout等をunmountで取消さない。 -->
    <div hidden={session.kind !== 'ready'} inert={session.kind !== 'ready'}>{@render children()}</div>
  {/if}
{/if}

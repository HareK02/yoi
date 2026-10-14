<script lang="ts" generics="T extends { deferredWorkspace?: Promise<DeferredResult<object>>; deferredPage?: Promise<DeferredResult<object>>; deferredCatalog?: unknown }">
  import { goto, invalidateAll } from '$app/navigation';
  import { page, navigating } from '$app/state';
  import type { Snippet } from 'svelte';
  import { resolvePageData, type DeferredResult, type DeferredFailure, type ResolvedPageData } from './deferred.ts';

  let { data, title, children, identity }: {
    identity?: string;
    data: T;
    title: string;
    children: Snippet<[ResolvedPageData<T>]>;
  } = $props();
  let resolved = $state.raw<{ path: string; value: ResolvedPageData<T> } | null>(null);
  let failure = $state.raw<{ path: string; value: DeferredFailure } | null>(null);
  let loading = $state(true);
  let renderFailure = $state.raw<{ path: string; error: unknown } | null>(null);
  let recoveryVersion = $state(0);
  let retrying = $state(false);
  let retryFailure = $state<string | null>(null);
  const errorMessage = (error: unknown) => {
    // SvelteKit may transform render exceptions into a serialized App.Error.
    if (error !== null && typeof error === 'object' && 'message' in error && typeof error.message === 'string') {
      return error.message;
    }
    return String(error);
  };

  async function retry() {
    if (retrying || loading) return;
    const requestedPath = path;
    retrying = true;
    retryFailure = null;
    try {
      await invalidateAll();
    } catch (error) {
      if (path === requestedPath) retryFailure = errorMessage(error);
    } finally {
      retrying = false;
    }
  }
  const path = $derived(identity ?? page.url.pathname);
  const current = $derived(resolved?.path === path ? resolved.value : null);
  const currentFailure = $derived(failure?.path === path ? failure.value : null);

  $effect(() => {
    const requestedPath = path;
    const requestedData = data;
    let active = true;
    loading = true;
    failure = null;
    retryFailure = null;
    void resolvePageData(requestedData).then(async (result) => {
      if (!active) return;
      if (result.ok) {
        // Never reset a failed boundary against stale data, including while Retry
        // invalidation is pending. Remount only after this load has fresh data.
        if (renderFailure?.path === requestedPath) recoveryVersion += 1;
        renderFailure = null;
        resolved = { path: requestedPath, value: result.data };
        loading = false;
      } else if (result.failure.redirect) {
        // 次のroute moduleを待っている間も、旧ページのcanonical遷移で奪い返さない。
        if (navigating.to && navigating.to.url.href !== page.url.href) return;
        resolved = null;
        try {
          await goto(result.failure.redirect, { replace: true });
        } catch (error) {
          if (active) {
            failure = { path: requestedPath, value: { message: String(error), status: null, redirect: null } };
            loading = false;
          }
        }
      } else {
        resolved = null;
        failure = { path: requestedPath, value: result.failure };
        loading = false;
      }
    });
    return () => { active = false; };
  });
</script>

<svelte:head>
  {#if currentFailure || renderFailure?.path === path}<title>{title} · Error</title>
  {:else if !current}<title>{title} · Loading</title>{/if}
</svelte:head>

{#if currentFailure}
  <section class="workspace-page deferred-page" aria-label={title}>
    <h1>{title}</h1>
    <p role="alert">{currentFailure.message}</p>
    {#if retryFailure}<p role="alert">{retryFailure}</p>{/if}
    <button type="button" class="workspace-secondary-button" disabled={retrying || loading} onclick={retry}>Retry</button>
  </section>
{:else if current}
  {#if loading}<p class="deferred-refresh" role="status">Updating…</p>{/if}
  {#key path}
    {#key recoveryVersion}
      <svelte:boundary onerror={(error) => { renderFailure = { path, error }; }}>
        {@render children(current)}
        {#snippet failed(error)}
          <section class="workspace-page deferred-page" aria-label={title}>
            <h1>{title}</h1>
            <p role="alert">{errorMessage(error)}</p>
            {#if retryFailure}<p role="alert">{retryFailure}</p>{/if}
            <button type="button" class="workspace-secondary-button" disabled={retrying || loading} onclick={retry}>Retry</button>
          </section>
        {/snippet}
      </svelte:boundary>
    {/key}
  {/key}
{:else}
  <section class="workspace-page deferred-page" aria-label={title} aria-busy="true">
    <h1>{title}</h1>
    <p role="status">Loading…</p>
  </section>
{/if}

<style>
  .deferred-page { min-width: 0; }
  .deferred-page h1 { color: var(--text-strong); font-size: var(--font-size-title); line-height: var(--line-height-title); margin: 0; }
  .deferred-page p, .deferred-refresh { color: var(--text-muted); }
  .deferred-refresh { margin: 0; font-size: var(--font-size-compact); }
</style>

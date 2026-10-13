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
  const path = $derived(identity ?? page.url.pathname);
  const current = $derived(resolved?.path === path ? resolved.value : null);
  const currentFailure = $derived(failure?.path === path ? failure.value : null);

  $effect(() => {
    const requestedPath = path;
    const requestedData = data;
    let active = true;
    loading = true;
    failure = null;
    void resolvePageData(requestedData).then(async (result) => {
      if (!active) return;
      if (result.ok) {
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
  {#if currentFailure}<title>{title} · Unavailable</title>
  {:else if !current}<title>{title} · Loading</title>{/if}
</svelte:head>

{#if currentFailure}
  <section class="workspace-page deferred-page" aria-label={title}>
    <h1>{title}</h1>
    <p role="alert">{currentFailure.message}</p>
    <button type="button" class="workspace-secondary-button" onclick={() => invalidateAll()}>Retry</button>
  </section>
{:else if current}
  {#if loading}<p class="deferred-refresh" role="status">Updating…</p>{/if}
  {#key path}{@render children(current)}{/key}
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

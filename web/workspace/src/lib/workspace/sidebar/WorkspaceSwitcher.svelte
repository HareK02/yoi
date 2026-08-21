<script lang="ts">
  import { onMount, tick } from "svelte";
  import {
    listWorkspaces,
    type WorkspaceCatalogRecord,
  } from "$lib/workspace/api/workspace-catalog";

  let {
    currentWorkspaceId,
    currentWorkspaceName,
  }: {
    currentWorkspaceId: string;
    currentWorkspaceName: string;
  } = $props();

  let workspaces = $state<WorkspaceCatalogRecord[]>([]);
  let loading = $state(true);
  let error = $state("");
  let open = $state(false);
  let root = $state.raw<HTMLDivElement>();
  let trigger = $state.raw<HTMLButtonElement>();
  let menu = $state.raw<HTMLDivElement>();

  const menuWorkspaces = $derived.by(() => {
    const entries = workspaces.map((workspace) => ({
      workspace_id: workspace.workspace_id,
      display_name: workspace.display_name,
    }));
    if (!entries.some((workspace) => workspace.workspace_id === currentWorkspaceId)) {
      entries.unshift({
        workspace_id: currentWorkspaceId,
        display_name: currentWorkspaceName,
      });
    }
    return entries;
  });

  async function loadWorkspaces() {
    loading = true;
    error = "";
    try {
      workspaces = await listWorkspaces(fetch);
    } catch (cause) {
      error = cause instanceof Error ? cause.message : "Failed to load Workspaces.";
    } finally {
      loading = false;
    }
  }

  function closeMenu() {
    open = false;
  }

  async function openMenu(focus: "none" | "first" | "last" = "none") {
    open = true;
    if (focus === "none") return;
    await tick();
    const items = menu?.querySelectorAll<HTMLElement>("[role='menuitem']");
    if (!items?.length) return;
    items[focus === "first" ? 0 : items.length - 1]?.focus();
  }

  function toggleMenu() {
    if (open) {
      closeMenu();
    } else {
      void openMenu();
    }
  }

  function handleTriggerKeydown(event: KeyboardEvent) {
    if (event.key === "ArrowDown") {
      event.preventDefault();
      void openMenu("first");
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      void openMenu("last");
    }
  }

  function handleMenuKeydown(event: KeyboardEvent) {
    if (event.key === "Escape") {
      event.preventDefault();
      closeMenu();
      trigger?.focus();
      return;
    }

    if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key) || !menu) return;
    const items = [...menu.querySelectorAll<HTMLElement>("[role='menuitem']")];
    if (!items.length) return;
    event.preventDefault();
    const currentIndex = items.indexOf(document.activeElement as HTMLElement);
    let nextIndex = currentIndex;
    if (event.key === "Home") nextIndex = 0;
    if (event.key === "End") nextIndex = items.length - 1;
    if (event.key === "ArrowDown") nextIndex = (currentIndex + 1) % items.length;
    if (event.key === "ArrowUp") {
      nextIndex = (currentIndex - 1 + items.length) % items.length;
    }
    items[nextIndex]?.focus();
  }

  function handleDocumentPointerDown(event: PointerEvent) {
    if (open && root && !root.contains(event.target as Node)) closeMenu();
  }

  onMount(() => {
    document.addEventListener("pointerdown", handleDocumentPointerDown);
    void loadWorkspaces();
    return () => document.removeEventListener("pointerdown", handleDocumentPointerDown);
  });
</script>

<div class="workspace-menu" bind:this={root}>
  <button
    bind:this={trigger}
    type="button"
    class="workspace-menu-trigger"
    aria-haspopup="menu"
    aria-expanded={open}
    aria-controls="workspace-menu-popover"
    onclick={toggleMenu}
    onkeydown={handleTriggerKeydown}
  >
    <span>{currentWorkspaceName}</span>
    <svg viewBox="0 0 24 24" aria-hidden="true">
      <path d="m8 10 4 4 4-4" />
    </svg>
  </button>

  {#if open}
    <div
      bind:this={menu}
      id="workspace-menu-popover"
      class="workspace-menu-popover"
      role="menu"
      tabindex="-1"
      aria-label="Workspace menu"
      onkeydown={handleMenuKeydown}
    >
      <a
        class="workspace-menu-item"
        href={`/w/${encodeURIComponent(currentWorkspaceId)}/settings`}
        role="menuitem"
        onclick={closeMenu}
      >
        <svg viewBox="0 0 24 24" aria-hidden="true">
          <path d="M12 15.5a3.5 3.5 0 1 0 0-7 3.5 3.5 0 0 0 0 7Z" />
          <path d="M19.4 15a1.8 1.8 0 0 0 .36 1.98l.06.06-2.12 2.12-.06-.06a1.8 1.8 0 0 0-1.98-.36 1.8 1.8 0 0 0-1.1 1.64v.12h-3v-.12a1.8 1.8 0 0 0-1.1-1.64 1.8 1.8 0 0 0-1.98.36l-.06.06-2.12-2.12.06-.06A1.8 1.8 0 0 0 6.6 15a1.8 1.8 0 0 0-1.64-1.1h-.12v-3h.12A1.8 1.8 0 0 0 6.6 9a1.8 1.8 0 0 0-.36-1.98l-.06-.06 2.12-2.12.06.06A1.8 1.8 0 0 0 10.34 5a1.8 1.8 0 0 0 1.1-1.64v-.12h3v.12A1.8 1.8 0 0 0 15.54 5a1.8 1.8 0 0 0 1.98-.36l.06-.06 2.12 2.12-.06.06A1.8 1.8 0 0 0 19.4 9a1.8 1.8 0 0 0 1.64 1.1h.12v3h-.12A1.8 1.8 0 0 0 19.4 15Z" />
        </svg>
        <span>Settings</span>
      </a>

      <div class="workspace-menu-separator" role="separator"></div>

      <div class="workspace-menu-heading">
        <span>Workspaces</span>
        <a
          class="workspace-menu-add"
          href="/#workspace-create-title"
          role="menuitem"
          aria-label="Create Workspace"
          title="Create Workspace"
          onclick={closeMenu}
        >+</a>
      </div>

      <div class="workspace-menu-list">
        {#each menuWorkspaces as workspace (workspace.workspace_id)}
          <a
            class:current={workspace.workspace_id === currentWorkspaceId}
            class="workspace-menu-item workspace-menu-workspace"
            href={`/w/${encodeURIComponent(workspace.workspace_id)}`}
            role="menuitem"
            aria-current={workspace.workspace_id === currentWorkspaceId ? "page" : undefined}
            onclick={closeMenu}
          >
            <span>{workspace.display_name}</span>
            {#if workspace.workspace_id === currentWorkspaceId}
              <svg class="workspace-menu-check" viewBox="0 0 24 24" aria-hidden="true">
                <path d="m6 12 4 4 8-8" />
              </svg>
            {/if}
          </a>
        {/each}
      </div>

      {#if loading}
        <p class="workspace-menu-status">Loading Workspaces…</p>
      {:else if error}
        <p class="workspace-menu-status error">{error}</p>
      {/if}
    </div>
  {/if}
</div>

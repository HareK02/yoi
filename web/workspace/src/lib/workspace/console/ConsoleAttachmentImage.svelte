<script lang="ts">
  import type { SessionToolAttachment } from "#lib/generated/protocol.ts";
  import {
    ConsoleAttachmentFetchError,
    loadConsoleAttachment,
  } from "./attachment-loader.ts";

  type Props = {
    attachment: SessionToolAttachment;
    url: string | null;
  };

  let { attachment, url }: Props = $props();
  let root: HTMLElement | null = $state(null);
  let objectUrl = $state<string | null>(null);
  let previewDialog: HTMLDialogElement | null = $state(null);
  let previewTrigger: HTMLButtonElement | null = $state(null);
  let closeButton: HTMLButtonElement | null = $state(null);
  let sourceVersion = 0;
  let status = $state<"idle" | "loading" | "ready" | "missing" | "expired" | "unauthorized" | "network" | "failed">("idle");

  async function load(): Promise<void> {
    if (!url || status === "loading" || status === "ready") return;
    const version = sourceVersion;
    status = "loading";
    try {
      const blob = await loadConsoleAttachment(url);
      if (version !== sourceVersion) return;
      if (objectUrl) URL.revokeObjectURL(objectUrl);
      objectUrl = URL.createObjectURL(blob);
      status = "ready";
    } catch (error) {
      if (version !== sourceVersion) return;
      status = error instanceof ConsoleAttachmentFetchError ? error.kind : "failed";
    }
  }

  function statusText(): string {
    switch (status) {
      case "idle": return url ? "Image waiting to load" : "Image Session is unavailable";
      case "loading": return "Loading image…";
      case "missing": return "Image is missing";
      case "expired": return "Image retention has expired";
      case "unauthorized": return "Image access was denied";
      case "network": return "Image network request failed";
      case "failed": return "Image could not be loaded";
      default: return "";
    }
  }

  function openPreview(): void {
    if (!previewDialog || previewDialog.open || status !== "ready") return;
    previewDialog.showModal();
    closeButton?.focus({ preventScroll: true });
  }

  function closePreview(): void {
    previewDialog?.close();
    previewTrigger?.focus({ preventScroll: true });
  }

  function previewKeydown(event: KeyboardEvent): void {
    // Console shortcuts must not run while the modal owns keyboard input.
    event.stopPropagation();
    if (event.key === "Escape") {
      event.preventDefault();
      closePreview();
    }
  }

  $effect(() => {
    // A reused Console row must not retain another attachment's preview.
    url;
    status = "idle";
    objectUrl = null;
    return () => {
      sourceVersion += 1;
      previewDialog?.close();
      if (objectUrl) URL.revokeObjectURL(objectUrl);
    };
  });

  $effect(() => {
    if (!root || !url || status !== "idle") return;
    if (typeof IntersectionObserver === "undefined") {
      void load();
      return;
    }
    const observer = new IntersectionObserver((entries) => {
      if (entries.some((entry) => entry.isIntersecting)) {
        observer.disconnect();
        void load();
      }
    }, { rootMargin: "160px" });
    observer.observe(root);
    return () => observer.disconnect();
  });
</script>

<div class="console-attachment" bind:this={root} data-attachment-id={attachment.attachment_id}>
  {#if status === "ready" && objectUrl}
    <button
      type="button"
      class="image-preview-trigger"
      aria-label="Enlarge tool output image"
      aria-haspopup="dialog"
      title="Click to enlarge"
      bind:this={previewTrigger}
      onclick={openPreview}
    >
      <img class="thumbnail" src={objectUrl} alt="Tool output attachment" />
    </button>
    <dialog
      class="image-preview"
      aria-label="Image preview"
      bind:this={previewDialog}
      onkeydown={previewKeydown}
      oncancel={(event) => { event.preventDefault(); closePreview(); }}
    >
      <button
        type="button"
        class="image-preview-backdrop"
        aria-label="Dismiss image preview"
        tabindex="-1"
        onclick={closePreview}
      ></button>
      <button
        type="button"
        class="image-preview-close"
        bind:this={closeButton}
        onclick={closePreview}
      >Close</button>
      <img class="image-preview-full" src={objectUrl} alt="Enlarged tool output attachment" />
    </dialog>
  {:else}
    <span>{statusText()}</span>
    {#if url && ["missing", "expired", "unauthorized", "network", "failed"].includes(status)}
      <button class="retry" type="button" onclick={() => void load()}>Retry</button>
    {/if}
  {/if}
</div>

<style>
  .console-attachment {
    margin-top: var(--space-2);
    color: var(--text-muted);
    font-size: var(--font-size-compact);
  }
  .image-preview-trigger {
    display: block;
    max-width: 100%;
    padding: 0;
    border: 0;
    border-radius: 0.5rem;
    background: transparent;
    cursor: zoom-in;
  }
  .image-preview-trigger:focus-visible {
    outline: 2px solid var(--text-strong);
    outline-offset: 3px;
  }
  .thumbnail {
    display: block;
    max-width: min(100%, 52rem);
    max-height: 42rem;
    object-fit: contain;
    border: 1px solid var(--line);
    border-radius: 0.5rem;
  }
  .image-preview {
    position: fixed;
    inset: 0;
    box-sizing: border-box;
    width: 100%;
    height: 100dvh;
    max-width: none;
    max-height: none;
    margin: 0;
    padding: max(1rem, env(safe-area-inset-top)) max(1rem, env(safe-area-inset-right))
      max(1rem, env(safe-area-inset-bottom)) max(1rem, env(safe-area-inset-left));
    border: 0;
    background: transparent;
    overflow: hidden;
    overscroll-behavior: contain;
  }
  .image-preview[open] {
    display: grid;
    grid-template-rows: auto minmax(0, 1fr);
    gap: var(--space-2);
  }
  .image-preview::backdrop {
    background: rgb(0 0 0 / 85%);
  }
  .image-preview-backdrop {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    padding: 0;
    border: 0;
    border-radius: 0;
    background: transparent;
    cursor: zoom-out;
  }
  .image-preview-close {
    position: relative;
    justify-self: end;
    padding: 0.5rem 1rem;
    color: white;
    background: #222;
    border: 1px solid #888;
    border-radius: 0.5rem;
    cursor: pointer;
  }
  .image-preview-close:focus-visible {
    outline: 2px solid white;
    outline-offset: 3px;
  }
  .image-preview-full {
    position: relative;
    display: block;
    align-self: center;
    justify-self: center;
    min-width: 0;
    min-height: 0;
    max-width: 100%;
    max-height: 100%;
    object-fit: contain;
  }
  .retry {
    margin-left: var(--space-2);
  }
</style>

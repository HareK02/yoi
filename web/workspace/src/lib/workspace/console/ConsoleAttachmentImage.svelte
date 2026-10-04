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
  let disposed = false;
  let status = $state<"idle" | "loading" | "ready" | "missing" | "expired" | "unauthorized" | "network" | "failed">("idle");

  async function load(): Promise<void> {
    if (!url || status === "loading" || status === "ready") return;
    status = "loading";
    try {
      const blob = await loadConsoleAttachment(url);
      if (disposed) return;
      if (objectUrl) URL.revokeObjectURL(objectUrl);
      objectUrl = URL.createObjectURL(blob);
      status = "ready";
    } catch (error) {
      if (disposed) return;
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

  $effect(() => () => {
    disposed = true;
    if (objectUrl) URL.revokeObjectURL(objectUrl);
  });
</script>

<div class="console-attachment" bind:this={root} data-attachment-id={attachment.attachment_id}>
  {#if status === "ready" && objectUrl}
    <img src={objectUrl} alt="Tool output attachment" />
  {:else}
    <span>{statusText()}</span>
    {#if url && ["missing", "expired", "unauthorized", "network", "failed"].includes(status)}
      <button type="button" onclick={() => void load()}>Retry</button>
    {/if}
  {/if}
</div>

<style>
  .console-attachment {
    margin-top: var(--space-2);
    color: var(--text-muted);
    font-size: var(--font-size-compact);
  }
  img {
    display: block;
    max-width: min(100%, 52rem);
    max-height: 42rem;
    object-fit: contain;
    border: 1px solid var(--line);
    border-radius: 0.5rem;
  }
  button {
    margin-left: var(--space-2);
  }
</style>

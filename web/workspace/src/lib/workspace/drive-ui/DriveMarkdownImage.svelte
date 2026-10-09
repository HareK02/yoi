<script lang="ts">
  // No Markdown image triggers an automatic network request. This includes external
  // tracking pixels, API URLs and cross-Workspace images. Open an authorized node URL
  // explicitly to use the bounded raster viewer.
  let { text = '', title }: { text?: string; title?: string } = $props();
</script>
<span class="drive-inline-image" {title}>[Image: {text || 'unnamed'} — open its Drive URL to view]</span>
<style>
  .drive-inline-image { color: var(--text-muted); overflow-wrap: anywhere; }
</style>

<script lang="ts">
  // Markdown画像は外部tracking pixel、API、別Workspaceを含めて自動取得しない。
  // 代替textだけを示し、明示的に開いた認証済みDrive URLでbounded raster表示を行う。
  let { text = '', title }: { text?: string; title?: string } = $props();
</script>
<span class="drive-inline-image" {title}>[Image: {text || 'unnamed'} — open its Drive URL to view]</span>
<style>
  .drive-inline-image { color: var(--text-muted); overflow-wrap: anywhere; }
</style>

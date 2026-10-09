<script lang="ts">
  import { setContext } from 'svelte';
  import DocumentMarkdown from '#lib/workspace/markdown/DocumentMarkdown.svelte';
  import { MARKDOWN_WORKSPACE_CONTEXT, type MarkdownWorkspace } from '#lib/workspace/markdown/link-policy.ts';
  import DriveMarkdownImage from './DriveMarkdownImage.svelte';
  let { text, workspaceId }: { text: string; workspaceId: string } = $props();
  setContext<MarkdownWorkspace>(MARKDOWN_WORKSPACE_CONTEXT, () => workspaceId);
</script>
<DocumentMarkdown {text} imageRenderer={DriveMarkdownImage} />

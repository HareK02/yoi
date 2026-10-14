<script lang="ts">
  let { content, onConstruct }: {
    content: { label: string; message?: string; serializedError?: boolean };
    onConstruct?: (label: string) => void;
  } = $props();

  // Model PageContent's constructor check when an ApiResult has null data.
  function construct() {
    onConstruct?.(content.label);
    if (content.message) {
      if (content.serializedError) throw { message: content.message };
      throw new Error(content.message);
    }
  }
  construct();
</script>

<p>{content.label}</p>
<label>Draft<input aria-label="Draft" /></label>

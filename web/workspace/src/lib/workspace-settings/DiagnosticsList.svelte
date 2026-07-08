<script lang="ts">
  import { diagnosticLabel, type Diagnostic } from './model';

  type Props = {
    diagnostics: Diagnostic[];
  };

  let { diagnostics }: Props = $props();
</script>

{#if diagnostics.length > 0}
  <ul class="settings-diagnostics-list">
    {#each diagnostics as diagnostic}
      <li class={diagnostic.severity}>
        <strong>{diagnosticLabel(diagnostic)}</strong>
        <span>{diagnostic.message}</span>
      </li>
    {/each}
  </ul>
{/if}

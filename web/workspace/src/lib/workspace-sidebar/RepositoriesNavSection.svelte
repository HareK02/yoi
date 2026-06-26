<script lang="ts">
  import type { WorkspaceResponse } from './types';

  type Props = {
    workspace: WorkspaceResponse | null;
    currentPath?: string;
  };

  let { workspace, currentPath = '/' }: Props = $props();
</script>

<section class="nav-section" aria-labelledby="repositories-heading">
  <div class="section-heading-row">
    <h2 id="repositories-heading">repositories</h2>
    <span class="section-count">1</span>
  </div>

  <ul class="nav-list" aria-label="Repositories">
    <li>
      <a class="nav-item" class:active={currentPath.startsWith('/repositories')} href="/repositories/local">
        <span class="item-title">{workspace?.display_name ?? 'local workspace'}</span>
        <span class="item-meta">local repository · read-only</span>
      </a>
    </li>
  </ul>
</section>

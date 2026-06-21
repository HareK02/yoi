<script lang="ts">
  import type { WorkspaceResponse } from './types';

  type Props = {
    workspace: WorkspaceResponse | null;
  };

  let { workspace }: Props = $props();
</script>

<section class="nav-section" aria-labelledby="repositories-heading">
  <div class="section-heading-row">
    <h2 id="repositories-heading">repositories</h2>
    <span class="section-count">1</span>
  </div>

  <ul class="nav-list" aria-label="Repositories">
    <li class="nav-item active">
      <span class="item-title">{workspace?.display_name ?? 'local workspace'}</span>
      <span class="item-meta">local project records</span>
    </li>
  </ul>

  <p class="section-note">
    Repository API is not wired yet; this placeholder keeps the navigation seam
    ready without adding repository authority.
  </p>
</section>

<style>
  .nav-section {
    display: grid;
    gap: 10px;
  }

  .section-heading-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
  }

  h2 {
    margin: 0;
    color: #94a3b8;
    font-size: 0.72rem;
    font-weight: 800;
    letter-spacing: 0.14em;
    text-transform: uppercase;
  }

  .section-count {
    border: 1px solid rgba(148, 163, 184, 0.22);
    border-radius: 999px;
    color: #cbd5e1;
    font-size: 0.72rem;
    line-height: 1;
    padding: 4px 8px;
  }

  .nav-list {
    display: grid;
    gap: 6px;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .nav-item {
    display: grid;
    gap: 3px;
    border: 1px solid rgba(148, 163, 184, 0.18);
    border-radius: 14px;
    background: rgba(15, 23, 42, 0.64);
    padding: 10px 12px;
    min-width: 0;
  }

  .nav-item.active {
    border-color: rgba(56, 189, 248, 0.36);
    background: rgba(14, 165, 233, 0.1);
  }

  .item-title,
  .item-meta {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .item-title {
    color: #e2e8f0;
    font-weight: 700;
  }

  .item-meta,
  .section-note {
    color: #94a3b8;
    font-size: 0.78rem;
  }

  .section-note {
    margin: 0;
    line-height: 1.45;
  }
</style>

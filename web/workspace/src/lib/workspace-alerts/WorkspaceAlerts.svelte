<script lang="ts">
  import { dismissWorkspaceAlert, workspaceAlerts, type WorkspaceAlertLevel } from './store';

  function label(level: WorkspaceAlertLevel): string {
    switch (level) {
      case 'success': return 'Success';
      case 'info': return 'Info';
      case 'warning': return 'Warning';
      case 'error': return 'Error';
      case 'system': return 'System';
      case 'debug': return 'Debug';
    }
  }
</script>

<div class="workspace-alerts" aria-live="polite" aria-atomic="false">
  {#each $workspaceAlerts as alert (alert.id)}
    <article class={`workspace-alert ${alert.level}`} role={alert.level === 'error' ? 'alert' : 'status'}>
      <div class="workspace-alert-icon" aria-hidden="true">
        {#if alert.level === 'success'}
          <svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="10" /><path d="m9 12 2 2 4-4" /></svg>
        {:else if alert.level === 'info'}
          <svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="10" /><path d="M12 16v-4" /><path d="M12 8h.01" /></svg>
        {:else if alert.level === 'warning' || alert.level === 'error'}
          <svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="10" /><line x1="12" x2="12" y1="8" y2="12" /><line x1="12" x2="12.01" y1="16" y2="16" /></svg>
        {:else if alert.level === 'system'}
          <svg viewBox="0 0 24 24"><path d="M9.671 4.136a2.34 2.34 0 0 1 4.659 0 2.34 2.34 0 0 0 3.319 1.915 2.34 2.34 0 0 1 2.33 4.033 2.34 2.34 0 0 0 0 3.831 2.34 2.34 0 0 1-2.33 4.033 2.34 2.34 0 0 0-3.319 1.915 2.34 2.34 0 0 1-4.659 0 2.34 2.34 0 0 0-3.32-1.915 2.34 2.34 0 0 1-2.33-4.033 2.34 2.34 0 0 0 0-3.831A2.34 2.34 0 0 1 6.35 6.051a2.34 2.34 0 0 0 3.319-1.915" /><circle cx="12" cy="12" r="3" /></svg>
        {:else}
          <svg viewBox="0 0 24 24"><path d="m8 2 1.88 1.88" /><path d="M14.12 3.88 16 2" /><path d="M9 7.13v-1a3.003 3.003 0 1 1 6 0v1" /><path d="M12 20c-3.3 0-6-2.7-6-6v-3a4 4 0 0 1 4-4h4a4 4 0 0 1 4 4v3c0 3.3-2.7 6-6 6" /><path d="M12 20v-9" /><path d="M6.53 9C4.6 8.8 3 7.1 3 5" /><path d="M6 13H2" /><path d="M3 21c0-2.1 1.7-3.9 3.8-4" /><path d="M20.97 5c0 2.1-1.6 3.8-3.5 4" /><path d="M22 13h-4" /><path d="M17.2 17c2.1.1 3.8 1.9 3.8 4" /></svg>
        {/if}
      </div>
      <div class="workspace-alert-body">
        <strong>{alert.title ?? label(alert.level)}</strong>
        <p>{alert.message}</p>
      </div>
      <button type="button" class="workspace-alert-dismiss" aria-label="Dismiss alert" onclick={() => dismissWorkspaceAlert(alert.id)}>×</button>
    </article>
  {/each}
</div>

<style>
  .workspace-alerts {
    position: fixed;
    top: 1rem;
    right: 1rem;
    z-index: 1000;
    display: grid;
    gap: 0.65rem;
    width: min(28rem, calc(100vw - 2rem));
    pointer-events: none;
  }

  .workspace-alert {
    --alert-color: var(--text);
    display: grid;
    grid-template-columns: auto minmax(0, 1fr) auto;
    gap: 0.7rem;
    align-items: start;
    padding: 0.8rem 0.85rem;
    border: 1px solid color-mix(in srgb, var(--alert-color) 45%, var(--line));
    border-radius: 0.85rem;
    background: color-mix(in srgb, var(--alert-color) 12%, var(--bg-raised) 88%);
    color: var(--text);
    box-shadow: 0 18px 50px rgb(0 0 0 / 0.28);
    pointer-events: auto;
  }

  .workspace-alert.success { --alert-color: oklch(70% 0.18 145); }
  .workspace-alert.info { --alert-color: oklch(68% 0.16 245); }
  .workspace-alert.warning { --alert-color: oklch(78% 0.18 85); }
  .workspace-alert.error { --alert-color: oklch(62% 0.2 30); }
  .workspace-alert.system { --alert-color: oklch(96% 0.01 260); }
  .workspace-alert.debug { --alert-color: oklch(68% 0.18 305); }

  .workspace-alert-icon {
    color: var(--alert-color);
    margin-top: 0.1rem;
  }

  .workspace-alert-icon svg {
    display: block;
    width: 1.15rem;
    height: 1.15rem;
    fill: none;
    stroke: currentColor;
    stroke-linecap: round;
    stroke-linejoin: round;
    stroke-width: 2;
  }

  .workspace-alert-body {
    min-width: 0;
  }

  .workspace-alert-body strong {
    display: block;
    margin-bottom: 0.15rem;
    color: var(--alert-color);
    font-size: 0.82rem;
    letter-spacing: 0.02em;
    text-transform: uppercase;
  }

  .workspace-alert-body p {
    margin: 0;
    overflow-wrap: anywhere;
    color: var(--text);
    font-size: 0.9rem;
    line-height: 1.35;
  }

  .workspace-alert-dismiss {
    width: 1.5rem;
    height: 1.5rem;
    padding: 0;
    border: 0;
    border-radius: 999px;
    background: transparent;
    color: var(--text-muted);
    cursor: pointer;
    font-size: 1.2rem;
    line-height: 1;
  }

  .workspace-alert-dismiss:hover,
  .workspace-alert-dismiss:focus-visible {
    background: color-mix(in srgb, var(--alert-color) 18%, transparent);
    color: var(--text);
  }
</style>

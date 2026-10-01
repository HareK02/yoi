<script lang="ts">
  import { onDestroy, type Snippet } from 'svelte';
  import Bevel from '$lib/workspace/ui/Bevel.svelte';
  import SidebarToggleIcon from './SidebarToggleIcon.svelte';
  import './sidebar.css';

  type Props = {
    children: Snippet<[]>;
    mode: 'pinned' | 'hover';
    open: boolean;
    mobile: boolean;
    onModeChange: (mode: 'pinned' | 'hover') => void;
    onOpenChange: (open: boolean) => void;
  };

  let { children, mode, open, mobile, onModeChange, onOpenChange }: Props = $props();
  let frame: HTMLElement;
  let pointerInside = false;
  let focusInside = false;
  let closeTimer: ReturnType<typeof setTimeout> | undefined;

  function clearCloseTimer() {
    clearTimeout(closeTimer);
    closeTimer = undefined;
  }

  function scheduleClose() {
    clearCloseTimer();
    if (mobile || pointerInside || focusInside) return;
    closeTimer = setTimeout(() => {
      if (!mobile && mode === 'hover' && !pointerInside && !focusInside) onOpenChange(false);
      closeTimer = undefined;
    }, 180);
  }

  function enter(event: PointerEvent) {
    if (mobile || event.pointerType !== 'mouse') return;
    pointerInside = true;
    clearCloseTimer();
    if (mode === 'hover') onOpenChange(true);
  }

  function leave(event: PointerEvent) {
    if (event.pointerType !== 'mouse') return;
    pointerInside = false;
    scheduleClose();
  }

  function focusIn() {
    focusInside = true;
    clearCloseTimer();
    if (!mobile && mode === 'hover') onOpenChange(true);
  }

  function focusOut(event: FocusEvent) {
    if (event.relatedTarget instanceof Node && frame.contains(event.relatedTarget)) return;
    focusInside = false;
    scheduleClose();
  }

  function toggleMode(event: MouseEvent) {
    const next = mode === 'pinned' ? 'hover' : 'pinned';
    // A pointer click should not leave keyboard focus holding the preview open.
    if (event.detail > 0) (event.currentTarget as HTMLButtonElement).blur();
    onModeChange(next);
    if (!mobile) onOpenChange(next === 'hover');
    scheduleClose();
  }

  $effect(() => {
    mobile;
    pointerInside = false;
    focusInside = false;
    clearCloseTimer();
  });
  onDestroy(clearCloseTimer);
</script>

<aside
  class="sidebar-frame"
  class:folded={!open}
  class:hover-mode={mode === 'hover'}
  aria-label="Sidebar"
  bind:this={frame}
  onfocusin={focusIn}
  onfocusout={focusOut}
>
  <Bevel as="div" class="sidebar-frame__bevel" top={false} bottom={false} left={false}>
    <div class="sidebar-frame__surface">
      <div
        class="sidebar-hover-region"
        onpointerenter={enter}
        onpointerleave={leave}
        onpointercancel={leave}
      >
        <div class="sidebar-frame-content" inert={!open} aria-hidden={!open}>
          {@render children()}
        </div>
      </div>

      <div class="sidebar-control-row">
        <button
          class="sidebar-fold-button"
          type="button"
          aria-label={mode === 'pinned' ? 'Unpin sidebar' : 'Pin sidebar'}
          aria-pressed={mode === 'pinned'}
          title={mode === 'pinned' ? 'Always visible — switch to show on hover' : 'Show on hover — pin sidebar'}
          onclick={toggleMode}
        >
          <SidebarToggleIcon open={mode === 'pinned'} />
        </button>
      </div>
    </div>
  </Bevel>
</aside>

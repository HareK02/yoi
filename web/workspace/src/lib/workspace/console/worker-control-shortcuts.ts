export type WorkerControlShortcut = "pause" | "cancel" | "resume";

export type WorkerControlShortcutEvent = {
  key: string;
  ctrlKey?: boolean;
  metaKey?: boolean;
  altKey?: boolean;
  shiftKey?: boolean;
  repeat?: boolean;
  isComposing?: boolean;
};

export type WorkerControlShortcutState = {
  protocolOpen: boolean;
  running: boolean;
  paused: boolean;
  composerFocused: boolean;
  draftBlank: boolean;
  editableTarget: boolean;
  hasSelection: boolean;
};

/** Resolve the TUI-compatible Worker control shortcut without side effects. */
export function resolveWorkerControlShortcut(
  event: WorkerControlShortcutEvent,
  state: WorkerControlShortcutState,
): WorkerControlShortcut | null {
  if (!state.protocolOpen || event.repeat || event.isComposing) return null;

  if (
    event.key === "Enter" && state.paused && state.composerFocused &&
    state.draftBlank && !event.ctrlKey && !event.metaKey && !event.altKey &&
    !event.shiftKey
  ) {
    return "resume";
  }

  if (
    !event.ctrlKey || event.metaKey || event.altKey || event.shiftKey ||
    state.editableTarget || state.hasSelection
  ) {
    return null;
  }

  switch (event.key.toLowerCase()) {
    case "c":
      return state.running ? "pause" : null;
    case "x":
      return state.running || state.paused ? "cancel" : null;
    default:
      return null;
  }
}

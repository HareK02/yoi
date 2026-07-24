export type ChatSubmitMode = "mod-enter" | "enter";
export type ChatSubmitModKey = "meta" | "ctrl" | "auto";

export type ChatSubmitOptions = {
  onSubmit: (value: string, ctx: { target: HTMLTextAreaElement }) => void;
  mode?: ChatSubmitMode;
  modKey?: ChatSubmitModKey;
  allowEmptySubmit?: boolean;
  stopPropagation?: boolean;
  enabled?: boolean;
};

type NormalizedChatSubmitOptions = Required<ChatSubmitOptions>;

export type ChatSubmitKeyEventLike = {
  key: string;
  shiftKey?: boolean;
  metaKey?: boolean;
  ctrlKey?: boolean;
  repeat?: boolean;
  isComposing?: boolean;
  keyCode?: number;
  which?: number;
};

function normalizeOptions(
  options: ChatSubmitOptions,
): NormalizedChatSubmitOptions {
  return {
    mode: "mod-enter",
    modKey: "auto",
    allowEmptySubmit: false,
    stopPropagation: false,
    enabled: true,
    ...options,
  };
}

function isImeEvent(event: ChatSubmitKeyEventLike): boolean {
  return event.isComposing === true ||
    event.key === "Process" ||
    event.keyCode === 229 ||
    event.which === 229;
}

function isApplePlatform(): boolean {
  if (typeof navigator === "undefined") {
    return true;
  }
  const platform = navigator.platform || "";
  const userAgent = navigator.userAgent || "";
  return /Mac|iPhone|iPad|iPod/.test(platform) ||
    /Mac|iPhone|iPad|iPod/.test(userAgent);
}

function resolveModKey(modKey: ChatSubmitModKey): "meta" | "ctrl" {
  if (modKey === "auto") {
    return isApplePlatform() ? "meta" : "ctrl";
  }
  return modKey;
}

function isModPressed(
  event: ChatSubmitKeyEventLike,
  modKey: ChatSubmitModKey,
): boolean {
  return resolveModKey(modKey) === "meta"
    ? event.metaKey === true
    : event.ctrlKey === true;
}

export function shouldSubmitChatKey(
  event: ChatSubmitKeyEventLike,
  options: Pick<NormalizedChatSubmitOptions, "mode" | "modKey" | "enabled"> & {
    isComposing?: boolean;
  },
): boolean {
  if (!options.enabled || event.repeat || event.key !== "Enter") {
    return false;
  }
  if (options.isComposing || isImeEvent(event)) {
    return false;
  }
  if (options.mode === "enter") {
    return event.shiftKey !== true;
  }
  return isModPressed(event, options.modKey);
}

export function chatSubmit(
  node: HTMLTextAreaElement,
  options: ChatSubmitOptions,
) {
  let current = normalizeOptions(options);
  let isComposing = false;

  function triggerSubmit() {
    if (!current.enabled || node.disabled || node.readOnly) {
      return;
    }
    const value = node.value ?? "";
    if (!current.allowEmptySubmit && value.trim() === "") {
      return;
    }
    current.onSubmit(value, { target: node });
  }

  function onKeyDown(event: KeyboardEvent) {
    if (isImeEvent(event)) {
      isComposing = true;
      return;
    }
    if (!shouldSubmitChatKey(event, { ...current, isComposing })) {
      return;
    }
    event.preventDefault();
    if (current.stopPropagation) {
      event.stopPropagation();
    }
    triggerSubmit();
  }

  function onKeyUp(event: KeyboardEvent) {
    if (!event.isComposing) {
      isComposing = false;
    }
  }

  function onCompositionStart() {
    isComposing = true;
  }

  function clearComposition() {
    isComposing = false;
  }

  node.addEventListener("keydown", onKeyDown);
  node.addEventListener("keyup", onKeyUp);
  node.addEventListener("compositionstart", onCompositionStart);
  node.addEventListener("compositionend", clearComposition);
  node.addEventListener("compositioncancel", clearComposition);

  return {
    update(nextOptions: ChatSubmitOptions) {
      current = normalizeOptions(nextOptions);
    },
    destroy() {
      node.removeEventListener("keydown", onKeyDown);
      node.removeEventListener("keyup", onKeyUp);
      node.removeEventListener("compositionstart", onCompositionStart);
      node.removeEventListener("compositionend", clearComposition);
      node.removeEventListener("compositioncancel", clearComposition);
    },
  };
}

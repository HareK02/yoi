export type ComposerDelivery = "submit" | "notify";

export type ComposerDeliveryState = {
  delivery: ComposerDelivery;
  workerState: string;
  protocolOpen: boolean;
  sending: boolean;
  hasText: boolean;
  hasAttachments: boolean;
};

/**
 * Resolve whether the current Composer draft can use one delivery action.
 * Immediate Submit is idle-only; Notify is running-only.
 */
export function canDeliverComposerDraft(state: ComposerDeliveryState): boolean {
  if (!state.protocolOpen || state.sending) return false;

  const hasInput = state.hasText || state.hasAttachments;
  switch (state.delivery) {
    case "submit":
      return state.workerState === "idle" && hasInput;
    case "notify":
      return state.workerState === "running" && state.hasText &&
        !state.hasAttachments;
  }
}

export function sendComposerDelivery<T>(
  state: ComposerDeliveryState,
  method: T,
  send: (method: T) => void,
): boolean {
  if (!canDeliverComposerDraft(state)) return false;
  send(method);
  return true;
}

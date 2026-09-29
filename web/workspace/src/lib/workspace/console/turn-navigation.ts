import type { SessionConversationTurn } from "$lib/generated/protocol";
import { type ConsoleTurnPreview, conversationTurnPreviews } from "./history";

export type TurnNavigationItem = ConsoleTurnPreview;

export function buildTurnNavigationItems(
  turns: readonly SessionConversationTurn[],
): TurnNavigationItem[] {
  return conversationTurnPreviews(turns);
}

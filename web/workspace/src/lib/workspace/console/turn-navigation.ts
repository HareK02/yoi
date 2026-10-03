import type { ConsoleLine } from "./model.ts";

export type ConsoleTurn = {
  id: string;
  user: string;
  assistant: string;
};

const PREVIEW_LIMIT = 1200;

function previewText(text: string): string {
  const compact = text.replace(/\s+/g, " ").trim();
  return compact.length > PREVIEW_LIMIT
    ? `${compact.slice(0, PREVIEW_LIMIT)}…`
    : compact;
}

/** One user message and its last nonempty assistant message before the next user. */
export function consoleTurns(lines: readonly ConsoleLine[]): ConsoleTurn[] {
  const turns: ConsoleTurn[] = [];
  let current: ConsoleTurn | undefined;
  for (const line of lines) {
    if (line.kind === "user") {
      current = { id: line.id, user: previewText(line.body), assistant: "" };
      turns.push(current);
    } else if (current && line.kind === "assistant") {
      const assistant = previewText(line.body);
      if (assistant) current.assistant = assistant;
    }
  }
  return turns;
}

import type { SessionConversationTurn } from "#lib/generated/protocol.ts";
import { type ConsoleTurnPreview, conversationTurnPreviews } from "./history";

export type TurnNavigationItem = ConsoleTurnPreview;

export function buildTurnNavigationItems(
  turns: readonly SessionConversationTurn[],
): TurnNavigationItem[] {
  return conversationTurnPreviews(turns);
}

import type { ReasoningConfig } from "$lib/generated/protocol";
import type { ConsoleWorkerMetadata } from "./model";
import { formatRunTokens } from "./run-status";

export function formatReasoning(reasoning: ReasoningConfig | null): string {
  if (!reasoning) return "reasoning unavailable";
  if (reasoning.kind === "effort") {
    const effort = reasoning.effort.trim();
    return effort.length > 0 ? effort : "reasoning unavailable";
  }
  if (!Number.isSafeInteger(reasoning.budget_tokens)) {
    return "reasoning unavailable";
  }
  if (reasoning.budget_tokens === -1) {
    return "dynamic token budget";
  }
  if (reasoning.budget_tokens <= 0) {
    return "reasoning unavailable";
  }
  return `${formatRunTokens(reasoning.budget_tokens)} token budget`;
}

export function formatModelSummary(
  metadata: ConsoleWorkerMetadata | null,
): string {
  if (!metadata?.model) return "Model unavailable";
  return `${metadata.model} · ${formatReasoning(metadata.reasoning)}`;
}

export function formatContextSummary(
  metadata: ConsoleWorkerMetadata | null,
): string {
  const used = metadata?.contextTokens;
  const limit = metadata?.contextWindow;
  if (
    used === null || used === undefined || limit === null ||
    limit === undefined || !Number.isSafeInteger(used) ||
    !Number.isSafeInteger(limit) || used < 0 || limit <= 0
  ) return "Context unavailable";
  const percent = Math.round((used / limit) * 100);
  if (!Number.isFinite(percent)) return "Context unavailable";
  const estimate = metadata?.contextSource === "estimated" ? "~" : "";
  return `Context ${estimate}${formatRunTokens(used)} / ${
    formatRunTokens(limit)
  } (${percent}%)`;
}

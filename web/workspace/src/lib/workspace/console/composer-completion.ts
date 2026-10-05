import { COMMANDS } from "./composer-command.ts";
import type {
  CompletionContext,
  FeatureInvocationDescriptor,
} from "#lib/generated/protocol.ts";
import { activeInvocationArgument, type SelectedInvocationOccurrence } from "./composer-invocation.ts";

export type ComposerCompletionKind = "command" | "file" | "feature" | "feature_argument";
export type ComposerCompletionToken = {
  sigil: ":" | "@" | "/" | "";
  kind: ComposerCompletionKind;
  start: number;
  end: number;
  prefix: string;
  context?: CompletionContext;
};
export type ComposerCompletionEntry = {
  value: string;
  is_dir?: boolean;
  description?: string | null;
  usage?: string | null;
  invocation?: FeatureInvocationDescriptor | null;
};
export type CompletionApplyResult = { value: string; cursor: number };
const ALIASES: Record<string, string> = { "?": "help", rollback: "rewind" };
export const COLON_COMMAND_COMPLETIONS: ComposerCompletionEntry[] = Object
  .entries(COMMANDS)
  .filter(([name]) => !ALIASES[name])
  .map(([value, spec]) => ({
    value,
    description: spec.description,
    usage: spec.usage,
  }));

export function completionTokenAt(
  value: string,
  cursor: number,
  descriptors: readonly FeatureInvocationDescriptor[] = [],
  selectedOccurrences?: readonly SelectedInvocationOccurrence[],
): ComposerCompletionToken | null {
  const boundedCursor = Math.max(0, Math.min(cursor, value.length));
  const activeArgument = activeInvocationArgument(value, boundedCursor, descriptors, selectedOccurrences);
  if (activeArgument) {
    return {
      sigil: "",
      kind: "feature_argument",
      start: activeArgument.start,
      end: activeArgument.end,
      prefix: activeArgument.prefix,
      context: {
        invocation: activeArgument.descriptor.identity,
        argument: activeArgument.argument?.name,
      },
    };
  }

  const before = value.slice(0, boundedCursor);
  const feature = /(^|\s)\/([a-z0-9_-]*)$/.exec(before);
  if (feature) {
    const prefix = feature[2] ?? "";
    const start = before.length - prefix.length - 1;
    const tail = /^[a-z0-9_-]*/.exec(value.slice(boundedCursor))?.[0] ?? "";
    return {
      sigil: "/",
      kind: "feature",
      start,
      end: boundedCursor + tail.length,
      prefix,
    };
  }

  const match = /(^|\s)([:@])([^\s\uFFF9\uFFFB]*)$/.exec(before);
  if (!match) return null;
  const sigil = match[2] as ":" | "@";
  const prefix = match[3] ?? "";
  const start = before.length - prefix.length - 1;
  // Commands are only recognized at the start of the draft; their arguments
  // must not accidentally become commands or file-reference completions.
  if (sigil === ":" && value.slice(0, start).trim()) return null;
  if (sigil === "@" && value.trimStart().startsWith(":")) return null;
  const tail = /^[^\s\uFFF9\uFFFB]*/.exec(value.slice(boundedCursor))?.[0] ??
    "";
  return {
    sigil,
    kind: sigil === ":" ? "command" : "file",
    start,
    end: boundedCursor + tail.length,
    prefix,
  };
}

export function localCommandCompletions(
  prefix: string,
): ComposerCompletionEntry[] {
  return COLON_COMMAND_COMPLETIONS.filter((entry) =>
    entry.value.startsWith(prefix) || Object.entries(ALIASES).some(
      ([alias, name]) => name === entry.value && alias.startsWith(prefix),
    )
  );
}

export function completionSelection(
  selected: number | null,
  count: number,
  direction: 1 | -1,
): number | null {
  if (!count) return null;
  return selected === null
    ? (direction === 1 ? 0 : count - 1)
    : (selected + direction + count) % count;
}

export function applyCompletion(
  value: string,
  token: ComposerCompletionToken,
  entry: ComposerCompletionEntry,
  action: "tab" | "accept" = "accept",
): CompletionApplyResult {
  if (token.kind === "feature") {
    const replacement = `/${entry.value}(`;
    return replaceCompletionRange(value, token, replacement);
  }
  if (token.kind === "feature_argument") {
    const argumentName = !token.context?.argument && entry.value.endsWith("=");
    const completed = entry.is_dir && !entry.value.endsWith("/") ? `${entry.value}/` : entry.value;
    const replacement = argumentName ? entry.value : JSON.stringify(completed);
    const result = replaceCompletionRange(value,
      argumentName && value[token.end] === "=" ? { ...token, end: token.end + 1 } : token,
      replacement,
    );
    // Keep directory drilling inside the quoted value instead of after its closing quote.
    if (entry.is_dir && !argumentName) result.cursor--;
    return result;
  }
  const suffix = entry.is_dir
    ? (entry.value.endsWith("/") ? "" : "/")
    : token.kind === "file" && action === "tab"
    ? ""
    : " ";
  return replaceCompletionRange(
    value,
    token,
    `${token.sigil}${entry.value}${suffix}`,
    suffix === " ",
  );
}

function replaceCompletionRange(
  value: string,
  token: ComposerCompletionToken,
  replacement: string,
  consumeSpace = false,
): CompletionApplyResult {
  const restStart = consumeSpace && value[token.end] === " "
    ? token.end + 1
    : token.end;
  return {
    value: `${value.slice(0, token.start)}${replacement}${value.slice(restStart)}`,
    cursor: token.start + replacement.length,
  };
}

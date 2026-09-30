import { COMMANDS } from "./composer-command.ts";

export type ComposerCompletionKind = "command" | "file";
export type ComposerCompletionToken = {
  sigil: ":" | "@";
  kind: ComposerCompletionKind;
  start: number;
  end: number;
  prefix: string;
};
export type ComposerCompletionEntry = {
  value: string;
  is_dir?: boolean;
  description?: string;
  usage?: string;
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
): ComposerCompletionToken | null {
  const boundedCursor = Math.max(0, Math.min(cursor, value.length));
  const before = value.slice(0, boundedCursor);
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
  const suffix = entry.is_dir
    ? (entry.value.endsWith("/") ? "" : "/")
    : token.kind === "file" && action === "tab"
    ? ""
    : " ";
  const replacement = `${token.sigil}${entry.value}${suffix}`;
  const restStart = suffix === " " && value[token.end] === " "
    ? token.end + 1
    : token.end;
  return {
    value: `${value.slice(0, token.start)}${replacement}${
      value.slice(restStart)
    }`,
    cursor: token.start + replacement.length,
  };
}

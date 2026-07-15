export type ComposerCompletionKind = "command" | "file" | "knowledge";

export type ComposerCompletionToken = {
  sigil: ":" | "@" | "#";
  kind: ComposerCompletionKind;
  start: number;
  end: number;
  prefix: string;
};

export type ComposerCompletionEntry = {
  value: string;
  is_dir?: boolean;
  description?: string;
};

export type CompletionApplyResult = {
  value: string;
  cursor: number;
};

export const COLON_COMMAND_COMPLETIONS: ComposerCompletionEntry[] = [
  { value: "help", description: "Show commands" },
  { value: "noop", description: "No-op" },
  { value: "compact", description: "Compact Worker context" },
  { value: "rewind", description: "List rewind targets" },
  { value: "rollback", description: "Alias for rewind" },
  { value: "peer", description: "Register metadata peer" },
  { value: "system", description: "Send system notification" },
];

export function completionTokenAt(
  value: string,
  cursor: number,
): ComposerCompletionToken | null {
  const boundedCursor = Math.max(0, Math.min(cursor, value.length));
  const before = value.slice(0, boundedCursor);
  const match = /(^|\s)([:@#])([^\s]*)$/.exec(before);
  if (!match) {
    return null;
  }
  const sigil = match[2] as ComposerCompletionToken["sigil"];
  const prefix = match[3] ?? "";
  const tokenStart = before.length - prefix.length - sigil.length;
  return {
    sigil,
    kind: completionKindForSigil(sigil),
    start: tokenStart,
    end: boundedCursor,
    prefix,
  };
}

export function localCommandCompletions(
  prefix: string,
): ComposerCompletionEntry[] {
  const normalized = prefix.toLowerCase();
  return COLON_COMMAND_COMPLETIONS.filter((entry) =>
    entry.value.toLowerCase().startsWith(normalized)
  );
}

export function applyCompletion(
  value: string,
  token: ComposerCompletionToken,
  entry: ComposerCompletionEntry,
): CompletionApplyResult {
  const suffix = entry.is_dir ? "/" : " ";
  const replacement = `${token.sigil}${entry.value}${suffix}`;
  const restStart = !entry.is_dir && value[token.end] === " "
    ? token.end + 1
    : token.end;
  const next = `${value.slice(0, token.start)}${replacement}${
    value.slice(restStart)
  }`;
  const cursor = token.start + replacement.length;
  return { value: next, cursor };
}

function completionKindForSigil(
  sigil: ComposerCompletionToken["sigil"],
): ComposerCompletionKind {
  switch (sigil) {
    case ":":
      return "command";
    case "@":
      return "file";
    case "#":
      return "knowledge";
  }
}

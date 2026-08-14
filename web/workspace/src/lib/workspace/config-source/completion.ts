import type { CompletionResult } from "@codemirror/autocomplete";

export type ConfigSourceCompletionResult = {
  from: number;
  items: ConfigSourceCompletionItem[];
};

type ConfigSourceCompletionItem = {
  label: string;
  kind: string;
  detail: string | null;
  priority: number;
};

export function toCodeMirrorCompletion(
  source: string,
  result: ConfigSourceCompletionResult | null,
): CompletionResult | null {
  if (!result) return null;
  return {
    from: utf8ByteOffsetToUtf16(source, result.from),
    options: result.items.map((item) => ({
      label: item.label,
      type: item.kind,
      detail: item.detail ?? undefined,
      boost: item.priority,
    })),
  };
}

function utf8ByteOffsetToUtf16(source: string, byteOffset: number): number {
  if (!Number.isSafeInteger(byteOffset) || byteOffset < 0) {
    throw new RangeError(
      "completion byte offset must be a non-negative integer",
    );
  }

  let bytes = 0;
  let utf16 = 0;
  for (const character of source) {
    if (bytes === byteOffset) return utf16;
    const codePoint = character.codePointAt(0)!;
    bytes += codePoint <= 0x7f
      ? 1
      : codePoint <= 0x7ff
      ? 2
      : codePoint <= 0xffff
      ? 3
      : 4;
    utf16 += character.length;
    if (bytes > byteOffset) {
      throw new RangeError("completion byte offset splits a UTF-8 code point");
    }
  }
  if (bytes === byteOffset) return utf16;
  throw new RangeError("completion byte offset is outside the source");
}

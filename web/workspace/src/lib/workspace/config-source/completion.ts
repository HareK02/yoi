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
  result: ConfigSourceCompletionResult | null,
): CompletionResult | null {
  if (!result) return null;
  return {
    from: result.from,
    options: result.items.map((item) => ({
      label: item.label,
      type: item.kind,
      detail: item.detail ?? undefined,
      boost: item.priority,
    })),
  };
}

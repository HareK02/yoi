import type { FailureReceipt } from "./receive.ts";

export type FailureNoticeModel = {
  explanation: string;
  supplements: string[];
  compensation: string[];
  details: { label: string; value: string }[];
};

const FALLBACKS = {
  unsupported:
    "The server returned an unsupported failure contract. The operation result has not been confirmed.",
  invalid:
    "The server response could not be interpreted safely. The operation result has not been confirmed.",
  too_large:
    "The server response exceeded the safe size limit. The operation result has not been confirmed.",
  transport:
    "The response could not be received. The operation result has not been confirmed.",
  http:
    "The request did not return a supported failure response. The operation result has not been confirmed.",
} as const;

// 原因の説明/秘匿はServerの公開projectionが所有する。UIでreasonを翻訳したり
// details/sourceを走査しない。相関参照は問い合わせ用で、再送keyではない。
export function failureNotice<D, E = undefined>(
  receipt: FailureReceipt<D, E>,
): FailureNoticeModel {
  if (receipt.kind !== "problem") {
    return {
      explanation: FALLBACKS[receipt.kind],
      supplements: [],
      compensation: [],
      details: [],
    };
  }
  const failure = receipt.problem.yoi_failure;
  const details = [
    { label: "Cause", value: `${failure.domain} / ${failure.reason}` },
    ...failure.context.operations.map((operation) => ({
      label: "Operation / stage",
      value: `${operation.operation} / ${operation.stage}`,
    })),
    ...(failure.context.correlation
      ? [{ label: "Support correlation", value: failure.context.correlation }]
      : []),
  ];
  return {
    explanation: failure.explanation,
    // 同義wrapperをtitle/detail/explanationの三重表示にしない。
    supplements: [
      ...new Set(failure.context.diagnostics.map((item) => item.explanation)),
      ...(failure.context.supplementary_omitted
        ? ["Some additional information was omitted by the server."]
        : []),
    ].filter((text) => text !== failure.explanation),
    compensation: [],
    details,
  };
}

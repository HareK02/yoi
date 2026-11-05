import { FAILURE_LIMITS } from "#lib/generated/error-contract.ts";
import type {
  FailureContext,
  FailureEnvelope,
  ProblemDetails,
} from "#lib/generated/error-contract.ts";
import {
  readBoundedText,
  ResponseByteLimitError,
  ResponseTextDecodeError,
} from "#lib/workspace/api/http.ts";

// domain固有detailsの検証は公開schemaを所有するadapterへ渡す。
// この受信結果は操作結果でも、別audienceへの再公開の許可でもない。
export type DetailsDecode<D> =
  | { kind: "known"; details: D }
  | { kind: "unsupported" }
  | { kind: "invalid" };
export type DetailsDecoder<D> = (
  envelope: FailureEnvelope<unknown>,
) => DetailsDecode<D>;

export type FailureReceipt<D> =
  | { kind: "problem"; problem: ProblemDetails<D> }
  | { kind: "unsupported" }
  | { kind: "invalid" }
  | { kind: "too_large" }
  | { kind: "transport" }
  | { kind: "http"; status: number };

class InvalidFailure extends Error {}
const invalid = (): never => {
  throw new InvalidFailure();
};
const utf8 = new TextEncoder();

export function failureRecord(value: unknown): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return invalid();
  }
  return value as Record<string, unknown>;
}

export function failureString(
  value: unknown,
  max: number = FAILURE_LIMITS.explanationBytes,
): string {
  if (
    typeof value !== "string" || value.length === 0 || value.includes("\0") ||
    utf8.encode(value).length > max
  ) {
    return invalid();
  }
  return value;
}
export function failureIdentifier(value: unknown): string {
  const text = failureString(value, FAILURE_LIMITS.identifierBytes);
  if (/[\x00-\x1f\x7f-\x9f]/u.test(text)) return invalid();
  return text;
}
function integer(value: unknown): number {
  if (
    typeof value !== "number" || !Number.isInteger(value) || value < 1 ||
    value > 65535
  ) return invalid();
  return value;
}
function list<T>(value: unknown, max: number, parse: (v: unknown) => T): T[] {
  if (!Array.isArray(value) || value.length > max) return invalid();
  return value.map(parse);
}
function optionalIdentifier(value: unknown): string | null {
  return value == null ? null : failureIdentifier(value);
}
function context(value: unknown): FailureContext {
  const raw = failureRecord(value);
  if (typeof raw.supplementary_omitted !== "boolean") return invalid();
  return {
    operations: list(raw.operations, FAILURE_LIMITS.contexts, (v) => {
      const op = failureRecord(v);
      return {
        operation: failureIdentifier(op.operation),
        stage: failureIdentifier(op.stage),
        operation_ref: optionalIdentifier(op.operation_ref),
      };
    }),
    causes: list(raw.causes, FAILURE_LIMITS.causes, (v) => {
      const cause = failureRecord(v);
      return {
        domain: failureIdentifier(cause.domain),
        reason: failureIdentifier(cause.reason),
        correlation: failureIdentifier(cause.correlation),
      };
    }),
    diagnostics: list(raw.diagnostics, FAILURE_LIMITS.diagnostics, (v) => {
      const diagnostic = failureRecord(v);
      return {
        code: failureIdentifier(diagnostic.code),
        explanation: failureString(diagnostic.explanation),
      };
    }),
    correlation: optionalIdentifier(raw.correlation),
    supplementary_omitted: raw.supplementary_omitted,
  };
}

// JSON.parse前に深さを制限する。重複keyはlast-winsで必須契約を上書きさせない。
function parseBounded(text: string): unknown {
  if (utf8.encode(text).length > FAILURE_LIMITS.bytes) {
    throw new ResponseByteLimitError();
  }
  const stack: (Set<string> | null)[] = [];
  for (let i = 0; i < text.length; i++) {
    const char = text[i];
    if (char === '"') {
      const start = i;
      for (i++; i < text.length; i++) {
        if (text[i] === "\\") i++;
        else if (text[i] === '"') break;
      }
      const token: unknown = JSON.parse(text.slice(start, i + 1));
      if (typeof token !== "string" || /[\uD800-\uDFFF]/u.test(token)) {
        return invalid();
      }
      let next = i + 1;
      while (/\s/u.test(text[next] ?? "") && next < text.length) next++;
      if (text[next] === ":") {
        const keys = stack.at(-1);
        if (!keys || keys.has(token)) return invalid();
        keys.add(token);
      }
    } else if (char === "{" || char === "[") {
      stack.push(char === "{" ? new Set<string>() : null);
      if (stack.length > FAILURE_LIMITS.depth) return invalid();
    } else if (char === "}" || char === "]") {
      if (!stack.length) return invalid();
      stack.pop();
    }
  }
  return JSON.parse(text) as unknown;
}

export function decodeFailure<D>(text: string, decode: DetailsDecoder<D>):
  | { kind: "known"; failure: FailureEnvelope<D> }
  | { kind: "unsupported" }
  | { kind: "invalid" }
  | { kind: "too_large" } {
  try {
    return decodeEnvelope(parseBounded(text), decode);
  } catch (error) {
    return {
      kind: error instanceof ResponseByteLimitError ? "too_large" : "invalid",
    };
  }
}
function decodeEnvelope<D>(value: unknown, decode: DetailsDecoder<D>):
  | { kind: "known"; failure: FailureEnvelope<D> }
  | { kind: "unsupported" }
  | { kind: "invalid" } {
  const raw = failureRecord(value);
  const version = integer(raw.version);
  const details_version = integer(raw.details_version);
  const required_extensions = list(
    raw.required_extensions,
    FAILURE_LIMITS.diagnostics,
    failureIdentifier,
  );
  const category = failureIdentifier(raw.category);
  const domain = failureIdentifier(raw.domain);
  const reason = failureIdentifier(raw.reason);
  const explanation = failureString(raw.explanation);
  const metadata = context(raw.context);
  if (version !== 1 || required_extensions.length !== 0) {
    return { kind: "unsupported" };
  }
  if (
    ![
      "authorization",
      "invalid_input",
      "conflict",
      "dependency",
      "internal",
      "unknown",
    ].includes(category)
  ) return { kind: "unsupported" };
  const envelope: FailureEnvelope<unknown> = {
    version,
    required_extensions,
    domain,
    reason,
    category: category as FailureEnvelope<unknown>["category"],
    details_version,
    details: raw.details,
    explanation,
    context: metadata,
  };
  const result = decode(envelope);
  return result.kind === "known"
    ? { kind: "known", failure: { ...envelope, details: result.details } }
    : result;
}

function uriReference(value: unknown): string {
  // RFC 3986のcomponentを検査するだけで、base解決/取得/正規化はしない。
  if (
    typeof value !== "string" ||
    utf8.encode(value).length > FAILURE_LIMITS.explanationBytes ||
    /%(?![0-9a-f]{2})/iu.test(value)
  ) return invalid();
  const match =
    /^(?:([a-z][a-z0-9+.-]*):)?(?:\/\/([^/?#]*))?([^?#]*)(?:\?([^#]*))?(?:#([^#]*))?$/iu
      .exec(value);
  if (!match) return invalid();
  const [, scheme, authority, path, query, fragment] = match;
  const pchar = /^[a-z0-9._~!$&'()*+,;=:@%/-]*$/iu;
  const queryChars = /^[a-z0-9._~!$&'()*+,;=:@%/?-]*$/iu;
  if (
    !pchar.test(path) || (query !== undefined && !queryChars.test(query)) ||
    (fragment !== undefined && !queryChars.test(fragment))
  ) return invalid();
  if (
    scheme === undefined && authority === undefined &&
    path.split("/")[0].includes(":")
  ) return invalid();
  if (authority !== undefined) {
    const parts = authority.split("@");
    if (
      parts.length > 2 ||
      (parts.length === 2 && !/^[a-z0-9._~!$&'()*+,;=:%-]*$/iu.test(parts[0]))
    ) return invalid();
    const hostPort = parts.at(-1)!;
    if (hostPort.startsWith("[")) {
      const literal = /^\[([^\]]+)\](?::[0-9]*)?$/u.exec(hostPort);
      if (!literal) return invalid();
      if (literal[1].startsWith("v") || literal[1].startsWith("V")) {
        if (!/^v[0-9a-f]+\.[a-z0-9._~!$&'()*+,;=:-]+$/iu.test(literal[1])) {
          return invalid();
        }
      } else {
        // URLのhost validatorだけをIPv6構文に利用し、値は一切置換しない。
        new URL(`http://[${literal[1]}]/`);
      }
    } else if (!/^[a-z0-9._~!$&'()*+,;=%-]*(?::[0-9]*)?$/iu.test(hostPort)) {
      return invalid();
    }
  }
  return value;
}

export function decodeProblem<D>(
  text: string,
  status: number,
  contentType: string,
  decode: DetailsDecoder<D>,
): FailureReceipt<D> {
  try {
    if (
      contentType.split(";")[0].trim().toLowerCase() !==
        "application/problem+json"
    ) return { kind: "http", status };
    const raw = failureRecord(parseBounded(text));
    if (
      !Number.isInteger(status) || status < 400 || status > 599 ||
      raw.status !== status
    ) return invalid();
    const type = uriReference(raw.type);
    const instance = uriReference(raw.instance);
    const title = failureString(raw.title);
    const detail = failureString(raw.detail);
    const failure = decodeEnvelope(raw.yoi_failure, decode);
    if (detail !== failureRecord(raw.yoi_failure).explanation) return invalid();
    if (failure.kind !== "known") return failure;
    return {
      kind: "problem",
      problem: {
        type,
        instance,
        status,
        title,
        detail,
        yoi_failure: failure.failure,
      },
    };
  } catch (error) {
    return {
      kind: error instanceof ResponseByteLimitError ? "too_large" : "invalid",
    };
  }
}

export async function receiveProblem<D>(
  response: Response,
  decode: DetailsDecoder<D>,
): Promise<FailureReceipt<D>> {
  const contentType = response.headers.get("content-type") ?? "";
  if (
    contentType.split(";")[0].trim().toLowerCase() !==
      "application/problem+json"
  ) {
    await response.body?.cancel().catch(() => {});
    return { kind: "http", status: response.status };
  }
  let text: string;
  try {
    text = await readBoundedText(response, FAILURE_LIMITS.bytes);
  } catch (error) {
    return {
      kind: error instanceof ResponseByteLimitError
        ? "too_large"
        : error instanceof ResponseTextDecodeError
        ? "invalid"
        : "transport",
    };
  }
  return decodeProblem(text, response.status, contentType, decode);
}

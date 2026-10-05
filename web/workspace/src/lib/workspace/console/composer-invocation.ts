import type {
  FeatureInvocation,
  FeatureInvocationDescriptor,
  InvocationArgumentDescriptor,
  InvocationArgumentValue,
  InvocationValue,
  Segment,
} from "#lib/generated/protocol.ts";

export type ActiveInvocationArgument = {
  descriptor: FeatureInvocationDescriptor;
  argument: InvocationArgumentDescriptor | null;
  start: number;
  end: number;
  prefix: string;
};

export function quoteInvocationString(value: string): string {
  return JSON.stringify(value);
}

export function invocationInput(invocation: FeatureInvocation): string {
  const argumentsText = invocation.arguments.map((argument) =>
    `${argument.name}=${invocationValueInput(argument.value)}`
  ).join(", ");
  return `/${invocation.name}(${argumentsText})`;
}

function invocationValueInput(value: InvocationValue): string {
  switch (value.kind) {
    case "string":
      return quoteInvocationString(value.value);
    case "integer":
    case "boolean":
      return String(value.value);
  }
}

export type SelectedInvocationOccurrence = {
  from: number;
  descriptor: FeatureInvocationDescriptor;
};

export function activeInvocationArgument(
  value: string,
  cursor: number,
  descriptors: readonly FeatureInvocationDescriptor[],
  selectedOccurrences?: readonly SelectedInvocationOccurrence[],
): ActiveInvocationArgument | null {
  const bounded = Math.max(0, Math.min(cursor, value.length));
  const occurrences = selectedOccurrences ?? activeInvocationOccurrences(value, bounded, descriptors);
  for (const { from: slash, descriptor } of occurrences) {
    if (slash < 0 || slash >= bounded) continue;
    const tail = value.slice(slash, bounded);
    if (![descriptor.name, ...descriptor.aliases].some((name) => tail.startsWith(`/${name}(`))) continue;
    const open = tail.indexOf("(");
    const argumentsText = tail.slice(open + 1);
    if (hasUnquotedClose(argumentsText)) continue;
    const currentOffset = lastUnquotedComma(argumentsText) + 1;
    const rawCurrent = argumentsText.slice(currentOffset);
    const leading = rawCurrent.length - rawCurrent.trimStart().length;
    const current = rawCurrent.trimStart();
    const equal = unquotedEqual(current);
    if (equal >= 0) {
      const name = current.slice(0, equal).trim();
      const argument = descriptor.arguments.find((candidate) => candidate.name === name) ?? null;
      let valueOffset = equal + 1;
      while (/\s/.test(current[valueOffset] ?? "")) valueOffset++;
      const prefix = invocationCompletionPrefix(current.slice(valueOffset));
      if (prefix === null) return null;
      return {
        descriptor,
        argument,
        start: slash + open + 1 + currentOffset + leading + valueOffset,
        end: invocationValueEnd(value, bounded, slash + open + 1 + currentOffset + leading + valueOffset),
        prefix,
      };
    }
    const position = countPositionalArguments(argumentsText.slice(0, currentOffset));
    const positional = descriptor.arguments.find((candidate) => candidate.position === position) ?? null;
    const looksLikeArgumentName = /^[a-z][a-z0-9_]*$/.test(current) &&
      descriptor.arguments.some((candidate) => candidate.name.startsWith(current));
    const prefix = invocationCompletionPrefix(current);
    if (prefix === null) return null;
    return {
      descriptor,
      argument: looksLikeArgumentName ? null : positional,
      start: slash + open + 1 + currentOffset + leading,
      end: looksLikeArgumentName
        ? bounded + (/^[a-z0-9_]*/.exec(value.slice(bounded))?.[0].length ?? 0)
        : invocationValueEnd(value, bounded, slash + open + 1 + currentOffset + leading),
      prefix,
    };
  }
  return null;
}

/** Scan forward and skip entire quoted argument regions, never treating a slash
 * inside an unfinished outer value as a new call. The editor supplies selected
 * occurrence anchors instead of relying on descriptor-only discovery.
 */
function activeInvocationOccurrences(
  value: string,
  cursor: number,
  descriptors: readonly FeatureInvocationDescriptor[],
): SelectedInvocationOccurrence[] {
  let active: SelectedInvocationOccurrence | null = null;
  scanEachUnquoted(value.slice(0, cursor), (char, slash) => {
    if (active) {
      if (char === ")") active = null;
      return;
    }
    if (char !== "/" || (slash > 0 && !/\s/.test(value[slash - 1]))) return;
    const descriptor = descriptors.find((candidate) =>
      [candidate.name, ...candidate.aliases].some((name) => value.startsWith(`/${name}(`, slash))
    );
    if (descriptor) active = { from: slash, descriptor };
  });
  return active ? [active] : [];
}

function invocationCompletionPrefix(value: string): string | null {
  let prefix = value;
  if (value.startsWith('"')) {
    try {
      prefix = JSON.parse(value.endsWith('"') ? value : `${value}"`);
    } catch {
      prefix = value.slice(1);
    }
  }
  try {
    return validInvocationString(prefix);
  } catch {
    // Never send a completion prefix that Rust's JSON reader cannot decode.
    return null;
  }
}

function invocationValueEnd(text: string, cursor: number, start: number): number {
  try {
    const parsed = parseRawValue(text, start);
    // Without a complete, valid token the suffix is untouched user input.
    return parsed && parsed.end >= cursor ? parsed.end : cursor;
  } catch {
    return cursor;
  }
}

export function selectedFeatureInvocationAt(
  text: string,
  start: number,
  descriptor: FeatureInvocationDescriptor,
): ParsedInvocationRange | null {
  const parsed = parseFeatureInvocation(text, start, descriptor);
  return parsed ? { start, end: parsed.end, invocation: parsed.invocation } : null;
}

export type ParsedInvocationRange = {
  start: number;
  end: number;
  invocation: FeatureInvocation;
};

/** Parse complete invocations selected from completion without promoting arbitrary text. */
export function selectedFeatureInvocationRanges(
  text: string,
  descriptors: readonly FeatureInvocationDescriptor[],
): ParsedInvocationRange[] {
  const ranges: ParsedInvocationRange[] = [];
  let search = 0;
  while (search < text.length) {
    const slash = text.indexOf("/", search);
    if (slash < 0) break;
    const descriptor = (slash === 0 || /\s/.test(text[slash - 1]))
      ? descriptors.find((candidate) =>
        [candidate.name, ...candidate.aliases].some((name) =>
          text.startsWith(`/${name}(`, slash)
        )
      )
      : undefined;
    if (!descriptor) {
      search = slash + 1;
      continue;
    }
    const parsed = parseFeatureInvocation(text, slash, descriptor);
    if (!parsed) {
      search = slash + 1;
      continue;
    }
    ranges.push({ start: slash, end: parsed.end, invocation: parsed.invocation });
    search = parsed.end;
  }
  return ranges;
}

export function finalizeSelectedFeatureInvocations(
  segments: readonly Segment[],
  descriptors: readonly FeatureInvocationDescriptor[],
): Segment[] {
  const result: Segment[] = [];
  for (const segment of segments) {
    if (segment.kind !== "text") {
      result.push(segment);
      continue;
    }
    result.push(...parseTextInvocations(segment.content, descriptors));
  }
  return coalesceText(result);
}

function parseTextInvocations(
  text: string,
  descriptors: readonly FeatureInvocationDescriptor[],
): Segment[] {
  const result: Segment[] = [];
  let emitted = 0;
  let search = 0;
  while (search < text.length) {
    const slash = text.indexOf("/", search);
    if (slash < 0) break;
    const leadingOk = slash === 0 || /\s/.test(text[slash - 1]);
    const descriptor = leadingOk
      ? descriptors.find((candidate) =>
        [candidate.name, ...candidate.aliases].some((name) =>
          text.startsWith(`/${name}(`, slash)
        )
      )
      : undefined;
    if (!descriptor) {
      search = slash + 1;
      continue;
    }
    const parsed = parseFeatureInvocation(text, slash, descriptor);
    if (!parsed) {
      search = slash + 1;
      continue;
    }
    appendText(result, text.slice(emitted, slash));
    result.push({ kind: "feature_invoke", invocation: parsed.invocation });
    emitted = parsed.end;
    search = parsed.end;
  }
  if (!result.length) return [{ kind: "text", content: text }];
  appendText(result, text.slice(emitted));
  return result;
}

function parseFeatureInvocation(
  text: string,
  start: number,
  descriptor: FeatureInvocationDescriptor,
): { invocation: FeatureInvocation; end: number } | null {
  let cursor = start + 1;
  while (/[a-z0-9_-]/.test(text[cursor] ?? "")) cursor++;
  const name = text.slice(start + 1, cursor);
  if (name !== descriptor.name && !descriptor.aliases.includes(name)) return null;
  while (/\s/.test(text[cursor] ?? "")) cursor++;
  if (text[cursor] !== "(") return null;
  cursor++;
  const values: InvocationArgumentValue[] = [];
  let position = 0;
  while (true) {
    while (/\s/.test(text[cursor] ?? "")) cursor++;
    if (text[cursor] === ")") {
      cursor++;
      break;
    }
    if (cursor >= text.length) return null;
    const itemStart = cursor;
    while (/[a-z0-9_]/.test(text[cursor] ?? "")) cursor++;
    const possibleName = text.slice(itemStart, cursor);
    while (/\s/.test(text[cursor] ?? "")) cursor++;
    let argument: InvocationArgumentDescriptor | undefined;
    if (possibleName && text[cursor] === "=") {
      argument = descriptor.arguments.find((candidate) => candidate.name === possibleName);
      if (!argument) throw new Error(`Unknown Feature argument: ${possibleName}`);
      cursor++;
      while (/\s/.test(text[cursor] ?? "")) cursor++;
    } else {
      cursor = itemStart;
      argument = descriptor.arguments.find((candidate) => candidate.position === position++);
      if (!argument) throw new Error("Too many positional Feature arguments.");
    }
    const parsed = parseRawValue(text, cursor);
    if (!parsed) return null;
    cursor = parsed.end;
    if (values.some((candidate) => candidate.name === argument.name)) {
      throw new Error(`Duplicate Feature argument: ${argument.name}`);
    }
    values.push({ name: argument.name, value: coerceValue(argument, parsed.value) });
    while (/\s/.test(text[cursor] ?? "")) cursor++;
    if (text[cursor] === ",") cursor++;
    else if (text[cursor] !== ")") return null;
  }
  for (const argument of descriptor.arguments) {
    if (argument.required && !values.some((value) => value.name === argument.name)) {
      throw new Error(`Missing required Feature argument: ${argument.name}`);
    }
  }
  return {
    invocation: {
      invocation_id: crypto.randomUUID(),
      identity: descriptor.identity,
      name,
      arguments: values,
    },
    end: cursor,
  };
}

function parseRawValue(text: string, start: number): { value: string; end: number } | null {
  if (text[start] === '"') {
    let cursor = start + 1;
    let escaped = false;
    while (cursor < text.length) {
      const char = text[cursor];
      if (!escaped && char === '"') {
        const literal = text.slice(start, cursor + 1);
        return { value: validInvocationString(JSON.parse(literal)), end: cursor + 1 };
      }
      escaped = !escaped && char === "\\";
      if (char !== "\\") escaped = false;
      cursor++;
    }
    return null;
  }
  let cursor = start;
  while (cursor < text.length && !/[\s,)]/.test(text[cursor])) cursor++;
  return cursor === start ? null : { value: validInvocationString(text.slice(start, cursor)), end: cursor };
}

/** Rust JSON strings contain Unicode scalar values, not lone UTF-16 code units.
 * JSON.parse alone permits unpaired surrogate escapes that serde_json rejects.
 */
function validInvocationString(value: string): string {
  for (let index = 0; index < value.length; index++) {
    const unit = value.charCodeAt(index);
    if (unit >= 0xD800 && unit <= 0xDBFF) {
      const next = value.charCodeAt(++index);
      if (next >= 0xDC00 && next <= 0xDFFF) continue;
    } else if (unit < 0xDC00 || unit > 0xDFFF) continue;
    throw new Error("Feature string arguments must not contain unpaired Unicode surrogates.");
  }
  return value;
}

function coerceValue(argument: InvocationArgumentDescriptor, value: string): InvocationValue {
  switch (argument.value_type.kind) {
    case "integer": {
      if (!/^-?\d+$/.test(value)) throw new Error(`Feature argument ${argument.name} must be an integer.`);
      const integer = Number(value);
      if (!Number.isSafeInteger(integer)) {
        throw new Error(`Feature argument ${argument.name} must be a safe integer.`);
      }
      if (integer < -2147483648 || integer > 2147483647) {
        throw new Error(`Feature argument ${argument.name} must be a signed 32-bit integer.`);
      }
      return { kind: "integer", value: integer };
    }
    case "boolean":
      if (value !== "true" && value !== "false") {
        throw new Error(`Feature argument ${argument.name} must be true or false.`);
      }
      return { kind: "boolean", value: value === "true" };
    case "enum":
      if (!argument.value_type.values.includes(value)) {
        throw new Error(`Feature argument ${argument.name} is not an allowed value.`);
      }
      return { kind: "string", value };
    case "string":
    case "worker_file":
    case "client_file":
      return { kind: "string", value };
  }
}

function appendText(segments: Segment[], content: string): void {
  if (!content) return;
  const prior = segments.at(-1);
  if (prior?.kind === "text") prior.content += content;
  else segments.push({ kind: "text", content });
}

function coalesceText(segments: Segment[]): Segment[] {
  const result: Segment[] = [];
  for (const segment of segments) {
    if (segment.kind === "text") appendText(result, segment.content);
    else result.push(segment);
  }
  return result;
}

// These helpers only inspect syntax before the cursor. They deliberately do not
// execute or finalize anything.
function hasUnquotedClose(value: string): boolean {
  return scanUnquoted(value, (char) => char === ")") >= 0;
}
function lastUnquotedComma(value: string): number {
  let last = -1;
  scanEachUnquoted(value, (char, index) => { if (char === ",") last = index; });
  return last;
}
function unquotedEqual(value: string): number {
  return scanUnquoted(value, (char) => char === "=");
}
function countPositionalArguments(value: string): number {
  let count = 0;
  let start = 0;
  scanEachUnquoted(value, (char, index) => {
    if (char !== ",") return;
    if (value.slice(start, index).trim() && unquotedEqual(value.slice(start, index)) < 0) count++;
    start = index + 1;
  });
  return count;
}
function scanUnquoted(value: string, predicate: (char: string) => boolean): number {
  let found = -1;
  scanEachUnquoted(value, (char, index) => {
    if (found < 0 && predicate(char)) found = index;
  });
  return found;
}
function scanEachUnquoted(value: string, visit: (char: string, index: number) => void): void {
  let quoted = false;
  let escaped = false;
  for (let index = 0; index < value.length; index++) {
    const char = value[index];
    if (!escaped && char === '"') quoted = !quoted;
    if (!quoted) visit(char, index);
    escaped = quoted && !escaped && char === "\\";
    if (char !== "\\") escaped = false;
  }
}

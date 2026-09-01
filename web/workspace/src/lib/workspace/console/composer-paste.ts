export const MAX_PLAIN_TEXT_PASTE_CHARS = 50;
export const MAX_PLAIN_TEXT_PASTE_LOGICAL_LINES = 3;

export type ComposerPastePresentation = "text" | "chip";

export interface ComposerPasteMeasurement {
  charCount: number;
  logicalLineCount: number;
  presentation: ComposerPastePresentation;
}

export interface ComposerPasteEvent {
  clipboardData: { getData(format: string): string } | null;
  preventDefault(): void;
}

/**
 * Count user-visible Unicode scalar values rather than UTF-16 code units.
 * JavaScript string iteration combines a valid surrogate pair into one value.
 */
export function unicodeScalarCount(content: string): number {
  let count = 0;
  for (const _value of content) count += 1;
  return count;
}

/**
 * Empty content has zero logical lines. Otherwise each LF, lone CR, or CRLF
 * advances one line; CRLF is one break rather than two.
 */
export function logicalLineCount(content: string): number {
  if (content.length === 0) return 0;

  let count = 1;
  for (let index = 0; index < content.length; index += 1) {
    const codeUnit = content.charCodeAt(index);
    if (codeUnit === 0x0d) {
      if (content.charCodeAt(index + 1) === 0x0a) index += 1;
      count += 1;
    } else if (codeUnit === 0x0a) {
      count += 1;
    }
  }
  return count;
}

export function measureComposerPaste(
  content: string,
): ComposerPasteMeasurement {
  const charCount = unicodeScalarCount(content);
  const logicalLineCountValue = logicalLineCount(content);
  const presentation = charCount <= MAX_PLAIN_TEXT_PASTE_CHARS &&
      logicalLineCountValue <= MAX_PLAIN_TEXT_PASTE_LOGICAL_LINES
    ? "text"
    : "chip";

  return {
    charCount,
    logicalLineCount: logicalLineCountValue,
    presentation,
  };
}

/**
 * Route a Browser paste without disrupting native short-text editing.
 *
 * Returning false means the caller must leave the event untouched, preserving
 * the Browser's cursor, selection replacement, and undo behavior. A chip paste
 * is consumed exactly once and handed to the compact-paste implementation.
 */
export function handleComposerPaste(
  event: ComposerPasteEvent,
  insertCompactPaste: (
    content: string,
    measurement: ComposerPasteMeasurement,
  ) => void,
): boolean {
  if (!event.clipboardData) return false;

  const content = event.clipboardData.getData("text/plain");
  const measurement = measureComposerPaste(content);
  if (measurement.presentation === "text") return false;

  event.preventDefault();
  insertCompactPaste(content, measurement);
  return true;
}

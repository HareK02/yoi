import type { Alert } from "#lib/generated/protocol.ts";
import type { ConsoleLine } from "./model.ts";

/** Stable IDs are authoritative; legacy replay can only use the original payload. */
export function alertIdentity(alert: Alert): string {
  return alert.id != null ? `id:${alert.id}` : `legacy:${
    JSON.stringify([
      alert.timestamp_ms,
      alert.level,
      alert.source,
      alert.message,
    ])
  }`;
}

/** Keep first-seen order while allowing replay to refresh anchor metadata. */
export function deduplicateAlertLines(lines: ConsoleLine[]): ConsoleLine[] {
  const result: ConsoleLine[] = [];
  const positions = new Map<string, number>();
  for (const line of lines) {
    if (!line.alert) {
      result.push(line);
      continue;
    }
    const key = alertIdentity(line.alert);
    const previous = positions.get(key);
    if (previous === undefined) {
      positions.set(key, result.length);
      result.push(line);
    } else {
      result[previous] = { ...line, id: result[previous]!.id };
    }
  }
  return result;
}

/**
 * Insert Alerts between source rows BEFORE any Read/activity aggregation.
 * Conversation order remains authoritative, even when timestamps are skewed.
 * An absent explicit anchor belongs before the loaded window, not at its tail;
 * its metadata remains intact so loading older history can resolve it later.
 */
export function orderAlertLines(lines: ConsoleLine[]): ConsoleLine[] {
  const ungrouped = lines.flatMap((line) => line.toolCallLines ?? [line]);
  const unique = deduplicateAlertLines(ungrouped);
  const source = unique.filter((line) => !line.alert);
  const slots = new Map<number, ConsoleLine[]>();
  for (const line of unique) {
    const alert = line.alert;
    if (!alert) continue;
    const position = alert.position;
    let index: number;
    switch (position?.kind) {
      case "start":
        index = 0;
        break;
      case "after_entry":
        index = source.findIndex((row) =>
          row.entryId === position.entry_id ||
          row.sourceEntryIds?.includes(position.entry_id)
        ) + 1;
        break;
      case "after_tool_call":
        index = source.findIndex((row) =>
          row.toolCall?.id === position.call_id
        ) + 1;
        break;
      case "after_in_flight":
        index =
          source.findIndex((row) => row.inFlightBlockId === position.block_id) +
          1;
        break;
      default: {
        // Legacy senders have no identity/anchor. Use emission time, never the
        // reconnect observation time, without sorting the conversation itself.
        const later = source.findIndex((row) =>
          row.timestampMs !== undefined && row.timestampMs > alert.timestamp_ms
        );
        index = later < 0 ? source.length : later;
      }
    }
    const slot = slots.get(index) ?? [];
    slot.push(line);
    slots.set(index, slot);
  }
  const result = [...(slots.get(0) ?? [])];
  source.forEach((line, index) => {
    result.push(line, ...(slots.get(index + 1) ?? []));
  });
  return result;
}

/** Live deltas lack block IDs; the emitting Alert identifies the pending row. */
export function attachAlertInFlightAnchor(
  lines: ConsoleLine[],
  alert: Alert,
): ConsoleLine[] {
  const position = alert.position;
  if (position?.kind !== "after_in_flight") return lines;
  if (lines.some((line) => line.inFlightBlockId === position.block_id)) {
    return lines;
  }
  const index = lines.findLastIndex((line) =>
    line.kind === "assistant" && !line.entryId
  );
  if (index < 0 || lines[index].inFlightBlockId !== undefined) return lines;
  return lines.map((line, rowIndex) =>
    rowIndex === index ? { ...line, inFlightBlockId: position.block_id } : line
  );
}

/** Resolve before the pending row is removed; replay may repeat either anchor. */
export function resolveCommittedAlertAnchors(
  lines: ConsoleLine[],
  blockId: number | undefined,
  entryId: string,
): ConsoleLine[] {
  if (blockId === undefined) return lines;
  return lines.map((line) =>
    line.alert?.position?.kind === "after_in_flight" &&
      line.alert.position.block_id === blockId
      ? {
        ...line,
        alert: {
          ...line.alert,
          position: { kind: "after_entry", entry_id: entryId },
        },
      }
      : line
  );
}

import init, {
  analyze_snapshot,
  apply_changes,
  changes_between,
  complete_current,
  evaluate_current,
  format_source,
  set_schema_bundle,
  set_snapshot,
} from "./generated/config_source_wasm.js";
import type { ConfigTreeChange } from "./types.ts";

export type ConfigSourceWorkerRequest =
  | {
    id: number;
    kind: "set_snapshot";
    snapshot: unknown;
    schemaBundle: unknown;
  }
  | { id: number; kind: "apply_changes"; changes: ConfigTreeChange[] }
  | { id: number; kind: "changes_between"; base: unknown; candidate: unknown }
  | { id: number; kind: "analyze"; path: string; source?: string }
  | { id: number; kind: "evaluate"; contract: unknown }
  | {
    id: number;
    kind: "complete";
    path: string;
    source: string;
    utf16Offset: number;
    explicit: boolean;
  }
  | { id: number; kind: "format"; source: string };

export type ConfigSourceWorkerResponse =
  | { id: number; ok: true; result: unknown }
  | { id: number; ok: false; error: unknown };

const ready = init();
let snapshot: unknown = null;

self.onmessage = async (
  event: MessageEvent<ConfigSourceWorkerRequest>,
): Promise<void> => {
  const request = event.data;
  try {
    await ready;
    let result: unknown;
    switch (request.kind) {
      case "set_snapshot":
        snapshot = request.snapshot;
        set_snapshot(request.snapshot);
        set_schema_bundle(request.schemaBundle);
        result = null;
        break;
      case "apply_changes":
        snapshot = apply_changes(request.changes);
        result = snapshot;
        break;
      case "changes_between":
        result = changes_between(request.base, request.candidate);
        break;
      case "analyze":
        if (!snapshot) {
          throw new Error("config source snapshot is not initialized");
        }
        result = analyze_snapshot(snapshot, request.path, request.source);
        break;
      case "evaluate":
        result = evaluate_current(request.contract);
        break;
      case "complete":
        result = complete_current(
          request.path,
          request.source,
          request.utf16Offset,
          request.explicit,
        );
        break;
      case "format":
        result = format_source(request.source);
        break;
    }
    self.postMessage({ id: request.id, ok: true, result });
  } catch (error) {
    self.postMessage({
      id: request.id,
      ok: false,
      error: error instanceof Error ? error.message : error,
    });
  }
};

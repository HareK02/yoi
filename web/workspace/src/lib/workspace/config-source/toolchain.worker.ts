import init, {
  analyze_snapshot,
  evaluate_snapshot,
  format_source,
} from "./generated/config_source_wasm.js";
import type {
  ConfigDiagnostic,
  ConfigTreeSnapshot,
  ToolchainContract,
} from "./types.ts";

export type ConfigSourceWorkerRequest =
  | {
    id: number;
    kind: "analyze";
    snapshot: ConfigTreeSnapshot;
    path: string;
    source?: string;
  }
  | {
    id: number;
    kind: "evaluate";
    snapshot: ConfigTreeSnapshot;
    contract: ToolchainContract;
  }
  | { id: number; kind: "format"; source: string };

export type ConfigSourceWorkerResponse =
  | { id: number; ok: true; result: unknown }
  | { id: number; ok: false; error: unknown };

const ready = init();

self.onmessage = async (
  event: MessageEvent<ConfigSourceWorkerRequest>,
): Promise<void> => {
  const request = event.data;
  try {
    await ready;
    let result: unknown;
    switch (request.kind) {
      case "analyze":
        result = analyze_snapshot(
          request.snapshot,
          request.path,
          request.source,
        ) as ConfigDiagnostic[];
        break;
      case "evaluate":
        result = evaluate_snapshot(request.snapshot, request.contract);
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
      error: normalizeError(error),
    });
  }
};

function normalizeError(error: unknown): unknown {
  if (error instanceof Error) return error.message;
  return error;
}

import type {
  ConfigDiagnostic,
  ConfigTreeChange,
  ConfigTreeSnapshot,
  ToolchainContract,
  WorkspaceConfigSchemaBundle,
} from "./types.ts";
import { jsonWorkerMessage } from "./toolchain-message.ts";
import {
  type ConfigSourceCompletionResult,
  toCodeMirrorCompletion,
} from "./completion.ts";
import type {
  ConfigSourceWorkerRequest,
  ConfigSourceWorkerResponse,
} from "./toolchain.worker.ts";

type Command =
  | Omit<Extract<ConfigSourceWorkerRequest, { kind: "set_snapshot" }>, "id">
  | Omit<Extract<ConfigSourceWorkerRequest, { kind: "apply_changes" }>, "id">
  | Omit<Extract<ConfigSourceWorkerRequest, { kind: "changes_between" }>, "id">
  | Omit<Extract<ConfigSourceWorkerRequest, { kind: "analyze" }>, "id">
  | Omit<Extract<ConfigSourceWorkerRequest, { kind: "evaluate" }>, "id">
  | Omit<Extract<ConfigSourceWorkerRequest, { kind: "complete" }>, "id">
  | Omit<Extract<ConfigSourceWorkerRequest, { kind: "format" }>, "id">;

export class ConfigSourceToolchain {
  #worker: Worker;
  #nextId = 1;
  #pending = new Map<
    number,
    { resolve: (value: unknown) => void; reject: (reason: unknown) => void }
  >();

  constructor(
    worker = new Worker(new URL("./toolchain.worker.ts", import.meta.url), {
      type: "module",
    }),
  ) {
    this.#worker = worker;
    worker.addEventListener(
      "message",
      (event: MessageEvent<ConfigSourceWorkerResponse>) => {
        const pending = this.#pending.get(event.data.id);
        if (!pending) return;
        this.#pending.delete(event.data.id);
        if (event.data.ok) pending.resolve(event.data.result);
        else pending.reject(event.data.error);
      },
    );
  }

  setSnapshot(
    snapshot: ConfigTreeSnapshot,
    schemaBundle: WorkspaceConfigSchemaBundle,
  ): Promise<void> {
    return this.#request({ kind: "set_snapshot", snapshot, schemaBundle });
  }
  applyChanges(changes: ConfigTreeChange[]): Promise<ConfigTreeSnapshot> {
    return this.#request({ kind: "apply_changes", changes });
  }
  changesBetween(
    base: ConfigTreeSnapshot,
    candidate: ConfigTreeSnapshot,
  ): Promise<ConfigTreeChange[]> {
    return this.#request({ kind: "changes_between", base, candidate });
  }
  analyze(path: string, source?: string): Promise<ConfigDiagnostic[]> {
    return this.#request({ kind: "analyze", path, source });
  }
  evaluate(contract: ToolchainContract) {
    return this.#request({ kind: "evaluate", contract });
  }
  async complete(
    path: string,
    source: string,
    utf16Offset: number,
    explicit = false,
  ): Promise<import("@codemirror/autocomplete").CompletionResult | null> {
    const result = await this.#request<ConfigSourceCompletionResult | null>({
      kind: "complete",
      path,
      source,
      utf16Offset,
      explicit,
    });
    return toCodeMirrorCompletion(source, result);
  }
  format(source: string): Promise<string> {
    return this.#request({ kind: "format", source });
  }
  close(): void {
    this.#worker.terminate();
    for (const pending of this.#pending.values()) {
      pending.reject(new Error("config source toolchain was closed"));
    }
    this.#pending.clear();
  }
  #request<T>(request: Command): Promise<T> {
    const id = this.#nextId++;
    const message = jsonWorkerMessage({ ...request, id });
    return new Promise<T>((resolve, reject) => {
      this.#pending.set(id, {
        resolve: (value) => resolve(value as T),
        reject,
      });
      this.#worker.postMessage(message);
    });
  }
}

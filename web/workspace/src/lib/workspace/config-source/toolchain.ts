import type {
  ConfigDiagnostic,
  ConfigTreeSnapshot,
  ToolchainContract,
} from "./types.ts";
import type {
  ConfigSourceWorkerRequest,
  ConfigSourceWorkerResponse,
} from "./toolchain.worker.ts";

type ConfigSourceWorkerCommand =
  | Omit<Extract<ConfigSourceWorkerRequest, { kind: "analyze" }>, "id">
  | Omit<Extract<ConfigSourceWorkerRequest, { kind: "evaluate" }>, "id">
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

  analyze(
    snapshot: ConfigTreeSnapshot,
    path: string,
    source?: string,
  ): Promise<ConfigDiagnostic[]> {
    return this.#request({ kind: "analyze", snapshot, path, source });
  }

  evaluate(snapshot: ConfigTreeSnapshot, contract: ToolchainContract) {
    return this.#request({ kind: "evaluate", snapshot, contract });
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

  #request<T>(request: ConfigSourceWorkerCommand): Promise<T> {
    const id = this.#nextId++;
    return new Promise<T>((resolve, reject) => {
      this.#pending.set(id, {
        resolve: (value) => resolve(value as T),
        reject,
      });
      this.#worker.postMessage({ ...request, id });
    });
  }
}

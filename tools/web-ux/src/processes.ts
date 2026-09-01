import { dirname, isAbsolute, resolve } from "@std/path";
import { bounded, redactText } from "./artifacts.ts";
import type { CaptureError, OwnedProcess } from "./types.ts";

export type RunningProcess = {
  id: string;
  pid: number;
  child: Deno.ChildProcess;
  output: Promise<void>;
};

async function appendOutput(
  stream: ReadableStream<Uint8Array>,
  destination: string,
  secrets: string[],
): Promise<void> {
  const file = await Deno.open(destination, {
    create: true,
    append: true,
    write: true,
    mode: 0o600,
  });
  try {
    const reader = stream.pipeThrough(new TextDecoderStream()).getReader();
    while (true) {
      const { value, done } = await reader.read();
      if (done) break;
      await file.write(new TextEncoder().encode(redactText(value, secrets)));
    }
  } finally {
    file.close();
  }
}

async function waitForReady(url: string, timeoutMs: number): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  let lastError = "not attempted";
  while (Date.now() < deadline) {
    try {
      const response = await fetch(url, { redirect: "manual", signal: AbortSignal.timeout(2_000) });
      if (response.status < 500) return;
      lastError = `HTTP ${response.status}`;
    } catch (error) {
      lastError = error instanceof Error ? error.message : String(error);
    }
    await new Promise((resolve) => setTimeout(resolve, 200));
  }
  throw new Error(`process readiness timed out for ${url}: ${bounded(lastError, 200)}`);
}

export async function startOwnedProcesses(
  specifications: OwnedProcess[],
  scenarioPath: string,
  logsDirectory: string,
  secrets: string[],
): Promise<RunningProcess[]> {
  const running: RunningProcess[] = [];
  await Deno.mkdir(logsDirectory, { recursive: true });
  try {
    for (const specification of specifications) {
      const cwd = specification.cwd === undefined
        ? dirname(resolve(scenarioPath))
        : isAbsolute(specification.cwd)
        ? specification.cwd
        : resolve(dirname(scenarioPath), specification.cwd);
      const child = new Deno.Command(specification.command, {
        args: specification.args ?? [],
        cwd,
        env: specification.env,
        stdin: "null",
        stdout: "piped",
        stderr: "piped",
      }).spawn();
      const stdout = appendOutput(
        child.stdout,
        `${logsDirectory}/${specification.id}.stdout.log`,
        secrets,
      );
      const stderr = appendOutput(
        child.stderr,
        `${logsDirectory}/${specification.id}.stderr.log`,
        secrets,
      );
      const process = {
        id: specification.id,
        pid: child.pid,
        child,
        output: Promise.all([stdout, stderr]).then(() => undefined),
      };
      running.push(process);
      if (specification.readyUrl) {
        await Promise.race([
          waitForReady(specification.readyUrl, specification.readyTimeoutMs ?? 30_000),
          child.status.then((status) => {
            throw new Error(
              `owned process ${specification.id} exited before readiness: ${status.code}`,
            );
          }),
        ]);
      }
    }
    return running;
  } catch (error) {
    await stopOwnedProcesses(running);
    throw error;
  }
}

async function descendantPids(parentPid: number): Promise<number[]> {
  if (Deno.build.os === "windows") return [];
  try {
    const result = await new Deno.Command("ps", {
      args: ["-eo", "pid=,ppid="],
      stdout: "piped",
      stderr: "null",
    }).output();
    if (!result.success) return [];
    const rows = new TextDecoder().decode(result.stdout).trim().split("\n").map((line) =>
      line.trim().split(/\s+/).map(Number)
    );
    const descendants: number[] = [];
    const queue = [parentPid];
    while (queue.length > 0) {
      const parent = queue.shift()!;
      for (const [pid, ppid] of rows) {
        if (ppid === parent && !descendants.includes(pid)) {
          descendants.push(pid);
          queue.push(pid);
        }
      }
    }
    return descendants.reverse();
  } catch {
    return [];
  }
}

function tryKill(pid: number, signal: Deno.Signal): void {
  try {
    Deno.kill(pid, signal);
  } catch (error) {
    if (!(error instanceof Deno.errors.NotFound)) throw error;
  }
}

export async function stopOwnedProcesses(processes: RunningProcess[]): Promise<CaptureError[]> {
  const diagnostics: CaptureError[] = [];
  for (const process of [...processes].reverse()) {
    try {
      for (const pid of await descendantPids(process.pid)) tryKill(pid, "SIGTERM");
      tryKill(process.pid, "SIGTERM");
      let timer: number | undefined;
      const exited = await Promise.race([
        process.child.status.then(() => true),
        new Promise<boolean>((resolve) => {
          timer = setTimeout(() => resolve(false), 3_000);
        }),
      ]).finally(() => clearTimeout(timer));
      if (!exited) {
        for (const pid of await descendantPids(process.pid)) tryKill(pid, "SIGKILL");
        tryKill(process.pid, "SIGKILL");
        await process.child.status;
      }
      await process.output;
    } catch (error) {
      diagnostics.push({
        kind: "tool",
        message: `failed to clean process ${process.id}: ${
          bounded(error instanceof Error ? error.message : String(error), 500)
        }`,
      });
    }
  }
  return diagnostics;
}

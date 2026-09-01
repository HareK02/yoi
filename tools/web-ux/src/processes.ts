import { dirname, isAbsolute, resolve } from "@std/path";
import { bounded, redactText, writePrivateJson } from "./artifacts.ts";
import type { CaptureError, OwnedProcess } from "./types.ts";

export const PROCESS_LOG_BYTE_LIMIT = 1024 * 1024;
const PROCESS_STOP_TIMEOUT_MS = 3_000;

export type RunningProcess = {
  id: string;
  pid: number;
  child: Deno.ChildProcess;
  status: Promise<Deno.CommandStatus>;
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
  const encoder = new TextEncoder();
  const overlapCharacters = Math.max(512, ...secrets.map((secret) => secret.length + 128));
  let pending = "";
  let bytesObserved = 0;
  let bytesWritten = 0;
  let truncated = false;
  const writeRedacted = async (value: string) => {
    const encoded = encoder.encode(redactText(value, secrets));
    const remaining = Math.max(0, PROCESS_LOG_BYTE_LIMIT - bytesWritten);
    if (encoded.length > remaining) truncated = true;
    if (remaining > 0) {
      const output = encoded.subarray(0, remaining);
      await file.write(output);
      bytesWritten += output.length;
    }
  };
  try {
    const reader = stream.pipeThrough(new TextDecoderStream()).getReader();
    while (true) {
      const { value, done } = await reader.read();
      if (done) break;
      bytesObserved += encoder.encode(value).length;
      pending += value;
      if (pending.length > overlapCharacters * 2) {
        const splitAt = pending.length - overlapCharacters;
        await writeRedacted(pending.slice(0, splitAt));
        pending = pending.slice(splitAt);
      }
    }
    await writeRedacted(pending);
  } finally {
    file.close();
    await writePrivateJson(`${destination}.meta.json`, {
      schemaVersion: 1,
      byteLimit: PROCESS_LOG_BYTE_LIMIT,
      bytesObserved,
      bytesWritten,
      truncated,
    });
  }
}

async function waitForReady(url: string, timeoutMs: number): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  let lastError = "not attempted";
  while (Date.now() < deadline) {
    try {
      const response = await fetch(url, { redirect: "manual", signal: AbortSignal.timeout(2_000) });
      const status = response.status;
      await response.body?.cancel();
      if (status < 500) return;
      lastError = `HTTP ${status}`;
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
      const status = child.status;
      const process = {
        id: specification.id,
        pid: child.pid,
        child,
        status,
        output: Promise.all([stdout, stderr]).then(() => undefined),
      };
      running.push(process);
      if (specification.readyUrl) {
        await Promise.race([
          waitForReady(specification.readyUrl, specification.readyTimeoutMs ?? 30_000),
          status.then((status) => {
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

async function livePids(pids: number[]): Promise<number[]> {
  if (Deno.build.os === "windows") return [];
  if (pids.length === 0) return [];
  try {
    const result = await new Deno.Command("ps", {
      args: ["-o", "pid=", "-p", pids.join(",")],
      stdout: "piped",
      stderr: "null",
    }).output();
    if (!result.success && result.code !== 1) return pids;
    const live = new Set(
      new TextDecoder().decode(result.stdout).trim().split(/\s+/).map(Number).filter(
        Number.isFinite,
      ),
    );
    return pids.filter((pid) => live.has(pid));
  } catch {
    return pids;
  }
}

async function waitForPidsToExit(pids: number[], timeoutMs: number): Promise<number[]> {
  const deadline = Date.now() + timeoutMs;
  let live = await livePids(pids);
  while (live.length > 0 && Date.now() < deadline) {
    await new Promise((resolve) => setTimeout(resolve, 50));
    live = await livePids(live);
  }
  return live;
}

export async function stopOwnedProcesses(processes: RunningProcess[]): Promise<CaptureError[]> {
  const diagnostics: CaptureError[] = [];
  for (const process of [...processes].reverse()) {
    try {
      const descendants = await descendantPids(process.pid);
      tryKill(process.pid, "SIGTERM");
      for (const pid of descendants) tryKill(pid, "SIGTERM");
      let timer: number | undefined;
      const [parentExited, liveDescendants] = await Promise.all([
        Promise.race([
          process.status.then(() => true),
          new Promise<boolean>((resolve) => {
            timer = setTimeout(() => resolve(false), PROCESS_STOP_TIMEOUT_MS);
          }),
        ]).finally(() => clearTimeout(timer)),
        waitForPidsToExit(descendants, PROCESS_STOP_TIMEOUT_MS),
      ]);
      if (!parentExited || liveDescendants.length > 0) {
        const lateDescendants = await descendantPids(process.pid);
        const forceTargets = [...new Set([...liveDescendants, ...lateDescendants])];
        for (const pid of forceTargets) tryKill(pid, "SIGKILL");
        tryKill(process.pid, "SIGKILL");
        await process.status;
        const survivors = await waitForPidsToExit(forceTargets, 1_000);
        if (survivors.length > 0) {
          throw new Error(`descendant processes did not exit: ${survivors.join(",")}`);
        }
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

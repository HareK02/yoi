// Bound top-level suites/shards, not total processes; large shards normally use two threads.
// Cargo selects targets/features. Retain all listed names; never skip cached successes.
import { spawn } from "node:child_process";
import process from "node:process";
import { availableParallelism } from "node:os";
import { fileURLToPath } from "node:url";
import { posix } from "node:path";

// Safety limits, NOT the separate 60s performance goal.
export const SUITE_TIMEOUT_MS = 180_000;
export const BUILD_TIMEOUT_MS = 1_800_000;
export const MAX_JOBS = 64;

export function defaultJobs(cpus = availableParallelism()): number {
  // Leave headroom for internal runners; higher top-level counts can be chosen
  // explicitly, but are not assumed to improve throughput or timing stability.
  return Math.min(32, Math.max(1, Math.ceil(cpus * 4 / 3)));
}

export function parseJobs(args: string[]): number {
  if (!args.length) return defaultJobs();
  if (args.length !== 2 || args[0] !== "--jobs") {
    throw new Error("usage: deno task test [--jobs N]");
  }
  const jobs = Number(args[1]);
  if (!Number.isInteger(jobs) || jobs < 1 || jobs > MAX_JOBS) {
    throw new Error(`jobs must be 1..${MAX_JOBS}`);
  }
  return jobs;
}

/** libtest's terse --list has one `name: test`/`name: benchmark` per line.
 * An empty listing is valid for a binary with no tests. Never silently ignore
 * unrecognized lines or deduplicate names: either would hide selection errors.
 * The listing does not identify ignored tests; retain them and libtest's defaults.
 */
export function parseRustListing(stdout: string): string[] {
  const names: string[] = [];
  const seen = new Set<string>();
  const lines = stdout.replaceAll("\r\n", "\n").split("\n");
  if (lines.at(-1) === "") lines.pop();
  for (const line of lines) {
    // Reject control bytes rather than treating terminal decoration as test names.
    // deno-lint-ignore no-control-regex
    const match = /^([^\x00-\x1f\x7f]+): (test|benchmark)$/.exec(line);
    if (!match) {
      throw new Error(`unknown Rust listing format: ${JSON.stringify(line)}`);
    }
    const name = match[1];
    if (seen.has(name)) throw new Error(`duplicate Rust listing name: ${name}`);
    seen.add(name);
    names.push(name);
  }
  return names;
}

export interface RustShard {
  task: TestCommand;
  selectedNames: string[];
}

export function shardRustCommand(
  task: TestCommand,
  names: string[],
): RustShard[] {
  if (new Set(names).size !== names.length) {
    throw new Error("cannot shard duplicate test names");
  }
  // RuntimeDirSandbox changes process-global paths. Do not overlap its writes
  // with restore guards in the same process, including small integration targets.
  const serial = ["worker", "worker-runtime", "yoi-workspace-server"].includes(
    task.name.split("/")[0],
  );
  if (names.length <= 72) {
    return [{
      task: serial
        ? { ...task, args: [...task.args, "--test-threads", "1"] }
        : task,
      selectedNames: [...names],
    }];
  }
  const count = Math.min(32, Math.ceil(names.length / 36));
  const shards: string[][] = Array.from({ length: count }, () => []);
  names.forEach((name, index) => shards[index % count].push(name));
  return shards.map((selectedNames, index) => ({
    task: {
      ...task,
      name: `${task.name}/shard-${
        String(index + 1).padStart(2, "0")
      }-of-${count}`,
      args: [
        ...task.args,
        "--exact",
        "--test-threads",
        serial ? "1" : "2",
        ...selectedNames,
      ],
    },
    selectedNames,
  }));
}

export async function discoverRustCommands(
  commands: TestCommand[],
  jobs: number,
  options: {
    output: string;
    signal?: AbortSignal;
    run?: (task: TestCommand, label: string) => Promise<ExecutionResult>;
  },
) {
  const binaries = await mapConcurrent(commands, jobs, async (task, index) => {
    const listingTask = {
      ...task,
      name: `${task.name}/list`,
      args: [...task.args, "--list", "--format", "terse"],
    };
    const label = `list-${String(index).padStart(3, "0")}`;
    const listing = await (options.run
      ? options.run(listingTask, label)
      : execute(listingTask, {
        output: options.output,
        signal: options.signal,
        label,
        timeoutMs: SUITE_TIMEOUT_MS,
      }));
    let shards: RustShard[] = [];
    let error: string | undefined;
    try {
      if (listing.code !== 0) {
        throw new Error(
          `listing failed with code ${listing.code}; see ${listing.log}.stderr`,
        );
      }
      shards = shardRustCommand(task, parseRustListing(listing.stdout)).map((
        shard,
        shardIndex,
      ) => ({
        ...shard,
        task: {
          ...shard.task,
          inventoryRef: { binaryIndex: index, shardIndex },
        },
      }));
    } catch (cause) {
      error = String(cause);
    }
    return {
      name: task.name,
      binary: task.command,
      listing: resultSummary(listing),
      error,
      selectedCases: error
        ? null
        : shards.reduce((sum, shard) =>
          sum + shard.selectedNames.length, 0),
      shards,
    };
  });
  const complete = binaries.every((binary) => !binary.error);
  const discoveredCases = binaries.reduce(
    (sum, binary) => sum + (binary.selectedCases ?? 0),
    0,
  );
  const inventory = {
    complete,
    rustBinaries: commands.length,
    // Includes ignored names, not a claim that ignored bodies were executed.
    selectedCases: complete ? discoveredCases : null,
    discoveredCases,
    binaries,
  };
  await Deno.writeTextFile(
    `${options.output}/inventory.json`,
    JSON.stringify(inventory, null, 2),
  );
  return {
    inventory,
    tasks: complete
      ? binaries.flatMap((binary) => binary.shards.map((shard) => shard.task))
      : [],
  };
}

function cancellationExitCode(reason: unknown): number {
  return reason === "SIGTERM" ? 143 : 130;
}

/** Keep the parent alive until active groups have been stopped and summaries saved. */
export function installCancellationHandlers() {
  const controller = new AbortController();
  const interrupt = () => controller.abort("SIGINT");
  const terminate = () => controller.abort("SIGTERM");
  Deno.addSignalListener("SIGINT", interrupt);
  Deno.addSignalListener("SIGTERM", terminate);
  return {
    signal: controller.signal,
    dispose() {
      Deno.removeSignalListener("SIGINT", interrupt);
      Deno.removeSignalListener("SIGTERM", terminate);
    },
  };
}

export interface ExecutionResult {
  name: string;
  code: number;
  signal: string | null;
  timedOut: boolean;
  cancelled: boolean;
  cancellationReason?: string;
  error?: string;
  seconds: number;
  stdout: string;
  stderr: string;
  log: string;
  timeoutMs: number;
  inventoryRef?: TestCommand["inventoryRef"];
}

/** Linux only: a new process group contains ordinary inherited descendants.
 * TERM then KILL targets the whole group, even if its leader already exited.
 * Node's close event reaps the direct child and drains its pipes. Descendants are
 * reaped by their parents/the OS; deliberately escaping via setsid is not covered.
 */
export async function execute(
  task: TestCommand,
  options: {
    output: string;
    label: string;
    timeoutMs: number;
    signal?: AbortSignal;
    killGraceMs?: number;
    closeGraceMs?: number;
  },
): Promise<ExecutionResult> {
  const {
    output,
    label,
    timeoutMs,
    signal: abortSignal,
    killGraceMs = 1_000,
    closeGraceMs = 5_000,
  } = options;
  if (Deno.build.os !== "linux") {
    throw new Error(
      "test runner process-group cleanup is supported only on Linux",
    );
  }
  for (const ms of [timeoutMs, killGraceMs, closeGraceMs]) {
    if (!Number.isSafeInteger(ms) || ms < 1 || ms > 2_147_483_647) {
      throw new Error(
        "process deadlines must be positive timer-safe milliseconds",
      );
    }
  }
  const start = performance.now();
  const stdoutFile = Deno.openSync(`${output}/${label}.stdout`, {
    write: true,
    create: true,
    truncate: true,
  });
  let stderrFile: Deno.FsFile;
  try {
    stderrFile = Deno.openSync(`${output}/${label}.stderr`, {
      write: true,
      create: true,
      truncate: true,
    });
  } catch (error) {
    stdoutFile.close();
    throw error;
  }
  const chunks: { stdout: Uint8Array[]; stderr: Uint8Array[] } = {
    stdout: [],
    stderr: [],
  };
  const errors: string[] = [];
  let timedOut = false;
  let cancelled = false;
  let deadline: ReturnType<typeof setTimeout> | undefined;
  let code = 1;
  let signal: string | null = null;
  let stop: (() => void) | undefined;
  function save(stream: "stdout" | "stderr", data: Uint8Array) {
    chunks[stream].push(data);
    try {
      const file = stream === "stdout" ? stdoutFile : stderrFile;
      let offset = 0;
      while (offset < data.length) {
        const written = file.writeSync(data.subarray(offset));
        if (!written) throw new Error("log write made no progress");
        offset += written;
      }
    } catch (error) {
      errors.push(`saving ${stream}: ${error}`);
      stop?.();
    }
  }
  const diagnostic = (text: string) =>
    save("stderr", new TextEncoder().encode(`\n[runner] ${text}\n`));
  const cancel = () => {
    cancelled = true;
    clearTimeout(deadline);
    diagnostic(`cancelled by ${String(abortSignal?.reason)}`);
    stop?.();
  };
  try {
    if (abortSignal?.aborted) {
      cancel(); // Queued commands are recorded as cancelled, never spawned.
    } else {
      abortSignal?.addEventListener("abort", cancel, { once: true });
      const child = spawn(task.command, task.args, {
        cwd: task.cwd,
        env: { ...process.env, ...task.env },
        detached: true,
        stdio: ["ignore", "pipe", "pipe"],
      });
      let closed = false;
      const close = new Promise<void>((resolve) => {
        child.once("close", (exitCode, exitSignal) => {
          closed = true;
          code = exitCode ?? 1;
          signal = exitSignal;
          resolve();
        });
      });
      child.on("error", (error) => {
        errors.push(String(error));
        stop?.();
      });
      child.stdout!.on("data", (data: Uint8Array) => save("stdout", data));
      child.stderr!.on("data", (data: Uint8Array) => save("stderr", data));
      for (const pipe of [child.stdout!, child.stderr!]) {
        pipe.on("error", (error) => {
          errors.push(`reading process output: ${error}`);
          stop?.();
        });
      }
      const killGroup = (sig: "SIGTERM" | "SIGKILL") => {
        if (child.pid === undefined) return;
        try {
          process.kill(-child.pid, sig);
        } catch (error) {
          if ((error as NodeJS.ErrnoException).code !== "ESRCH") {
            errors.push(`process-group ${sig}: ${error}`);
          }
        }
      };
      let stopping: Promise<void> | undefined;
      let resolveStopped: () => void;
      const stopped = new Promise<void>((resolve) => resolveStopped = resolve);
      stop = () => {
        if (stopping) return;
        stopping = (async () => {
          killGroup("SIGTERM");
          // Do not cancel escalation on leader close: descendants may still live,
          // including ones that closed stdout/stderr or ignore SIGTERM.
          await new Promise((resolve) => setTimeout(resolve, killGraceMs));
          killGroup("SIGKILL");
          let drainTimer: ReturnType<typeof setTimeout> | undefined;
          await Promise.race([
            close,
            new Promise<void>((resolve) => {
              drainTimer = setTimeout(resolve, closeGraceMs);
            }),
          ]);
          clearTimeout(drainTimer);
          if (!closed) {
            errors.push(
              "cleanup deadline exceeded: child/pipes did not close after SIGKILL",
            );
            child.stdout!.destroy();
            child.stderr!.destroy();
            child.unref();
          }
          resolveStopped();
        })();
      };
      deadline = setTimeout(() => {
        timedOut = true;
        diagnostic(`timeout after ${timeoutMs}ms; terminating process group`);
        stop!();
      }, timeoutMs);
      try {
        await Promise.race([close, stopped]);
        clearTimeout(deadline);
        if (stopping) await stopping;
      } finally {
        clearTimeout(deadline);
      }
    }
  } catch (error) {
    errors.push(String(error));
  } finally {
    abortSignal?.removeEventListener("abort", cancel);
    clearTimeout(deadline);
    stop = undefined;
    if (errors.length) diagnostic(errors.join("\n"));
    stdoutFile.close();
    stderrFile.close();
  }
  function text(parts: Uint8Array[]) {
    const decoder = new TextDecoder();
    return parts.map((part) => decoder.decode(part, { stream: true })).join(
      "",
    ) +
      decoder.decode();
  }
  const result: ExecutionResult = {
    name: task.name,
    code: cancelled
      ? cancellationExitCode(abortSignal?.reason)
      : timedOut
      ? 124
      : errors.length
      ? 1
      : code,
    signal,
    timedOut,
    cancelled,
    ...(cancelled ? { cancellationReason: String(abortSignal?.reason) } : {}),
    ...(errors.length ? { error: errors.join("\n") } : {}),
    seconds: (performance.now() - start) / 1000,
    stdout: text(chunks.stdout),
    stderr: text(chunks.stderr),
    log: label,
    timeoutMs,
    ...(task.inventoryRef ? { inventoryRef: task.inventoryRef } : {}),
  };
  // A per-command record also survives early metadata/build failures.
  await Deno.writeTextFile(
    `${output}/${label}.result.json`,
    JSON.stringify(resultSummary(result), null, 2),
  );
  return result;
}

export function resultSummary(
  { stdout: _stdout, stderr: _stderr, ...summary }: ExecutionResult,
) {
  return summary;
}

export interface CargoArtifact {
  reason?: string;
  package_id?: string;
  executable?: string | null;
  profile?: { test?: boolean };
  target?: { name: string };
}

export interface TestCommand {
  name: string;
  command: string;
  args: string[];
  cwd: string;
  env?: Record<string, string | undefined>;
  // Exact link from each execution result into inventory.json, even when two
  // Cargo targets happen to share the same package/target display name.
  inventoryRef?: { binaryIndex: number; shardIndex: number };
}

// Walk with lstat before creating any child: recursive mkdir/realPath alone
// would follow an existing symlink first. Reject aliases rather than silently
// canonicalizing a caller's checkout. As with the Rust guard, this assumes the
// owned checkout is not concurrently replaced by an adversarial filesystem actor.
async function ownedTestDirectory(checkout: string, relative: string) {
  if (
    !posix.isAbsolute(checkout) || checkout === "/" ||
    posix.normalize(checkout) !== checkout
  ) throw new Error("test checkout must be an absolute normal-only path");

  async function checkDirectory(path: string, owned: boolean) {
    const stat = await Deno.lstat(path);
    if (!stat.isDirectory || stat.isSymlink) {
      throw new Error(
        `test isolation refuses non-directory or symlink: ${path}`,
      );
    }
    if (owned && process.getuid && stat.uid !== process.getuid()) {
      throw new Error(
        `test isolation directory is not owned by this user: ${path}`,
      );
    }
    if (await Deno.realPath(path) !== path) {
      throw new Error(
        `test isolation refuses non-canonical directory: ${path}`,
      );
    }
  }
  let checkoutPath = "";
  for (const part of checkout.slice(1).split("/")) {
    checkoutPath += `/${part}`;
    await checkDirectory(checkoutPath, false);
  }
  let path = checkout;
  for (const part of relative.split("/")) {
    if (!part || part === "." || part === "..") {
      throw new Error("invalid owned test directory component");
    }
    path += `/${part}`;
    try {
      await Deno.mkdir(path, { mode: 0o700 });
    } catch (error) {
      if (!(error instanceof Deno.errors.AlreadyExists)) throw error;
    }
    await checkDirectory(path, true);
  }
  return path;
}

/** Prepare child-only isolation for the maintained normal-test entrypoint.
 * Each wrap gets a fresh allocation table, even for repeated/sanitized labels or
 * concurrent runners using the same output id. Resources stay under target for
 * diagnosis; callers with temporary checkouts own their recursive cleanup.
 */
export async function prepareTestIsolation(checkout: string, runId: string) {
  const ownedDirectory = (relative: string) =>
    ownedTestDirectory(checkout, relative);
  const prefix = "target/restore-test-environment";
  await ownedDirectory(prefix);
  // Keep tempfile Unix-socket references short. tempfile owns unique resources
  // here; neither runners nor suites remove/replace this shared directory.
  const tmp = await ownedDirectory("target/t");
  const digest = new Uint8Array(
    await crypto.subtle.digest("SHA-256", new TextEncoder().encode(runId)),
  );
  const id = Array.from(
    digest.slice(0, 6),
    (byte) => byte.toString(16).padStart(2, "0"),
  ).join("");
  const relativeRoot = `${prefix}/runner-${id}-${crypto.randomUUID()}`;
  const root = await ownedDirectory(relativeRoot);
  let next = 0;
  return {
    root,
    async wrapCommand(task: TestCommand, label: string): Promise<TestCommand> {
      const safeLabel = label.replaceAll(/[^a-zA-Z0-9_-]/g, "-").slice(0, 40) ||
        "command";
      const relative = `${relativeRoot}/${safeLabel}-${next++}`;
      // Recheck shared ancestors as well as the command namespace on every wrap.
      await ownedDirectory("target/t");
      const namespace = await ownedDirectory(relative);
      for (
        const dir of [
          "config",
          "data",
          "run",
          "home",
          "git-template/info",
          "git-template/hooks",
        ]
      ) {
        await ownedDirectory(`${relative}/${dir}`);
      }
      const inherited = { ...process.env, ...task.env };
      const env: NonNullable<TestCommand["env"]> = { ...task.env };
      // Undefined shadows inherited entries in execute's merge; node spawn omits
      // them. Strip indexed Git config and less common Git path overrides too.
      for (const key of Object.keys(inherited)) {
        if (/^GIT_/i.test(key) || /(^|_)proxy$/i.test(key)) {
          env[key] = undefined;
        }
      }
      Object.assign(env, {
        HOME: `${namespace}/home`,
        YOI_HOME: `${namespace}/data`,
        YOI_CONFIG_DIR: `${namespace}/config`,
        YOI_DATA_DIR: `${namespace}/data`,
        YOI_RUNTIME_DIR: `${namespace}/run`,
        TMPDIR: tmp,
        TMP: tmp,
        TEMP: tmp,
        GIT_TEMPLATE_DIR: `${namespace}/git-template`,
        GIT_CONFIG_GLOBAL: "/dev/null",
        GIT_CONFIG_SYSTEM: "/dev/null",
        GIT_CONFIG_NOSYSTEM: "1",
        GIT_ATTR_NOSYSTEM: "1",
        GIT_CEILING_DIRECTORIES: checkout,
        GIT_AUTHOR_NAME: "Normal test runner",
        GIT_AUTHOR_EMAIL: "normal-tests@example.invalid",
        GIT_COMMITTER_NAME: "Normal test runner",
        GIT_COMMITTER_EMAIL: "normal-tests@example.invalid",
        RUSTC_WRAPPER: "",
        RUSTC_WORKSPACE_WRAPPER: "",
        SCCACHE_DISABLE: "1",
      });
      // HOME is isolated, but toolchain/cache discovery must keep its ordinary
      // host locations (these are compiler settings, not live product state).
      for (const key of ["CARGO_HOME", "RUSTUP_HOME"] as const) {
        const fallback = key === "CARGO_HOME" ? ".cargo" : ".rustup";
        env[key] = inherited[key] ||
          (process.env.HOME ? `${process.env.HOME}/${fallback}` : undefined);
      }
      return { ...task, env };
    },
  };
}

export function rustTestCommands(
  artifacts: CargoArtifact[],
  packages: { id: string; name: string; manifest_path: string }[],
): TestCommand[] {
  const commands = new Map<string, TestCommand>();
  for (const artifact of artifacts) {
    if (
      artifact.reason !== "compiler-artifact" || !artifact.profile?.test ||
      !artifact.executable
    ) continue;
    const pkg = packages.find((pkg) => pkg.id === artifact.package_id);
    if (!pkg || !artifact.target) {
      throw new Error("test artifact has no Cargo package/target");
    }
    const cwd = pkg.manifest_path.replace(/[/\\][^/\\]+$/, "");
    commands.set(artifact.executable, {
      name: `${pkg.name}/${artifact.target.name}`,
      command: artifact.executable,
      args: [],
      cwd,
      env: { CARGO_MANIFEST_DIR: cwd },
    });
  }
  if (!commands.size) throw new Error("Cargo produced no test executables");
  return [...commands.values()];
}

export async function mapConcurrent<T, R>(
  items: T[],
  jobs: number,
  run: (item: T, index: number) => Promise<R>,
): Promise<R[]> {
  if (!Number.isInteger(jobs) || jobs < 1) {
    throw new Error("jobs must be a positive integer");
  }
  const results: R[] = new Array(items.length);
  let next = 0;
  // Drain every active worker even if one command fails to launch/write logs;
  // callers must not remove signal handlers while detached groups are active.
  const workers = await Promise.allSettled(
    Array.from({ length: Math.min(jobs, items.length) }, async () => {
      for (;;) {
        const index = next++;
        if (index >= items.length) return;
        results[index] = await run(items[index], index);
      }
    }),
  );
  const failure = workers.find((worker) => worker.status === "rejected");
  if (failure?.status === "rejected") throw failure.reason;
  return results;
}

function jsonLines(text: string): CargoArtifact[] {
  return text.split("\n").filter((line) => line.startsWith("{")).map((line) =>
    JSON.parse(line)
  );
}

async function main() {
  const root = fileURLToPath(new URL("../", import.meta.url)).replace(
    /[/\\]$/,
    "",
  );
  const jobs = parseJobs(Deno.args);
  const outputId = `${
    new Date().toISOString().replaceAll(":", "-")
  }-${crypto.randomUUID()}`;
  const isolation = await prepareTestIsolation(root, outputId);
  const output = await ownedTestDirectory(
    root,
    `target/normal-tests/${outputId}`,
  );
  const cancellation = installCancellationHandlers();
  const run = async (
    task: TestCommand,
    label: string,
    timeoutMs = SUITE_TIMEOUT_MS,
  ) =>
    execute(await isolation.wrapCommand(task, label), {
      output,
      label,
      timeoutMs,
      signal: cancellation.signal,
    });
  const summary: Record<string, unknown> = { phase: "metadata", failed: false };
  let executionStart: number | undefined;
  const writeSummary = () =>
    Deno.writeTextFile(
      `${output}/summary.json`,
      JSON.stringify(
        {
          jobs,
          concurrency:
            "top-level suites/shards; worker, worker-runtime and yoi-workspace-server use 1 test thread; other large Rust shards use 2, other suites retain internal parallelism",
          platform: "linux",
          suiteTimeoutMs: SUITE_TIMEOUT_MS,
          buildTimeoutMs: BUILD_TIMEOUT_MS,
          ...summary,
        },
        null,
        2,
      ),
    );
  console.log(
    `Logs: ${output}; suite timeout: ${
      SUITE_TIMEOUT_MS / 1000
    }s; build timeout: ${
      BUILD_TIMEOUT_MS / 1000
    }s (not the 60s performance goal).`,
  );
  try {
    const metadata = await run({
      name: "metadata",
      command: "cargo",
      args: ["metadata", "--no-deps", "--format-version", "1"],
      cwd: root,
    }, "metadata");
    summary.preparation = [resultSummary(metadata)];
    if (metadata.code) {
      Deno.exitCode = metadata.code;
      throw new Error(`metadata failed: ${metadata.stderr}`);
    }
    summary.phase = "build";
    console.log(
      "Building all normal Rust test targets (separate from execution timing)…",
    );
    const build = await run(
      {
        name: "build",
        command: "cargo",
        args: [
          "test",
          "--workspace",
          "--exclude",
          "yoi-e2e",
          "--no-run",
          "--message-format=json",
        ],
        cwd: root,
      },
      "build",
      BUILD_TIMEOUT_MS,
    );
    console.log(`Build: ${build.seconds.toFixed(2)}s`);
    summary.buildSeconds = build.seconds;
    summary.preparation = [resultSummary(metadata), resultSummary(build)];
    if (build.code) {
      Deno.exitCode = build.code;
      throw new Error(`build failed: ${build.stderr}\n${build.stdout}`);
    }
    // Everything after compilation, including --list discovery, is execution time.
    executionStart = performance.now();
    summary.phase = "discovery";
    const rust = rustTestCommands(
      jsonLines(build.stdout),
      JSON.parse(metadata.stdout).packages,
    );
    // Cargo reports dependencies first; running that order leaves the largest
    // application suites until the end. Use executable size only as a scheduling
    // hint. Every discovered target still runs, with no cached success/skip state.
    const sizes = new Map(
      await Promise.all(
        rust.map(async (task) =>
          [task.command, (await Deno.stat(task.command)).size] as const
        ),
      ),
    );
    rust.sort((a, b) =>
      sizes.get(b.command)! - sizes.get(a.command)! ||
      a.name.localeCompare(b.name)
    );
    summary.rustBinaries = rust.length;
    console.log(`Listing all ${rust.length} Rust binaries before sharding…`);
    const discoveryStart = performance.now();
    const discovery = await discoverRustCommands(rust, jobs, {
      output,
      signal: cancellation.signal,
      run,
    });
    summary.discoverySeconds = (performance.now() - discoveryStart) / 1000;
    summary.inventory = "inventory.json";
    summary.discoveryComplete = discovery.inventory.complete;
    summary.selectedCases = discovery.inventory.selectedCases;
    summary.discovery = discovery.inventory.binaries.map((
      { name, listing, error },
    ) => ({ name, listing, error }));
    if (!discovery.inventory.complete) {
      throw new Error(
        "Rust discovery failed; see summary.json, inventory.json and list-*.stderr",
      );
    }
    summary.phase = "execution";
    const tasks: TestCommand[] = [
      {
        name: "rust/doctests",
        command: "cargo",
        args: [
          "test",
          "--workspace",
          "--exclude",
          "yoi-e2e",
          "--doc",
          "--no-fail-fast",
        ],
        cwd: root,
      },
      {
        name: "web/unit",
        command: "deno",
        args: ["task", "test"],
        cwd: `${root}/web/workspace`,
      },
      {
        name: "web/component",
        command: "deno",
        args: ["task", "test:component"],
        cwd: `${root}/web/workspace`,
      },
      {
        name: "web-ux/unit",
        command: "deno",
        args: ["task", "test"],
        cwd: `${root}/tools/web-ux`,
      },
      {
        name: "tools/unit",
        command: "deno",
        args: [
          "test",
          "--allow-read",
          "--allow-write",
          "--allow-run",
          "--allow-env",
          "--allow-sys=uid",
          "tools/test.test.ts",
          "tools/check-state-contract-names.test.ts",
        ],
        cwd: root,
      },
      ...discovery.tasks,
    ];
    console.log(
      `Running ${rust.length} Rust binaries (${discovery.inventory.selectedCases} selected names, including ignored; ${discovery.tasks.length} suites/shards), doctests, Web and runner suites; at most ${jobs} top-level suites/shards (worker/worker-runtime/yoi-workspace-server: 1 test thread; other large Rust shards: 2; other internal parallelism unchanged; NOT a total process limit). Logs: ${output}`,
    );
    const results: ReturnType<typeof resultSummary>[] = [];
    summary.results = results;
    await mapConcurrent(tasks, jobs, async (task, index) => {
      const label = `${String(index).padStart(3, "0")}-${
        task.name.replaceAll(/[^\w.-]/g, "-")
      }`;
      const result = await run(task, label);
      console.log(
        `${result.code ? "FAIL" : "PASS"} ${task.name} (${
          result.seconds.toFixed(2)
        }s)`,
      );
      if (result.code) {
        console.error(result.stdout.slice(-8000), result.stderr.slice(-8000));
      }
      results[index] = resultSummary(result);
    });
    const seconds = (performance.now() - executionStart) / 1000;
    const failed = results.filter((result) => result.code !== 0);
    summary.failed = failed.length > 0;
    console.log(
      `Execution: ${seconds.toFixed(2)}s; ${
        results.length - failed.length
      } suites passed, ${failed.length} failed. Build: ${
        build.seconds.toFixed(2)
      }s.`,
    );
    if (failed.length) Deno.exitCode = 1;
  } catch (error) {
    summary.failed = true;
    summary.error = String(error);
    console.error(error);
    if (!Deno.exitCode) Deno.exitCode = 1;
  } finally {
    if (executionStart !== undefined) {
      summary.executionSeconds = (performance.now() - executionStart) / 1000;
    }
    try {
      if (cancellation.signal.aborted) {
        summary.cancelled = true;
        summary.cancellationReason = String(cancellation.signal.reason);
        summary.failed = true;
        Deno.exitCode = cancellationExitCode(cancellation.signal.reason);
      }
      await writeSummary();
    } finally {
      cancellation.dispose();
    }
  }
}

if (import.meta.main) await main();

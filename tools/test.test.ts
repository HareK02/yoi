import {
  defaultJobs,
  discoverRustCommands,
  execute,
  installCancellationHandlers,
  mapConcurrent,
  parseJobs,
  parseRustListing,
  prepareTestIsolation,
  resultSummary,
  rustTestCommands,
  shardRustCommand,
} from "./test.ts";
import { spawn } from "node:child_process";
import process from "node:process";
import { fileURLToPath } from "node:url";

function assert(
  condition: unknown,
  message = "assertion failed",
): asserts condition {
  if (!condition) throw new Error(message);
}

Deno.test("Cargo artifact selection retains every test target and deduplicates executable reports", () => {
  const test = (path: string, target: string) => ({
    reason: "compiler-artifact",
    package_id: "pkg",
    executable: path,
    target: { name: target },
    profile: { test: true },
  });
  const commands = rustTestCommands([
    test("/repo/target/lib-test", "lib"),
    test("/repo/target/integration", "integration"),
    test("/repo/target/integration", "integration"),
    { ...test("/repo/target/app", "app"), profile: { test: false } },
    { reason: "build-finished" },
  ], [{
    id: "pkg",
    name: "example",
    manifest_path: "/repo/crates/example/Cargo.toml",
  }]);
  assert(commands.length === 2);
  assert(commands[0].cwd === "/repo/crates/example");
  assert(commands[0].env?.CARGO_MANIFEST_DIR === commands[0].cwd);
  assert(commands[1].name === "example/integration");
  assert(
    commands.every((command) => command.args.length === 0),
    "no filtering or ignore flags",
  );
});

Deno.test("missing Cargo package or executables fails instead of claiming an empty suite passed", () => {
  for (
    const artifacts of [[], [{
      reason: "compiler-artifact",
      package_id: "missing",
      executable: "/test",
      profile: { test: true },
      target: { name: "test" },
    }]]
  ) {
    let failed = false;
    try {
      rustTestCommands(artifacts, []);
    } catch {
      failed = true;
    }
    assert(failed);
  }
});

Deno.test("bounded scheduling runs every command once and retains failing results", async () => {
  let active = 0, maximum = 0;
  const seen: number[] = [];
  const results = await mapConcurrent([0, 1, 2, 3, 4], 2, async (item) => {
    active++;
    maximum = Math.max(active, maximum);
    await Promise.resolve();
    seen.push(item);
    active--;
    return item === 2 ? 1 : 0;
  });
  assert(maximum === 2);
  assert(seen.sort().join() === "0,1,2,3,4");
  assert(results.join() === "0,0,1,0,0");
  for (const jobs of [0, -1, 1.5]) {
    let failed = false;
    try {
      await mapConcurrent([], jobs, () => Promise.resolve(0));
    } catch {
      failed = true;
    }
    assert(failed);
  }
});

function assertThrows(fn: () => unknown) {
  let failed = false;
  try {
    fn();
  } catch {
    failed = true;
  }
  assert(failed, "expected failure");
}

Deno.test("CPU-derived jobs accommodates concurrent suites within the CLI bound", () => {
  assert(defaultJobs(24) === 32);
  assert(defaultJobs(128) === 32);
  assert(defaultJobs(6) === 8);
  assert(defaultJobs(1) === 2);
  assert(parseJobs([]) === defaultJobs());
  assert(parseJobs(["--jobs", "64"]) === 64);
  assert(parseJobs(["--jobs", "1"]) === 1);
  for (
    const args of [
      ["--jobs", "65"],
      ["--jobs", "0"],
      ["--jobs", "1.5"],
      ["--jobs", "NaN"],
      ["--jobs"],
      ["--other", "4"],
    ]
  ) {
    assertThrows(() => parseJobs(args));
  }
});

Deno.test("Rust terse listing accepts every name and fails closed on unknown or duplicate lines", () => {
  assert(parseRustListing("").length === 0);
  assert(
    parseRustListing("unit::a: test\r\nunit::bench: benchmark\r\n").join() ===
      "unit::a,unit::bench",
  );
  for (
    const text of [
      "unit::a\n",
      "unit::a: ignored\n",
      "unit::a: test\n1 test, 0 benchmarks\n",
      "unit::a: test\nunit::a: test\n",
      "\n",
      "unit::a: test\n\n",
      "\x1b[31munit::a: test\n",
    ]
  ) {
    assertThrows(() => parseRustListing(text));
  }
});

function listedNames(count: number) {
  return Array.from({ length: count }, (_, i) => `unit::case_${i}`);
}

Deno.test("round-robin shards retain every listed name exactly once at thresholds and the 32-shard cap", () => {
  const task = {
    name: "pkg/binary",
    command: "/test-binary",
    args: [],
    cwd: "/pkg",
    env: { CARGO_MANIFEST_DIR: "/pkg" },
  };
  for (const count of [0, 1, 72, 73, 108, 1135, 1152, 1153, 2000]) {
    const names = parseRustListing(
      listedNames(count).map((name) => `${name}: test\n`).join(""),
    );
    const shards = shardRustCommand(task, names);
    const assigned = shards.flatMap((shard) => shard.selectedNames);
    assert(assigned.length === count && new Set(assigned).size === count);
    assert(
      [...assigned].sort().join("\n") === [...names].sort().join("\n"),
      "no names may be excluded",
    );
    if (count <= 72) {
      assert(
        shards.length === 1 && shards[0].task === task,
        "small binaries must be unchanged",
      );
    } else {
      const expected = Math.min(32, Math.ceil(count / 36));
      assert(shards.length === expected);
      const lengths = shards.map((shard) => shard.selectedNames.length);
      assert(Math.max(...lengths) - Math.min(...lengths) <= 1);
      shards.forEach((shard, index) => {
        assert(
          shard.selectedNames.join() ===
            names.filter((_, i) => i % expected === index).join(),
          "must distribute round-robin",
        );
        assert(
          shard.task.args.slice(0, 3).join() === "--exact,--test-threads,2",
        );
        assert(shard.task.args.slice(3).join() === shard.selectedNames.join());
        assert(
          !shard.task.args.includes("--ignored") &&
            !shard.task.args.includes("--include-ignored"),
        );
        assert(shard.task.cwd === task.cwd && shard.task.env === task.env);
      });
    }
  }
  assertThrows(() => shardRustCommand(task, ["same", "same"]));
});

Deno.test("global runtime-path packages serialize every normal target without losing names", () => {
  for (const pkg of ["worker", "worker-runtime", "yoi-workspace-server"]) {
    for (const target of ["lib", "controller_restore_test"]) {
      const task = {
        name: `${pkg}/${target}`,
        command: "/test-binary",
        args: [],
        cwd: "/pkg",
      };
      for (const count of [0, 1, 59, 72, 73, 1135]) {
        const names = listedNames(count);
        const shards = shardRustCommand(task, names);
        assert(
          shards.flatMap((shard) => shard.selectedNames).sort().join() ===
            [...names].sort().join(),
        );
        for (const shard of shards) {
          const args = shard.task.args;
          assert(args[args.indexOf("--test-threads") + 1] === "1");
          if (count > 72) {
            assert(args[0] === "--exact");
            assert(args.slice(3).join() === shard.selectedNames.join());
          } else {
            assert(args.join() === "--test-threads,1");
          }
          assert(!args.includes("--ignored") && !args.includes("--skip"));
        }
      }
      assert(task.args.length === 0, "do not mutate discovered commands");
    }
  }
});

Deno.test("scheduler drains active workers before propagating a callback error", async () => {
  let drained = false;
  let failed = false;
  try {
    await mapConcurrent([0, 1], 2, async (item) => {
      if (!item) throw new Error("callback failed");
      await new Promise((resolve) => setTimeout(resolve, 10));
      drained = true;
    });
  } catch {
    failed = true;
  }
  assert(failed && drained);
});

const fixturePath = fileURLToPath(import.meta.url);
function fixtureArgs(mode: string, output: string) {
  return [
    "run",
    "--allow-read",
    "--allow-write",
    "--allow-run",
    "--allow-env",
    fixturePath,
    mode,
    output,
  ];
}
function fixtureTask(mode: string, output: string) {
  return {
    name: `fixture/${mode}`,
    command: Deno.execPath(),
    args: fixtureArgs(mode, output),
    cwd: output,
  };
}
async function waitUntil(
  check: () => boolean | Promise<boolean>,
  message: string,
) {
  const end = performance.now() + 10_000;
  while (performance.now() < end) {
    if (await check()) return;
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
  throw new Error(message);
}
function absent(pid: number) {
  try {
    process.kill(pid, 0);
    return false;
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ESRCH") return true;
    throw error;
  }
}
function emergencyKill(pid: number) {
  try {
    process.kill(-pid, "SIGKILL");
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ESRCH") throw error;
  }
}

async function assertRejects(
  run: () => Promise<unknown>,
  message: string,
) {
  try {
    await run();
  } catch (error) {
    assert(String(error).includes(message), String(error));
    return;
  }
  throw new Error(`expected rejection containing ${message}`);
}

Deno.test({
  name:
    "isolated real children use bounded disjoint tables and sanitized Git/compiler environments",
  ignore: Deno.build.os !== "linux",
  async fn() {
    const checkout = await Deno.realPath(await Deno.makeTempDir());
    const environmentSnapshot = () =>
      JSON.stringify(
        Object.entries(process.env).sort(([a], [b]) => a.localeCompare(b)),
      );
    const before = environmentSnapshot();
    try {
      // An ancestor repository must not make an uninitialized tempfile look like
      // a repository. No developer repository or configuration is borrowed.
      for (const dir of [".git/objects", ".git/refs"]) {
        await Deno.mkdir(`${checkout}/${dir}`, { recursive: true });
      }
      await Deno.writeTextFile(
        `${checkout}/.git/HEAD`,
        "ref: refs/heads/main\n",
      );
      const [first, second] = await Promise.all([
        prepareTestIsolation(checkout, "same/output/id"),
        prepareTestIsolation(checkout, "same/output/id"),
      ]);
      assert(
        first.root !== second.root,
        "concurrent runner ids must not collide",
      );
      const task = {
        ...fixtureTask("isolation-env", checkout),
        env: {
          CARGO_MANIFEST_DIR: `${checkout}/crates/example`,
          CARGO_HOME: `${checkout}/compiler-cache`,
          RUSTUP_HOME: `${checkout}/toolchain-cache`,
          YOI_HOME: "/live/yoi",
          YOI_CONFIG_DIR: "/live/config",
          YOI_DATA_DIR: "/live/data",
          YOI_RUNTIME_DIR: "/live/run",
          HOME: "/live/home",
          TMPDIR: "/deleted/compiler-tmp",
          GIT_DIR: "/live/git",
          GIT_WORK_TREE: "/live/tree",
          GIT_CONFIG: "/live/gitconfig",
          GIT_CONFIG_GLOBAL: "/live/gitconfig",
          GIT_CONFIG_SYSTEM: "/live/system-gitconfig",
          GIT_TEMPLATE_DIR: "/live/template",
          GIT_CONFIG_PARAMETERS: "'core.hooksPath'='/live/hooks'",
          GIT_CONFIG_COUNT: "1",
          GIT_CONFIG_KEY_0: "core.hooksPath",
          GIT_CONFIG_VALUE_0: "/live/hooks",
          HTTP_PROXY: "http://live.invalid",
          https_proxy: "http://live.invalid",
          ALL_PROXY: "http://live.invalid",
          NO_PROXY: "live.invalid",
          RUSTC_WRAPPER: "/live/sccache",
          RUSTC_WORKSPACE_WRAPPER: "/live/wrapper",
          SCCACHE_DISABLE: "0",
        },
      };
      const taskBefore = JSON.stringify(task);
      const commands = await Promise.all([
        first.wrapCommand(task, "pkg/a"),
        first.wrapCommand(task, "pkg?a"), // same sanitized label
        first.wrapCommand(task, "pkg/a"), // same exact label
        second.wrapCommand(task, "pkg/a"),
      ]);
      assert(JSON.stringify(task) === taskBefore);
      assert(
        environmentSnapshot() === before,
        "host environment changed",
      );
      const results = await Promise.all(
        commands.map((command, index) =>
          execute(command, {
            output: checkout,
            label: `environment-${index}`,
            timeoutMs: 10_000,
          })
        ),
      );
      const tables = new Set<string>();
      for (const [index, result] of results.entries()) {
        assert(result.code === 0 && !result.error, JSON.stringify(result));
        const observed = JSON.parse(result.stdout);
        const env = observed.env as Record<string, string>;
        assert(observed.cwd === checkout && commands[index].cwd === task.cwd);
        assert(commands[index].args === task.args);
        assert(env.CARGO_MANIFEST_DIR === task.env.CARGO_MANIFEST_DIR);
        assert(env.CARGO_HOME === task.env.CARGO_HOME);
        assert(env.RUSTUP_HOME === task.env.RUSTUP_HOME);
        const namespace = env.YOI_RUNTIME_DIR.replace(/\/run$/, "");
        const prefix = `${checkout}/target/restore-test-environment/`;
        assert(namespace.startsWith(prefix));
        const suffix = namespace.slice(prefix.length);
        assert(
          suffix.split("/").every((part) => /^[a-zA-Z0-9_-]+$/.test(part)),
        );
        assert(env.YOI_CONFIG_DIR === `${namespace}/config`);
        assert(env.YOI_DATA_DIR === `${namespace}/data`);
        assert(
          env.YOI_HOME === env.YOI_DATA_DIR,
          "secret data must equal data",
        );
        assert(env.HOME === `${namespace}/home`);
        for (
          const dir of [
            namespace,
            env.YOI_CONFIG_DIR,
            env.YOI_DATA_DIR,
            env.YOI_RUNTIME_DIR,
            env.HOME,
          ]
        ) {
          assert((await Deno.lstat(dir)).isDirectory);
          assert(await Deno.realPath(dir) === dir);
        }
        for (const key of ["TMPDIR", "TMP", "TEMP"]) {
          assert(env[key] === `${checkout}/target/t`);
        }
        for (
          const key of [
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_CONFIG",
            "GIT_CONFIG_PARAMETERS",
            "GIT_CONFIG_COUNT",
            "GIT_CONFIG_KEY_0",
            "GIT_CONFIG_VALUE_0",
            "HTTP_PROXY",
            "https_proxy",
            "ALL_PROXY",
            "NO_PROXY",
          ]
        ) {
          assert(!(key in env), `${key} leaked to child`);
        }
        assert(env.GIT_CONFIG_GLOBAL === "/dev/null");
        assert(env.GIT_CONFIG_SYSTEM === "/dev/null");
        assert(env.GIT_CONFIG_NOSYSTEM === "1");
        assert(env.GIT_CEILING_DIRECTORIES === checkout);
        assert(env.GIT_TEMPLATE_DIR === `${namespace}/git-template`);
        assert(observed.globalConfig === "");
        assert(observed.ancestorDiscovered === false);
        assert(observed.hooks.length === 0 && observed.info.length === 0);
        assert(observed.author.includes("normal-tests@example.invalid"));
        assert(env.GIT_AUTHOR_NAME === env.GIT_COMMITTER_NAME);
        assert(env.GIT_AUTHOR_EMAIL === env.GIT_COMMITTER_EMAIL);
        assert(env.RUSTC_WRAPPER === "" && env.RUSTC_WORKSPACE_WRAPPER === "");
        assert(env.SCCACHE_DISABLE === "1");
        const table = `${env.YOI_RUNTIME_DIR}/workers.json`;
        tables.add(table);
        assert(await Deno.readTextFile(table) === String(observed.pid));
      }
      assert(
        tables.size === commands.length,
        "independent commands shared a table",
      );
      assert(environmentSnapshot() === before);
    } finally {
      await Deno.remove(checkout, { recursive: true });
    }
  },
});

Deno.test("isolation refuses lexical aliases and symlinked owned paths before external writes", async () => {
  for (
    const unsafe of ["target", "target/restore-test-environment", "target/t"]
  ) {
    const checkout = await Deno.realPath(await Deno.makeTempDir());
    const outside = await Deno.realPath(await Deno.makeTempDir());
    try {
      await Deno.writeTextFile(`${outside}/sentinel`, "untouched");
      if (unsafe !== "target") await Deno.mkdir(`${checkout}/target`);
      await Deno.symlink(outside, `${checkout}/${unsafe}`);
      await assertRejects(
        () => prepareTestIsolation(checkout, "run"),
        "symlink",
      );
      assert(
        Array.from(Deno.readDirSync(outside)).map((entry) => entry.name)
          .join() ===
          "sentinel",
      );
      assert(await Deno.readTextFile(`${outside}/sentinel`) === "untouched");
    } finally {
      await Deno.remove(checkout, { recursive: true });
      await Deno.remove(outside, { recursive: true });
    }
  }
  const checkout = await Deno.realPath(await Deno.makeTempDir());
  try {
    await Deno.symlink(checkout, `${checkout}/alias`);
    await assertRejects(
      () => prepareTestIsolation(`${checkout}/alias`, "run"),
      "symlink",
    );
    for (const suffix of ["/../escape", "/./child", "//child"]) {
      await assertRejects(
        () => prepareTestIsolation(`${checkout}${suffix}`, "run"),
        "normal-only",
      );
    }
    assert(
      Array.from(Deno.readDirSync(checkout)).map((entry) => entry.name)
        .join() ===
        "alias",
    );
  } finally {
    await Deno.remove(checkout, { recursive: true });
  }
});

Deno.test("command wrapping rechecks owned ancestors and refuses symlinked namespace children", async () => {
  for (
    const unsafe of [
      "ancestor",
      "tmp",
      "config",
      "data",
      "run",
      "home",
      "git-template",
      "git-template/info",
      "git-template/hooks",
    ]
  ) {
    const checkout = await Deno.realPath(await Deno.makeTempDir());
    const outside = await Deno.realPath(await Deno.makeTempDir());
    try {
      const isolation = await prepareTestIsolation(checkout, "run");
      let path: string;
      if (unsafe === "ancestor") {
        path = isolation.root;
        await Deno.remove(path);
      } else if (unsafe === "tmp") {
        path = `${checkout}/target/t`;
        await Deno.remove(path);
      } else {
        const namespace = `${isolation.root}/unsafe-0`;
        await Deno.mkdir(namespace);
        if (unsafe.includes("/")) await Deno.mkdir(`${namespace}/git-template`);
        path = `${namespace}/${unsafe}`;
      }
      await Deno.symlink(outside, path);
      await assertRejects(
        () => isolation.wrapCommand(fixtureTask("success", checkout), "unsafe"),
        "symlink",
      );
      assert(Array.from(Deno.readDirSync(outside)).length === 0);
    } finally {
      await Deno.remove(checkout, { recursive: true });
      await Deno.remove(outside, { recursive: true });
    }
  }
});

for (const [mode, code] of [["success", 0], ["nonzero", 7]] as const) {
  Deno.test({
    name: `execute streams real ${mode} child output and records its exit`,
    ignore: Deno.build.os !== "linux",
    async fn() {
      const output = await Deno.makeTempDir();
      try {
        const result = await execute({
          ...fixtureTask(mode, output),
          inventoryRef: { binaryIndex: 3, shardIndex: 1 },
        }, {
          output,
          label: mode,
          timeoutMs: 10_000,
        });
        assert(result.code === code, JSON.stringify(result));
        assert(!result.timedOut);
        assert(!result.error, result.error);
        assert(result.stdout.includes("fixture stdout 日本語"));
        assert(result.stderr.includes("fixture stderr"));
        assert(
          await Deno.readTextFile(`${output}/${mode}.stdout`) === result.stdout,
        );
        assert(
          await Deno.readTextFile(`${output}/${mode}.stderr`) === result.stderr,
        );
        const summary = JSON.parse(
          await Deno.readTextFile(`${output}/${mode}.result.json`),
        );
        assert(summary.code === code && summary.timedOut === false);
        assert(summary.log === mode && summary.timeoutMs === 10_000);
        assert(
          summary.inventoryRef.binaryIndex === 3 &&
            summary.inventoryRef.shardIndex === 1,
          "results must link unambiguously to inventory entries",
        );
        assert(
          !("stdout" in summary),
          "summary references logs, not full output",
        );
        assert(
          absent(Number(await Deno.readTextFile(`${output}/parent.pid`))),
          "direct child must be reaped",
        );
      } finally {
        await Deno.remove(output, { recursive: true });
      }
    },
  });
}

Deno.test({
  name:
    "execute preserves live stdout/stderr and times out a group containing a grandchild",
  ignore: Deno.build.os !== "linux",
  async fn() {
    const output = await Deno.makeTempDir();
    let parent: number | undefined;
    let finished = false;
    const running = execute(fixtureTask("tree", output), {
      output,
      label: "tree",
      timeoutMs: 4_000,
      killGraceMs: 500,
    }).finally(() => finished = true);
    try {
      await waitUntil(async () => {
        const text = await Deno.readTextFile(`${output}/tree.stdout`);
        return text.includes("grandchild ready");
      }, "grandchild did not become ready");
      parent = Number(await Deno.readTextFile(`${output}/parent.pid`));
      const grandchild = Number(
        await Deno.readTextFile(`${output}/grandchild.pid`),
      );
      assert(!finished, "logs must be visible before execute resolves");
      assert(
        (await Deno.readTextFile(`${output}/tree.stderr`)).includes(
          "fixture stderr",
        ),
      );
      assert(
        !absent(parent) && !absent(grandchild),
        "both processes must actually be running",
      );
      const result = await running;
      assert(result.timedOut && result.code === 124, JSON.stringify(result));
      assert(!result.error, result.error);
      assert(result.seconds < 10, "timeout cleanup must be bounded");
      assert(result.stderr.includes("timeout after 4000ms"));
      assert(
        result.stderr.includes("grandchild reaped"),
        "fixture parent must reap its child",
      );
      assert(absent(parent), "direct child must be reaped by runner");
      assert(absent(grandchild), "grandchild must be stopped and reaped");
      const summary = JSON.parse(
        await Deno.readTextFile(`${output}/tree.result.json`),
      );
      assert(summary.timedOut && summary.code === 124);
      assert(summary.name === "fixture/tree" && summary.log === "tree");
    } finally {
      if (parent !== undefined) emergencyKill(parent);
      await running;
      await Deno.remove(output, { recursive: true });
    }
  },
});

Deno.test({
  name:
    "execute escalates SIGTERM to SIGKILL and reaps a stubborn direct child",
  ignore: Deno.build.os !== "linux",
  async fn() {
    const output = await Deno.makeTempDir();
    try {
      const result = await execute(fixtureTask("stubborn", output), {
        output,
        label: "stubborn",
        timeoutMs: 2_000,
        killGraceMs: 100,
      });
      assert(result.stdout.includes("stubborn ready"));
      assert(result.stderr.includes("ignored SIGTERM"));
      assert(
        result.timedOut && result.code === 124 && result.signal === "SIGKILL",
        JSON.stringify(result),
      );
      assert(!result.error, result.error);
      assert(result.seconds < 8);
      assert(absent(Number(await Deno.readTextFile(`${output}/parent.pid`))));
    } finally {
      await Deno.remove(output, { recursive: true });
    }
  },
});

Deno.test({
  name: "execute records spawn failure instead of an empty passing suite",
  ignore: Deno.build.os !== "linux",
  async fn() {
    const output = await Deno.makeTempDir();
    try {
      const result = await execute({
        name: "missing",
        command: `${output}/does-not-exist`,
        args: [],
        cwd: output,
      }, { output, label: "missing", timeoutMs: 2_000, killGraceMs: 10 });
      assert(result.code !== 0 && !result.timedOut);
      assert(result.error?.includes("ENOENT"), JSON.stringify(result));
      assert(
        (await Deno.readTextFile(`${output}/missing.stderr`)).includes(
          "ENOENT",
        ),
      );
      const summary = JSON.parse(
        await Deno.readTextFile(`${output}/missing.result.json`),
      );
      assert(summary.code !== 0 && summary.error.includes("ENOENT"));
    } finally {
      await Deno.remove(output, { recursive: true });
    }
  },
});

Deno.test({
  name:
    "runtime listing persists complete name inventory for large and small binaries",
  ignore: Deno.build.os !== "linux",
  async fn() {
    const output = await Deno.realPath(await Deno.makeTempDir());
    try {
      const isolation = await prepareTestIsolation(output, "discovery");
      const wrappedTables = new Set<string>();
      const discovery = await discoverRustCommands(
        [
          fixtureTask("list-large", output),
          fixtureTask("list-small", output),
        ],
        2,
        {
          output,
          run: async (task, label) => {
            const wrapped = await isolation.wrapCommand(task, label);
            wrappedTables.add(`${wrapped.env!.YOI_RUNTIME_DIR}/workers.json`);
            return await execute(wrapped, { output, label, timeoutMs: 10_000 });
          },
        },
      );
      assert(
        wrappedTables.size === 2,
        "all discovery launches must use the callback",
      );
      assert(discovery.inventory.complete);
      assert(
        discovery.inventory.rustBinaries === 2 &&
          discovery.inventory.selectedCases === 1137,
      );
      assert(discovery.tasks.length === 33);
      const persisted = JSON.parse(
        await Deno.readTextFile(`${output}/inventory.json`),
      ) as typeof discovery.inventory;
      assert(persisted.complete && persisted.selectedCases === 1137);
      const large = persisted.binaries[0];
      assert(large.shards.length === 32 && large.selectedCases === 1135);
      persisted.binaries.forEach((binary, binaryIndex) =>
        binary.shards.forEach((shard, shardIndex) => {
          assert(shard.task.inventoryRef?.binaryIndex === binaryIndex);
          assert(shard.task.inventoryRef?.shardIndex === shardIndex);
        })
      );
      const assigned = large.shards.flatMap((shard) => shard.selectedNames);
      assert(new Set(assigned).size === 1135 && assigned.length === 1135);
      assert([...assigned].sort().join() === listedNames(1135).sort().join());
      assert(
        persisted.binaries[1].shards[0].task.args.join() ===
          fixtureTask("list-small", output).args.join(),
      );
      assert(
        persisted.binaries.every((binary) =>
          binary.listing.code === 0 && binary.listing.seconds > 0
        ),
      );
    } finally {
      await Deno.remove(output, { recursive: true });
    }
  },
});

for (const mode of ["list-nonzero", "list-unknown"]) {
  Deno.test({
    name:
      `discovery ${mode} fails with saved evidence rather than running a partial selection`,
    ignore: Deno.build.os !== "linux",
    async fn() {
      const output = await Deno.makeTempDir();
      try {
        const discovery = await discoverRustCommands(
          [
            fixtureTask("list-small", output),
            fixtureTask(mode, output),
          ],
          2,
          { output },
        );
        assert(!discovery.inventory.complete);
        assert(discovery.inventory.selectedCases === null);
        assert(discovery.tasks.length === 0);
        const persisted = JSON.parse(
          await Deno.readTextFile(`${output}/inventory.json`),
        ) as typeof discovery.inventory;
        assert(
          !persisted.complete && persisted.binaries[1].shards.length === 0,
        );
        assert(
          persisted.binaries[1].error?.includes(
            mode === "list-nonzero" ? "code 7" : "unknown Rust listing format",
          ),
        );
        const listing = JSON.parse(
          await Deno.readTextFile(`${output}/list-001.result.json`),
        );
        assert(listing.code === (mode === "list-nonzero" ? 7 : 0));
        assert(
          persisted.discoveredCases === 2,
          "partial discovery must not be presented as complete selection",
        );
      } finally {
        await Deno.remove(output, { recursive: true });
      }
    },
  });
}

async function textIfPresent(path: string): Promise<string> {
  try {
    return await Deno.readTextFile(path);
  } catch (error) {
    if (error instanceof Deno.errors.NotFound) return "";
    throw error;
  }
}

for (const signal of ["SIGINT", "SIGTERM"] as const) {
  Deno.test({
    name:
      `parent ${signal} cleans up active groups, reaps children and does not spawn queued commands`,
    ignore: Deno.build.os !== "linux",
    async fn() {
      const output = await Deno.makeTempDir();
      const parent = new Deno.Command(Deno.execPath(), {
        args: fixtureArgs("runner-parent", output),
        stdout: "piped",
        stderr: "piped",
      }).spawn();
      const done = parent.output();
      const pids: number[] = [];
      try {
        await waitUntil(
          async () =>
            (await textIfPresent(`${output}/tree/tree.stdout`)).includes(
              "grandchild ready",
            ) &&
            (await textIfPresent(`${output}/stubborn/stubborn.stdout`))
              .includes("stubborn ready"),
          "parent runner fixtures did not become ready",
        );
        for (
          const file of [
            "tree/parent.pid",
            "tree/grandchild.pid",
            "stubborn/parent.pid",
          ]
        ) {
          pids.push(Number(await Deno.readTextFile(`${output}/${file}`)));
        }
        process.kill(parent.pid, signal);
        const status = await done;
        assert(
          status.code === (signal === "SIGINT" ? 130 : 143),
          new TextDecoder().decode(status.stderr),
        );
        assert(absent(parent.pid));
        assert(
          pids.every(absent),
          "active children and grandchild must be reaped",
        );
        const results = JSON.parse(
          await Deno.readTextFile(`${output}/cancel-summary.json`),
        ) as ReturnType<typeof resultSummary>[];
        assert(results.length === 3);
        assert(
          results.every((result) =>
            result.cancelled && result.cancellationReason === signal &&
            !result.timedOut && !result.error
          ),
        );
        assert(
          results[1].signal === "SIGKILL",
          "stubborn child requires escalation on parent cancellation too",
        );
        assert(
          await textIfPresent(`${output}/queued/parent.pid`) === "",
          "cancelled queue must not spawn",
        );
        assert(
          (await Deno.readTextFile(`${output}/tree/tree.stderr`)).includes(
            "grandchild reaped",
          ),
        );
      } finally {
        if (!absent(parent.pid)) process.kill(parent.pid, "SIGTERM");
        await done;
        for (const pid of pids) emergencyKill(pid);
        await Deno.remove(output, { recursive: true });
      }
    },
  });
}

// Real subprocess fixtures live here rather than introducing another executable.
// A watchdog is independent of execute, so a broken runner cannot leak fixtures
// forever. The tree's parent cooperatively reaps its child on TERM; KILL behavior
// is exercised separately by the stubborn fixture.
if (import.meta.main) {
  const [mode, output] = Deno.args;
  setTimeout(() => Deno.exit(99), 20_000);
  if (mode.startsWith("list-")) {
    assert(
      Deno.args.slice(2).join() === "--list,--format,terse",
      "discovery must use libtest terse listing",
    );
    if (mode === "list-unknown") {
      console.log("unexpected harness output");
    } else {
      console.log(
        listedNames(mode === "list-large" ? 1135 : 2).map((name) =>
          `${name}: test`
        ).join("\n"),
      );
    }
    Deno.exit(mode === "list-nonzero" ? 7 : 0);
  } else if (mode === "isolation-env") {
    const env = Deno.env.toObject();
    const scratch = await Deno.makeTempDir({ dir: env.TMPDIR });
    const text = (bytes: Uint8Array) => new TextDecoder().decode(bytes);
    async function git(args: string[]) {
      return await new Deno.Command("git", {
        args,
        cwd: scratch,
        stdout: "piped",
        stderr: "piped",
      }).output();
    }
    try {
      const ownedRepo = await git([
        "-C",
        output,
        "rev-parse",
        "--show-toplevel",
      ]);
      assert(
        ownedRepo.success && text(ownedRepo.stdout).trim() === output,
        "owned ancestor repository fixture must be discoverable at its root",
      );
      const ancestor = await git(["rev-parse", "--show-toplevel"]);
      const init = await git(["init", "--quiet"]);
      assert(init.success, text(init.stderr));
      const global = await git(["config", "--global", "--list"]);
      assert(global.success, text(global.stderr));
      const author = await git(["var", "GIT_AUTHOR_IDENT"]);
      assert(author.success, text(author.stderr));
      await Deno.writeTextFile(
        `${env.YOI_RUNTIME_DIR}/workers.json`,
        String(Deno.pid),
      );
      console.log(JSON.stringify({
        env,
        cwd: Deno.cwd(),
        pid: Deno.pid,
        ancestorDiscovered: ancestor.success,
        globalConfig: text(global.stdout),
        author: text(author.stdout),
        hooks: Array.from(Deno.readDirSync(`${scratch}/.git/hooks`)),
        info: Array.from(Deno.readDirSync(`${scratch}/.git/info`)),
      }));
    } finally {
      await Deno.remove(scratch, { recursive: true });
    }
    Deno.exit(0);
  } else if (mode === "runner-parent") {
    const cancellation = installCancellationHandlers();
    try {
      for (const dir of ["tree", "stubborn", "queued"]) {
        await Deno.mkdir(`${output}/${dir}`);
      }
      const results = await mapConcurrent(
        ["tree", "stubborn", "queued"],
        2,
        async (name) => {
          const dir = `${output}/${name}`;
          return resultSummary(
            await execute(
              fixtureTask(name === "queued" ? "success" : name, dir),
              {
                output: dir,
                label: name,
                timeoutMs: 15_000,
                killGraceMs: 200,
                signal: cancellation.signal,
              },
            ),
          );
        },
      );
      await Deno.writeTextFile(
        `${output}/cancel-summary.json`,
        JSON.stringify(results),
      );
      Deno.exitCode = results[0].code;
    } finally {
      cancellation.dispose();
    }
    Deno.exit(Deno.exitCode);
  } else if (mode === "grandchild") {
    Deno.addSignalListener("SIGTERM", () => Deno.exit(0));
    Deno.writeTextFileSync(`${output}/grandchild.pid`, String(Deno.pid));
    console.log("grandchild ready");
  } else {
    Deno.writeTextFileSync(`${output}/parent.pid`, String(Deno.pid));
    console.log("fixture stdout 日本語");
    console.error("fixture stderr");
    if (mode === "success" || mode === "nonzero") {
      Deno.exit(mode === "success" ? 0 : 7);
    } else if (mode === "stubborn") {
      Deno.addSignalListener("SIGTERM", () => console.error("ignored SIGTERM"));
      console.log("stubborn ready");
    } else if (mode === "tree") {
      // The grandchild inherits the new group from execute's direct child.
      // The parent intentionally does NOT forward TERM: only group signaling
      // can stop the grandchild before its independent watchdog.
      Deno.addSignalListener("SIGTERM", () => {});
      const child = spawn(Deno.execPath(), fixtureArgs("grandchild", output), {
        stdio: "inherit",
      });
      child.on("close", () => {
        console.error("grandchild reaped");
        Deno.exit(0);
      });
    } else {
      throw new Error(`unknown fixture mode: ${mode}`);
    }
  }
}

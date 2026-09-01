import { assertEquals, assertRejects, assertStringIncludes } from "@std/assert";
import { join } from "@std/path";
import {
  assertBundleIsSecretFree,
  redactText,
  safeUrl,
  writePrivateJson,
} from "../src/artifacts.ts";
import {
  PROCESS_LOG_BYTE_LIMIT,
  startOwnedProcesses,
  stopOwnedProcesses,
} from "../src/processes.ts";

Deno.test("redaction removes common credentials and query values", () => {
  const redacted = redactText(
    "Authorization: Bearer abc.def cookie=session-value token=secret-value",
    ["abc.def"],
  );
  assertStringIncludes(redacted, "[REDACTED]");
  assertEquals(redacted.includes("abc.def"), false);
  assertEquals(redacted.includes("session-value"), false);
  assertEquals(
    safeUrl("https://user:pass@example.test/path?token=secret#fragment"),
    "https://example.test/path?token=%5BREDACTED%5D",
  );
  assertRejects(
    async () => assertBundleIsSecretFree('{"authorization":"Bearer abc"}'),
    Error,
    "forbidden secret marker",
  );
});

Deno.test("private JSON state uses owner-only permissions", async () => {
  const directory = await Deno.makeTempDir();
  try {
    const path = join(directory, "state", "owner.json");
    await writePrivateJson(path, { cookies: [], origins: [] });
    assertEquals(JSON.parse(await Deno.readTextFile(path)), { cookies: [], origins: [] });
    if (Deno.build.os !== "windows") assertEquals((await Deno.stat(path)).mode! & 0o777, 0o600);
  } finally {
    await Deno.remove(directory, { recursive: true });
  }
});

Deno.test("owned process is terminated and its logs are redacted", async () => {
  const directory = await Deno.makeTempDir();
  const scenario = join(directory, "scenario.json");
  await Deno.writeTextFile(scenario, "{}");
  try {
    const processes = await startOwnedProcesses(
      [{
        id: "fixture",
        command: Deno.execPath(),
        args: ["eval", 'console.log("authorization: secret-value"); setInterval(() => {}, 1000)'],
      }],
      scenario,
      join(directory, "logs"),
      ["secret-value"],
    );
    assertEquals(processes.length, 1);
    const logPath = join(directory, "logs", "fixture.stdout.log");
    for (let attempt = 0; attempt < 20; attempt++) {
      try {
        if ((await Deno.readTextFile(logPath)).length > 0) break;
      } catch {
        // The output pump creates the file asynchronously.
      }
      await new Promise((resolve) => setTimeout(resolve, 10));
    }
    const diagnostics = await stopOwnedProcesses(processes);
    assertEquals(diagnostics, []);
    const log = await Deno.readTextFile(logPath);
    assertEquals(log.includes("secret-value"), false);
    assertStringIncludes(log, "[REDACTED]");
    const metadata = JSON.parse(await Deno.readTextFile(`${logPath}.meta.json`));
    assertEquals(metadata.truncated, false);
  } finally {
    await Deno.remove(directory, { recursive: true });
  }
});

Deno.test("owned process logs stop at the byte limit and record truncation", async () => {
  const directory = await Deno.makeTempDir();
  const scenario = join(directory, "scenario.json");
  await Deno.writeTextFile(scenario, "{}");
  try {
    const processes = await startOwnedProcesses(
      [{
        id: "large-output",
        command: Deno.execPath(),
        args: [
          "eval",
          `console.log("x".repeat(${
            PROCESS_LOG_BYTE_LIMIT + 32_768
          })); setInterval(() => {}, 1000)`,
        ],
      }],
      scenario,
      join(directory, "logs"),
      [],
    );
    const logPath = join(directory, "logs", "large-output.stdout.log");
    for (let attempt = 0; attempt < 100; attempt++) {
      try {
        if ((await Deno.stat(logPath)).size >= PROCESS_LOG_BYTE_LIMIT) break;
      } catch {
        // The output pump creates the file asynchronously.
      }
      await new Promise((resolve) => setTimeout(resolve, 10));
    }
    assertEquals(await stopOwnedProcesses(processes), []);
    assertEquals((await Deno.stat(logPath)).size, PROCESS_LOG_BYTE_LIMIT);
    const metadata = JSON.parse(await Deno.readTextFile(`${logPath}.meta.json`));
    assertEquals(metadata.byteLimit, PROCESS_LOG_BYTE_LIMIT);
    assertEquals(metadata.truncated, true);
    assertEquals(metadata.bytesWritten, PROCESS_LOG_BYTE_LIMIT);
  } finally {
    await Deno.remove(directory, { recursive: true });
  }
});

Deno.test("forced cleanup terminates a TERM-resistant descendant", async () => {
  if (Deno.build.os === "windows") return;
  const directory = await Deno.makeTempDir();
  const scenario = join(directory, "scenario.json");
  const childPidPath = join(directory, "child.pid");
  await Deno.writeTextFile(scenario, "{}");
  try {
    const childProgram = 'Deno.addSignalListener("SIGTERM", () => {}); setInterval(() => {}, 1000)';
    const parentProgram = `
      const child = new Deno.Command(Deno.execPath(), {
        args: ["eval", ${JSON.stringify(childProgram)}],
        stdout: "null",
        stderr: "null"
      }).spawn();
      Deno.writeTextFileSync(Deno.args[0], String(child.pid));
      Deno.addSignalListener("SIGTERM", () => {});
      setInterval(() => {}, 1000);
    `;
    const processes = await startOwnedProcesses(
      [{
        id: "process-tree",
        command: Deno.execPath(),
        args: ["eval", parentProgram, childPidPath],
      }],
      scenario,
      join(directory, "logs"),
      [],
    );
    let childPid = 0;
    for (let attempt = 0; attempt < 100; attempt++) {
      try {
        childPid = Number(await Deno.readTextFile(childPidPath));
        if (childPid > 0) break;
      } catch {
        // The fixture publishes its descendant PID after spawn.
      }
      await new Promise((resolve) => setTimeout(resolve, 10));
    }
    assertEquals(childPid > 0, true);
    assertEquals(await stopOwnedProcesses(processes), []);
    const status = await new Deno.Command("ps", {
      args: ["-p", String(childPid), "-o", "pid="],
      stdout: "piped",
      stderr: "null",
    }).output();
    assertEquals(new TextDecoder().decode(status.stdout).trim(), "");
  } finally {
    await Deno.remove(directory, { recursive: true });
  }
});

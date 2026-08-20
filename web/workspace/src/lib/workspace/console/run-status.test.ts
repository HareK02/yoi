// @ts-nocheck
import {
  applyRunActivityEvent,
  emptyRunActivityStats,
  formatRunElapsed,
  formatRunElapsedCompact,
  formatRunTokens,
} from "./run-status.ts";

function assertEquals(actual: unknown, expected: unknown): void {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      `expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`,
    );
  }
}

Deno.test("run activity follows TUI request and net-token accounting", () => {
  let stats = applyRunActivityEvent(
    emptyRunActivityStats(),
    { event: "invoke_start", data: { kind: "user_send" } },
    1_000,
  );
  stats = applyRunActivityEvent(
    stats,
    { event: "turn_start", data: { turn: 1 } },
    1_010,
  );
  stats = applyRunActivityEvent(
    stats,
    {
      event: "usage",
      data: {
        input_tokens: 25_000,
        cache_read_input_tokens: 20_000,
        output_tokens: 3_000,
      },
    },
    1_020,
  );
  stats = applyRunActivityEvent(
    stats,
    { event: "turn_start", data: { turn: 2 } },
    1_030,
  );

  assertEquals(stats, {
    startedAtMs: 1_000,
    requests: 2,
    uploadTokens: 5_000,
    outputTokens: 3_000,
  });
});

Deno.test("new invoke and running snapshot reset run activity", () => {
  const previous = {
    startedAtMs: 1,
    requests: 3,
    uploadTokens: 100,
    outputTokens: 20,
  };
  assertEquals(
    applyRunActivityEvent(
      previous,
      { event: "invoke_start", data: { kind: "notify" } },
      9_000,
    ),
    { startedAtMs: 9_000, requests: 0, uploadTokens: 0, outputTokens: 0 },
  );
  assertEquals(
    applyRunActivityEvent(
      previous,
      {
        event: "snapshot",
        data: {
          entries: [],
          greeting: { text: "", profile: "" },
          status: "idle",
          in_flight: {},
          internal_workers: [],
        },
      },
      10_000,
    ),
    emptyRunActivityStats(),
  );
});

Deno.test("run status formatting matches the compact TUI shape", () => {
  assertEquals(formatRunElapsed(88_900), "1m 28s");
  assertEquals(formatRunElapsed(3_723_000), "1h 2m 3s");
  assertEquals(formatRunElapsedCompact(620_000), "10m20s");
  assertEquals(formatRunTokens(25_000), "25.0k");
  assertEquals(formatRunTokens(3_000), "3.0k");
  assertEquals(formatRunTokens(999), "999");
});

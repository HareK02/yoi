// @ts-nocheck
import {
  applyRunActivityEvent,
  emptyRunActivityStats,
  formatRunElapsed,
  formatRunElapsedCompact,
  formatRunTokens,
  resolveCompactionStatusPresentation,
  visibleCompactionProgress,
} from "./run-status.ts";
import {
  formatContextSummary,
  formatModelSummary,
  formatReasoning,
} from "./worker-metadata.ts";

function assertEquals(actual: unknown, expected: unknown): void {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      `expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`,
    );
  }
}

Deno.test("worker metadata formats effort, budget, estimate, and unknown safely", () => {
  assertEquals(
    formatModelSummary({
      model: "gpt-6-astra",
      reasoning: { kind: "effort", effort: "high" },
      contextWindow: 272_000,
      contextTokens: 142_000,
      contextSource: "measured",
    }),
    "gpt-6-astra · high",
  );
  assertEquals(
    formatReasoning({ kind: "budget_tokens", budget_tokens: 16_384 }),
    "16.4k token budget",
  );
  assertEquals(
    formatContextSummary({
      model: "model",
      reasoning: null,
      contextWindow: 272_000,
      contextTokens: 142_000,
      contextSource: "estimated",
    }),
    "Context ~142.0k / 272.0k (52%)",
  );
  assertEquals(formatModelSummary(null), "Model unavailable");
  assertEquals(
    formatReasoning({ kind: "effort", effort: "" }),
    "reasoning unavailable",
  );
  assertEquals(
    formatReasoning({ kind: "budget_tokens", budget_tokens: Number.NaN }),
    "reasoning unavailable",
  );
  assertEquals(
    formatReasoning({ kind: "budget_tokens", budget_tokens: -1 }),
    "reasoning unavailable",
  );
  assertEquals(formatContextSummary(null), "Context unavailable");
  assertEquals(
    formatContextSummary({
      model: null,
      reasoning: null,
      contextWindow: 0,
      contextTokens: 0,
      contextSource: null,
    }),
    "Context unavailable",
  );
});

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
          state: {
            revision: 0,
            last_command_id: 0,
            state: { kind: "idle" },
          },
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

Deno.test("WorkerRunStatus presentation follows compaction and busy state", () => {
  const manual = {
    phase: "preparing",
    started_at_ms: 1_700_000_000_000,
    trigger: "manual",
  } as const;
  const automatic = {
    phase: "summarizing",
    started_at_ms: 1_700_000_000_000,
    trigger: "request_threshold",
  } as const;
  const maintenance = {
    last_command_id: 7,
    state: {
      kind: "busy",
      state: { kind: "maintenance", state: "compacting" },
    },
  } as const;
  const run = {
    last_command_id: 7,
    state: {
      kind: "busy",
      state: { kind: "run", state: "running" },
    },
  } as const;
  const idle = {
    last_command_id: 7,
    state: { kind: "idle" },
  } as const;

  assertEquals(visibleCompactionProgress(manual, maintenance), manual);
  assertEquals(visibleCompactionProgress(automatic, run), automatic);
  assertEquals(visibleCompactionProgress(manual, run), null);
  assertEquals(visibleCompactionProgress(automatic, maintenance), null);
  assertEquals(visibleCompactionProgress(manual, idle), null);
  assertEquals(visibleCompactionProgress(manual, null), null);
  assertEquals(visibleCompactionProgress(null, maintenance), null);

  assertEquals(resolveCompactionStatusPresentation(manual, maintenance), {
    progress: manual,
    label: "Compacting · preparing",
  });
  assertEquals(resolveCompactionStatusPresentation(automatic, run), {
    progress: automatic,
    label: "Compacting · summarizing",
  });
  assertEquals(resolveCompactionStatusPresentation(manual, run), null);
  assertEquals(
    resolveCompactionStatusPresentation(automatic, maintenance),
    null,
  );
  assertEquals(resolveCompactionStatusPresentation(manual, idle), null);
});

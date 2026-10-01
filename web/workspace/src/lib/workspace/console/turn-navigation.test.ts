declare const Deno: { test(name: string, fn: () => void): void };

import type { ConsoleLine } from "./model.ts";
import { consoleTurns } from "./turn-navigation.ts";

function line(id: string, kind: ConsoleLine["kind"], body: string, title = kind): ConsoleLine {
  return { id, kind, body, title, source: "event" };
}

function equal(actual: unknown, expected: unknown) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(`Expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`);
  }
}

Deno.test("turn navigation pairs each user with the final assistant report, excluding progress, tools and thinking", () => {
  equal(consoleTurns([
    line("orphan", "assistant", "Earlier response"),
    line("u1", "user", "First\n question"),
    line("a1", "assistant", "Checking now."),
    line("t", "tool", "Tool output"),
    line("progress", "assistant", "Still working."),
    line("thinking", "thinking", "Private reasoning"),
    line("a2", "assistant", "Here is the answer."),
    line("empty", "assistant", " \n "),
    line("stats", "run_stats", "Tokens"),
    line("u2", "user", "Second question"),
    line("a3", "assistant", "Second answer"),
  ]), [
    { id: "u1", user: "First question", assistant: "Here is the answer." },
    { id: "u2", user: "Second question", assistant: "Second answer" },
  ]);
});

Deno.test("unanswered and consecutive user messages retain their own bars", () => {
  equal(consoleTurns([
    line("u1", "user", "One"),
    line("u2", "user", "Two"),
    line("system", "system", "Not an answer"),
  ]), [
    { id: "u1", user: "One", assistant: "" },
    { id: "u2", user: "Two", assistant: "" },
  ]);
  equal(consoleTurns([]), []);
});

Deno.test("streaming updates change the preview without creating another turn", () => {
  const user = line("u", "user", "Question");
  const answer = { ...line("a", "assistant", "Hi"), streaming: true };
  equal(consoleTurns([user, answer]), [{ id: "u", user: "Question", assistant: "Hi" }]);
  equal(consoleTurns([user, { ...answer, body: "Hi there", streaming: false }]), [
    { id: "u", user: "Question", assistant: "Hi there" },
  ]);
  equal(answer.body, "Hi");
});

Deno.test("snapshot-restored assistant text is previewed but thinking is not", () => {
  equal(consoleTurns([
    line("u", "user", "Question"),
    line("reasoning", "thinking", "Private"),
    line("text", "assistant", "Answer in progress"),
  ]), [{ id: "u", user: "Question", assistant: "Answer in progress" }]);
});

Deno.test("turn preview text is bounded and recomputed on view or snapshot replacement", () => {
  const [turn] = consoleTurns([
    line("u", "user", "x".repeat(2000)),
    line("a", "assistant", "y".repeat(2000)),
    line("b", "assistant", "z".repeat(2000)),
  ]);
  equal(turn.user, "x".repeat(1200) + "…");
  equal(turn.assistant, "z".repeat(1200) + "…");
  equal(consoleTurns([line("other", "user", "Other view")]), [
    { id: "other", user: "Other view", assistant: "" },
  ]);
  equal(consoleTurns([]), []);
});

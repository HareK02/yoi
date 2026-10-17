import type { TicketAssignmentTrackingProjection } from "#lib/generated/ticket-api.ts";
import { parseTicketDetail } from "#lib/workspace/api/ticket-browser.ts";
import {
  answerOperation,
  parseAnswerReceipt,
  parseAssignmentsPage,
  parseTracking,
  parseTrackingHistory,
} from "#lib/workspace/api/ticket-operations.ts";
import {
  currentOrchestratorAssignment,
  handlingSummary,
} from "#lib/workspace/tickets/ticket-panel.ts";

// Rust側は隔離DBの本番router応答bodyを再投影せず文字列で渡す。
// expectedは同じDBのstore再readから取り、Web用期待JSONを複製しない。
export interface TicketLifecycleFixture {
  expected: {
    workspace_id: string;
    ticket_id: string;
    old_assignment_id: string;
    new_assignment_id: string;
    old_result: string;
    done_event_id: string;
    new_event_id: string;
  };
  done_detail: string;
  closed_detail: string;
  reopened_detail: string;
  reissued_detail: string;
  assignment_pages: string[];
  retained_old: string;
  old_history_pages: string[];
  answer_receipt: string;
  answered_detail: string;
}

function assert(value: unknown, message: string): asserts value {
  if (!value) throw new Error(message);
}
function canonical(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(canonical);
  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value).sort(([a], [b]) => a.localeCompare(b))
        .map(([key, item]) => [key, canonical(item)]),
    );
  }
  return value;
}
function equal(actual: unknown, expected: unknown, message: string) {
  assert(
    JSON.stringify(canonical(actual)) === JSON.stringify(canonical(expected)),
    `${message}: ${JSON.stringify(actual)} != ${JSON.stringify(expected)}`,
  );
}
function current(detail: ReturnType<typeof parseTicketDetail>) {
  const row = currentOrchestratorAssignment(detail);
  assert(row, "detail must retain a current Orchestrator assignment");
  return row;
}
function rejectsAnswer(row: TicketAssignmentTrackingProjection) {
  let rejected = false;
  try {
    answerOperation(
      row.workspace_id,
      row.ticket_id,
      row,
      "not a request",
      "no-send",
    );
  } catch {
    rejected = true;
  }
  assert(rejected, "done/past observation must never generate an Answer");
}

export function assertTicketLifecycle(fixture: TicketLifecycleFixture) {
  const expected = fixture.expected;
  const done = parseTicketDetail(JSON.parse(fixture.done_detail));
  const closed = parseTicketDetail(JSON.parse(fixture.closed_detail));
  const reopened = parseTicketDetail(JSON.parse(fixture.reopened_detail));
  const reissued = parseTicketDetail(JSON.parse(fixture.reissued_detail));
  const answered = parseTicketDetail(JSON.parse(fixture.answered_detail));
  assert(
    expected.old_result.length > 0,
    "fixture must contain an actual old result",
  );
  equal(done.id, expected.ticket_id, "detail must identify the real Ticket");
  equal(done.state, "open", "done does not close the case");
  equal(closed.state, "closed", "explicit close is a case decision");
  equal(reopened.state, "open", "explicit reopen is a case decision");
  equal(
    closed.closure_history.length,
    1,
    "close must remain in public history",
  );
  equal(
    reopened.closure_history.length,
    2,
    "reopen must retain the prior closure",
  );
  for (const detail of [done, closed, reopened]) {
    const row = current(detail);
    equal(
      row.workspace_id,
      expected.workspace_id,
      "workspace scope survives parser",
    );
    equal(
      row.assignment_id,
      expected.old_assignment_id,
      "case action is not reissue",
    );
    equal(
      row.last_event_id,
      expected.done_event_id,
      "case action is not a handling update",
    );
    equal(row.tracking?.status, "done", "case action must not revive handling");
    equal(
      row.tracking?.result,
      expected.old_result,
      "public detail retains the result",
    );
    equal(
      handlingSummary(row),
      expected.old_result,
      "production summary uses done result",
    );
    rejectsAnswer(row);
  }

  const retained = parseTracking(JSON.parse(fixture.retained_old));
  equal(
    retained.length,
    1,
    "retained read must identify exactly the old assignment",
  );
  const old = retained[0];
  equal(
    old.assignment_id,
    expected.old_assignment_id,
    "retained identity survives parser",
  );
  equal(
    old.current,
    false,
    "old done is no longer current after explicit reissue",
  );
  equal(
    old.last_event_id,
    expected.done_event_id,
    "replacement does not rewrite old handling",
  );
  equal(old.tracking?.status, "done", "replacement preserves done");
  equal(
    handlingSummary(old),
    expected.old_result,
    "retained summary keeps the old result",
  );
  equal(
    old.release?.action,
    "replaced",
    "replacement is distinct from handling done",
  );
  equal(
    old.release?.replacement_assignment_id,
    expected.new_assignment_id,
    "release links the new identity",
  );
  rejectsAnswer(old);

  assert(
    fixture.assignment_pages.length >= 2,
    "exercise the real assignment page cursor",
  );
  const pages = fixture.assignment_pages.map((body) =>
    parseAssignmentsPage(JSON.parse(body))
  );
  for (let index = 0; index < pages.length; index++) {
    const page = pages[index];
    if (index < pages.length - 1) {
      assert(
        page.next_assignment_id,
        "nonterminal assignment page must expose its cursor",
      );
      equal(
        page.next_assignment_id,
        page.items.at(-1)?.assignment_id,
        "cursor binds the last assignment",
      );
    } else {
      assert(
        page.next_assignment_id == null,
        "last assignment page must terminate",
      );
    }
  }
  const rows = pages.flatMap((page) => page.items);
  equal(rows.length, 2, "traversal retains both old and new delegations");
  equal(
    new Set(rows.map((row) => row.assignment_id)).size,
    2,
    "assignment pages do not duplicate rows",
  );
  equal(
    rows.find((row) => row.assignment_id === old.assignment_id),
    old,
    "page and retained projection agree",
  );
  const next = current(reissued);
  equal(
    next.assignment_id,
    expected.new_assignment_id,
    "explicit request creates a new current identity",
  );
  assert(
    next.assignment_id !== old.assignment_id,
    "reissue must not reuse done identity",
  );
  equal(
    next.last_event_id,
    expected.new_event_id,
    "new current observation survives parser",
  );
  equal(
    next.tracking?.status,
    "waiting",
    "new handling starts waiting, not old done",
  );
  assert(
    next.tracking?.result == null && next.release == null,
    "new row must not inherit result/release",
  );
  equal(
    handlingSummary(next),
    next.tracking?.waiting_reason ?? "Waiting reason not reported",
    "new summary never borrows historical result",
  );
  equal(
    rows.find((row) => row.assignment_id === next.assignment_id),
    next,
    "page and current detail agree",
  );
  equal(reissued.state, "open", "reissue keeps the case open");
  equal(
    reissued.closure_history,
    reopened.closure_history,
    "reissue does not change case history",
  );

  assert(
    fixture.old_history_pages.length >= 2,
    "exercise the real handling page cursor",
  );
  const histories = fixture.old_history_pages.map((body) =>
    parseTrackingHistory(JSON.parse(body))
  );
  for (let index = 0; index < histories.length; index++) {
    const page = histories[index];
    if (index < histories.length - 1) {
      assert(
        page.next_event_id,
        "nonterminal history page must expose its cursor",
      );
      equal(
        page.next_event_id,
        page.items.at(-1)?.event_id,
        "history cursor binds the last event",
      );
    } else {
      assert(page.next_event_id == null, "last history page must terminate");
    }
  }
  const events = histories.flatMap((page) => page.items);
  equal(events.length, 2, "old waiting and done history survive replacement");
  equal(
    new Set(events.map((entry) => entry.event_id)).size,
    2,
    "history pages do not duplicate events",
  );
  equal(
    events[0].tracking.status,
    "waiting",
    "initial handling is not overwritten",
  );
  equal(
    events[1].event_id,
    expected.done_event_id,
    "history retains last observed event",
  );
  equal(
    events[1].tracking.result,
    expected.old_result,
    "history parser retains old result",
  );

  const operation = answerOperation(
    expected.workspace_id,
    expected.ticket_id,
    next,
    "Continue the new request",
    "web-current-answer",
  );
  equal(operation.method, "POST", "production Answer method");
  equal(
    operation.path,
    `/api/w/${encodeURIComponent(expected.workspace_id)}/tickets/${
      encodeURIComponent(expected.ticket_id)
    }/assignment-answer`,
    "production Answer target",
  );
  equal(JSON.parse(operation.body!), {
    assignment_id: expected.new_assignment_id,
    expected_last_event_id: expected.new_event_id,
    operation_key: "web-current-answer",
    body: "Continue the new request",
  }, "production Answer uses only the actionable current CAS");
  const receipt = parseAnswerReceipt(JSON.parse(fixture.answer_receipt));
  equal(
    receipt.assignment_id,
    next.assignment_id,
    "real saved Answer belongs to new delegation",
  );
  assert(
    receipt.input_event_id.length > 0,
    "real Answer receipt exposes saved input",
  );
  equal(
    current(answered),
    next,
    "Answer must not infer inprogress or replace waiting",
  );
  equal(answered.state, reissued.state, "Answer does not change case state");
  equal(
    answered.closure_history,
    reissued.closure_history,
    "Answer preserves closure history",
  );
}

if (import.meta.main) {
  // パイプだけを入力境界とし、認証情報・実DBパス・HTTP接続は要求しない。
  const fixture: TicketLifecycleFixture = JSON.parse(
    await new Response(Deno.stdin.readable).text(),
  );
  assertTicketLifecycle(fixture);
  console.log(
    "real Ticket bytes → production parser/operation/summary lifecycle passed",
  );
}

declare const Deno: {
  test(name: string, fn: () => void): void;
};

import {
  applyTaskSnapshotText,
  applyTaskToolCall,
  emptyConsoleTaskState,
  taskCounts,
} from "./tasks.ts";

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

Deno.test("TaskCreate and TaskUpdate mirror the active TUI task store", () => {
  let state = emptyConsoleTaskState();
  state = applyTaskToolCall(
    state,
    "TaskCreate",
    JSON.stringify({ subject: "first", description: "First detail" }),
  );
  state = applyTaskToolCall(
    state,
    "TaskCreate",
    JSON.stringify({ subject: "second", description: "Second detail" }),
  );
  state = applyTaskToolCall(
    state,
    "TaskUpdate",
    JSON.stringify({ taskid: 2, status: "inprogress", subject: "working" }),
  );

  assert(state.tasks.length === 2, "both active tasks should remain visible");
  assert(
    state.tasks[0].taskid === 1,
    "TaskCreate should allocate sequential ids",
  );
  assert(
    state.tasks[1].status === "inprogress",
    "TaskUpdate should change status",
  );
  assert(
    state.tasks[1].subject === "working",
    "TaskUpdate should change subject",
  );
  assert(
    state.tasks[1].description === "Second detail",
    "omitted fields should keep their prior values",
  );
  const counts = taskCounts(state.tasks);
  assert(counts.pending === 1, "one task should be pending");
  assert(counts.inprogress === 1, "one task should be in progress");
});

Deno.test("completed tasks remain visible until explicitly deleted", () => {
  let state = emptyConsoleTaskState();
  for (const subject of ["complete", "delete"]) {
    state = applyTaskToolCall(
      state,
      "TaskCreate",
      JSON.stringify({ subject, description: "detail" }),
    );
  }
  state = applyTaskToolCall(
    state,
    "TaskUpdate",
    JSON.stringify({ taskid: 1, status: "completed" }),
  );
  state = applyTaskToolCall(
    state,
    "TaskUpdate",
    JSON.stringify({ taskid: 2, status: "deleted" }),
  );
  assert(state.tasks.length === 1, "only deleted tasks should be forgotten");
  const counts = taskCounts(state.tasks);
  assert(counts.completed === 1, "completed tasks should remain counted");
  assert(counts.deleted === 0, "deleted tasks should be forgotten");
  assert(counts.active === 0, "completed tasks should not be shown as active");
});

Deno.test("session TaskStore snapshot replaces stale state and advances ids", () => {
  let state = applyTaskToolCall(
    emptyConsoleTaskState(),
    "TaskCreate",
    JSON.stringify({ subject: "stale", description: "" }),
  );
  const snapshot = `[Session TaskStore snapshot]

TaskStore: 1 active task(s) (pending: 0, inprogress: 1)

\`\`\`json
{
  "tasks": [
    {"taskid": 4, "status": "completed", "subject": "old", "description": ""},
    {"taskid": 7, "status": "inprogress", "subject": "restored", "description": "detail"}
  ]
}
\`\`\`
`;
  state = applyTaskSnapshotText(state, snapshot);
  assert(state.tasks.length === 2, "snapshot should restore retained tasks");
  assert(state.tasks[0].taskid === 4, "the completed task should be restored");
  assert(state.tasks[1].taskid === 7, "the active task should be restored");
  assert(
    state.nextTaskId === 8,
    "next id should follow the highest retained task id",
  );
  state = applyTaskToolCall(
    state,
    "TaskCreate",
    JSON.stringify({ subject: "next", description: "" }),
  );
  assert(state.tasks[2].taskid === 8, "new task should use the advanced id");
});

Deno.test("malformed task events are resilient no-ops", () => {
  const state = emptyConsoleTaskState();
  assert(
    applyTaskToolCall(state, "TaskCreate", '{"subject":1}') === state,
    "invalid TaskCreate should be ignored",
  );
  assert(
    applyTaskToolCall(state, "TaskUpdate", "not json") === state,
    "invalid TaskUpdate should be ignored",
  );
  assert(
    applyTaskSnapshotText(state, "ordinary system message") === state,
    "unrelated system messages should be ignored",
  );
});

export type ConsoleTaskStatus =
  | "pending"
  | "inprogress"
  | "completed"
  | "deleted";
type SnapshotTask = ConsoleTask;

export type ConsoleTask = {
  taskid: number;
  status: ConsoleTaskStatus;
  subject: string;
  description: string;
};

export type ConsoleTaskState = {
  tasks: ConsoleTask[];
  nextTaskId: number;
};

type TaskUpdate = {
  taskid: number;
  status?: ConsoleTaskStatus;
  subject?: string;
  description?: string;
};

export function emptyConsoleTaskState(): ConsoleTaskState {
  return { tasks: [], nextTaskId: 1 };
}

export function applyTaskToolCall(
  state: ConsoleTaskState,
  name: string,
  argumentsJson: string,
): ConsoleTaskState {
  const argumentsValue = parseRecord(argumentsJson);
  if (!argumentsValue) return state;

  if (name === "TaskCreate") {
    const subject = stringField(argumentsValue, "subject");
    const description = stringField(argumentsValue, "description");
    if (subject === undefined || description === undefined) return state;
    return {
      tasks: [
        ...state.tasks,
        {
          taskid: state.nextTaskId,
          status: "pending",
          subject,
          description,
        },
      ],
      nextTaskId: state.nextTaskId + 1,
    };
  }

  if (name !== "TaskUpdate") return state;
  const update = taskUpdate(argumentsValue);
  if (!update) return state;
  const index = state.tasks.findIndex((task) => task.taskid === update.taskid);
  if (index < 0) return state;

  const current = state.tasks[index];
  const status = update.status ?? current.status;
  const tasks = [...state.tasks];
  tasks[index] = {
    taskid: current.taskid,
    status,
    subject: update.subject ?? current.subject,
    description: update.description ?? current.description,
  };
  return { tasks, nextTaskId: state.nextTaskId };
}

export function applyTaskSnapshotText(
  state: ConsoleTaskState,
  text: string,
): ConsoleTaskState {
  const tasks = parseTaskSnapshotText(text);
  if (!tasks) return state;
  return {
    tasks,
    nextTaskId: Math.max(1, ...tasks.map((task) => task.taskid + 1)),
  };
}

export function parseTaskSnapshotText(text: string): ConsoleTask[] | null {
  if (!text.startsWith("[Session TaskStore snapshot]")) return null;
  const startMarker = "```json\n";
  const start = text.indexOf(startMarker);
  if (start < 0) return null;
  const jsonStart = start + startMarker.length;
  const end = text.indexOf("\n```", jsonStart);
  if (end < 0) return null;

  let value: unknown;
  try {
    value = JSON.parse(text.slice(jsonStart, end));
  } catch {
    return null;
  }
  if (!isRecord(value) || !Array.isArray(value.tasks)) return null;

  const tasks: ConsoleTask[] = [];
  for (const candidate of value.tasks) {
    const task = taskEntry(candidate);
    if (!task) return null;
    tasks.push(task);
  }
  return tasks;
}

export function taskCounts(tasks: ConsoleTask[]): {
  pending: number;
  inprogress: number;
  completed: number;
  deleted: number;
  active: number;
  total: number;
} {
  let pending = 0;
  let inprogress = 0;
  let completed = 0;
  let deleted = 0;
  for (const task of tasks) {
    switch (task.status) {
      case "pending":
        pending += 1;
        break;
      case "inprogress":
        inprogress += 1;
        break;
      case "completed":
        completed += 1;
        break;
      case "deleted":
        deleted += 1;
        break;
    }
  }
  return {
    pending,
    inprogress,
    completed,
    deleted,
    active: pending + inprogress,
    total: tasks.length,
  };
}

function taskEntry(value: unknown): SnapshotTask | null {
  if (!isRecord(value)) return null;
  const taskid = integerField(value, "taskid");
  const status = statusField(value, "status");
  const subject = stringField(value, "subject");
  const description = stringField(value, "description");
  if (
    taskid === undefined ||
    status === undefined ||
    subject === undefined ||
    description === undefined
  ) return null;
  return { taskid, status, subject, description };
}

function taskUpdate(value: Record<string, unknown>): TaskUpdate | null {
  const taskid = integerField(value, "taskid");
  if (taskid === undefined) return null;
  const statusValue = value.status;
  const status = statusValue === undefined
    ? undefined
    : statusField(value, "status");
  if (statusValue !== undefined && status === undefined) return null;
  const subjectValue = value.subject;
  const subject = subjectValue === undefined
    ? undefined
    : stringField(value, "subject");
  if (subjectValue !== undefined && subject === undefined) return null;
  const descriptionValue = value.description;
  const description = descriptionValue === undefined
    ? undefined
    : stringField(value, "description");
  if (descriptionValue !== undefined && description === undefined) return null;
  return { taskid, status, subject, description };
}

function parseRecord(value: string): Record<string, unknown> | null {
  try {
    const parsed: unknown = JSON.parse(value);
    return isRecord(parsed) ? parsed : null;
  } catch {
    return null;
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function stringField(
  value: Record<string, unknown>,
  key: string,
): string | undefined {
  return typeof value[key] === "string" ? value[key] : undefined;
}

function integerField(
  value: Record<string, unknown>,
  key: string,
): number | undefined {
  const field = value[key];
  return typeof field === "number" && Number.isSafeInteger(field) && field >= 0
    ? field
    : undefined;
}

function statusField(
  value: Record<string, unknown>,
  key: string,
): ConsoleTaskStatus | undefined {
  const field = value[key];
  return field === "pending" ||
      field === "inprogress" ||
      field === "completed" ||
      field === "deleted"
    ? field
    : undefined;
}

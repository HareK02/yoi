<script lang="ts">
  import { taskCounts, type ConsoleTask } from "./tasks.ts";

  type WorkerViewTab = {
    sessionId: string | null;
    label: string;
  };

  type Props = {
    tasks: ConsoleTask[];
    mode: "mini" | "pane";
    paneOpen?: boolean;
    paneId?: string;
    onTogglePane?: () => void;
    workerViews?: WorkerViewTab[];
    selectedWorkerViewSessionId?: string | null;
    onSelectWorkerView?: (sessionId: string | null) => void;
  };

  let {
    tasks,
    mode,
    paneOpen = false,
    paneId,
    onTogglePane,
    workerViews = [],
    selectedWorkerViewSessionId = null,
    onSelectWorkerView = () => {},
  }: Props = $props();
  const counts = $derived(taskCounts(tasks));
  const activeTasks = $derived(
    tasks
      .filter((task) => task.status === "pending" || task.status === "inprogress")
      .slice(0, 3),
  );

  function taskNoun(count: number): "task" | "tasks" {
    return count === 1 ? "task" : "tasks";
  }

  function mark(status: ConsoleTask["status"]): string {
    switch (status) {
      case "pending":
        return "[ ]";
      case "inprogress":
        return "[~]";
      case "completed":
        return "[x]";
      case "deleted":
        return "[-]";
    }
  }
</script>

{#if mode === "mini" && (tasks.length > 0 || workerViews.length > 1 || paneOpen)}
  <section class="task-mini" aria-label="Worker task summary">
    {#each activeTasks as task (task.taskid)}
      <div class="task-mini-row">
        <span class:inprogress={task.status === "inprogress"} class="task-mark">
          {mark(task.status)}
        </span>
        <span class="task-subject">{task.subject.split("\n", 1)[0]}</span>
      </div>
    {/each}
    <div class="task-summary-row">
      <button
        type="button"
        class="task-summary"
        aria-expanded={paneOpen}
        aria-controls={paneId}
        onclick={onTogglePane}
      >
        {counts.total} {taskNoun(counts.total)} — pending: {counts.pending}, inprogress: {counts.inprogress}, completed: {counts.completed}
      </button>
      {#if workerViews.length > 1}
        <span class="worker-view-tabs" role="group" aria-label="Worker transcript view">
          <span aria-hidden="true">[ </span>
          {#each workerViews as view, index (view.sessionId ?? "main")}
            {#if index > 0}<span aria-hidden="true"> | </span>{/if}
            <button
              type="button"
              aria-pressed={view.sessionId === selectedWorkerViewSessionId}
              class:active={view.sessionId === selectedWorkerViewSessionId}
              onclick={() => onSelectWorkerView(view.sessionId)}
            >{view.label}</button>
          {/each}
          <span aria-hidden="true"> ]</span>
        </span>
      {/if}
    </div>
  </section>
{:else if mode === "pane"}
  <aside id={paneId} class="task-pane" aria-label="Worker tasks">
    <h3>Tasks ({counts.total})</h3>

    {#if tasks.length === 0}
      <p class="task-empty">(no tasks)</p>
    {:else}
      <ol class="task-list">
        {#each tasks as task (task.taskid)}
          {@const subjectLines = task.subject.split("\n")}
          <li>
            <div class="task-heading">
              <span class="task-id">#{task.taskid}</span>
              <span
                class:inprogress={task.status === "inprogress"}
                class:completed={task.status === "completed"}
                class:deleted={task.status === "deleted"}
                class="task-mark"
              >{mark(task.status)}</span>
              <span>{subjectLines[0] ?? ""}</span>
            </div>
            {#each subjectLines.slice(1) as subjectLine}
              <div class="task-continuation">{subjectLine}</div>
            {/each}
            {#if task.description}
              {#each task.description.split("\n") as descriptionLine}
                <div class="task-continuation task-description">{descriptionLine}</div>
              {/each}
            {/if}
          </li>
        {/each}
      </ol>
    {/if}
  </aside>
{/if}

<style>
  .task-mini,
  .task-pane {
    font-family: var(--font-mono);
  }

  .task-mini {
    display: grid;
    gap: var(--space-1);
    min-width: 0;
    padding-inline: var(--space-3);
    font-size: var(--font-size-compact);
    line-height: var(--line-height-compact);
  }

  .task-mini-row,
  .task-heading,
  .task-summary-row {
    display: flex;
    min-width: 0;
    gap: var(--space-2);
  }

  .task-summary-row {
    align-items: baseline;
  }

  .task-summary {
    padding: 0;
    border: 0;
    background: transparent;
    font: inherit;
    text-align: left;
    cursor: pointer;
    flex: 1 1 auto;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .task-summary:hover,
  .task-summary:focus-visible {
    color: var(--text);
  }

  .task-summary:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }

  .worker-view-tabs {
    display: flex;
    flex: 0 1 auto;
    min-width: 0;
    max-width: 60%;
    margin-left: auto;
    overflow-x: auto;
    color: var(--text-muted);
    scrollbar-width: none;
    white-space: nowrap;
  }

  .worker-view-tabs::-webkit-scrollbar {
    display: none;
  }

  .worker-view-tabs button {
    flex: 0 0 auto;
    min-width: 0;
    margin: 0;
    padding: 0;
    border: 0;
    background: transparent;
    color: inherit;
    font: inherit;
    line-height: inherit;
    cursor: pointer;
  }

  .worker-view-tabs button:hover,
  .worker-view-tabs button:focus-visible {
    color: var(--text);
  }

  .worker-view-tabs button:focus-visible {
    outline: 1px solid currentcolor;
    outline-offset: 2px;
  }

  .worker-view-tabs button.active {
    color: var(--accent);
    font-weight: 700;
  }

  .task-mark,
  .task-id {
    flex: 0 0 auto;
    color: var(--text-muted);
    white-space: nowrap;
  }

  .task-mark.inprogress {
    color: var(--warning);
    font-weight: 700;
  }

  .task-mark.completed {
    color: var(--success);
  }

  .task-mark.deleted {
    color: var(--danger);
  }

  .task-subject,
  .task-summary {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .task-summary,
  .task-id,
  .task-empty,
  .task-description {
    color: var(--text-muted);
  }

  .task-pane {
    min-width: 0;
    min-height: 0;
    overflow: auto;
    padding-inline: var(--space-4);
    border-left: 1px solid var(--line);
  }

  .task-pane h3 {
    margin: 0 0 var(--space-4);
    color: var(--accent);
    font-size: var(--font-size-body);
  }

  .task-empty {
    margin: 0;
  }

  .task-list {
    display: grid;
    gap: var(--space-4);
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .task-heading {
    align-items: baseline;
  }

  .task-continuation {
    padding-left: 4ch;
    white-space: pre-wrap;
  }

  .task-description {
    font-size: var(--font-size-body);
    line-height: var(--line-height-body);
  }

  @media (max-width: 960px) {
    .task-pane {
      border-left: 0;
      padding-top: var(--space-3);
    }
  }
</style>

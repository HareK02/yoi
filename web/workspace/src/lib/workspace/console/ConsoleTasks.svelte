<script lang="ts">
  import { taskCounts, type ConsoleTask } from "./tasks.ts";

  type WorkerViewTab = {
    sessionId: string | null;
    label: string;
  };

  type Props = {
    tasks: ConsoleTask[];
    mode: "mini" | "pane";
    workerViews?: WorkerViewTab[];
    selectedWorkerViewSessionId?: string | null;
    onSelectWorkerView?: (sessionId: string | null) => void;
  };

  let {
    tasks,
    mode,
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

{#if mode === "mini" && (tasks.length > 0 || workerViews.length > 1)}
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
      <span class="task-summary">
        {counts.total} task(s) — pending: {counts.pending}, inprogress: {counts.inprogress}, completed: {counts.completed}, deleted: {counts.deleted}
      </span>
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
  <aside class="task-pane" aria-label="Worker tasks">
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
    gap: 0.1rem;
    min-width: 0;
    margin-bottom: -0.75rem;
    padding-inline: 0.75rem;
    font-size: 0.8rem;
    line-height: 1.35;
  }

  .task-mini-row,
  .task-heading,
  .task-summary-row {
    display: flex;
    min-width: 0;
    gap: 0.5rem;
  }

  .task-summary-row {
    align-items: baseline;
  }

  .task-summary {
    flex: 1 1 auto;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
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
    padding-inline: 1rem;
    border-left: 1px solid var(--line);
  }

  .task-pane h3 {
    margin: 0 0 1rem;
    color: var(--accent);
    font-size: 0.9rem;
  }

  .task-empty {
    margin: 0;
  }

  .task-list {
    display: grid;
    gap: 1rem;
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
    font-size: 0.8rem;
    line-height: 1.45;
  }

  @media (max-width: 900px) {
    .task-pane {
      display: none;
    }
  }
</style>

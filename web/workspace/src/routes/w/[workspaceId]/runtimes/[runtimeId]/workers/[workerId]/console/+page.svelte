<script lang="ts">
  import { tick } from 'svelte';
  import RichMarkdown from '$lib/workspace-console/RichMarkdown.svelte';
  import {
    projectConsole,
    type ConsoleLine
  } from '$lib/workspace-console/model';
  import { workspaceApiPath } from '$lib/workspace-api/http';
  import type {
    ClientWorkerEventWsFrame,
    Diagnostic,
    Worker,
    WorkerInputResult
  } from '$lib/workspace-sidebar/types';

  type Props = {
    data: {
      workspaceId: string;
      runtimeId: string;
      workerId: string;
    };
  };

  let { data }: Props = $props();

  const workspaceId = $derived(data.workspaceId);
  const runtimeId = $derived(data.runtimeId);
  const workerId = $derived(data.workerId);

  function workerApiPath(path: string): string {
    return workspaceApiPath(workspaceId, path);
  }

  let worker = $state<Worker | null>(null);
  let workerError = $state<string | null>(null);
  let draft = $state('');
  let sending = $state(false);
  let sendError = $state<string | null>(null);
  let streamState = $state<'connecting' | 'open' | 'closed' | 'error'>('connecting');
  let streamDiagnostics = $state<Diagnostic[]>([]);
  let workerDetailsOpen = $state(false);
  let consoleBodyElement: HTMLElement | null = null;
  let autoFollowConsole = $state(true);
  const CONSOLE_BOTTOM_THRESHOLD_PX = 48;
  let observedEvents = $state<Array<{ cursor: string; event: ClientWorkerEventWsFrame & { kind: 'event' } }>>([]);
  let seenObservationEventIds = new Set<string>();
  let nextReloadToken = 0;
  let reloadToken = $state(0);

  type ConsoleTarget = {
    runtimeId: string;
    workerId: string;
  };

  const consoleTarget = $derived({ runtimeId, workerId });

  const projection = $derived(
    projectConsole(observedEvents.map((item) => ({ cursor: item.cursor, event: item.event.envelope.payload })))
  );
  const lines = $derived(projection.lines);
  const diagnostics = $derived(mergeDiagnostics(worker?.diagnostics ?? [], streamDiagnostics));
  const canSend = $derived(Boolean(worker?.capabilities.can_accept_input) && draft.trim().length > 0 && !sending);

  async function getJson<T>(path: string): Promise<T> {
    const response = await fetch(path);
    if (!response.ok) {
      throw new Error(`GET ${path} failed: ${response.status}`);
    }
    return response.json() as Promise<T>;
  }

  async function postJson<T>(path: string, body: unknown, timeoutMs = 30_000): Promise<T> {
    const controller = new AbortController();
    const timeout = window.setTimeout(() => controller.abort(), timeoutMs);
    try {
      const response = await fetch(path, {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify(body),
        signal: controller.signal
      });
      if (!response.ok) {
        let detail = '';
        try {
          detail = await response.text();
        } catch {
          detail = '';
        }
        throw new Error(`POST ${path} failed: ${response.status}${detail ? ` ${detail}` : ''}`);
      }
      return response.json() as Promise<T>;
    } finally {
      window.clearTimeout(timeout);
    }
  }

  async function loadWorker(target: ConsoleTarget) {
    workerError = null;
    try {
      worker = await getJson<Worker>(
        workerApiPath(`/runtimes/${encodeURIComponent(target.runtimeId)}/workers/${encodeURIComponent(target.workerId)}`)
      );
    } catch (error) {
      workerError = error instanceof Error ? error.message : String(error);
      worker = null;
    }
  }

  async function loadConsoleData(target: ConsoleTarget) {
    await loadWorker(target);
  }

  function advanceReloadToken(): number {
    nextReloadToken += 1;
    reloadToken = nextReloadToken;
    return nextReloadToken;
  }

  function resetObservedEvents() {
    observedEvents = [];
    seenObservationEventIds = new Set();
  }

  function rememberObservationEvent(eventId: string): boolean {
    if (seenObservationEventIds.has(eventId)) {
      return false;
    }
    seenObservationEventIds.add(eventId);
    return true;
  }

  async function sendMessage(event: SubmitEvent) {
    event.preventDefault();
    const content = draft.trim();
    if (!content || sending || !worker?.capabilities.can_accept_input) {
      return;
    }

    sending = true;
    sendError = null;
    try {
      const result = await postJson<WorkerInputResult>(
        workerApiPath(`/runtimes/${encodeURIComponent(runtimeId)}/workers/${encodeURIComponent(workerId)}/input`),
        { kind: 'user', content }
      );
      if (result.state === 'accepted') {
        draft = '';
      } else {
        sendError = diagnosticsToText(result.diagnostics) || `Input was ${result.state}.`;
      }
    } catch (error) {
      sendError = error instanceof Error ? error.message : String(error);
    } finally {
      sending = false;
    }
  }

  function connectObservation(targetWorker: Worker | null, token: number, target: ConsoleTarget) {
    if (!targetWorker) {
      streamState = 'closed';
      return;
    }
    streamState = 'connecting';
    const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
    const wsPath = workerApiPath(`/runtimes/${encodeURIComponent(target.runtimeId)}/workers/${encodeURIComponent(
      target.workerId
    )}/events/ws`);
    const ws = new WebSocket(`${protocol}//${window.location.host}${wsPath}`);

    ws.onopen = () => {
      if (token === reloadToken) {
        streamState = 'open';
      }
    };
    ws.onmessage = (message) => {
      if (token !== reloadToken) {
        return;
      }
      try {
        const frame = JSON.parse(String(message.data)) as ClientWorkerEventWsFrame;
        if (frame.kind === 'event') {
          if (!rememberObservationEvent(frame.envelope.event_id)) {
            return;
          }
          observedEvents = [
            ...observedEvents,
            {
              cursor: frame.envelope.cursor,
              event: frame
            }
          ].slice(-500);
        } else {
          streamDiagnostics = [
            ...streamDiagnostics,
            {
              code: frame.diagnostic.code,
              severity: 'warning',
              message: frame.diagnostic.message
            }
          ];
        }
      } catch (error) {
        streamDiagnostics = [
          ...streamDiagnostics,
          {
            code: 'worker_observation_frame_invalid',
            severity: 'warning',
            message: error instanceof Error ? error.message : String(error)
          }
        ];
      }
    };
    ws.onerror = () => {
      if (token === reloadToken) {
        streamState = 'error';
        streamDiagnostics = [
          ...streamDiagnostics,
          {
            code: 'worker_observation_ws_error',
            severity: 'error',
            message: 'Worker observation WebSocket failed.'
          }
        ];
      }
    };
    ws.onclose = () => {
      if (token === reloadToken && streamState !== 'error') {
        streamState = 'closed';
      }
    };

    return () => ws.close();
  }

  function mergeDiagnostics(...groups: Diagnostic[][]): Diagnostic[] {
    return groups.flat();
  }

  function diagnosticsToText(items: Diagnostic[]): string {
    return items.map((item) => `${item.severity}: ${item.message}`).join('\n');
  }

  function lineClass(line: ConsoleLine): string {
    return line.error ? 'error' : line.kind;
  }

  function toolClass(line: ConsoleLine): string {
    const name = line.toolCall?.name?.toLowerCase() ?? '';
    const state = line.toolCall?.state ?? (line.streaming ? 'streaming' : 'done');
    return [name ? `tool-${name}` : '', `tool-state-${state}`].filter(Boolean).join(' ');
  }

  function isNearConsoleBottom(element: HTMLElement): boolean {
    return element.scrollHeight - element.scrollTop - element.clientHeight <= CONSOLE_BOTTOM_THRESHOLD_PX;
  }

  function handleConsoleScroll() {
    if (!consoleBodyElement) {
      return;
    }
    autoFollowConsole = isNearConsoleBottom(consoleBodyElement);
  }

  async function scrollConsoleToBottom() {
    await tick();
    if (!consoleBodyElement) {
      return;
    }
    consoleBodyElement.scrollTop = consoleBodyElement.scrollHeight;
    autoFollowConsole = true;
  }

  const scrollFollowKey = $derived(
    lines
      .map((line) => `${line.source}:${line.kind}:${line.body.length}:${line.streaming ? 'streaming' : 'done'}`)
      .join('|')
  );

  $effect(() => {
    scrollFollowKey;
    if (autoFollowConsole) {
      void scrollConsoleToBottom();
    }
  });

  $effect(() => {
    const target = consoleTarget;
    resetObservedEvents();
    streamDiagnostics = [];
    advanceReloadToken();
    void loadConsoleData(target);
  });

  $effect(() => connectObservation(worker, reloadToken, consoleTarget));
</script>

<svelte:head>
  <title>Worker Console · Yoi Workspace</title>
  <meta name="description" content="Worker attach console through Workspace Backend APIs" />
</svelte:head>

<div class="console-shell worker-console-shell">
    <section class="console-header card">
      <div>
        <h2>{worker?.label ?? workerId}</h2>
      </div>
      <div class="console-header-actions">
        <div class="console-status-pill" class:warn={streamState !== 'open'}>
          {worker?.state ?? 'unknown'} · {worker?.status ?? 'loading'} · stream {streamState}
        </div>
        <button type="button" class="secondary-button" aria-expanded={workerDetailsOpen} onclick={() => workerDetailsOpen = !workerDetailsOpen}>
          Details
        </button>
      </div>
    </section>

    <section class="console-body" bind:this={consoleBodyElement} onscroll={handleConsoleScroll}>
      <article class="card console-card worker-console-card">
        {#if projection.status || projection.usage}
          <p class="section-note">
            {#if projection.status}status: {projection.status}{/if}
            {#if projection.status && projection.usage} · {/if}
            {#if projection.usage}usage: {projection.usage}{/if}
          </p>
        {/if}

        {#if workerError}
          <p class="error">{workerError}</p>
        {/if}

        {#if lines.length === 0}
          <p>No console output is available for this Worker yet.</p>
        {:else}
          <ol class="console-log">
            {#each lines as item}
              <li class={`console-line ${lineClass(item)} ${toolClass(item)}`} class:error-line={item.error}>
                {#if lineClass(item) !== 'assistant' && lineClass(item) !== 'user'}
                  <div class="message-heading">
                    <span>{item.title}</span>
                    {#if item.streaming}<small>streaming</small>{/if}
                  </div>
                {:else if item.streaming}
                  <div class="message-heading streaming-heading">
                    <small>streaming</small>
                  </div>
                {/if}
                <RichMarkdown text={item.body || '—'} />
                {#if item.diff}
                  <pre class="console-diff" aria-label="Edit diff">{#each item.diff as diffLine}
<span class={`diff-line ${diffLine.kind}`}><span class="diff-gutter">{diffLine.oldNumber ?? ''}</span><span class="diff-gutter">{diffLine.newNumber ?? ''}</span><span class="diff-marker">{diffLine.kind === 'add' ? '+' : diffLine.kind === 'remove' ? '-' : ' '}</span><span class="diff-content">{diffLine.content}</span></span>{/each}</pre>
                {/if}
                {#if item.detail}
                  <details class="message-detail">
                    <summary>detail</summary>
                    <p>{item.detail}</p>
                  </details>
                {/if}
              </li>
            {/each}
          </ol>
        {/if}
      </article>

    </section>

    {#if workerDetailsOpen}
      <aside class="console-side-panel" aria-label="Worker detail">
        <header class="side-panel-header">
          <h3>Worker detail</h3>
          <button type="button" class="secondary-button" onclick={() => workerDetailsOpen = false}>Close</button>
        </header>
        {#if worker}
          <dl>
            <div>
              <dt>Runtime</dt>
              <dd><code>{worker.runtime_id}</code></dd>
            </div>
            <div>
              <dt>Worker</dt>
              <dd><code>{worker.worker_id}</code></dd>
            </div>
            <div>
              <dt>Host</dt>
              <dd><code>{worker.host_id}</code></dd>
            </div>
            <div>
              <dt>Role / profile</dt>
              <dd>{worker.role ?? 'unknown'} / {worker.profile ?? 'unknown'}</dd>
            </div>
            <div>
              <dt>Workspace</dt>
              <dd>{worker.workspace.visibility} · {worker.workspace.identity}</dd>
            </div>
            <div>
              <dt>Implementation</dt>
              <dd>{worker.implementation.kind} · {worker.implementation.display_hint}</dd>
            </div>
          </dl>
          <details class="metadata-details">
            <summary>Capabilities</summary>
            <ul>
              <li>input: {worker.capabilities.can_accept_input ? 'available' : 'unsupported'}</li>
              <li>stop: {worker.capabilities.can_stop ? 'available' : 'unsupported'}</li>
              <li>follow-up spawn: {worker.capabilities.can_spawn_followup ? 'available' : 'unsupported'}</li>
            </ul>
          </details>
        {:else if !workerError}
          <p>Loading Worker detail…</p>
        {/if}

        {#if diagnostics.length > 0}
          <details class="metadata-details" open={streamState === 'error'}>
            <summary>Diagnostics ({diagnostics.length})</summary>
            <ul>
              {#each diagnostics as diagnostic}
                <li>
                  <strong>{diagnostic.severity}</strong>
                  <code>{diagnostic.code}</code>
                  <span>{diagnostic.message}</span>
                </li>
              {/each}
            </ul>
          </details>
        {/if}
      </aside>
    {/if}

    <form class="console-composer card" onsubmit={sendMessage}>
      <textarea
        id="worker-console-message"
        aria-label="Console input"
        bind:value={draft}
        disabled={!worker?.capabilities.can_accept_input || sending}
      ></textarea>
      <div class="composer-actions">
        <button type="submit" disabled={!canSend}>{sending ? 'Sending…' : 'Send'}</button>
        {#if sendError}<p class="error">{sendError}</p>{/if}
      </div>
    </form>
</div>

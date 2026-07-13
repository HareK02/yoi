<script lang="ts">
  import { tick } from 'svelte';
  import ConsoleLineItem from '$lib/workspace-console/ConsoleLineItem.svelte';
  import { chatSubmit } from '$lib/workspace-console/chat-submit';
  import { buildComposerRequest } from '$lib/workspace-console/composer-command';
  import {
    applyCompletion,
    completionTokenAt,
    localCommandCompletions,
    type ComposerCompletionEntry,
    type ComposerCompletionToken
  } from '$lib/workspace-console/composer-completion';
  import { fitTextarea } from '$lib/workspace-console/textarea-fit';
  import {
    createConsoleProjector,
    type ConsoleEventInput,
    type ConsoleProjection
  } from '$lib/workspace-console/model';
  import { workspaceApiPath } from '$lib/workspace-api/http';
  import type {
    ClientWorkerEventWsFrame,
    Diagnostic,
    Worker,
    WorkerInputResult,
    PodProtocolEvent
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

  type WorkerCompletionsResult = {
    kind: 'file' | 'knowledge' | 'workflow';
    prefix: string;
    entries: ComposerCompletionEntry[];
    diagnostics: Diagnostic[];
  };

  let worker = $state<Worker | null>(null);
  let liveWorkerState = $state<string | null>(null);
  let workerError = $state<string | null>(null);
  let draft = $state('');
  let completionEntries = $state<ComposerCompletionEntry[]>([]);
  let completionToken = $state<ComposerCompletionToken | null>(null);
  let completionBusy = $state(false);
  let completionError = $state<string | null>(null);
  let sending = $state(false);
  let sendError = $state<string | null>(null);
  let composerNotice = $state<string | null>(null);
  let streamState = $state<'connecting' | 'open' | 'closed' | 'error'>('connecting');
  let streamDiagnostics = $state<Diagnostic[]>([]);
  let workerDetailsOpen = $state(false);
  let consoleBodyElement: HTMLElement | null = null;
  let autoFollowConsole = $state(true);
  const CONSOLE_BOTTOM_THRESHOLD_PX = 48;
  const consoleProjector = createConsoleProjector();
  let consoleProjection = $state.raw<ConsoleProjection>(consoleProjector.snapshot());
  let seenObservationEventIds = new Set<string>();
  let pendingObservationEvents: ConsoleEventInput[] = [];
  let pendingObservedStates: Array<string | null> = [];
  let pendingStreamDiagnostics: Diagnostic[] = [];
  let observationFlushHandle: number | null = null;
  let nextReloadToken = 0;
  let reloadToken = $state(0);

  type ConsoleTarget = {
    runtimeId: string;
    workerId: string;
  };

  const consoleTarget = $derived({ runtimeId, workerId });

  const lines = $derived(consoleProjection.lines);
  const diagnostics = $derived(mergeDiagnostics(worker?.diagnostics ?? [], streamDiagnostics));
  const workerState = $derived(liveWorkerState ?? worker?.state ?? 'loading');
  const inputReady = $derived(workerState === 'idle');
  const canSend = $derived(inputReady && draft.trim().length > 0 && !sending);

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
      const payload = await getJson<Worker>(
        workerApiPath(`/runtimes/${encodeURIComponent(target.runtimeId)}/workers/${encodeURIComponent(target.workerId)}`)
      );
      worker = payload;
      liveWorkerState = payload.state;
    } catch (error) {
      workerError = error instanceof Error ? error.message : String(error);
      worker = null;
      liveWorkerState = null;
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
    cancelObservationFlush();
    consoleProjection = consoleProjector.reset();
    seenObservationEventIds = new Set();
  }

  function cancelObservationFlush() {
    if (observationFlushHandle !== null) {
      window.cancelAnimationFrame(observationFlushHandle);
      observationFlushHandle = null;
    }
    pendingObservationEvents = [];
    pendingObservedStates = [];
    pendingStreamDiagnostics = [];
  }

  function scheduleObservationFlush() {
    if (observationFlushHandle !== null) {
      return;
    }
    observationFlushHandle = window.requestAnimationFrame(() => {
      flushObservationBatch();
    });
  }

  function flushObservationBatch() {
    observationFlushHandle = null;
    const eventBatch = pendingObservationEvents;
    const stateBatch = pendingObservedStates;
    const diagnosticBatch = pendingStreamDiagnostics;
    pendingObservationEvents = [];
    pendingObservedStates = [];
    pendingStreamDiagnostics = [];

    if (eventBatch.length > 0) {
      const latestState = stateBatch.findLast((state) => state !== null);
      if (latestState) {
        liveWorkerState = latestState;
      }
      consoleProjection = consoleProjector.append(eventBatch);
    }

    if (diagnosticBatch.length > 0) {
      streamDiagnostics = [...streamDiagnostics, ...diagnosticBatch];
    }
  }

  function queueObservationEvent(frame: ClientWorkerEventWsFrame & { kind: 'event' }) {
    if (!rememberObservationEvent(frame.envelope.event_id)) {
      return;
    }
    pendingObservationEvents.push({
      eventId: frame.envelope.event_id,
      event: frame.envelope.payload
    });
    pendingObservedStates.push(workerStateFromProtocolEvent(frame.envelope.payload));
    scheduleObservationFlush();
  }

  function queueObservationDiagnostic(diagnostic: Diagnostic) {
    pendingStreamDiagnostics.push(diagnostic);
    scheduleObservationFlush();
  }

  function rememberObservationEvent(eventId: string): boolean {
    if (seenObservationEventIds.has(eventId)) {
      return false;
    }
    seenObservationEventIds.add(eventId);
    return true;
  }

  async function applyComposerCompletion(event: KeyboardEvent) {
    const target = event.currentTarget;
    if (!(target instanceof HTMLTextAreaElement)) {
      return;
    }
    const token = completionTokenAt(draft, target.selectionStart ?? draft.length);
    completionToken = token;
    completionError = null;
    if (!token) {
      completionEntries = [];
      return;
    }

    completionBusy = true;
    try {
      const entries = await resolveCompletionEntries(token);
      completionEntries = entries;
      if (entries.length === 0) {
        completionError = `No completions for ${token.sigil}${token.prefix}`;
        return;
      }
      const applied = applyCompletion(draft, token, entries[0]);
      draft = applied.value;
      await tick();
      target.setSelectionRange(applied.cursor, applied.cursor);
      composerNotice = entries.length > 1
        ? `Completed ${token.sigil}${entries[0].value}; ${entries.length - 1} more candidate(s)`
        : null;
    } catch (error) {
      completionError = error instanceof Error ? error.message : String(error);
    } finally {
      completionBusy = false;
    }
  }

  async function resolveCompletionEntries(
    token: ComposerCompletionToken
  ): Promise<ComposerCompletionEntry[]> {
    if (token.kind === 'command') {
      return localCommandCompletions(token.prefix);
    }
    const result = await postJson<WorkerCompletionsResult>(
      workerApiPath(
        `/runtimes/${encodeURIComponent(runtimeId)}/workers/${encodeURIComponent(workerId)}/completions`
      ),
      { kind: token.kind, prefix: token.prefix }
    );
    if (result.diagnostics.length > 0 && result.entries.length === 0) {
      throw new Error(diagnosticsToText(result.diagnostics));
    }
    return result.entries;
  }

  function handleComposerKeydown(event: KeyboardEvent) {
    if (event.key !== 'Tab') {
      return;
    }
    event.preventDefault();
    void applyComposerCompletion(event);
  }

  async function submitDraft(value = draft) {
    const command = buildComposerRequest(value);
    if (!command.ok) {
      composerNotice = null;
      sendError = command.message;
      return;
    }
    composerNotice = command.notice ?? null;
    if (!command.request) {
      draft = '';
      return;
    }
    if (sending || !inputReady) {
      return;
    }

    sending = true;
    sendError = null;
    try {
      const result = await postJson<WorkerInputResult>(
        workerApiPath(`/runtimes/${encodeURIComponent(runtimeId)}/workers/${encodeURIComponent(workerId)}/input`),
        command.request
      );
      if (result.state === 'accepted') {
        draft = '';
        liveWorkerState = 'running';
      } else {
        sendError = diagnosticsToText(result.diagnostics) || `Input was ${result.state}.`;
      }
    } catch (error) {
      sendError = error instanceof Error ? error.message : String(error);
    } finally {
      sending = false;
    }
  }

  async function sendMessage(event: SubmitEvent) {
    event.preventDefault();
    await submitDraft();
  }

  function workerStateFromProtocolEvent(event: PodProtocolEvent): string | null {
    switch (event.event) {
      case 'snapshot':
      case 'status':
        return event.data.status;
      case 'shutdown':
        return 'shutdown';
      default:
        return null;
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
          queueObservationEvent(frame);
        } else {
          queueObservationDiagnostic({
            code: frame.diagnostic.code,
            severity: 'warning',
            message: frame.diagnostic.message
          });
        }
      } catch (error) {
        queueObservationDiagnostic({
          code: 'worker_observation_frame_invalid',
          severity: 'warning',
          message: error instanceof Error ? error.message : String(error)
        });
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
    liveWorkerState = null;
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
          {workerState} · stream {streamState}
        </div>
        <button type="button" class="secondary-button" aria-expanded={workerDetailsOpen} onclick={() => workerDetailsOpen = !workerDetailsOpen}>
          Details
        </button>
      </div>
    </section>

    <section class="console-body" bind:this={consoleBodyElement} onscroll={handleConsoleScroll}>
      <article class="card console-card worker-console-card">
        {#if consoleProjection.status || consoleProjection.usage}
          <p class="section-note">
            {#if consoleProjection.status}status: {consoleProjection.status}{/if}
            {#if consoleProjection.status && consoleProjection.usage} · {/if}
            {#if consoleProjection.usage}usage: {consoleProjection.usage}{/if}
          </p>
        {/if}

        {#if workerError}
          <p class="error">{workerError}</p>
        {/if}

        {#if lines.length === 0}
          <p>No console output is available for this Worker yet.</p>
        {:else}
          <ol class="console-log">
            {#each lines as item (item.id)}
              <ConsoleLineItem {item} />
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
        aria-keyshortcuts="Meta+Enter Control+Enter"
        bind:value={draft}
        use:chatSubmit={{
          enabled: inputReady && !sending,
          onSubmit: (value) => void submitDraft(value)
        }}
        use:fitTextarea={{ value: draft, maxRows: 10 }}
        onkeydown={handleComposerKeydown}
        disabled={!inputReady || sending}
      ></textarea>
      {#if completionBusy || completionError || completionEntries.length > 0}
        <div class="composer-completions" aria-live="polite">
          {#if completionBusy}
            <span>completing…</span>
          {:else if completionError}
            <span class="error">{completionError}</span>
          {:else}
            <span>Tab: {completionToken?.sigil}{completionEntries[0]?.value}</span>
            {#if completionEntries.length > 1}
              <span>{completionEntries.length - 1} more</span>
            {/if}
          {/if}
        </div>
      {/if}
      <div class="composer-actions">
        <button type="submit" disabled={!canSend}>{sending ? 'Sending…' : 'Send'}</button>
        {#if composerNotice}<p class="section-note">{composerNotice}</p>{/if}
      {#if sendError}<p class="error">{sendError}</p>{/if}
      </div>
    </form>
</div>

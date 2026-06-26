<script lang="ts">
  import WorkspaceSidebar from '$lib/workspace-sidebar/WorkspaceSidebar.svelte';
  import {
    projectConsole,
    workerConsolePath,
    type ConsoleLine
  } from '$lib/workspace-console/model';
  import type {
    ClientWorkerEventWsFrame,
    Diagnostic,
    Worker,
    WorkerInputResult,
    WorkerTranscriptProjection,
    WorkspaceResponse
  } from '$lib/workspace-sidebar/types';

  type Props = {
    data: {
      runtimeId: string;
      workerId: string;
    };
  };

  let { data }: Props = $props();

  const runtimeId = $derived(data.runtimeId);
  const workerId = $derived(data.workerId);
  const currentPath = $derived(workerConsolePath(runtimeId, workerId));

  let workspace = $state<WorkspaceResponse | null>(null);
  let workspaceError = $state<string | null>(null);
  let worker = $state<Worker | null>(null);
  let workerError = $state<string | null>(null);
  let transcript = $state<WorkerTranscriptProjection | null>(null);
  let transcriptError = $state<string | null>(null);
  let draft = $state('');
  let sending = $state(false);
  let sendError = $state<string | null>(null);
  let streamState = $state<'connecting' | 'open' | 'unsupported' | 'closed' | 'error'>('connecting');
  let streamDiagnostics = $state<Diagnostic[]>([]);
  let observedEvents = $state<Array<{ cursor: string; event: ClientWorkerEventWsFrame & { kind: 'event' } }>>([]);
  let nextReloadToken = 0;
  let reloadToken = $state(0);

  type ConsoleTarget = {
    runtimeId: string;
    workerId: string;
  };

  const consoleTarget = $derived({ runtimeId, workerId });

  const projection = $derived(
    projectConsole(
      transcript?.items ?? [],
      observedEvents.map((item) => ({ cursor: item.cursor, event: item.event.envelope.payload }))
    )
  );
  const lines = $derived(projection.lines);
  const diagnostics = $derived(
    mergeDiagnostics(worker?.diagnostics ?? [], transcript?.diagnostics ?? [], streamDiagnostics)
  );
  const canSend = $derived(Boolean(worker?.capabilities.can_accept_input) && draft.trim().length > 0 && !sending);
  const transcriptOnly = $derived(
    worker && !worker.capabilities.can_stream_events
      ? 'Streaming observation is not available for this Worker. Console is using bounded transcript plus manual refresh.'
      : null
  );

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

  async function loadWorkspace() {
    workspaceError = null;
    try {
      workspace = await getJson<WorkspaceResponse>('/api/workspace');
    } catch (error) {
      workspaceError = error instanceof Error ? error.message : String(error);
      workspace = null;
    }
  }

  async function loadWorker(target: ConsoleTarget) {
    workerError = null;
    try {
      worker = await getJson<Worker>(
        `/api/runtimes/${encodeURIComponent(target.runtimeId)}/workers/${encodeURIComponent(target.workerId)}`
      );
    } catch (error) {
      workerError = error instanceof Error ? error.message : String(error);
      worker = null;
    }
  }

  async function loadTranscript(target: ConsoleTarget) {
    transcriptError = null;
    try {
      transcript = await getJson<WorkerTranscriptProjection>(
        `/api/runtimes/${encodeURIComponent(target.runtimeId)}/workers/${encodeURIComponent(target.workerId)}/transcript?limit=200`
      );
    } catch (error) {
      transcriptError = error instanceof Error ? error.message : String(error);
      transcript = null;
    }
  }

  async function loadConsoleData(target: ConsoleTarget) {
    await Promise.all([loadWorker(target), loadTranscript(target)]);
  }

  function advanceReloadToken(): number {
    nextReloadToken += 1;
    reloadToken = nextReloadToken;
    return nextReloadToken;
  }

  async function refreshConsole() {
    advanceReloadToken();
    await loadConsoleData(consoleTarget);
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
        `/api/runtimes/${encodeURIComponent(runtimeId)}/workers/${encodeURIComponent(workerId)}/input`,
        { kind: 'user', content }
      );
      if (result.state === 'accepted') {
        draft = '';
      } else {
        sendError = diagnosticsToText(result.diagnostics) || `Input was ${result.state}.`;
      }
      await loadTranscript(consoleTarget);
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
    if (!targetWorker.capabilities.can_stream_events) {
      streamState = 'unsupported';
      streamDiagnostics = [
        {
          code: 'worker_streaming_unsupported',
          severity: 'info',
          message: 'This Worker does not expose backend-proxied observation streaming; transcript refresh remains available.'
        }
      ];
      return;
    }

    streamState = 'connecting';
    const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
    const ws = new WebSocket(
      `${protocol}//${window.location.host}/api/runtimes/${encodeURIComponent(target.runtimeId)}/workers/${encodeURIComponent(
        target.workerId
      )}/events/ws`
    );

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
            severity: 'warning',
            message: 'Backend observation WebSocket failed; transcript refresh remains available.'
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

  $effect(() => {
    void loadWorkspace();
  });

  $effect(() => {
    const target = consoleTarget;
    observedEvents = [];
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

<div class="workspace-layout">
  <WorkspaceSidebar {workspace} {workspaceError} {currentPath} />

  <main class="shell console-shell worker-console-shell">
    <section class="console-header card">
      <div>
        <p class="eyebrow">Worker attach Console</p>
        <h2>{worker?.label ?? workerId}</h2>
        <p class="section-note">
          Target authority is <code>runtime_id</code> + <code>worker_id</code>. Browser traffic uses Workspace Backend Worker APIs only;
          Runtime endpoints, credentials, socket paths, and session paths are not exposed.
        </p>
      </div>
      <div class="console-status-pill" class:warn={streamState !== 'open'}>
        {worker?.state ?? 'unknown'} · {worker?.status ?? 'loading'} · stream {streamState}
      </div>
    </section>

    <section class="console-grid">
      <article class="card transcript-card worker-transcript-card">
        <header class="transcript-toolbar">
          <div>
            <h3>Transcript and protocol events</h3>
            {#if projection.status || projection.usage}
              <p class="section-note">
                {#if projection.status}status: {projection.status}{/if}
                {#if projection.status && projection.usage} · {/if}
                {#if projection.usage}usage: {projection.usage}{/if}
              </p>
            {/if}
          </div>
          <button type="button" class="secondary-button" onclick={refreshConsole}>Refresh</button>
        </header>

        {#if workerError}
          <p class="error">{workerError}</p>
        {/if}
        {#if transcriptError}
          <p class="error">{transcriptError}</p>
        {/if}
        {#if transcriptOnly}
          <p class="section-note degrade-note">{transcriptOnly}</p>
        {/if}

        {#if lines.length === 0}
          <p>No transcript items or observation events are available for this Worker yet.</p>
        {:else}
          <ol class="transcript worker-transcript">
            {#each lines as item}
              <li class:assistant={lineClass(item) === 'assistant'} class:user={lineClass(item) === 'user'} class:system={lineClass(item) !== 'assistant' && lineClass(item) !== 'user'} class:error-line={item.error}>
                <div class="message-heading">
                  <span>{item.title}</span>
                  <small>{item.source}{item.streaming ? ' · streaming' : ''}</small>
                </div>
                <pre>{item.body || '—'}</pre>
                {#if item.detail || item.cursor}
                  <details class="message-detail">
                    <summary>metadata</summary>
                    {#if item.detail}<p>{item.detail}</p>{/if}
                    {#if item.cursor}<code>{item.cursor}</code>{/if}
                  </details>
                {/if}
              </li>
            {/each}
          </ol>
        {/if}
      </article>

      <aside class="console-side-card card">
        <h3>Worker detail</h3>
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
              <li>stream: {worker.capabilities.can_stream_events ? 'available' : 'unsupported'}</li>
              <li>bounded transcript: {worker.capabilities.can_read_bounded_transcript ? 'available' : 'unsupported'}</li>
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
    </section>

    <form class="console-composer card" onsubmit={sendMessage}>
      <label for="worker-console-message">Send user input</label>
      <textarea
        id="worker-console-message"
        bind:value={draft}
        placeholder={worker?.capabilities.can_accept_input ? 'Message this Worker through the Backend input API…' : 'Input is unsupported for this Worker'}
        disabled={!worker?.capabilities.can_accept_input || sending}
      ></textarea>
      <div class="composer-actions">
        <button type="submit" disabled={!canSend}>{sending ? 'Sending…' : 'Send'}</button>
        {#if sendError}<p class="error">{sendError}</p>{/if}
      </div>
    </form>
  </main>
</div>

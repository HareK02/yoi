<script lang="ts">
  import WorkspaceSidebar from '$lib/workspace-sidebar/WorkspaceSidebar.svelte';
  import type {
    CompanionMessageResponse,
    CompanionState,
    CompanionStatusResponse,
    CompanionTranscriptItem,
    CompanionTranscriptProjection,
    Diagnostic,
    WorkspaceResponse
  } from '$lib/workspace-sidebar/types';

  let workspace = $state<WorkspaceResponse | null>(null);
  let workspaceError = $state<string | null>(null);
  let status = $state<CompanionStatusResponse | null>(null);
  let transcript = $state<CompanionTranscriptProjection | null>(null);
  let draft = $state('');
  let operationState = $state<CompanionState>('ready');
  let error = $state<string | null>(null);
  let timeoutNotice = $state<string | null>(null);
  let requestId = 0;

  const currentPath = '/console';
  const messages = $derived(transcript?.items ?? []);
  const diagnostics = $derived(mergeDiagnostics(status?.diagnostics ?? [], transcript?.diagnostics ?? []));
  const sending = $derived(operationState === 'busy');
  const canSend = $derived(draft.trim().length > 0 && !sending);

  async function getJson<T>(path: string): Promise<T> {
    const response = await fetch(path);
    if (!response.ok) {
      throw new Error(`GET ${path} failed: ${response.status}`);
    }
    return response.json() as Promise<T>;
  }

  async function postJson<T>(path: string, body: unknown, timeoutMs = 45_000): Promise<T> {
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
    } catch (requestError) {
      if (requestError instanceof DOMException && requestError.name === 'AbortError') {
        operationState = 'timeout';
        timeoutNotice = 'Workspace server request timed out before a Companion response arrived.';
      }
      throw requestError;
    } finally {
      window.clearTimeout(timeout);
    }
  }

  async function loadWorkspace() {
    workspaceError = null;
    try {
      workspace = await getJson<WorkspaceResponse>('/api/workspace');
    } catch (loadError) {
      workspaceError = loadError instanceof Error ? loadError.message : String(loadError);
      workspace = null;
    }
  }

  async function loadCompanion() {
    error = null;
    timeoutNotice = null;
    try {
      const [nextStatus, nextTranscript] = await Promise.all([
        getJson<CompanionStatusResponse>('/api/companion/status'),
        getJson<CompanionTranscriptProjection>('/api/companion/transcript?limit=200')
      ]);
      status = nextStatus;
      transcript = nextTranscript;
      operationState = nextStatus.state === 'error' ? 'error' : 'ready';
    } catch (loadError) {
      error = loadError instanceof Error ? loadError.message : String(loadError);
      operationState = 'error';
    }
  }

  async function sendMessage(event: SubmitEvent) {
    event.preventDefault();
    const content = draft.trim();
    if (!content || sending) {
      return;
    }
    const currentRequest = ++requestId;
    error = null;
    timeoutNotice = null;
    operationState = 'busy';
    try {
      const response = await postJson<CompanionMessageResponse>('/api/companion/messages', { content });
      if (currentRequest !== requestId) {
        return;
      }
      operationState = response.state;
      transcript = response.transcript;
      if (response.worker || status) {
        status = {
          state: response.state === 'accepted' ? 'ready' : response.state,
          worker: response.worker ?? status?.worker ?? null,
          transport: status?.transport ?? {
            kind: 'providerless_backend_internal',
            completion: 'synchronous_request_response',
            limitation: 'Companion transport metadata was not available during this response.'
          },
          diagnostics: response.diagnostics
        };
      }
      if (response.state === 'accepted') {
        draft = '';
        operationState = 'ready';
      } else if (response.state === 'busy') {
        error = 'Companion is busy with another message.';
      } else if (response.state === 'rejected') {
        error = diagnosticsToText(response.diagnostics) || 'Companion rejected the message.';
      } else if (response.state === 'error') {
        error = diagnosticsToText(response.diagnostics) || 'Companion returned an error.';
      }
    } catch (sendError) {
      if (currentRequest !== requestId) {
        return;
      }
      if (operationState !== 'timeout') {
        operationState = 'error';
      }
      error = sendError instanceof Error ? sendError.message : String(sendError);
    }
  }

  async function cancelMessage() {
    ++requestId;
    operationState = 'cancelled';
    try {
      const response = await postJson<CompanionMessageResponse>('/api/companion/cancel', { reason: 'browser_cancel' }, 10_000);
      transcript = response.transcript;
      status = status
        ? { ...status, state: response.state, diagnostics: response.diagnostics }
        : status;
    } catch (cancelError) {
      error = cancelError instanceof Error ? cancelError.message : String(cancelError);
      operationState = 'error';
    }
  }

  function mergeDiagnostics(...groups: Diagnostic[][]): Diagnostic[] {
    return groups.flat();
  }

  function diagnosticsToText(items: Diagnostic[]): string {
    return items.map((item) => `${item.severity}: ${item.message}`).join('\n');
  }

  function itemClass(item: CompanionTranscriptItem): string {
    if (item.role === 'assistant') {
      return 'assistant';
    }
    if (item.role === 'user') {
      return 'user';
    }
    return 'system';
  }

  $effect(() => {
    void loadWorkspace();
    void loadCompanion();
  });
</script>

<svelte:head>
  <title>Companion Console · Yoi Workspace</title>
  <meta name="description" content="Workspace Companion Web Console MVP" />
</svelte:head>

<div class="workspace-layout">
  <WorkspaceSidebar {workspace} {workspaceError} {currentPath} />

  <main class="shell console-shell">
    <section class="console-header card">
      <div>
        <p class="eyebrow">Backend-internal Companion</p>
        <h2>Companion Console</h2>
        <p class="section-note">
          Browser traffic stays behind Workspace API projections. No Worker socket, session path,
          runtime credential, or local session file is exposed to the frontend.
        </p>
      </div>
      <div class="console-status" data-state={operationState}>
        <span>{operationState}</span>
        {#if status?.worker}
          <small>{status.worker.label}</small>
        {:else}
          <small>worker pending</small>
        {/if}
      </div>
    </section>

    {#if status?.transport}
      <section class="card console-transport" aria-label="Companion transport">
        <div>
          <dt>Transport</dt>
          <dd>{status.transport.kind}</dd>
        </div>
        <div>
          <dt>Completion</dt>
          <dd>{status.transport.completion}</dd>
        </div>
        <p>{status.transport.limitation}</p>
      </section>
    {/if}

    {#if error || timeoutNotice || diagnostics.length > 0}
      <section class="card console-diagnostics" aria-label="Companion diagnostics">
        {#if timeoutNotice}
          <p class="diagnostic warning">{timeoutNotice}</p>
        {/if}
        {#if error}
          <p class="diagnostic error">{error}</p>
        {/if}
        {#each diagnostics as diagnostic}
          <p class={`diagnostic ${diagnostic.severity}`}>{diagnostic.code}: {diagnostic.message}</p>
        {/each}
      </section>
    {/if}

    <section class="card transcript-card" aria-label="Companion transcript">
      <div class="runtime-heading">
        <h3>Transcript</h3>
        <span>{transcript?.total_items ?? 0} items</span>
      </div>
      {#if messages.length === 0}
        <p class="empty-state">No Companion messages yet. Send a message to exercise the backend boundary.</p>
      {:else}
        <ol class="transcript-list">
          {#each messages as message (message.sequence)}
            <li class={`transcript-item ${itemClass(message)}`}>
              <div class="message-meta">
                <strong>{message.role}</strong>
                <span>{message.status}</span>
                <time datetime={message.created_at}>{message.created_at}</time>
              </div>
              <p>{message.content}</p>
            </li>
          {/each}
        </ol>
      {/if}
    </section>

    <form class="card composer-card" onsubmit={sendMessage}>
      <label for="companion-message">Message Companion</label>
      <textarea
        id="companion-message"
        bind:value={draft}
        rows="4"
        maxlength="8000"
        placeholder="Ask or note something for the backend Companion boundary…"
        disabled={sending}
      ></textarea>
      <div class="composer-actions">
        <span>{draft.trim().length}/8000</span>
        <button type="button" class="secondary" onclick={loadCompanion} disabled={sending}>Refresh</button>
        <button type="button" class="secondary" onclick={cancelMessage} disabled={!sending}>Cancel</button>
        <button type="submit" disabled={!canSend}>Send</button>
      </div>
    </form>
  </main>
</div>

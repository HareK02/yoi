<script lang="ts">
  import { goto } from '$app/navigation';
  import { untrack } from 'svelte';
  import { workspaceApiPath } from '$lib/workspace/api/http';
  import { formatCurrentWorkdirRevision } from '$lib/workspace/settings/workdir-revision';
  import { buildBrowserCreateWorkerRequest, defaultWorkerLaunchForm } from '$lib/workspace/sidebar/worker-launch';
  import type {
    BrowserCreateWorkerResponse,
    BrowserWorkingDirectoryCreateResponse,
    Diagnostic,
    WorkerLaunchOptionsResponse,
    WorkingDirectorySummary,
  } from '$lib/workspace/sidebar/types';
  import type { PageProps } from './$types';

  type DisplayError = {
    message: string;
    diagnostics: Diagnostic[];
  };

  type ErrorPayload = {
    error?: { message?: string; code?: string } | string;
    message?: string;
    diagnostics?: Diagnostic[];
  };

  function workdirOptionLabel(directory: WorkingDirectorySummary): string {
    const provider = data.repositories?.items.find((repository) => repository.id === directory.repository_id)
      ?.provider;
    return `${directory.repository_id} · ${formatCurrentWorkdirRevision(directory, provider)}`;
  }

  let { data }: PageProps = $props();
  let workspaceId = $derived(data.workspaceId);
  const ticketContext = untrack(() => data.ticketContext);

  const NEW_WORKING_DIRECTORY_VALUE = '__new_working_directory__';

  let loading = $state(true);
  let options = $state<WorkerLaunchOptionsResponse | null>(null);
  let optionsError = $state<string | null>(null);
  let submitting = $state(false);
  let submitError = $state<DisplayError | null>(null);
  let displayName = $state(
    ticketContext
      ? `${ticketContext.ticketTitle} · ${ticketContext.ticketRole || 'Worker'}`
      : 'Worker',
  );
  let runtimeId = $state('');
  let profile = $state(
    ticketContext
      ? ticketContext.ticketRole === 'reviewer'
        ? 'builtin:reviewer'
        : 'builtin:coder'
      : '',
  );
  let initialText = $state(ticketContext?.initialInput ?? '');
  let workingDirectoryId = $state('');
  let workingDirectoryRepositoryId = $state(ticketContext?.repositoryId ?? '');
  let workingDirectorySelector = $state(ticketContext?.refSelector ?? 'HEAD');
  let relativeCwd = $state('');
  let creatingWorkingDirectory = $state(false);
  let isNewWorkingDirectorySelected = $derived(workingDirectoryId === NEW_WORKING_DIRECTORY_VALUE);
  let selectedRuntime = $derived(options?.runtimes.find((runtime) => runtime.runtime_id === runtimeId));
  let selectedRuntimeAllowsNoWorkdir = $derived(selectedRuntime?.working_directory_required === false);
  let hasSelectedExistingWorkdir = $derived(Boolean(
    workingDirectoryId && !isNewWorkingDirectorySelected && !selectedRuntimeAllowsNoWorkdir,
  ));
  let availableWorkingDirectories = $derived(
    selectedRuntimeAllowsNoWorkdir
      ? []
      : (options?.working_directories ?? []).filter((directory) =>
        directory.status === 'active' &&
        directory.cleanliness === 'clean' &&
        directory.primary_worker_id == null &&
        directory.occupied_by == null
      ),
  );
  let canStartWorker = $derived(Boolean(
    runtimeId &&
      profile &&
      (hasSelectedExistingWorkdir || (selectedRuntimeAllowsNoWorkdir && !isNewWorkingDirectorySelected)),
  ));

  function workerApiPath(path: string): string {
    return workspaceApiPath(workspaceId, path);
  }

  $effect(() => {
    if (!workspaceId) {
      loading = false;
      optionsError = 'workspace id is unavailable';
      return;
    }
    const controller = new AbortController();
    void loadLaunchOptions(controller.signal);
    return () => controller.abort();
  });

  $effect(() => {
    if (selectedRuntimeAllowsNoWorkdir && workingDirectoryId) {
      workingDirectoryId = '';
      relativeCwd = '';
    }
  });

  async function loadLaunchOptions(signal?: AbortSignal) {
    loading = true;
    optionsError = null;
    try {
      const response = await fetch(workerApiPath('/workers/launch-options'), { signal });
      if (!response.ok) {
        throw new Error(`worker launch options failed (${response.status})`);
      }
      const payload = (await response.json()) as WorkerLaunchOptionsResponse;
      options = payload;
      const form = defaultWorkerLaunchForm(payload, {
        runtime_id: runtimeId,
        display_name: displayName,
        profile,
        initial_text: initialText,
        working_directory_id: workingDirectoryId,
        working_directory_repository_id: workingDirectoryRepositoryId,
        working_directory_selector: workingDirectorySelector,
        relative_cwd: relativeCwd,
      });
      runtimeId = form.runtime_id;
      displayName = form.display_name;
      profile = form.profile;
      workingDirectoryId = form.working_directory_id ||
        (ticketContext?.repositoryId ? NEW_WORKING_DIRECTORY_VALUE : '');
      workingDirectoryRepositoryId = form.working_directory_repository_id;
      workingDirectorySelector = form.working_directory_selector;
      relativeCwd = form.relative_cwd;
    } catch (err) {
      if (err instanceof DOMException && err.name === 'AbortError') {
        return;
      }
      optionsError = err instanceof Error ? err.message : 'worker launch options failed';
    } finally {
      if (!signal?.aborted) {
        loading = false;
      }
    }
  }

  async function createWorkingDirectory() {
    if (!runtimeId) {
      submitError = { message: 'select a runtime before creating a workdir', diagnostics: [] };
      return;
    }
    if (selectedRuntimeAllowsNoWorkdir) {
      submitError = { message: 'embedded Runtime does not create workdirs', diagnostics: [] };
      return;
    }
    if (!workingDirectoryRepositoryId) {
      submitError = { message: 'select a repository before creating a workdir', diagnostics: [] };
      return;
    }
    creatingWorkingDirectory = true;
    submitError = null;
    try {
      const response = await fetch(
        workerApiPath(`/runtimes/${encodeURIComponent(runtimeId)}/working-directories`), {
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify({
            runtime_id: runtimeId,
            repository_id: workingDirectoryRepositoryId,
            selector: workingDirectorySelector || null,
          }),
        },
      );
      if (!response.ok) {
        submitError = await responseDisplayError(response, 'workdir create failed');
        return;
      }
      const payload = (await response.json()) as BrowserWorkingDirectoryCreateResponse;
      const items = options?.working_directories ?? [];
      options = options
        ? {
          ...options,
          working_directories: [
            ...items.filter((item) => item.working_directory_id !== payload.item.working_directory_id),
            payload.item,
          ],
        }
        : options;
      workingDirectoryId = payload.item.working_directory_id;
    } catch (err) {
      submitError = exceptionDisplayError(err, 'workdir create failed');
    } finally {
      creatingWorkingDirectory = false;
    }
  }

  async function createWorker() {
    if (!workspaceId) {
      submitError = { message: 'workspace id is unavailable', diagnostics: [] };
      return;
    }
    if (isNewWorkingDirectorySelected || (!workingDirectoryId && !selectedRuntimeAllowsNoWorkdir)) {
      submitError = { message: 'select or create a workdir before starting a Worker; only embedded Runtime can start without one', diagnostics: [] };
      return;
    }

    submitError = null;
    submitting = true;
    try {
      const response = await fetch(workerApiPath('/workers'), {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify(buildBrowserCreateWorkerRequest({
          runtime_id: runtimeId,
          display_name: displayName,
          profile,
          initial_text: initialText,
          working_directory_id: workingDirectoryId,
          working_directory_repository_id: workingDirectoryRepositoryId,
          working_directory_selector: workingDirectorySelector,
          relative_cwd: relativeCwd,
        })),
      });
      if (!response.ok) {
        submitError = await responseDisplayError(response, 'worker create failed');
        return;
      }
      const payload = (await response.json()) as BrowserCreateWorkerResponse;
      await goto(payload.console_href);
    } catch (err) {
      submitError = exceptionDisplayError(err, 'worker create failed');
    } finally {
      submitting = false;
    }
  }

  async function responseDisplayError(response: Response, fallback: string): Promise<DisplayError> {
    try {
      const payload = (await response.json()) as ErrorPayload;
      const diagnostics = Array.isArray(payload.diagnostics) ? payload.diagnostics : [];
      if (typeof payload.error === 'object' && payload.error?.message) {
        return {
          message: `${payload.error.code ?? 'request_failed'}: ${payload.error.message}`,
          diagnostics,
        };
      }
      if (payload.message) {
        const code = typeof payload.error === 'string' ? payload.error : 'request_failed';
        return { message: `${code}: ${payload.message}`, diagnostics };
      }
    } catch {
      // fall through
    }
    return { message: `${fallback} (${response.status})`, diagnostics: [] };
  }

  function exceptionDisplayError(err: unknown, fallback: string): DisplayError {
    return {
      message: err instanceof Error ? err.message : fallback,
      diagnostics: [],
    };
  }
</script>

<svelte:head>
  <title>New Worker · Yoi Workspace</title>
</svelte:head>

<section class="worker-new-page" aria-labelledby="new-worker-heading">
  <header class="worker-new-page-header">
    <div>
      <h1 id="new-worker-heading">New Worker</h1>
      <p>Create a Worker on a selected Runtime. Workdir-less conversation Workers are only available on embedded Runtime.</p>
    </div>
  </header>

  {#if ticketContext}
    <aside class="worker-ticket-context">
      <div>
        <span>Ticket {ticketContext.ticketRole || 'Worker'}</span>
        <strong>{ticketContext.ticketTitle}</strong>
        <code>{ticketContext.ticketId}</code>
      </div>
      <a href={`/w/${workspaceId}/tickets/${encodeURIComponent(ticketContext.ticketId)}`}>View ticket</a>
    </aside>
  {/if}

  {#if loading}
    <p class="section-state">Loading launch options…</p>
  {:else if optionsError}
    <p class="section-state error">{optionsError}</p>
  {:else}
    <form class="worker-launch-form" onsubmit={(event) => { event.preventDefault(); void createWorker(); }}>
      <section class="worker-form-section" aria-labelledby="worker-location-heading">
        <h2 id="worker-location-heading">Location</h2>

        <div class="worker-launch-sentence">
          <span>Run at</span>
          <select class="worker-inline-select wd-select" bind:value={workingDirectoryId} aria-label="Workdir">
            {#if selectedRuntimeAllowsNoWorkdir}
              <option value="">No workdir</option>
            {:else}
              <option value="" disabled>Select workdir</option>
              {#each availableWorkingDirectories as directory}
                <option value={directory.working_directory_id}>
                  {workdirOptionLabel(directory)}
                </option>
              {/each}
              <option value={NEW_WORKING_DIRECTORY_VALUE}>New workdir…</option>
            {/if}
          </select>
          <span>in</span>
          <select class="worker-inline-select runtime-select" bind:value={runtimeId} required aria-label="Runtime">
            {#if options?.runtimes.length}
              {#each options.runtimes as runtime}
                <option value={runtime.runtime_id} disabled={!runtime.can_spawn_worker}>
                  {runtime.display_name}
                </option>
              {/each}
            {:else}
              <option value="" disabled>No Runtime options</option>
            {/if}
          </select>
        </div>

        {#if !selectedRuntimeAllowsNoWorkdir && !workingDirectoryId && !isNewWorkingDirectorySelected}
          <p class="worker-workdir-note">This Runtime requires a selected workdir before starting a Worker.</p>
        {:else if selectedRuntimeAllowsNoWorkdir && !workingDirectoryId}
          <p class="worker-workdir-note">No filesystem tools or Bash will be available without a workdir.</p>
        {/if}

        {#if isNewWorkingDirectorySelected}
          <div class="new-working-directory-panel">
            <h3>New workdir</h3>
            <div class="new-working-directory-fields">
              <label>
                <span>Repository</span>
                <select bind:value={workingDirectoryRepositoryId}>
                  {#if options?.repositories.length}
                    {#each options.repositories as repository}
                      <option value={repository.id}>{repository.display_name}</option>
                    {/each}
                  {:else}
                    <option value="" disabled>No configured repositories</option>
                  {/if}
                </select>
              </label>
              <label>
                <span>Selector</span>
                <input bind:value={workingDirectorySelector} autocomplete="off" placeholder="HEAD" />
              </label>
            </div>
            <button type="button" disabled={creatingWorkingDirectory || !runtimeId || !workingDirectoryRepositoryId} onclick={() => void createWorkingDirectory()}>
              {creatingWorkingDirectory ? 'Creating…' : 'Create workdir'}
            </button>
          </div>
        {/if}

        {#if hasSelectedExistingWorkdir || isNewWorkingDirectorySelected}
          <label class="relative-cwd-field">
            <span>Relative cwd inside workdir</span>
            <input bind:value={relativeCwd} autocomplete="off" placeholder="Optional path inside workdir" />
          </label>
        {/if}
      </section>

      <section class="worker-form-section" aria-labelledby="worker-details-heading">
        <h2 id="worker-details-heading">Worker</h2>
        <div class="worker-detail-grid">
          <label>
            <span>Display name</span>
            <input bind:value={displayName} required maxlength="80" autocomplete="off" />
          </label>
          <label>
            <span>Profile</span>
            <select bind:value={profile} required>
              {#if options?.profiles.length}
                {#each options.profiles as candidate}
                  <option value={candidate.id}>{candidate.label}</option>
                {/each}
              {:else}
                <option value="" disabled>No profile candidates</option>
              {/if}
            </select>
          </label>
        </div>
        <label>
          <span>Initial text</span>
          <textarea bind:value={initialText} rows="7" placeholder="Optional first instruction"></textarea>
        </label>
      </section>

      {#if submitError}
        <div class="section-state error worker-submit-error">
          <p>{submitError.message}</p>
          {#if submitError.diagnostics.length > 0}
            <ul class="worker-error-diagnostics">
              {#each submitError.diagnostics as diagnostic}
                <li class={diagnostic.severity}>
                  <strong>{diagnostic.code}</strong>
                  <span>{diagnostic.message}</span>
                </li>
              {/each}
            </ul>
          {/if}
        </div>
      {/if}
      <div class="worker-new-actions">
        <button type="submit" disabled={submitting || !canStartWorker}>
          {submitting ? 'Starting…' : 'Start Worker'}
        </button>
        <a class="secondary-link" href={`/w/${workspaceId}`}>Cancel</a>
      </div>
    </form>
  {/if}
</section>

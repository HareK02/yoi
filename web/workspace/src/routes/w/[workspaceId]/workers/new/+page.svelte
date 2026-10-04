<script lang="ts">
  import { goto } from '$app/navigation';
  import { untrack } from 'svelte';
  import { readBoundedJson, workspaceApiPath } from '#lib/workspace/api/http.ts';
  import {
    parseWorkingDirectoryCreateResponse,
    validateWorkingDirectoryCreateRequest,
  } from '#lib/workspace/api/workdirs.ts';
  import {
    parseBrowserCreateWorkerResponse,
    parseWorkerApiError,
    parseWorkerLaunchOptionsResponse,
  } from '#lib/workspace/api/workers.ts';
  import { parseSubjektivSubjectListResponse } from '#lib/workspace/memory/api.ts';
  import { formatCurrentWorkdirRevision } from '#lib/workspace/settings/workdir-revision.ts';
  import { formatExternalGrantPermissionLevel } from '#lib/workspace/settings/workdir-permissions.ts';
  import {
    buildCreateWorkspaceWorkerRequest,
    defaultWorkerLaunchForm,
    workerLaunchAttachmentError,
  } from '#lib/workspace/sidebar/worker-launch.ts';
  import type { WorkerLaunchAttachmentFormState } from '#lib/workspace/sidebar/worker-launch.ts';
  import type {
    Diagnostic,
    WorkerLaunchOptionsResponse,
    WorkingDirectorySummary,
  } from '#lib/workspace/sidebar/types.ts';
  import type { SubjektivSubjectResponse } from '#lib/generated/memory-api.ts';
  import type { PageProps } from './$types';

  type DisplayError = {
    message: string;
    diagnostics: Diagnostic[];
  };

  function workdirOptionLabel(directory: WorkingDirectorySummary): string {
    const repositoryKey = directory.source.kind === 'repository'
      ? directory.source.repository_key
      : null;
    const provider = repositoryKey
      ? data.repositories?.items.find((repository) => repository.repository_key === repositoryKey)?.provider
      : null;
    const label = directory.display_name ?? repositoryKey ?? 'External Workdir';
    if (directory.source.kind === 'external_grant') {
      return `${label} · External · ${formatExternalGrantPermissionLevel(directory.source.grant_permissions)}`;
    }
    return `${label} · ${formatCurrentWorkdirRevision(directory, provider)}`;
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
  let subjektivSubjectId = $state('');
  let subjects = $state<SubjektivSubjectResponse[]>([]);
  let subjectsLoading = $state(false);
  let subjectsLoaded = $state(false);
  let subjectsError = $state<string | null>(null);
  let subjectsNextCursor = $state<string | null>(null);
  let subjectsHaveMore = $state(false);
  let previousProfile = $state('');
  let initialText = $state(ticketContext?.initialInput ?? '');
  let workdirAttachments = $state<WorkerLaunchAttachmentFormState[]>([]);
  let workingDirectoryDisplayName = $state('');
  let workingDirectoryRepositoryKey = $state(ticketContext?.repositoryKey ?? '');
  let workingDirectorySelector = $state(ticketContext?.refSelector ?? 'HEAD');
  let creatingWorkingDirectory = $state(false);
  let newWorkingDirectoryAttachmentIndex = $derived(
    workdirAttachments.findIndex((attachment) => attachment.working_directory_id === NEW_WORKING_DIRECTORY_VALUE),
  );
  let isNewWorkingDirectorySelected = $derived(newWorkingDirectoryAttachmentIndex >= 0);
  let selectedRuntime = $derived(options?.runtimes.find((runtime) => runtime.runtime_id === runtimeId));
  let selectedProfile = $derived(options?.profiles.find((candidate) => candidate.id === profile));
  let subjektivConnectionAvailable = $derived(selectedProfile?.feature_connections.subjektiv === true);
  let activeSubjects = $derived(subjects.filter((subject) => subject.state === 'active'));
  let selectedSubject = $derived(activeSubjects.find((subject) => subject.id === subjektivSubjectId));
  let selectedRuntimeAllowsNoWorkdir = $derived(selectedRuntime?.working_directory_required === false);
  let availableWorkingDirectories = $derived(
    selectedRuntimeAllowsNoWorkdir
      ? []
      : (options?.working_directories ?? []).filter((directory) =>
        directory.status === 'active' &&
        (directory.source.kind === 'external_grant' || directory.cleanliness === 'clean') &&
        directory.occupied_by == null
      ),
  );
  let attachmentValidationError = $derived(
    isNewWorkingDirectorySelected
      ? 'Create the new Workdir before starting the Worker.'
      : workerLaunchAttachmentError(workdirAttachments),
  );
  let canStartWorker = $derived(Boolean(
    runtimeId &&
      profile &&
      (selectedRuntimeAllowsNoWorkdir
        ? workdirAttachments.length === 0
        : workdirAttachments.length > 0 && !attachmentValidationError),
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
    if (selectedRuntimeAllowsNoWorkdir && workdirAttachments.length > 0) {
      workdirAttachments = [];
    } else if (selectedRuntime?.working_directory_required === true && workdirAttachments.length === 0) {
      workdirAttachments = [{
        alias: 'workdir',
        working_directory_id: availableWorkingDirectories[0]?.working_directory_id ?? '',
        relative_cwd: '',
      }];
    }
  });

  $effect(() => {
    if (profile !== previousProfile) {
      subjektivSubjectId = '';
      previousProfile = profile;
    }
    if (subjektivConnectionAvailable && !subjectsLoaded && !subjectsLoading) {
      void loadSubjects(false);
    }
  });

  function nextAttachmentAlias(): string {
    const aliases = new Set(workdirAttachments.map((attachment) => attachment.alias.trim()));
    if (!aliases.has('workdir')) return 'workdir';
    let suffix = 2;
    while (aliases.has(`workdir${suffix}`)) suffix += 1;
    return `workdir${suffix}`;
  }

  function addAttachment(): void {
    const selectedWorkdirs = new Set(
      workdirAttachments.map((attachment) => attachment.working_directory_id),
    );
    const available = availableWorkingDirectories.find((directory) =>
      !selectedWorkdirs.has(directory.working_directory_id)
    );
    workdirAttachments = [...workdirAttachments, {
      alias: nextAttachmentAlias(),
      working_directory_id: available?.working_directory_id ?? '',
      relative_cwd: '',
    }];
  }

  function removeAttachment(index: number): void {
    workdirAttachments = workdirAttachments.filter((_, candidateIndex) => candidateIndex !== index);
  }

  function workdirSelectedByAnotherAttachment(workdirId: string, index: number): boolean {
    return workdirAttachments.some((attachment, candidateIndex) =>
      candidateIndex !== index && attachment.working_directory_id === workdirId
    );
  }

  async function loadLaunchOptions(signal?: AbortSignal) {
    loading = true;
    optionsError = null;
    try {
      const response = await fetch(workerApiPath('/workers/launch-options'), { signal });
      if (!response.ok) {
        throw new Error(`worker launch options failed (${response.status})`);
      }
      const payload = parseWorkerLaunchOptionsResponse(
        await readBoundedJson(response, 8 * 1024 * 1024),
      );
      options = payload;
      const form = defaultWorkerLaunchForm(payload, {
        runtime_id: runtimeId,
        display_name: displayName,
        profile,
        subjektiv_subject_id: subjektivSubjectId,
        initial_text: initialText,
        workdir_attachments: workdirAttachments,
        working_directory_repository_key: workingDirectoryRepositoryKey,
        working_directory_selector: workingDirectorySelector,
      });
      runtimeId = form.runtime_id;
      displayName = form.display_name;
      profile = form.profile;
      subjektivSubjectId = form.subjektiv_subject_id;
      const runtimeRequiresWorkdir = payload.runtimes.find((runtime) =>
        runtime.runtime_id === form.runtime_id
      )?.working_directory_required !== false;
      workdirAttachments = form.workdir_attachments;
      if (
        runtimeRequiresWorkdir &&
        ticketContext?.repositoryKey &&
        workdirAttachments.every((attachment) => !attachment.working_directory_id)
      ) {
        workdirAttachments = [{
          alias: 'workdir',
          working_directory_id: NEW_WORKING_DIRECTORY_VALUE,
          relative_cwd: '',
        }];
      }
      workingDirectoryRepositoryKey = form.working_directory_repository_key;
      workingDirectorySelector = form.working_directory_selector;
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

  async function loadSubjects(append: boolean) {
    if (!workspaceId || subjectsLoading) return;
    const cursor = append ? subjectsNextCursor : null;
    if (append && !cursor) return;
    subjectsLoading = true;
    subjectsLoaded = true;
    subjectsError = null;
    try {
      const query = new URLSearchParams({ limit: '100' });
      if (cursor) query.set('cursor', cursor);
      const response = await fetch(workerApiPath(`/subjektiv/subjects?${query}`));
      if (!response.ok) {
        throw new Error(`subject list failed (${response.status})`);
      }
      const payload = parseSubjektivSubjectListResponse(
        await readBoundedJson(response, 8 * 1024 * 1024),
      );
      if (append) {
        const byId = new Map(subjects.map((subject) => [subject.id, subject]));
        for (const subject of payload.items) byId.set(subject.id, subject);
        subjects = [...byId.values()];
      } else {
        subjects = payload.items;
      }
      subjectsNextCursor = payload.next_cursor ?? null;
      subjectsHaveMore = payload.has_more;
      if (
        subjektivSubjectId &&
        !subjects.some((subject) => subject.id === subjektivSubjectId && subject.state === 'active')
      ) {
        subjektivSubjectId = '';
      }
    } catch (err) {
      subjectsError = err instanceof Error ? err.message : 'subject list failed';
    } finally {
      subjectsLoading = false;
    }
  }

  async function createWorkingDirectory() {
    const attachmentIndex = newWorkingDirectoryAttachmentIndex;
    if (attachmentIndex < 0) {
      submitError = { message: 'select New workdir on an attachment first', diagnostics: [] };
      return;
    }
    if (!runtimeId) {
      submitError = { message: 'select a runtime before creating a workdir', diagnostics: [] };
      return;
    }
    if (selectedRuntimeAllowsNoWorkdir) {
      submitError = { message: 'embedded Runtime does not create workdirs', diagnostics: [] };
      return;
    }
    if (!workingDirectoryRepositoryKey) {
      submitError = { message: 'select a repository before creating a workdir', diagnostics: [] };
      return;
    }
    creatingWorkingDirectory = true;
    submitError = null;
    try {
      const request = validateWorkingDirectoryCreateRequest({
        runtime_id: runtimeId,
        ...(workingDirectoryDisplayName.trim() ? { display_name: workingDirectoryDisplayName.trim() } : {}),
        repository_key: workingDirectoryRepositoryKey,
        ...(workingDirectorySelector ? { selector: workingDirectorySelector } : {}),
      });
      const response = await fetch(
        workerApiPath(`/runtimes/${encodeURIComponent(runtimeId)}/working-directories`), {
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify(request),
        },
      );
      if (!response.ok) {
        submitError = await responseDisplayError(response, 'workdir create failed');
        return;
      }
      const payload = parseWorkingDirectoryCreateResponse(await response.json());
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
      workdirAttachments = workdirAttachments.map((attachment, index) =>
        index === attachmentIndex && attachment.working_directory_id === NEW_WORKING_DIRECTORY_VALUE
          ? { ...attachment, working_directory_id: payload.item.working_directory_id }
          : attachment
      );
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
    if (selectedRuntimeAllowsNoWorkdir && workdirAttachments.length > 0) {
      submitError = { message: 'the selected Runtime must start without Workdir attachments', diagnostics: [] };
      return;
    }
    if (!selectedRuntimeAllowsNoWorkdir && workdirAttachments.length === 0) {
      submitError = { message: 'add at least one Workdir attachment before starting a Worker; only embedded Runtime can start without one', diagnostics: [] };
      return;
    }
    if (attachmentValidationError) {
      submitError = { message: attachmentValidationError, diagnostics: [] };
      return;
    }

    submitError = null;
    submitting = true;
    try {
      const response = await fetch(workerApiPath('/workers'), {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify(buildCreateWorkspaceWorkerRequest({
          runtime_id: runtimeId,
          display_name: displayName,
          profile,
          subjektiv_subject_id: subjektivSubjectId,
          initial_text: initialText,
          workdir_attachments: workdirAttachments,
          working_directory_repository_key: workingDirectoryRepositoryKey,
          working_directory_selector: workingDirectorySelector,
        })),
      });
      if (!response.ok) {
        submitError = await responseDisplayError(response, 'worker create failed');
        return;
      }
      const payload = parseBrowserCreateWorkerResponse(
        await readBoundedJson(response, 8 * 1024 * 1024),
      );
      await goto(payload.console_href);
    } catch (err) {
      submitError = exceptionDisplayError(err, 'worker create failed');
    } finally {
      submitting = false;
    }
  }

  async function responseDisplayError(response: Response, fallback: string): Promise<DisplayError> {
    try {
      const payload = parseWorkerApiError(
        await readBoundedJson(response, 1024 * 1024),
      );
      return {
        message: `${payload.error}: ${payload.message}`,
        diagnostics: payload.diagnostics ?? [],
      };
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
          <span>Run in</span>
          <select class="worker-inline-select runtime-select" bind:value={runtimeId} required aria-label="Runtime">
            {#if options?.runtimes.length}
              {#each options.runtimes as runtime}
                <option value={runtime.runtime_id} disabled={!runtime.worker_creation_available}>
                  {runtime.display_name}
                </option>
              {/each}
            {:else}
              <option value="" disabled>No Runtime options</option>
            {/if}
          </select>
        </div>

        {#if selectedRuntimeAllowsNoWorkdir}
          <p class="worker-workdir-note">No filesystem tools or Bash will be available without a Workdir.</p>
        {:else}
          <div class="worker-attachment-heading">
            <h3>Initial Workdir attachments</h3>
            <button type="button" class="secondary-button" onclick={addAttachment}>Add attachment</button>
          </div>

          <div class="worker-attachment-list">
            {#each workdirAttachments as attachment, index}
              <div class="worker-attachment-row">
                <label>
                  <span>Alias</span>
                  <input
                    bind:value={attachment.alias}
                    autocomplete="off"
                    maxlength="64"
                    placeholder="workdir"
                    aria-label={`Attachment ${index + 1} alias`}
                  />
                </label>
                <label>
                  <span>Workdir</span>
                  <select bind:value={attachment.working_directory_id} aria-label={`Attachment ${index + 1} Workdir`}>
                    <option value="" disabled>Select Workdir</option>
                    {#each availableWorkingDirectories as directory}
                      <option
                        value={directory.working_directory_id}
                        disabled={workdirSelectedByAnotherAttachment(directory.working_directory_id, index)}
                      >
                        {workdirOptionLabel(directory)}
                      </option>
                    {/each}
                    <option
                      value={NEW_WORKING_DIRECTORY_VALUE}
                      disabled={isNewWorkingDirectorySelected && newWorkingDirectoryAttachmentIndex !== index}
                    >New Workdir…</option>
                  </select>
                </label>
                <label>
                  <span>Relative cwd</span>
                  <input
                    bind:value={attachment.relative_cwd}
                    autocomplete="off"
                    placeholder="Optional path inside Workdir"
                    aria-label={`Attachment ${index + 1} relative cwd`}
                  />
                </label>
                <button
                  type="button"
                  class="worker-attachment-remove"
                  onclick={() => removeAttachment(index)}
                  disabled={creatingWorkingDirectory && newWorkingDirectoryAttachmentIndex === index}
                  aria-label={`Remove attachment ${index + 1}`}
                >Remove</button>
              </div>
            {/each}
          </div>

          {#if attachmentValidationError}
            <p class="worker-workdir-note error">{attachmentValidationError}</p>
          {/if}
        {/if}

        {#if isNewWorkingDirectorySelected}
          <div class="new-working-directory-panel">
            <h3>New Workdir for “{workdirAttachments[newWorkingDirectoryAttachmentIndex]?.alias || `attachment ${newWorkingDirectoryAttachmentIndex + 1}`}”</h3>
            <div class="new-working-directory-fields">
              <label>
                <span>Display name</span>
                <input bind:value={workingDirectoryDisplayName} autocomplete="off" placeholder="Optional label" maxlength="80" />
              </label>
              <label>
                <span>Repository</span>
                <select bind:value={workingDirectoryRepositoryKey}>
                  {#if options?.repositories.length}
                    {#each options.repositories as repository}
                      <option value={repository.repository_key}>{repository.repository_key}</option>
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
            <button type="button" disabled={creatingWorkingDirectory || !runtimeId || !workingDirectoryRepositoryKey} onclick={() => void createWorkingDirectory()}>
              {creatingWorkingDirectory ? 'Creating…' : 'Create Workdir'}
            </button>
          </div>
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

        {#if subjektivConnectionAvailable}
          <div class="worker-feature-connection" data-worker-feature-connection="subjektiv">
            <div class="worker-feature-connection-heading">
              <div>
                <h3>Subject Memory</h3>
                <p>Optionally connect this Worker to one existing active subject.</p>
              </div>
              <span>Optional</span>
            </div>
            <label>
              <span>Subject</span>
              <select bind:value={subjektivSubjectId} aria-label="Subject Memory connection">
                <option value="">Do not connect a subject</option>
                {#each activeSubjects as subject (subject.id)}
                  <option value={subject.id}>{subject.role} · {subject.id}</option>
                {/each}
              </select>
            </label>
            {#if selectedSubject}
              <p class="worker-feature-connection-selection">
                <strong>{selectedSubject.role}</strong>
                <code title={selectedSubject.id}>{selectedSubject.id}</code>
                {#if selectedSubject.current_worker}
                  <span>Current Worker: {selectedSubject.current_worker.display_name}</span>
                {/if}
              </p>
            {/if}
            {#if subjectsLoading && subjects.length === 0}
              <p class="worker-feature-connection-state" role="status">Loading authorized subjects…</p>
            {:else if subjectsError}
              <div class="worker-feature-connection-state is-error" role="alert">
                <span>{subjectsError}</span>
                <button type="button" class="secondary-button" onclick={() => void loadSubjects(false)}>Retry</button>
              </div>
            {:else if subjectsLoaded && activeSubjects.length === 0}
              <p class="worker-feature-connection-state" role="status">No active subjects are available.</p>
            {/if}
            {#if subjectsHaveMore}
              <button
                type="button"
                class="secondary-button worker-subject-load-more"
                disabled={subjectsLoading}
                onclick={() => void loadSubjects(true)}
              >{subjectsLoading ? 'Loading…' : 'Load more subjects'}</button>
            {/if}
            <p class="worker-feature-connection-note">Profile enablement does not connect a subject by itself.</p>
          </div>
        {/if}
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

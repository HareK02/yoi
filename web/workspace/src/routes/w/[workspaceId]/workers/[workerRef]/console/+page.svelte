<script lang="ts">
    import { invalidateAll } from "$app/navigation";
    import { tick, untrack, type SvelteComponent } from "svelte";
    import { prefersReducedMotion } from "svelte/motion";
    import { fly } from "svelte/transition";
    import ConsoleLineItem from "#lib/workspace/console/ConsoleLineItem.svelte";
    import ConsoleTurnNavigation from "#lib/workspace/console/ConsoleTurnNavigation.svelte";
    import ConsoleDisplayStateView from "#lib/workspace/console/ConsoleDisplayState.svelte";
    import ConsoleTasks from "#lib/workspace/console/ConsoleTasks.svelte";
    import { namePastedImage } from "#lib/workspace/console/composer-paste.ts";
    import ComposerInput from "#lib/workspace/console/ComposerInput.svelte";
    import type { ComposerDraftSnapshot } from "#lib/workspace/console/composer-draft.ts";
    import {
        canDeliverComposerDraft,
        sendComposerDelivery,
        type ComposerDelivery,
    } from "#lib/workspace/console/composer-delivery.ts";
    import {
        buildComposerSegmentsRequest,
        type WorkerConsoleInputRequest,
    } from "#lib/workspace/console/composer-command.ts";
    import { FileCompletions } from "#lib/workspace/console/file-completions.ts";
    import WorkerRunStatus from "#lib/workspace/console/WorkerRunStatus.svelte";
    import WorkerContextStatus from "#lib/workspace/console/WorkerContextStatus.svelte";
    import { resolveWorkerControlShortcut } from "#lib/workspace/console/worker-control-shortcuts.ts";
    import {
        consoleWorkerViews,
        createConsoleProjector,
        isConsoleProjectionEvent,
        mergeCommittedHistoryLines,
        projectConsoleLines,
        projectSessionHistoryEntries,
        resolveConsoleViewScrollTop,
        resolveConsoleWorkerView,
        type ConsoleEventInput,
        type ConsoleProjection,
        type ConsoleViewMode,
        type ConsoleViewScroll,
    } from "#lib/workspace/console/model.ts";
    import type {
        Event as ProtocolEvent,
        Method as ProtocolMethod,
        PendingSubmissionsSnapshot,
        RewindTarget,
        Segment,
        SessionHistoryPage,
        SessionToolAttachment,
    } from "#lib/generated/protocol.ts";
    import {
        MAX_FILES_PER_SUBMISSION,
        uploadAttachment,
        validateAttachmentFile,
        type ComposerAttachment,
    } from "#lib/workspace/console/composer-attachments.ts";
    import { pushWorkspaceAlert } from "#lib/workspace/alerts/store.ts";
    import {
        boundedConsoleReason,
        displayedConsoleSource,
        type ConsoleDisplaySource,
        type ConsoleDisplayState,
    } from "#lib/workspace/console/console-display-state.ts";
    import { workspaceApiPath } from "#lib/workspace/api/http.ts";
    import { workspaceMultiplexer, type WorkspaceMultiplexerSubscription } from "#lib/workspace/multiplexer.ts";
    import {
        isCurrentWorkerSessionRequest,
        resolveWorkerSessionTarget,
        workerSessionAction,
        workerSessionRequestInit,
        type WorkerSessionObservation,
        type WorkerSessionRequestIdentity,
        type WorkerSessionTarget,
    } from "#lib/workspace/session-observation.ts";
    import {
        applyConsoleHistoryPage,
        beginConsoleHistoryRequest,
        conversationTurnPreviewsFromLines,
        emptyConsoleHistoryState,
        failConsoleHistoryRequest,
        historyEntries,
        setConsoleHistoryTopEdge,
        shouldLoadHistoryAtTop,
        type ConsoleHistoryState,
    } from "#lib/workspace/console/history.ts";
    import type { TurnNavigationItem } from "#lib/workspace/console/turn-navigation.ts";
    import type { Diagnostic, Worker } from "#lib/workspace/sidebar/types.ts";

    type Props = {
        data: {
            workspaceId: string;
            runtimeId: string | null;
            workerId: string | null;
            worker: Worker | null;
            workerError: string | null;
        };
    };

    let { data }: Props = $props();

    const workspaceId = $derived(data.workspaceId);
    const runtimeId = $derived(data.runtimeId);
    const workerId = $derived(data.workerId);

    function workerApiPath(path: string): string {
        return workspaceApiPath(workspaceId, path);
    }

    let worker = $state<Worker | null>(untrack(() => data.worker));
    let liveWorkerState = $state<string | null>(
        untrack(() => data.worker?.state ?? null),
    );
    let workerError = $state<string | null>(untrack(() => data.workerError));
    type ComposerInputHandle = {
        snapshot(): ComposerDraftSnapshot;
        focus(): void;
        containsTarget(target: EventTarget | null): boolean;
        cursor(): number;
        replaceRange(from: number, to: number, content: string): void;
        clear(): void;
        restoreSegments(
            segments: readonly Segment[],
            preserveExactText?: boolean,
        ): void;
    };

    type ComposerDraftCache = {
        segments: Segment[];
        preserveExactText: boolean;
    };

    const EMPTY_DRAFT: ComposerDraftSnapshot = {
        document: "",
        content: "",
        segments: [],
        pastes: [],
        textPastes: [],
    };

    let draft = $state<ComposerDraftSnapshot>(EMPTY_DRAFT);
    let attachments = $state<ComposerAttachment[]>([]);
    let nextAttachmentId = 1;
    let fileInput: HTMLInputElement | null = null;
    let isDraggingFiles = $state(false);
    let sending = $state(false);
    let rewindTargets = $state<RewindTarget[]>([]);
    let rewindHeadEntries = $state(0);
    let protocolState = $state<"connecting" | "open" | "closed" | "error">(
        "connecting",
    );
    let consoleDisplayState = $state<ConsoleDisplayState>({
        kind: "loading",
        stage: "session",
    });
    let protocolSubscription: WorkspaceMultiplexerSubscription | null = null;
    let pendingSubmissions = $state<PendingSubmissionsSnapshot>({
        revision: 0,
        notification_count: 0,
        head_id: null,
        submissions: [],
    });
    let pendingSubmissionItems = $derived(pendingSubmissions.submissions ?? []);
    const fileCompletions = new FileCompletions((prefix) => sendProtocolMethod({
        method: "list_completions", params: { kind: "file", prefix },
    }));
    let streamDiagnostics = $state<Diagnostic[]>([]);
    let workerDetailsOpen = $state(false);
    let taskPaneOpen = $state(false);
    let selectedWorkerViewSessionId = $state<string | null>(null);
    let workerViewSelectionGeneration = 0;
    let consoleViewMode = $state<ConsoleViewMode>("overview");
    let consoleBodyElement: HTMLElement | null = null;
    let turnNavigationElement = $state<HTMLElement | null>(null);
    let composerInputElement = $state<
        (SvelteComponent & ComposerInputHandle) | null
    >(null);
    const composerDrafts = new Map<string, ComposerDraftCache>();
    let activeComposerTargetKey = $state(untrack(() =>
        runtimeId && workerId ? `${workspaceId}:${runtimeId}:${workerId}` : "",
    ));
    let autoFollowConsole = $state(true);
    const consoleViewScroll = new Map<string, ConsoleViewScroll>();
    const CONSOLE_BOTTOM_THRESHOLD_PX = 48;
    const consoleProjector = createConsoleProjector();
    let consoleProjection = $state.raw<ConsoleProjection>(
        consoleProjector.snapshot(),
    );
    let historyByView = $state.raw<Record<string, ConsoleHistoryState>>({});
    const pendingHistoryRefreshes = new Set<string>();
    let pendingObservationEvents: ConsoleEventInput[] = [];
    let pendingInitialSnapshotApplication: {
        eventId: string;
        token: number;
        source: ConsoleDisplaySource;
    } | null = null;
    let protocolEventSequence = 0;
    let pendingStreamDiagnostics: Diagnostic[] = [];
    let observationFlushHandle: number | null = null;
    let nextReloadToken = 0;
    let reloadToken = $state(0);

    type ConsoleTarget = WorkerSessionTarget;

    const consoleTarget = $derived(
        resolveWorkerSessionTarget(workspaceId, runtimeId, workerId),
    );
    const controlAlertId = $derived(
        `worker-console-control:${runtimeId ?? "unresolved"}:${workerId ?? "unresolved"}`,
    );

    const workerViews = $derived(consoleWorkerViews(consoleProjection));
    const selectedWorkerView = $derived(
        resolveConsoleWorkerView(
            consoleProjection,
            selectedWorkerViewSessionId,
        ),
    );
    const selectedConsoleProjection = $derived(selectedWorkerView.console);
    const selectedHistoryKey = $derived(
        `${workspaceId}:${runtimeId ?? ""}:${workerId ?? ""}:${selectedWorkerViewSessionId ?? "main"}`,
    );
    const selectedHistory = $derived(
        historyByView[selectedHistoryKey] ?? emptyConsoleHistoryState(),
    );
    const selectedAttachmentSessionId = $derived(
        selectedWorkerView.sessionId ?? selectedHistory.sessionId,
    );
    function attachmentUrl(attachment: SessionToolAttachment): string | null {
        if (!runtimeId || !workerId || !selectedAttachmentSessionId) return null;
        return workerApiPath(
            `runtimes/${encodeURIComponent(runtimeId)}/workers/${encodeURIComponent(workerId)}` +
            `/sessions/${encodeURIComponent(selectedAttachmentSessionId)}` +
            `/attachments/${encodeURIComponent(attachment.attachment_id)}`,
        );
    }
    const committedHistoryLines = $derived(
        projectSessionHistoryEntries(
            historyEntries(selectedHistory),
            selectedConsoleProjection.cwd,
        ),
    );
    const lines = $derived(
        projectConsoleLines(
            mergeCommittedHistoryLines(
                committedHistoryLines,
                selectedConsoleProjection.lines,
            ),
            consoleViewMode,
        ),
    );
    const turnNavigationItems = $derived(
        conversationTurnPreviewsFromLines(lines),
    );
    const tasks = $derived(selectedConsoleProjection.tasks);
    const diagnostics = $derived(
        mergeDiagnostics(worker?.diagnostics ?? [], streamDiagnostics),
    );
    const workerState = $derived(
        liveWorkerState ?? (worker?.state === "stopped" ? "stopped" : "loading"),
    );
    const workerRunning = $derived(workerState === "running");
    const workerPaused = $derived(workerState === "paused");
    const composerEditable = $derived(protocolState === "open" && !sending);
    const draftHasText = $derived(draft.content.trim().length > 0);
    const draftHasAttachments = $derived(attachments.length > 0);
    const canSubmitDraft = $derived(
        canDeliverComposerDraft({
            delivery: "submit",
            workerState,
            protocolOpen: protocolState === "open",
            sending,
            hasText: draftHasText,
            hasAttachments: draftHasAttachments,
        }),
    );
    const canQueueDraft = $derived(
        canDeliverComposerDraft({
            delivery: "queue",
            workerState,
            protocolOpen: protocolState === "open",
            sending,
            hasText: draftHasText,
            hasAttachments: draftHasAttachments,
        }),
    );
    const canNotifyDraft = $derived(
        canDeliverComposerDraft({
            delivery: "notify",
            workerState,
            protocolOpen: protocolState === "open",
            sending,
            hasText: draftHasText,
            hasAttachments: draftHasAttachments,
        }),
    );
    const canStopFromComposer = $derived(workerRunning && composerEditable);
    const composerSubmitDisabled = $derived(
        workerRunning ? !canStopFromComposer : !canSubmitDraft,
    );

    async function getJson<T>(path: string): Promise<T> {
        const response = await fetch(path);
        if (!response.ok) {
            throw new Error(`GET ${path} failed: ${response.status}`);
        }
        return response.json() as Promise<T>;
    }

    type WorkerSessionHistoryResponse =
        | { availability: "page"; page: SessionHistoryPage }
        | {
            availability: "unavailable";
            reason: string;
            message: string;
        };

    function mainHistoryKey(target: ConsoleTarget): string {
        return `${target.workspaceId}:${target.runtimeId}:${target.workerId}:main`;
    }

    function historyStateFor(key: string): ConsoleHistoryState {
        return historyByView[key] ?? emptyConsoleHistoryState();
    }

    function setHistoryState(key: string, state: ConsoleHistoryState) {
        historyByView = { ...historyByView, [key]: state };
    }

    async function requestHistoryPage(
        target: ConsoleTarget,
        key: string,
        cursor: string | null,
        token: number,
    ) {
        const started = beginConsoleHistoryRequest(historyStateFor(key), cursor);
        if (!started) return;
        setHistoryState(key, started);

        const anchors = cursor === null ? null : captureHistoryAnchors();
        const query = new URLSearchParams({ limit: "5" });
        if (cursor) query.set("cursor", cursor);
        const path = workerApiPath(
            `/runtimes/${encodeURIComponent(target.runtimeId)}/workers/${encodeURIComponent(target.workerId)}/session/history?${query}`,
        );
        try {
            const response = await fetch(path, {
                credentials: "same-origin",
                headers: { accept: "application/json" },
            });
            if (response.status === 404 || response.status === 405) {
                throw new Error("Earlier conversation is not supported by this Runtime version.");
            }
            if (!response.ok) {
                throw new Error(`History request failed (${response.status}).`);
            }
            const payload = (await response.json()) as WorkerSessionHistoryResponse;
            if (token !== reloadToken || mainHistoryKey(target) !== key) return;
            if (pendingHistoryRefreshes.has(key)) return;
            if (payload.availability === "unavailable") {
                setHistoryState(
                    key,
                    failConsoleHistoryRequest(historyStateFor(key), payload.message),
                );
                return;
            }
            setHistoryState(
                key,
                applyConsoleHistoryPage(historyStateFor(key), payload.page, cursor),
            );
            await tick();
            restoreHistoryAnchors(anchors);
        } catch (error) {
            if (
                token !== reloadToken ||
                mainHistoryKey(target) !== key ||
                pendingHistoryRefreshes.has(key)
            )
                return;
            setHistoryState(
                key,
                failConsoleHistoryRequest(
                    historyStateFor(key),
                    error instanceof Error ? error.message : String(error),
                ),
            );
        } finally {
            const refreshAgain = pendingHistoryRefreshes.delete(key);
            if (
                refreshAgain &&
                token === reloadToken &&
                mainHistoryKey(target) === key
            ) {
                const stale = historyStateFor(key);
                setHistoryState(key, {
                    ...stale,
                    status: stale.turns.length > 0 ? "ready" : "idle",
                    error: null,
                    requestedCursor: null,
                });
                void requestHistoryPage(target, key, null, token);
            }
        }
    }

    type HistoryAnchor = { id: string; top: number };
    type HistoryAnchors = {
        transcript: HistoryAnchor | null;
        navigation: HistoryAnchor | null;
    };

    function captureElementAnchor(
        container: HTMLElement | null,
        selector: string,
        dataKey: "consoleLineId" | "turnId",
    ): HistoryAnchor | null {
        const first = container?.querySelector<HTMLElement>(selector);
        const id = first?.dataset[dataKey] ?? "";
        return first && id ? { id, top: first.getBoundingClientRect().top } : null;
    }

    function captureHistoryAnchors(): HistoryAnchors {
        return {
            transcript: captureElementAnchor(
                consoleBodyElement,
                "[data-console-line-id]",
                "consoleLineId",
            ),
            navigation: captureElementAnchor(
                turnNavigationElement,
                "[data-turn-id]",
                "turnId",
            ),
        };
    }

    function restoreElementAnchor(
        container: HTMLElement | null,
        selector: string,
        anchor: HistoryAnchor | null,
    ) {
        if (!anchor || !container) return;
        const target = container.querySelector<HTMLElement>(selector);
        if (!target) return;
        container.scrollTop += target.getBoundingClientRect().top - anchor.top;
    }

    function restoreHistoryAnchors(anchors: HistoryAnchors | null) {
        if (!anchors) return;
        restoreElementAnchor(
            consoleBodyElement,
            `[data-console-line-id="${CSS.escape(anchors.transcript?.id ?? "")}"]`,
            anchors.transcript,
        );
        restoreElementAnchor(
            turnNavigationElement,
            `[data-turn-id="${CSS.escape(anchors.navigation?.id ?? "")}"]`,
            anchors.navigation,
        );
    }

    function loadEarlierHistory() {
        const target = consoleTarget;
        if (!target || selectedWorkerViewSessionId !== null) return;
        const key = mainHistoryKey(target);
        const state = historyStateFor(key);
        if (!state.hasMore || !state.cursor) return;
        void requestHistoryPage(target, key, state.cursor, reloadToken);
    }

    function retryHistoryLoad() {
        const target = consoleTarget;
        if (!target || selectedWorkerViewSessionId !== null) return;
        const key = mainHistoryKey(target);
        const state = historyStateFor(key);
        void requestHistoryPage(
            target,
            key,
            state.hasMore ? state.cursor : null,
            reloadToken,
        );
    }

    function fenceCurrentHistoryForRewind() {
        const target = consoleTarget;
        if (!target) return;
        const key = mainHistoryKey(target);
        const current = historyStateFor(key);
        const loading = current.status === "loading";
        const reset = emptyConsoleHistoryState();
        setHistoryState(key, {
            ...reset,
            status: loading ? "loading" : "idle",
            requestedCursor: loading ? current.requestedCursor : null,
            topEdgeArmed: current.topEdgeArmed,
        });
        if (loading) pendingHistoryRefreshes.add(key);
    }

    function refreshCurrentHistory() {
        const target = consoleTarget;
        if (!target) return;
        const key = mainHistoryKey(target);
        if (historyStateFor(key).status === "loading") {
            pendingHistoryRefreshes.add(key);
            return;
        }
        void requestHistoryPage(target, key, null, reloadToken);
    }

    function handleHistoryTopEdge(atTop: boolean) {
        if (selectedWorkerViewSessionId !== null) return;
        const key = selectedHistoryKey;
        const state = historyStateFor(key);
        if (shouldLoadHistoryAtTop(state, atTop)) {
            loadEarlierHistory();
        } else if (!atTop) {
            const rearmed = setConsoleHistoryTopEdge(state, false);
            if (rearmed !== state) setHistoryState(key, rearmed);
        }
    }

    function jumpToConversationTurn(item: TurnNavigationItem) {
        const line = lines.find(
            (candidate) => candidate.entryId === item.lineId || candidate.id === item.lineId,
        );
        const target = line
            ? consoleBodyElement?.querySelector(
                `[data-console-line-id="${CSS.escape(line.id)}"]`,
            )
            : null;
        if (target instanceof HTMLElement && line) {
            selectConsoleTurn(line.id);
        }
    }

    async function loadWorker(target: ConsoleTarget, token: number) {
        workerError = null;
        try {
            const payload = await getJson<Worker>(
                workspaceApiPath(
                    target.workspaceId,
                    `/runtimes/${encodeURIComponent(target.runtimeId)}/workers/${encodeURIComponent(target.workerId)}`,
                ),
            );
            if (token !== reloadToken) return;
            worker = payload;
            liveWorkerState = payload.state;
        } catch (error) {
            if (token !== reloadToken) return;
            workerError =
                error instanceof Error ? error.message : String(error);
            worker = null;
            liveWorkerState = null;
        }
    }

    function advanceReloadToken(): number {
        nextReloadToken += 1;
        reloadToken = nextReloadToken;
        return nextReloadToken;
    }

    function beginConsoleLoad(reason = "Waiting for a fresh conversation snapshot.") {
        const source = displayedConsoleSource(consoleDisplayState);
        consoleDisplayState = source
            ? {
                kind: "stale",
                source,
                phase: "reconnecting",
                reason: boundedConsoleReason(reason, "Reconnecting to live updates."),
            }
            : { kind: "loading", stage: "session" };
        protocolState = "connecting";
    }

    function waitForInitialSnapshot() {
        if (displayedConsoleSource(consoleDisplayState) === null) {
            consoleDisplayState = { kind: "loading", stage: "snapshot" };
        }
    }

    function completeInitialSnapshot(
        token: number,
        source: ConsoleDisplaySource,
    ) {
        if (token !== reloadToken) return;
        consoleDisplayState = { kind: "ready", source };
    }

    function showConsoleUnavailable(message: string) {
        const reason = boundedConsoleReason(
            message,
            "The Worker Session is not available.",
        );
        const source = displayedConsoleSource(consoleDisplayState);
        consoleDisplayState = source
            ? { kind: "stale", source, phase: "disconnected", reason }
            : { kind: "unavailable", reason };
    }

    function showConsoleFailure(message?: string) {
        const reason = boundedConsoleReason(
            message,
            "The Worker Session connection ended before a snapshot was received.",
        );
        const source = displayedConsoleSource(consoleDisplayState);
        consoleDisplayState = source
            ? { kind: "stale", source, phase: "disconnected", reason }
            : { kind: "failed", reason };
    }

    function showConsoleReconnecting(message?: string) {
        const source = displayedConsoleSource(consoleDisplayState);
        if (!source) return;
        consoleDisplayState = {
            kind: "stale",
            source,
            phase: "reconnecting",
            reason: boundedConsoleReason(
                message,
                "Waiting for a fresh conversation snapshot.",
            ),
        };
    }

    function retryConsoleLoad() {
        if (displayedConsoleSource(consoleDisplayState) === null) {
            resetObservedEvents();
        }
        const target = consoleTarget;
        beginConsoleLoad();
        const token = advanceReloadToken();
        if (!target) {
            void invalidateAll();
            return;
        }
        if (!worker) void loadWorker(target, token);
    }

    function resetObservedEvents() {
        cancelObservationFlush();
        consoleProjection = consoleProjector.reset();
    }

    function cancelObservationFlush() {
        if (observationFlushHandle !== null) {
            window.cancelAnimationFrame(observationFlushHandle);
            observationFlushHandle = null;
        }
        pendingObservationEvents = [];
        pendingInitialSnapshotApplication = null;
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
        const initialSnapshotApplication = pendingInitialSnapshotApplication;
        const diagnosticBatch = pendingStreamDiagnostics;
        pendingObservationEvents = [];
        pendingInitialSnapshotApplication = null;
        pendingStreamDiagnostics = [];

        if (eventBatch.length > 0) {
            consoleProjection = consoleProjector.append(eventBatch);
            liveWorkerState = consoleProjection.status === "shutdown"
                ? "shutdown"
                : workerStateFromSnapshot(consoleProjection.workerState);
            if (
                eventBatch.some((event) => event.event.event === "rewind_applied")
            ) {
                fenceCurrentHistoryForRewind();
            }
            if (
                eventBatch.some((event) =>
                    event.event.event === "snapshot" ||
                    event.event.event === "segment_rotated" ||
                    event.event.event === "rewind_applied" ||
                    event.event.event === "user_message" ||
                    event.event.event === "session_entry_committed"
                )
            ) {
                refreshCurrentHistory();
            }
            if (
                initialSnapshotApplication &&
                eventBatch.some(
                    (event) => event.eventId === initialSnapshotApplication.eventId,
                )
            ) {
                completeInitialSnapshot(
                    initialSnapshotApplication.token,
                    initialSnapshotApplication.source,
                );
            }
        }

        if (diagnosticBatch.length > 0) {
            streamDiagnostics = [...streamDiagnostics, ...diagnosticBatch];
        }
    }

    function handleIncomingProtocolEvent(
        payload: ProtocolEvent,
        initialSnapshot?: { token: number; source: ConsoleDisplaySource },
    ) {
        handleProtocolCommandEvent(payload);
        if (payload.event === "snapshot") {
            pendingSubmissions = payload.data.session.pending_submissions;
        } else if (payload.event === "segment_rotated") {
            pendingSubmissions = payload.data.session.pending_submissions;
        } else if (payload.event === "pending_submissions_changed") {
            pendingSubmissions = payload.data.pending;
        }
        if (payload.event === "error") {
            queueObservationDiagnostic({
                code: payload.data.code,
                severity: "error",
                message: payload.data.message,
            });
        }
        if (!isConsoleProjectionEvent(payload)) {
            return;
        }

        const eventId = `protocol-${++protocolEventSequence}`;
        const observedAtMs = Date.now();
        pendingObservationEvents.push({
            eventId,
            event: payload,
            observedAtMs,
        });
        if (initialSnapshot && payload.event === "snapshot") {
            pendingInitialSnapshotApplication = {
                eventId,
                token: initialSnapshot.token,
                source: initialSnapshot.source,
            };
        }
        scheduleObservationFlush();
    }

    function queueObservationDiagnostic(diagnostic: Diagnostic) {
        pendingStreamDiagnostics.push(diagnostic);
        scheduleObservationFlush();
    }

    function handleComposerKeydown(event: KeyboardEvent) {
        if (event.key === "PageUp" || event.key === "PageDown") {
            event.preventDefault();
            scrollConsoleByPage(event.key === "PageDown" ? 1 : -1);
        }
    }

    function scrollConsoleByPage(direction: 1 | -1) {
        if (!consoleBodyElement) {
            return;
        }
        consoleBodyElement.scrollBy({
            top: direction * Math.max(consoleBodyElement.clientHeight * 0.86, 1),
            behavior: "auto",
        });
    }

    function sendControl(method: ProtocolMethod, label: string) {
        try {
            sendProtocolMethod(method);
            pushWorkspaceAlert(
                "info",
                `${label} sent through Worker protocol.`,
                { id: controlAlertId, title: "Worker control" },
            );
        } catch (error) {
            const message = error instanceof Error ? error.message : String(error);
            pushWorkspaceAlert("error", message, {
                id: controlAlertId,
                title: "Worker control failed",
            });
        }
    }

    let nextWorkerCommandId = 1;

    function lifecycleMethod(
        command: "pause" | "cancel" | "resume" | "compact",
    ): ProtocolMethod | null {
        const state = consoleProjection.workerState;
        if (!state) {
            reportComposerError("Worker state snapshot is not available; reconnect before sending control.");
            return null;
        }
        const commandId = Math.max(
            nextWorkerCommandId,
            state.last_command_id + 1,
        );
        nextWorkerCommandId = commandId + 1;
        const envelope = {
            command_id: commandId,
        };
        switch (command) {
            case "pause":
                return { method: "pause", params: { command: envelope } };
            case "cancel":
                return { method: "cancel", params: { command: envelope } };
            case "resume":
                return { method: "resume", params: { command: envelope } };
            case "compact":
                return { method: "compact", params: { command: envelope } };
        }
    }

    function sendWorkerControl(command: "pause" | "cancel" | "resume") {
        const label = command[0].toUpperCase() + command.slice(1);
        const method = lifecycleMethod(command);
        if (method) sendControl(method, label);
    }

    function isEditableTarget(target: EventTarget | null): boolean {
        return (
            target instanceof HTMLInputElement ||
            target instanceof HTMLTextAreaElement ||
            target instanceof HTMLSelectElement ||
            (target instanceof HTMLElement && target.isContentEditable)
        );
    }

    function targetHasSelection(target: EventTarget | null): boolean {
        if (
            target instanceof HTMLInputElement ||
            target instanceof HTMLTextAreaElement
        ) {
            return (
                target.selectionStart !== null &&
                target.selectionEnd !== null &&
                target.selectionStart !== target.selectionEnd
            );
        }
        return Boolean(window.getSelection()?.toString());
    }

    function handleWorkerControlShortcut(event: KeyboardEvent) {
        const composerFocused = composerInputElement?.containsTarget(event.target) ?? false;
        const command = resolveWorkerControlShortcut(event, {
            protocolOpen: protocolState === "open",
            running: workerRunning,
            paused: workerPaused,
            composerFocused,
            draftBlank: draft.content.trim().length === 0,
            editableTarget: isEditableTarget(event.target) && !composerFocused,
            hasSelection: targetHasSelection(event.target),
        });
        if (!command) return;

        event.preventDefault();
        event.stopPropagation();
        sendWorkerControl(command);
    }

    function rewindTo(target: RewindTarget) {
        sendControl(
            {
                method: "rewind_to",
                params: {
                    target: target.id,
                    expected_head_entries: rewindHeadEntries,
                },
            },
            `Rewind to ${target.preview || "target"}`,
        );
    }

    function composerRequestToProtocolMethod(
        request: WorkerConsoleInputRequest,
    ): ProtocolMethod {
        switch (request.kind) {
            case "user":
                return {
                    method: "submit",
                    params: {
                        submission_request_id: crypto.randomUUID(),
                        input: request.segments ?? [
                            { kind: "text", content: request.content },
                        ],
                    },
                };
            case "notify":
                return {
                    method: "notify",
                    params: {
                        notification_request_id: crypto.randomUUID(),
                        message: request.content,
                    },
                };
            case "compact": {
                const method = lifecycleMethod("compact");
                if (!method) throw new Error("Worker state snapshot is not available");
                return method;
            }
            case "list_rewind_targets":
                return { method: "list_rewind_targets" };
            case "register_peer":
                return {
                    method: "register_peer",
                    params: { name: request.content },
                };
        }
    }

    function cachedComposerDraft(snapshot: ComposerDraftSnapshot): ComposerDraftCache {
        return {
            segments: [...snapshot.segments],
            preserveExactText: snapshot.textPastes.length > 0,
        };
    }

    function handleComposerChange(snapshot: ComposerDraftSnapshot) {
        draft = snapshot;
        composerDrafts.set(activeComposerTargetKey, cachedComposerDraft(snapshot));
    }

    function discardAllAttachments(): void {
        const discarded = attachments;
        attachments = [];
        for (const attachment of discarded) {
            attachment.request?.abort();
            if (attachment.reference) {
                void fetch(
                    `${attachment.uploadPath}/attachments/${encodeURIComponent(attachment.reference.artifact_id)}`,
                    { method: "DELETE" },
                ).catch(() => undefined);
            }
        }
    }

    function switchComposerTarget(target: ConsoleTarget) {
        const nextKey = `${target.workspaceId}:${target.runtimeId}:${target.workerId}`;
        if (nextKey === activeComposerTargetKey) return;
        if (activeComposerTargetKey) discardAllAttachments();
        if (composerInputElement) {
            composerDrafts.set(
                activeComposerTargetKey,
                cachedComposerDraft(composerInputElement.snapshot()),
            );
        }
        activeComposerTargetKey = nextKey;
        const restored = composerDrafts.get(nextKey) ?? {
            segments: [],
            preserveExactText: false,
        };
        void tick().then(() => {
            if (activeComposerTargetKey !== nextKey) return;
            composerInputElement?.restoreSegments(
                restored.segments,
                restored.preserveExactText,
            );
        });
    }

    function handleComposerCommand() {
        void submitDraft(composerInputElement?.snapshot() ?? draft);
    }

    function handleComposerSubmit() {
        if ((composerInputElement?.snapshot() ?? draft).document.trimStart().startsWith(":")) {
            handleComposerCommand();
            return;
        }
        if (workerRunning) {
            sendWorkerControl("cancel");
            return;
        }
        void submitDraft(composerInputElement?.snapshot() ?? draft);
    }

    function handleQueueSubmit() {
        void submitDraft(composerInputElement?.snapshot() ?? draft, "queue");
    }

    function handleNotifySubmit() {
        void submitDraft(composerInputElement?.snapshot() ?? draft, "notify");
    }

    function attachmentPath(): string {
        const target = consoleTarget;
        if (!target) {
            throw new Error("Worker execution target is unavailable.");
        }
        return `/api/w/${encodeURIComponent(target.workspaceId)}/runtimes/${encodeURIComponent(target.runtimeId)}/workers/${encodeURIComponent(target.workerId)}`;
    }

    function updateAttachment(id: number, update: Partial<ComposerAttachment>): void {
        attachments = attachments.map((attachment) =>
            attachment.id === id ? { ...attachment, ...update } : attachment,
        );
    }

    function startAttachmentUpload(attachment: ComposerAttachment): void {
        const error = validateAttachmentFile(attachment.file);
        if (error) {
            updateAttachment(attachment.id, { state: "failed", error, request: null });
            return;
        }
        updateAttachment(attachment.id, {
            state: "uploading",
            progress: 0,
            reference: null,
            error: null,
            request: null,
        });
        const request = uploadAttachment(
            attachment.uploadPath,
            attachment.file,
            attachment.uploadId,
            {
            progress: (progress) => updateAttachment(attachment.id, { progress }),
            complete: (reference) =>
                updateAttachment(attachment.id, {
                    state: "uploaded",
                    progress: 1,
                    reference,
                    error: null,
                    request: null,
                }),
            failed: (message) =>
                updateAttachment(attachment.id, {
                    state: "failed",
                    error: message,
                    request: null,
                }),
        });
        updateAttachment(attachment.id, { request });
    }

    function addPastedImages(files: File[]): void {
        addAttachmentFiles(files.map(namePastedImage));
    }

    function addAttachmentFiles(files: Iterable<File>): void {
        if (!composerEditable) return;
        const available = Math.max(0, MAX_FILES_PER_SUBMISSION - attachments.length);
        for (const file of Array.from(files).slice(0, available)) {
            const attachment: ComposerAttachment = {
                id: nextAttachmentId++,
                file,
                uploadPath: attachmentPath(),
                uploadId: crypto.randomUUID(),
                state: "uploading",
                progress: 0,
                reference: null,
                error: null,
                request: null,
            };
            attachments = [...attachments, attachment];
            startAttachmentUpload(attachment);
        }
    }

    async function removeAttachment(attachment: ComposerAttachment): Promise<void> {
        attachment.request?.abort();
        attachments = attachments.filter((candidate) => candidate.id !== attachment.id);
        if (attachment.reference) {
            await fetch(`${attachment.uploadPath}/attachments/${encodeURIComponent(attachment.reference.artifact_id)}`, {
                method: "DELETE",
            }).catch(() => undefined);
        }
    }

    function retryAttachment(attachment: ComposerAttachment): void {
        startAttachmentUpload(attachment);
    }

    function handleFileInput(event: Event): void {
        const input = event.currentTarget as HTMLInputElement;
        if (input.files) addAttachmentFiles(input.files);
        input.value = "";
    }

    function handleFileDragOver(event: DragEvent): void {
        if (!event.dataTransfer?.types.includes("Files")) return;
        event.preventDefault();
        isDraggingFiles = true;
    }

    function handleFileDrop(event: DragEvent): void {
        if (!event.dataTransfer?.types.includes("Files")) return;
        event.preventDefault();
        isDraggingFiles = false;
        if (event.dataTransfer?.files) addAttachmentFiles(event.dataTransfer.files);
    }

    function reportComposerError(message: string) {
        pushWorkspaceAlert("error", message, {
            id: controlAlertId,
            title: "Console input",
        });
    }

    async function submitDraft(
        value: ComposerDraftSnapshot,
        delivery: ComposerDelivery = "submit",
    ) {
        if (delivery === "notify" && attachments.length > 0) {
            reportComposerError("Notify accepts text only; remove attachments or queue a Submit.");
            return;
        }
        const incompleteAttachment = attachments.find((attachment) =>
            attachment.state !== "uploaded" || !attachment.reference
        );
        if (incompleteAttachment) {
            reportComposerError(incompleteAttachment.state === "uploading"
                ? "Wait for file uploads to finish before sending."
                : incompleteAttachment.error ?? "Retry or remove the failed attachment.");
            return;
        }
        const attachmentSegments: Segment[] = attachments.map((attachment) => ({
            kind: "uploaded_file",
            file: attachment.reference!,
        }));
        const command = buildComposerSegmentsRequest(
            [...value.segments, ...attachmentSegments],
            {
            preserveExactText: value.textPastes.length > 0,
        });
        if (!command.ok) {
            reportComposerError(command.message);
            return;
        }
        if (!command.request) {
            if (command.notice) pushWorkspaceAlert("info", command.notice, { title: "Console command" });
            composerInputElement?.clear();
            return;
        }
        const deliveryState = {
            delivery,
            workerState,
            protocolOpen: protocolState === "open",
            sending,
            hasText: value.content.trim().length > 0,
            hasAttachments: attachments.length > 0,
        };
        // Commands are controls, not idle-only chat submissions. Keep the
        // removed header controls available while the Worker is busy too.
        const isCommand = delivery === "submit" && command.request.kind !== "user";
        if (isCommand ? !composerEditable : !canDeliverComposerDraft(deliveryState)) {
            return;
        }

        let request: WorkerConsoleInputRequest = command.request;
        if (delivery === "queue" && request.kind !== "user") {
            reportComposerError("Queue accepts ordinary input, not a Composer command.");
            return;
        }
        if (delivery === "notify") {
            if (request.kind !== "user") {
                reportComposerError("Notify accepts ordinary text, not a Composer command.");
                return;
            }
            request = { kind: "notify", content: request.content };
        }
        sending = true;
        try {
            const method = composerRequestToProtocolMethod(request);
            if (isCommand) {
                sendProtocolMethod(method);
            } else if (!sendComposerDelivery(deliveryState, method, sendProtocolMethod)) {
                return;
            }
            composerInputElement?.recordHistory(value);
            composerInputElement?.clear();
            attachments = [];
            if (method.method === "submit" && delivery === "submit") {
                liveWorkerState = "running";
            }
        } catch (error) {
            reportComposerError(error instanceof Error ? error.message : String(error));
        } finally {
            sending = false;
        }
    }

    function sendMessage(event: SubmitEvent) {
        event.preventDefault();
        handleComposerSubmit();
    }

    function workerStateFromSnapshot(
        snapshot: ConsoleProjection["workerState"],
    ): string | null {
        if (!snapshot) return null;
        return snapshot.state.kind === "idle"
            ? "idle"
            : snapshot.state.state.kind === "run" &&
                  snapshot.state.state.state === "paused"
              ? "paused"
              : "running";
    }

    function connectProtocolTransport(
        targetWorker: Worker | null,
        token: number,
        target: ConsoleTarget,
    ) {
        protocolState = "connecting";
        const controller = new AbortController();
        let closeLiveSubscription: (() => void) | undefined;
        const requestIdentity: WorkerSessionRequestIdentity = {
            token,
            workspaceId: target.workspaceId,
            runtimeId: target.runtimeId,
            workerId: target.workerId,
        };
        void fetch(
            workerApiPath(
                `/runtimes/${encodeURIComponent(target.runtimeId)}/workers/${encodeURIComponent(target.workerId)}/session`,
            ),
            workerSessionRequestInit(controller.signal),
        )
            .then(async (response) => {
                if (!response.ok) {
                    throw new Error(`failed to observe Worker Session (${response.status})`);
                }
                return (await response.json()) as WorkerSessionObservation;
            })
            .then((observation) => {
                const currentTarget = consoleTarget;
                if (!currentTarget) return;
                const currentIdentity: WorkerSessionRequestIdentity = {
                    token: reloadToken,
                    ...currentTarget,
                };
                if (
                    !isCurrentWorkerSessionRequest(requestIdentity, currentIdentity) ||
                    controller.signal.aborted
                )
                    return;
                const action = workerSessionAction(observation);
                if (action.kind === "show_unavailable") {
                    protocolState = "closed";
                    showConsoleUnavailable(action.message);
                    streamDiagnostics = [
                        ...streamDiagnostics,
                        {
                            code: "retained_session_unavailable",
                            severity: "warning",
                            message: action.message,
                        },
                    ];
                    return;
                }
                if (
                    action.kind === "subscribe_live" &&
                    targetWorker &&
                    targetWorker.state === "stopped"
                ) {
                    protocolState = "closed";
                    const message =
                        "The Session is live but the Worker is stopped. Reload to refresh its state.";
                    showConsoleFailure(message);
                    streamDiagnostics = [
                        ...streamDiagnostics,
                        {
                            code: "worker_session_state_changed",
                            severity: "warning",
                            message,
                        },
                    ];
                    return;
                }
                if (action.kind === "subscribe_live") {
                    waitForInitialSnapshot();
                    closeLiveSubscription = connectLiveProtocolTransport(token, target);
                    return;
                }
                handleIncomingProtocolEvent({
                    event: "snapshot",
                    data: {
                        session: action.snapshot,
                        greeting: {
                            worker_name: "retained worker",
                            cwd: "",
                            provider: "retained session",
                            model: "",
                            scope_summary: "read-only retained session",
                            tools: [],
                            context_window: 0,
                            context_tokens: 0,
                        },
                        state: { last_command_id: 0, state: { kind: "idle" } },
                        in_flight: { responses: [], commands: [] },
                        internal_workers: [],
                    },
                } as ProtocolEvent, { token, source: "retained" });
                protocolState = "closed";
                streamDiagnostics = [
                    ...streamDiagnostics,
                    {
                        code: "retained_session_read_only",
                        severity: "info",
                        message: "Showing a read-only retained Session snapshot.",
                    },
                ];
            })
            .catch((error) => {
                if (token !== reloadToken || controller.signal.aborted) return;
                protocolState = "error";
                const message = error instanceof Error ? error.message : String(error);
                showConsoleFailure(message);
                streamDiagnostics = [
                    ...streamDiagnostics,
                    {
                        code: "worker_session_observation_failed",
                        severity: "warning",
                        message,
                    },
                ];
            });
        return () => {
            controller.abort();
            closeLiveSubscription?.();
        };
    }

    function connectLiveProtocolTransport(token: number, target: ConsoleTarget) {
        protocolState = "connecting";
        const subscription = workspaceMultiplexer(target.workspaceId).subscribe(
            {
                topic: "worker_protocol",
                worker_id: target.workerId,
                runtime_id: target.runtimeId,
            },
            {
                onFrame: (frame) => {
                    if (token !== reloadToken) return;
                    try {
                        if (
                            frame.frame === "response" &&
                            frame.message.result === "subscribed" &&
                            frame.message.payload.snapshot.topic === "worker_protocol"
                        ) {
                            let initialSnapshotQueued = false;
                            for (const event of frame.message.payload.snapshot.data.events) {
                                if (!initialSnapshotQueued && event.event === "snapshot") {
                                    handleIncomingProtocolEvent(event, {
                                        token,
                                        source: "live",
                                    });
                                    initialSnapshotQueued = true;
                                } else {
                                    handleIncomingProtocolEvent(event);
                                }
                            }
                            if (!initialSnapshotQueued) {
                                throw new Error(
                                    "The live subscription did not provide an initial snapshot.",
                                );
                            }
                            fileCompletions.reset();
                            protocolState = "open";
                        } else if (
                            frame.frame === "event" &&
                            frame.message.event === "event" &&
                            frame.message.data.payload.event === "worker_protocol"
                        ) {
                            handleIncomingProtocolEvent(frame.message.data.payload.data.event);
                        } else if (
                            frame.frame === "event" &&
                            frame.message.event === "subscription_closed"
                        ) {
                            protocolState = "closed";
                            showConsoleFailure(frame.message.data.message);
                            rejectPendingCompletion(new Error(frame.message.data.message));
                        } else if (
                            frame.frame === "response" &&
                            frame.message.result === "subscription_rejected"
                        ) {
                            protocolState = "error";
                            throw new Error(frame.message.payload.message);
                        }
                    } catch (error) {
                        protocolState = "error";
                        const message = error instanceof Error
                            ? error.message
                            : String(error);
                        showConsoleFailure(message);
                        streamDiagnostics = [
                            ...streamDiagnostics,
                            {
                                code: "worker_protocol_frame_invalid",
                                severity: "warning",
                                message,
                            },
                        ];
                    }
                },
                onStatus: (status, failure) => {
                    if (token !== reloadToken) return;
                    protocolState = status === "open" ? "connecting" : status;
                    if (status === "connecting" || status === "open") {
                        showConsoleReconnecting(failure?.message);
                    }
                    if (status === "closed") {
                        showConsoleFailure(failure?.message);
                        rejectPendingCompletion(new Error("Worker protocol WebSocket closed."));
                    }
                },
            },
        );
        protocolSubscription = subscription;
        return () => {
            if (protocolSubscription === subscription) {
                protocolSubscription = null;
                fileCompletions.close();
            }
            subscription.close();
        };
    }

    function sendProtocolMethod(method: ProtocolMethod) {
        if (!protocolSubscription || protocolState !== "open") {
            throw new Error("Worker protocol WebSocket is not open.");
        }
        protocolSubscription.sendWorkerMethod(method);
    }

    function handleProtocolCommandEvent(event: ProtocolEvent) {
        if (event.event === "completions") {
            if (event.data.kind === "file") fileCompletions.receive(event.data.entries);
            return;
        }
        if (event.event === "rewind_targets") {
            rewindHeadEntries = event.data.head_entries;
            rewindTargets = event.data.targets;
            pushWorkspaceAlert(
                "info",
                event.data.targets.length === 0
                    ? "No rewind targets are available."
                    : `Loaded ${event.data.targets.length} rewind target(s).`,
                { id: controlAlertId, title: "Rewind targets" },
            );
            return;
        }
        if (event.event === "error") {
            const error = new Error(event.data.message);
            if (fileCompletions.pending) {
                rejectPendingCompletion(error);
            }
            streamDiagnostics = [
                ...streamDiagnostics,
                {
                    code: event.data.code,
                    severity: "error",
                    message: event.data.message,
                },
            ];
        }
    }

    function rejectPendingCompletion(error: Error) {
        fileCompletions.close(error);
    }

    function mergeDiagnostics(...groups: Diagnostic[][]): Diagnostic[] {
        return groups.flat();
    }

    function diagnosticsToText(items: Diagnostic[]): string {
        return items
            .map((item) => `${item.severity}: ${item.message}`)
            .join("\n");
    }

    function consoleWorkerViewKey(sessionId: string | null): string {
        return sessionId === null ? "main" : `internal:${sessionId}`;
    }

    function consoleWorkerViewSelectionIsResolved(): boolean {
        return selectedWorkerViewSessionId === selectedWorkerView.sessionId;
    }

    function rememberConsoleWorkerViewScroll() {
        if (!consoleBodyElement || !consoleWorkerViewSelectionIsResolved()) return;
        consoleViewScroll.set(consoleWorkerViewKey(selectedWorkerView.sessionId), {
            top: consoleBodyElement.scrollTop,
            autoFollow: autoFollowConsole,
        });
    }

    async function selectConsoleWorkerView(
        sessionId: string | null,
        rememberCurrent = true,
    ) {
        if (sessionId === selectedWorkerViewSessionId) return;
        const generation = ++workerViewSelectionGeneration;
        if (rememberCurrent) rememberConsoleWorkerViewScroll();
        const target = workerViews.find((view) => view.sessionId === sessionId) ??
            workerViews[0];
        const targetScroll = consoleViewScroll.get(
            consoleWorkerViewKey(target.sessionId),
        );
        autoFollowConsole = targetScroll?.autoFollow ?? true;
        selectedWorkerViewSessionId = target.sessionId;
        await tick();
        if (generation !== workerViewSelectionGeneration) return;
        if (!consoleBodyElement) return;
        consoleBodyElement.scrollTop = resolveConsoleViewScrollTop(
            targetScroll,
            consoleBodyElement.scrollHeight,
            consoleBodyElement.clientHeight,
        );
    }

    function isNearConsoleBottom(element: HTMLElement): boolean {
        return (
            element.scrollHeight - element.scrollTop - element.clientHeight <=
            CONSOLE_BOTTOM_THRESHOLD_PX
        );
    }

    function selectConsoleTurn(id: string) {
        if (!consoleBodyElement || !consoleWorkerViewSelectionIsResolved()) return;
        const target = consoleBodyElement.querySelector<HTMLElement>(
            `[data-console-line-id="${CSS.escape(id)}"]`,
        );
        if (!target) return;
        // Stop following before moving so streaming output cannot undo the jump.
        autoFollowConsole = false;
        consoleBodyElement.scrollTop +=
            target.getBoundingClientRect().top -
            consoleBodyElement.getBoundingClientRect().top;
        handleConsoleScroll();
    }

    function handleConsoleScroll() {
        if (!consoleWorkerViewSelectionIsResolved() || !consoleBodyElement) {
            return;
        }
        autoFollowConsole = isNearConsoleBottom(consoleBodyElement);
        rememberConsoleWorkerViewScroll();
        handleHistoryTopEdge(consoleBodyElement.scrollTop <= 1);
    }

    function jumpToLatest() {
        if (!consoleWorkerViewSelectionIsResolved()) return;
        autoFollowConsole = true;
        void scrollConsoleToBottom();
    }

    async function scrollConsoleToBottom() {
        if (!consoleWorkerViewSelectionIsResolved()) return;
        const sessionId = selectedWorkerView.sessionId;
        await tick();
        if (
            !consoleBodyElement ||
            !autoFollowConsole ||
            !consoleWorkerViewSelectionIsResolved() ||
            selectedWorkerView.sessionId !== sessionId
        ) {
            return;
        }
        consoleBodyElement.scrollTop = consoleBodyElement.scrollHeight;
        autoFollowConsole = true;
        rememberConsoleWorkerViewScroll();
    }

    const scrollFollowKey = $derived(
        lines
            .map(
                (line) =>
                    `${line.source}:${line.kind}:${line.body.length}:${line.streaming ? "streaming" : "done"}`,
            )
            .join("|"),
    );

    $effect(() => {
        scrollFollowKey;
        if (!consoleWorkerViewSelectionIsResolved()) return;
        if (autoFollowConsole) {
            void scrollConsoleToBottom();
        }
    });

    $effect(() => {
        const activeViewKeys = new Set(
            workerViews.map((view) => consoleWorkerViewKey(view.sessionId)),
        );
        for (const key of consoleViewScroll.keys()) {
            if (!activeViewKeys.has(key)) consoleViewScroll.delete(key);
        }
        const resolvedSessionId = selectedWorkerView.sessionId;
        if (resolvedSessionId !== selectedWorkerViewSessionId) {
            void selectConsoleWorkerView(resolvedSessionId, false);
        }
    });

    $effect(() => {
        const target = consoleTarget;
        const targetWorker = data.worker;
        const targetWorkerError = data.workerError;
        workerViewSelectionGeneration += 1;
        selectedWorkerViewSessionId = null;
        consoleViewScroll.clear();
        autoFollowConsole = true;
        resetObservedEvents();
        taskPaneOpen = false;
        worker = targetWorker;
        workerError = targetWorkerError;
        liveWorkerState = targetWorker?.state ?? null;
        streamDiagnostics = [];
        consoleDisplayState = { kind: "loading", stage: "session" };
        const token = advanceReloadToken();
        if (!target) {
            untrack(discardAllAttachments);
            protocolState = "error";
            consoleDisplayState = {
                kind: "unavailable",
                reason: boundedConsoleReason(
                    targetWorkerError,
                    "Worker execution target is unavailable.",
                ),
            };
            return;
        }
        switchComposerTarget(target);
        protocolState = "connecting";
        if (!targetWorker) void loadWorker(target, token);
    });

    $effect(() => {
        return () => discardAllAttachments();
    });

    $effect(() => {
        const target = consoleTarget;
        const targetWorker = worker;
        const token = reloadToken;
        if (!target) return;
        if (
            targetWorker &&
            (targetWorker.runtime_id !== target.runtimeId ||
                targetWorker.worker_id !== target.workerId)
        ) {
            return;
        }
        return connectProtocolTransport(targetWorker, token, target);
    });
</script>

<svelte:window onkeydown={handleWorkerControlShortcut} />

<svelte:head>
    <title>Worker Console · Yoi Workspace</title>
    <meta
        name="description"
        content="Worker attach console through Workspace Backend APIs"
    />
</svelte:head>

<div class="console-shell worker-console-shell">
    <section class="console-header card" aria-label="Worker controls">
        <div class="console-header-actions">
            <div
                class="console-view-modes"
                role="group"
                aria-label="Console display mode"
            >
                <button
                    type="button"
                    class:active={consoleViewMode === "overview"}
                    aria-pressed={consoleViewMode === "overview"}
                    onclick={() => (consoleViewMode = "overview")}
                >
                    Overview
                </button>
                <button
                    type="button"
                    class:active={consoleViewMode === "normal"}
                    aria-pressed={consoleViewMode === "normal"}
                    onclick={() => (consoleViewMode = "normal")}
                >
                    Normal
                </button>
            </div>
        </div>
    </section>

    {#if rewindTargets.length > 0}
        <section class="card rewind-targets" aria-label="Rewind targets">
            <h3>Rewind targets</h3>
            <div class="rewind-target-list">
                {#each rewindTargets as target (JSON.stringify(target.id))}
                    <button
                        type="button"
                        class="secondary-button"
                        disabled={protocolState !== "open" || !target.eligible}
                        title={target.disabled_reason ?? target.warning ?? undefined}
                        onclick={() => rewindTo(target)}
                    >
                        {target.preview || `${target.turn_index}`}
                        {#if target.warning || target.disabled_reason}
                            <span>{target.warning ?? target.disabled_reason}</span>
                        {/if}
                    </button>
                {/each}
            </div>
        </section>
    {/if}

    <div class:with-task-pane={taskPaneOpen} class="console-history">
        <section class="console-body">
        <div
            class="console-scroll"
            bind:this={consoleBodyElement}
            onscroll={handleConsoleScroll}
        >
            <article
                class="card console-card worker-console-card"
                aria-label={`${selectedWorkerView.label} transcript`}
            >
                {#if workerError}
                    <p class="error">{workerError}</p>
                {/if}

                {#if selectedWorkerViewSessionId === null && (selectedHistory.hasMore || selectedHistory.error || selectedHistory.status === "loading")}
                    <div class="conversation-history-boundary">
                        <button
                            type="button"
                            disabled={selectedHistory.status === "loading"}
                            onclick={selectedHistory.error ? retryHistoryLoad : loadEarlierHistory}
                        >
                            {selectedHistory.status === "loading"
                                ? "Loading earlier conversation…"
                                : selectedHistory.error
                                  ? "Earlier conversation unavailable · retry"
                                  : "Earlier conversation available · load 5 turns"}
                        </button>
                    </div>
                {:else if selectedWorkerViewSessionId === null && selectedHistory.status === "ready"}
                    <div class="conversation-history-boundary complete">Start of conversation</div>
                {/if}

                <ConsoleDisplayStateView
                    state={consoleDisplayState}
                    hasContent={lines.length > 0}
                    hasUnfilteredContent={selectedConsoleProjection.lines.length > 0}
                    onRetry={retryConsoleLoad}
                />

                {#if (consoleDisplayState.kind === "ready" || consoleDisplayState.kind === "stale") && lines.length > 0}
                    <ol class="console-log">
                        {#each lines as item (item.id)}
                            <ConsoleLineItem {item} {attachmentUrl} />
                        {/each}
                    </ol>
                {/if}
            </article>
        </div>

            <ConsoleTurnNavigation
                items={turnNavigationItems}
                hasMore={selectedWorkerViewSessionId === null && selectedHistory.hasMore}
                loading={selectedHistory.status === "loading"}
                error={selectedWorkerViewSessionId === null
                    ? selectedHistory.error
                    : "Earlier history is unavailable for direct SubWorker views."}
                onLoadMore={selectedHistory.error ? retryHistoryLoad : loadEarlierHistory}
                onTopEdgeChange={handleHistoryTopEdge}
                onTurnClick={jumpToConversationTurn}
                historyAvailable={selectedWorkerViewSessionId === null}
                bind:element={turnNavigationElement}
            />
            {#if !autoFollowConsole && lines.length > 0 && (consoleDisplayState.kind === "ready" || consoleDisplayState.kind === "stale")}
                <button
                    class="console-jump-latest"
                    transition:fly={{ y: 6, duration: prefersReducedMotion.current ? 0 : 160 }}
                    type="button"
                    aria-label="Jump to latest"
                    title="Jump to latest"
                    onclick={jumpToLatest}
                >
                    <svg viewBox="0 0 24 24" aria-hidden="true">
                        <path d="M12 4V16M7 11L12 16L17 11M5 20H19" />
                    </svg>
                </button>
            {/if}
        </section>

        {#if taskPaneOpen}
            <ConsoleTasks {tasks} mode="pane" paneId="console-task-pane" />
        {/if}
    </div>

    {#if workerDetailsOpen}
        <aside class="console-side-panel" aria-label="Worker detail">
            <header class="side-panel-header">
                <h3>Worker detail</h3>
                <button
                    type="button"
                    class="secondary-button"
                    onclick={() => (workerDetailsOpen = false)}>Close</button
                >
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
                        <dt>Profile</dt>
                        <dd>{worker.profile ?? "unknown"}</dd>
                    </div>
                    <div>
                        <dt>Workspace</dt>
                        <dd>
                            {worker.workspace.visibility} · {worker.workspace
                                .identity}
                        </dd>
                    </div>
                    <div>
                        <dt>Implementation</dt>
                        <dd>
                            {worker.implementation.kind} · {worker
                                .implementation.display_hint}
                        </dd>
                    </div>
                </dl>
            {:else if !workerError}
                <p>Loading Worker detail…</p>
            {/if}

            {#if diagnostics.length > 0}
                <details
                    class="metadata-details"
                    open={protocolState === "error"}
                >
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

    {#if workerRunning}
        <WorkerRunStatus
            startedAtMs={consoleProjection.runActivity.startedAtMs}
            requests={consoleProjection.runActivity.requests}
            uploadTokens={consoleProjection.runActivity.uploadTokens}
            outputTokens={consoleProjection.runActivity.outputTokens}
            compaction={consoleProjection.compaction}
            workerState={consoleProjection.workerState}
        />
    {/if}

    <ConsoleTasks
        {tasks}
        mode="mini"
        paneId="console-task-pane"
        paneOpen={taskPaneOpen}
        onTogglePane={() => {
            taskPaneOpen = !taskPaneOpen;
            if (taskPaneOpen) workerDetailsOpen = false;
        }}
        workerViews={workerViews.map(({ sessionId, label }) => ({
            sessionId,
            label,
        }))}
        selectedWorkerViewSessionId={selectedWorkerView.sessionId}
        onSelectWorkerView={(sessionId) => {
            void selectConsoleWorkerView(sessionId);
        }}
    />

    {#if pendingSubmissionItems.length > 0 || pendingSubmissions.notification_count > 0}
        <section class="pending-submissions" class:has-both={pendingSubmissionItems.length > 0 && pendingSubmissions.notification_count > 0} aria-label="Pending activations">
            {#if pendingSubmissionItems.length > 0}
                <div class="pending-column pending-queue" role="group" aria-label="Queued inputs">
                    <h2 class="pending-submissions-label">{pendingSubmissionItems.length} Queued</h2>
                    <ol aria-label="Queued inputs">
                        {#each pendingSubmissionItems as submission, index (submission.submission_id)}
                            <li>
                                <span class="pending-submission-preview" title={submission.preview || "Preview unavailable"}>
                                    {submission.preview || "Preview unavailable"}
                                </span>
                                <button
                                    class="pending-icon-button"
                                    type="button"
                                    aria-label={`Cancel queued input ${index + 1}`}
                                    title="Cancel queued input"
                                    disabled={protocolState !== "open"}
                                    onclick={() => sendControl({
                                        method: "cancel_pending_submission",
                                        params: {
                                            submission_id: submission.submission_id,
                                            expected_revision: pendingSubmissions.revision,
                                        },
                                    }, "Pending submission cancellation")}
                                >
                                    <svg viewBox="0 0 24 24" aria-hidden="true">
                                        <path d="M5 12h14" />
                                    </svg>
                                </button>
                            </li>
                        {/each}
                    </ol>
                </div>
            {/if}
            {#if pendingSubmissions.notification_count > 0}
                <div class="pending-column pending-notifications" role="group" aria-label="Notifications">
                    <h2 class="pending-submissions-label">Notifications</h2>
                    <ol aria-label="Pending notifications">
                        {#each pendingSubmissions.notification_previews ?? [] as preview}
                            <li>
                                <span class="pending-submission-preview" title={preview}>{preview}</span>
                            </li>
                        {:else}
                            <li>
                                <span class="pending-submission-preview" title="Notification previews are unavailable on this Runtime">
                                    {pendingSubmissions.notification_count} pending · Preview unavailable
                                </span>
                            </li>
                        {/each}
                    </ol>
                </div>
            {/if}
        </section>
    {/if}

    <form class="console-composer" onsubmit={sendMessage}>
        <div
            class="composer-input-shell"
            role="group"
            aria-label="Message composer and file drop area"
            class:dragging-files={isDraggingFiles}
            ondragover={handleFileDragOver}
            ondragleave={() => (isDraggingFiles = false)}
            ondrop={handleFileDrop}
        >
            <input
                class="attachment-file-input"
                bind:this={fileInput}
                type="file"
                multiple
                accept="text/*,application/json,application/pdf,image/png,image/jpeg,image/gif,image/webp"
                onchange={handleFileInput}
            />
            <ComposerInput
                bind:this={composerInputElement}
                historyScope={workspaceId}
                ariaLabel="Console input"
                ariaKeyShortcuts="Meta+Enter Control+Enter"
                disabled={!composerEditable}
                onchange={handleComposerChange}
                onkeydown={handleComposerKeydown}
                completionScope={activeComposerTargetKey}
                resolveFileCompletions={(prefix, signal) => fileCompletions.request(prefix, signal)}
                oncommand={handleComposerCommand}
                onsubmit={handleComposerSubmit}
                onpasteimages={addPastedImages}
            />
            {#if attachments.length > 0}
                <div class="composer-attachments" aria-live="polite">
                    {#each attachments as attachment (attachment.id)}
                        <div class:failed={attachment.state === "failed"} class="composer-attachment">
                            <span class="attachment-name" title={attachment.file.name}>{attachment.file.name}</span>
                            {#if attachment.state === "uploading"}
                                <progress max="1" value={attachment.progress} aria-label={`Uploading ${attachment.file.name}`}></progress>
                                <span>{Math.round(attachment.progress * 100)}%</span>
                            {:else if attachment.state === "failed"}
                                <span class="error">{attachment.error}</span>
                                <button type="button" onclick={() => retryAttachment(attachment)}>Retry</button>
                            {:else}
                                <span>Ready</span>
                            {/if}
                            <button
                                type="button"
                                aria-label={`Remove ${attachment.file.name}`}
                                onclick={() => void removeAttachment(attachment)}
                            >×</button>
                        </div>
                    {/each}
                </div>
            {/if}
            <div class="composer-input-footer">
                <div class="composer-footer-slot">
                    <button
                        class="composer-attach-button"
                        type="button"
                        aria-label="Attach file"
                        title="Attach file"
                        disabled={!composerEditable || attachments.length >= MAX_FILES_PER_SUBMISSION}
                        onclick={() => fileInput?.click()}
                    >
                        <svg
                            class="composer-attach-icon"
                            aria-hidden="true"
                            viewBox="0 0 24 24"
                        >
                            <path d="M12 5V19M5 12H19" />
                        </svg>
                    </button>
                </div>
                <div class="composer-submit-actions">
                    {#if workerRunning || workerPaused}
                        <button
                            class="composer-queue-button"
                            type="button"
                            aria-label="Queue Submit"
                            title="Queue Submit"
                            disabled={!canQueueDraft}
                            onclick={handleQueueSubmit}
                        >
                            <svg
                                class="composer-queue-icon"
                                aria-hidden="true"
                                viewBox="0 0 24 24"
                            >
                                <path d="M4 6H20M4 12H14M4 18H10M18 14V22M14 18H22" />
                            </svg>
                        </button>
                    {/if}
                    {#if workerRunning}
                        <button
                            class="composer-notify-button"
                            type="button"
                            aria-label="Notify Worker"
                            title="Notify Worker"
                            disabled={!canNotifyDraft}
                            onclick={handleNotifySubmit}
                        >
                            <svg
                                class="composer-notify-icon"
                                aria-hidden="true"
                                viewBox="0 0 24 24"
                            >
                                <path d="M18 8A6 6 0 0 0 6 8C6 15 3 15 3 17H21C21 15 18 15 18 8Z" />
                                <path d="M10 21H14" />
                            </svg>
                        </button>
                    {/if}
                    <button
                        class="composer-send-button"
                        class:stop={workerRunning}
                        type="submit"
                        aria-label={workerRunning
                            ? "Stop Worker"
                            : sending
                              ? "Sending message"
                              : "Send message"}
                        disabled={composerSubmitDisabled}
                    >
                        {#if workerRunning}
                            <svg
                                class="composer-send-icon"
                                aria-hidden="true"
                                viewBox="0 0 24 24"
                            >
                                <path d="M7 7H17V17H7Z" />
                            </svg>
                        {:else}
                            <svg
                                class="composer-send-icon"
                                aria-hidden="true"
                                viewBox="0 0 24 24"
                            >
                                <path d="M8 6L12 2L16 6" />
                                <path d="M12 2V22" />
                            </svg>
                        {/if}
                    </button>
                </div>
            </div>
        </div>
    </form>
    <WorkerContextStatus
        metadata={selectedConsoleProjection.workerMetadata}
        detailsOpen={workerDetailsOpen}
        onToggleDetails={() => {
            workerDetailsOpen = !workerDetailsOpen;
            if (workerDetailsOpen) taskPaneOpen = false;
        }}
    />
</div>

<style>
    .console-shell {
        width: 100%;
        max-width: 920px;
        margin-inline: auto;
    }

    .worker-console-shell {
        display: flex;
        flex-direction: column;
        gap: var(--space-2);
        min-height: 0;
        height: calc(100dvh - (var(--space-4) * 2));
        overflow: hidden;
    }

    .worker-console-shell > .console-history {
        flex: 1 1 auto;
        min-height: 0;
        overflow: hidden;
        overscroll-behavior: contain;
    }

    .console-header {
        display: flex;
        min-width: 0;
        align-items: center;
        justify-content: flex-end;
        gap: var(--space-4);
    }

    .console-card {
        display: grid;
        align-content: start;
        gap: var(--space-4);
        min-height: 100%;
    }

    .console-header-actions {
        display: flex;
        flex: 0 0 auto;
        align-items: center;
        justify-content: flex-end;
        flex-wrap: wrap;
        gap: var(--space-2);
    }

    .console-view-modes {
        display: inline-flex;
        overflow: hidden;
        border: 1px solid var(--line);
        border-radius: 0.55rem;
        background: var(--bg-raised);
    }

    .console-view-modes button {
        border: 0;
        background: transparent;
        color: var(--text-muted);
        padding: var(--space-1) var(--space-2);
        font: inherit;
        font-size: var(--font-size-compact);
        line-height: var(--line-height-compact);
        font-weight: 700;
        cursor: pointer;
    }

    .console-view-modes button + button {
        border-left: 1px solid var(--line);
    }

    .console-view-modes button:hover {
        color: var(--text-strong);
    }

    .console-view-modes button.active {
        background: var(--accent);
        color: var(--bg);
    }

    .rewind-targets {
        display: flex;
        align-items: center;
        gap: var(--space-3);
        padding: var(--space-3);
    }

    .rewind-targets h3 {
        margin: 0;
        font-size: var(--font-size-body);
    }

    .rewind-target-list {
        display: flex;
        flex-wrap: wrap;
        gap: var(--space-2);
    }

    .rewind-target-list span {
        margin-left: var(--space-2);
        color: var(--text-muted);
    }

    .console-history {
        display: grid;
        min-height: 0;
        flex: 1;
        grid-template-columns: minmax(0, 1fr);
    }

    .console-history.with-task-pane {
        grid-template-columns: minmax(0, 2fr) minmax(18rem, 1fr);
    }

    .console-body {
        display: grid;
        grid-template-columns: minmax(0, 1fr) max-content;
        grid-template-rows: minmax(0, 1fr);
        min-width: 0;
        min-height: 0;
    }

    .console-scroll {
        grid-area: 1 / 1;
        height: 100%;
        min-width: 0;
        min-height: 0;
        overflow-y: auto;
        scrollbar-width: none;
    }

    .console-jump-latest {
        grid-area: 1 / 1;
        align-self: end;
        justify-self: center;
        z-index: 2;
        display: grid;
        place-items: center;
        width: 36px;
        height: 36px;
        padding: 0;
        margin-bottom: var(--space-3);
        border: 1px solid var(--line);
        border-radius: 50%;
        background: var(--bg-raised);
        color: var(--text-strong);
        box-shadow: var(--shadow-overlay);
        cursor: pointer;
    }

    .console-jump-latest:hover {
        color: var(--tui-cyan);
        border-color: var(--tui-cyan);
    }

    @media (prefers-reduced-motion: no-preference) {
        .console-jump-latest {
            transition: translate 140ms ease-out, color 140ms ease-out,
                border-color 140ms ease-out;
        }

        .console-jump-latest:hover {
            translate: 0 -2px;
        }
    }

    .console-jump-latest:focus-visible {
        outline: 2px solid var(--tui-cyan);
        outline-offset: 2px;
    }

    .console-jump-latest svg {
        width: 20px;
        height: 20px;
        fill: none;
        stroke: currentColor;
        stroke-width: 1.75;
        stroke-linecap: round;
        stroke-linejoin: round;
    }

    .console-scroll::-webkit-scrollbar {
        display: none;
    }

    .conversation-history-boundary {
        display: flex;
        justify-content: center;
        color: var(--text-muted);
        font-size: var(--font-size-compact);
        padding-bottom: var(--space-3);
    }

    .conversation-history-boundary button {
        border: 0;
        background: transparent;
        color: var(--tui-cyan);
        cursor: pointer;
        font: inherit;
    }

    .conversation-history-boundary button:disabled {
        cursor: wait;
        opacity: 0.65;
    }

    .conversation-history-boundary.complete::before,
    .conversation-history-boundary.complete::after {
        content: "";
        align-self: center;
        width: 2rem;
        margin: 0 var(--space-2);
        border-top: 1px solid var(--line);
    }

    .pending-submissions {
        display: grid;
        grid-template-columns: minmax(0, 1fr);
        gap: var(--space-3);
        flex: 0 0 auto;
        min-width: 0;
        margin: 0 var(--space-3);
        color: var(--text-muted);
        font-family: var(--font-mono);
        font-size: var(--font-size-compact);
        line-height: var(--line-height-compact);
    }

    .pending-submissions.has-both {
        grid-template-columns: repeat(2, minmax(0, 1fr));
    }

    .pending-column {
        display: grid;
        align-content: start;
        gap: var(--space-1);
        min-width: 0;
    }

    .pending-submissions li {
        display: grid;
        grid-template-columns: minmax(0, 1fr);
        height: var(--line-height-compact);
        margin: 0;
        padding: 0;
        align-items: center;
    }

    .pending-queue li {
        grid-template-columns: minmax(0, 1fr) auto;
        gap: var(--space-2);
    }

    .pending-notifications {
        text-align: right;
    }

    .pending-submissions-label {
        margin: 0;
        padding: 0;
        font: inherit;
        color: inherit;
    }

    .pending-submissions ol {
        display: grid;
        gap: var(--space-1);
        max-height: calc(var(--line-height-compact) * 4 + var(--space-1) * 3);
        overflow-y: auto;
        margin: 0;
        padding: 0;
        list-style: none;
    }

    .pending-submission-preview,
    .pending-submissions-label {
        min-width: 0;
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
    }

    .pending-icon-button {
        opacity: 0;
        pointer-events: none;
        display: inline-flex;
        align-items: center;
        justify-content: center;
        width: var(--space-6);
        height: var(--line-height-compact);
        padding: 0;
        border: 0;
        border-radius: var(--radius-soft);
        background: transparent;
        color: var(--text-muted);
        cursor: pointer;
    }

    .pending-queue li:hover .pending-icon-button,
    .pending-queue li:focus-within .pending-icon-button {
        opacity: 1;
        pointer-events: auto;
    }

    @media (hover: none) {
        .pending-icon-button {
            opacity: 1;
            pointer-events: auto;
        }
    }

    .pending-icon-button:hover:not(:disabled) {
        background: var(--interactive-hover);
        color: var(--text);
    }

    .pending-icon-button:focus-visible {
        outline: 2px solid var(--accent);
        outline-offset: -2px;
    }

    .pending-icon-button:disabled {
        color: var(--text-faint);
        cursor: default;
    }

    .pending-icon-button svg {
        width: var(--space-4);
        height: var(--space-4);
        fill: none;
        stroke: currentColor;
        stroke-width: 1.5;
        stroke-linecap: round;
        stroke-linejoin: round;
    }

    .console-log {
        display: grid;
        align-content: start;
        gap: var(--space-3);
        min-height: 0;
        margin: 0;
        padding: 0;
        list-style: none;
    }

    .console-side-panel {
        position: fixed;
        top: 0;
        right: 0;
        bottom: 0;
        z-index: 5;
        display: grid;
        align-content: start;
        gap: var(--space-4);
        width: min(32rem, 100vw);
        padding: var(--space-6);
        overflow-y: auto;
        border-left: 1px solid var(--line);
        background: var(--bg);
    }

    .side-panel-header {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: var(--space-3);
    }

    .console-side-panel dl,
    .console-side-panel ul {
        display: grid;
        gap: var(--space-2);
        margin: 0;
        padding: 0;
        list-style: none;
    }

    .console-side-panel dt {
        color: var(--text-muted);
        font-size: var(--font-size-compact);
        line-height: var(--line-height-compact);
        font-weight: 800;
        letter-spacing: 0.05em;
        text-transform: uppercase;
    }

    .console-side-panel dd {
        margin: 0;
        color: var(--text-strong);
        font-weight: 700;
    }

    .metadata-details {
        color: var(--text-muted);
        font-size: var(--font-size-compact);
        line-height: var(--line-height-compact);
    }

    .metadata-details summary {
        cursor: pointer;
        font-weight: 800;
    }

    .console-composer {
        position: sticky;
        bottom: 0;
        z-index: 2;
        flex: 0 0 auto;
        display: grid;
        gap: var(--space-3);
        background: var(--bg);
    }

    .composer-input-shell {
        position: relative;
        border: 1px solid var(--line);
        border-radius: 18px;
        background: var(--bg-raised);
        cursor: text;
        padding: var(--space-1);
    }

    .composer-input-shell:focus-within {
        border-color: color-mix(in srgb, var(--tui-cyan) 60%, var(--line));
        box-shadow: 0 0 0 1px color-mix(in srgb, var(--tui-cyan) 18%, transparent);
    }

    .composer-input-shell.dragging-files {
        border-color: var(--accent);
        background: color-mix(in srgb, var(--accent) 8%, var(--bg-raised));
    }

    .attachment-file-input {
        position: absolute;
        width: 1px;
        height: 1px;
        overflow: hidden;
        clip: rect(0 0 0 0);
    }

    .composer-attachments {
        display: flex;
        flex-wrap: wrap;
        gap: var(--space-2);
        padding: 0 var(--space-3) var(--space-2);
    }

    .composer-attachment {
        display: inline-flex;
        align-items: center;
        gap: var(--space-2);
        max-width: 100%;
        border: 1px solid var(--line);
        border-radius: 999px;
        padding: var(--space-1) var(--space-2);
        font: 500 var(--font-size-compact) / var(--line-height-compact) var(--font-mono);
    }

    .composer-attachment.failed {
        border-color: var(--danger);
    }

    .composer-attachment progress {
        width: 4rem;
    }

    .attachment-name {
        max-width: 16rem;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }

    .composer-attachment button,
    .composer-attach-button,
    .composer-queue-button,
    .composer-notify-button {
        border: 0;
        background: transparent;
        color: var(--text-muted);
        cursor: pointer;
        pointer-events: auto;
        font: inherit;
    }

    .composer-attach-button,
    .composer-queue-button,
    .composer-notify-button {
        display: inline-grid;
        flex: 0 0 auto;
        width: 2.35rem;
        height: 2.35rem;
        place-items: center;
        border-radius: 999px;
        padding: 0;
    }

    .composer-attach-button:hover:not(:disabled),
    .composer-queue-button:hover:not(:disabled),
    .composer-notify-button:hover:not(:disabled) {
        background: var(--bg-subtle);
        color: var(--text-strong);
    }

    .composer-attach-button:disabled,
    .composer-queue-button:disabled,
    .composer-notify-button:disabled {
        cursor: not-allowed;
        opacity: 0.55;
    }

    .composer-input-footer {
        display: grid;
        grid-template-columns: minmax(0, 1fr) auto;
        gap: var(--space-2);
        align-items: end;
        min-height: 2.35rem;
        padding: 0 var(--space-1) var(--space-1) var(--space-2);
    }

    .composer-footer-slot {
        display: flex;
        align-items: center;
        gap: var(--space-2);
        min-width: 0;
    }

    .composer-send-button {
        display: inline-grid;
        width: 2.35rem;
        height: 2.35rem;
        place-items: center;
        border: 0;
        border-radius: 999px;
        background: var(--accent);
        color: var(--bg);
        cursor: pointer;
        padding: 0;
        pointer-events: auto;
    }

    .composer-send-button.stop {
        background: var(--danger);
        color: var(--bg);
    }

    .composer-send-button:disabled {
        cursor: not-allowed;
        opacity: 0.55;
    }

    .composer-attach-icon,
    .composer-queue-icon,
    .composer-notify-icon,
    .composer-send-icon {
        width: 1.2rem;
        height: 1.2rem;
        fill: none;
        stroke: currentColor;
        stroke-linecap: round;
        stroke-linejoin: round;
        stroke-width: 2;
    }

    .composer-submit-actions {
        display: flex;
        align-items: center;
        gap: var(--space-2);
    }

    @media (max-width: 960px) {
        .console-history.with-task-pane {
            grid-template-columns: minmax(0, 1fr);
            grid-template-rows: minmax(0, 1fr) minmax(0, 1fr);
        }

        .console-header {
            align-items: stretch;
            flex-direction: column;
        }

        .console-header-actions {
            align-self: flex-end;
        }
    }
</style>

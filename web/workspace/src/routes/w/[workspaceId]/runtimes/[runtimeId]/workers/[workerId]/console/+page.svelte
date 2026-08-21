<script lang="ts">
    import { tick, untrack } from "svelte";
    import ConsoleLineItem from "$lib/workspace/console/ConsoleLineItem.svelte";
    import ConsoleTasks from "$lib/workspace/console/ConsoleTasks.svelte";
    import ConsoleTimeline from "$lib/workspace/console/ConsoleTimeline.svelte";
    import { chatSubmit } from "$lib/workspace/console/chat-submit";
    import {
        buildComposerRequest,
        type WorkerConsoleInputRequest,
    } from "$lib/workspace/console/composer-command";
    import {
        applyCompletion,
        completionTokenAt,
        localCommandCompletions,
        type ComposerCompletionEntry,
        type ComposerCompletionToken,
    } from "$lib/workspace/console/composer-completion";
    import WorkerRunStatus from "$lib/workspace/console/WorkerRunStatus.svelte";
    import { fitTextarea } from "$lib/workspace/console/textarea-fit";
    import { resolveWorkerControlShortcut } from "$lib/workspace/console/worker-control-shortcuts";
    import {
        consoleWorkerViews,
        createConsoleProjector,
        isConsoleProjectionEvent,
        projectConsoleLines,
        resolveConsoleViewScrollTop,
        resolveConsoleWorkerView,
        selectConsoleTimelineLines,
        type ConsoleEventInput,
        type ConsoleLine,
        type ConsoleProjection,
        type ConsoleViewMode,
        type ConsoleViewScroll,
    } from "$lib/workspace/console/model";
    import type { Event as ProtocolEvent, Method as ProtocolMethod, RewindTarget, Segment } from "$lib/generated/protocol";
    import { pushWorkspaceAlert } from "$lib/workspace/alerts/store";
    import { workspaceApiPath } from "$lib/workspace/api/http";
    import { workspaceMultiplexer, type WorkspaceMultiplexerSubscription } from "$lib/workspace/multiplexer";
    import type {
        Diagnostic,
        Worker,
        PodProtocolEvent,
    } from "$lib/workspace/sidebar/types";

    type Props = {
        data: {
            workspaceId: string;
            runtimeId: string;
            workerId: string;
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

    type TimelineKind = "turn" | "assistant";

    type TimelineMark = {
        id: string;
        lineId: string;
        label: string;
        detail: string;
        timeLabel: string;
        position: number;
        sourcePosition: number;
        kind: TimelineKind;
    };

    type TimelineScaleSegment = {
        sourceStart: number;
        sourceEnd: number;
        targetStart: number;
        targetEnd: number;
    };

    type TimelineScale = {
        segments: TimelineScaleSegment[];
    };

    type TimelineLayout = {
        marks: TimelineMark[];
        scale: TimelineScale;
        axisSize: number;
    };

    type ScrollMetrics = {
        top: number;
        height: number;
        client: number;
    };

    let worker = $state<Worker | null>(untrack(() => data.worker));
    let liveWorkerState = $state<string | null>(
        untrack(() => data.worker?.state ?? null),
    );
    let workerError = $state<string | null>(untrack(() => data.workerError));
    let draft = $state("");
    let completionEntries = $state<ComposerCompletionEntry[]>([]);
    let completionToken = $state<ComposerCompletionToken | null>(null);
    let completionBusy = $state(false);
    let completionError = $state<string | null>(null);
    let sending = $state(false);
    let sendError = $state<string | null>(null);
    let rewindTargets = $state<RewindTarget[]>([]);
    let rewindHeadEntries = $state(0);
    let composerNotice = $state<string | null>(null);
    let protocolState = $state<"connecting" | "open" | "closed" | "error">(
        "connecting",
    );
    let protocolSubscription: WorkspaceMultiplexerSubscription | null = null;
    let pendingCompletionRequest: {
        resolve: (entries: ComposerCompletionEntry[]) => void;
        reject: (error: Error) => void;
        timeout: number;
    } | null = null;
    let streamDiagnostics = $state<Diagnostic[]>([]);
    let workerDetailsOpen = $state(false);
    let taskPaneOpen = $state(false);
    let selectedWorkerViewSessionId = $state<string | null>(null);
    let workerViewSelectionGeneration = 0;
    let timelineOpen = $state(false);
    let consoleViewMode = $state<ConsoleViewMode>("overview");
    let consoleBodyElement: HTMLElement | null = null;
    let composerTextareaElement: HTMLTextAreaElement | null = null;
    let timelineRailDragCleanup: (() => void) | null = null;
    let autoFollowConsole = $state(true);
    let consoleScroll = $state<ScrollMetrics>({ top: 0, height: 1, client: 1 });
    const consoleViewScroll = new Map<string, ConsoleViewScroll>();
    const eventObservedAtById = new Map<string, number>();
    let nextEventObservedAtVersion = 0;
    let eventObservedAtVersion = $state(0);
    const CONSOLE_BOTTOM_THRESHOLD_PX = 48;
    const TIMELINE_EDGE_PX = 42;
    const TIMELINE_MIN_MARK_GAP_PX = 40;
    const TIMELINE_AXIS_PADDING_PX = 42;
    const consoleProjector = createConsoleProjector();
    let consoleProjection = $state.raw<ConsoleProjection>(
        consoleProjector.snapshot(),
    );
    let pendingObservationEvents: ConsoleEventInput[] = [];
    let protocolEventSequence = 0;
    let pendingObservedStates: Array<string | null> = [];
    let pendingStreamDiagnostics: Diagnostic[] = [];
    let observationFlushHandle: number | null = null;
    let nextReloadToken = 0;
    let reloadToken = $state(0);

    type ConsoleTarget = {
        workspaceId: string;
        runtimeId: string;
        workerId: string;
    };

    const consoleTarget = $derived({ workspaceId, runtimeId, workerId });
    const controlAlertId = $derived(
        `worker-console-control:${runtimeId}:${workerId}`,
    );

    const workerViews = $derived(consoleWorkerViews(consoleProjection));
    const selectedWorkerView = $derived(
        resolveConsoleWorkerView(
            consoleProjection,
            selectedWorkerViewSessionId,
        ),
    );
    const selectedConsoleProjection = $derived(selectedWorkerView.console);
    const lines = $derived(
        projectConsoleLines(selectedConsoleProjection.lines, consoleViewMode),
    );
    const tasks = $derived(selectedConsoleProjection.tasks);
    const timelineLayout = $derived(
        buildTimelineLayout(lines, eventObservedAtVersion, consoleScroll),
    );
    const timelineMarks = $derived(timelineLayout.marks);
    const timelineAxisStyle = $derived(
        timelineAxisStyleFor(timelineLayout, consoleScroll),
    );
    const timelineThumb = $derived(
        timelineThumbStyle(consoleScroll, timelineLayout),
    );
    const diagnostics = $derived(
        mergeDiagnostics(worker?.diagnostics ?? [], streamDiagnostics),
    );
    const workerState = $derived(liveWorkerState ?? worker?.state ?? "loading");
    const workerRunning = $derived(workerState === "running");
    const workerPaused = $derived(workerState === "paused");
    const inputReady = $derived(workerState === "idle");
    const composerEditable = $derived(protocolState === "open" && !sending);
    const canSubmitDraft = $derived(inputReady && composerEditable);
    const canSend = $derived(canSubmitDraft && draft.trim().length > 0);
    const canStopFromComposer = $derived(workerRunning && composerEditable);
    const composerSubmitDisabled = $derived(
        workerRunning ? !canStopFromComposer : !canSend,
    );

    async function getJson<T>(path: string): Promise<T> {
        const response = await fetch(path);
        if (!response.ok) {
            throw new Error(`GET ${path} failed: ${response.status}`);
        }
        return response.json() as Promise<T>;
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

    function advanceEventObservedAtVersion() {
        nextEventObservedAtVersion += 1;
        eventObservedAtVersion = nextEventObservedAtVersion;
    }

    function resetObservedEvents() {
        cancelObservationFlush();
        consoleProjection = consoleProjector.reset();
        eventObservedAtById.clear();
        advanceEventObservedAtVersion();
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
            advanceEventObservedAtVersion();
        }

        if (diagnosticBatch.length > 0) {
            streamDiagnostics = [...streamDiagnostics, ...diagnosticBatch];
        }
    }

    function handleIncomingProtocolEvent(payload: ProtocolEvent) {
        handleProtocolCommandEvent(payload);
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
        eventObservedAtById.set(eventId, observedAtMs);
        pendingObservationEvents.push({
            eventId,
            event: payload,
            observedAtMs,
        });
        pendingObservedStates.push(workerStateFromProtocolEvent(payload));
        scheduleObservationFlush();
    }

    function queueObservationDiagnostic(diagnostic: Diagnostic) {
        pendingStreamDiagnostics.push(diagnostic);
        scheduleObservationFlush();
    }

    async function applyComposerCompletion(event: KeyboardEvent) {
        const target = event.currentTarget;
        if (!(target instanceof HTMLTextAreaElement)) {
            return;
        }
        const token = completionTokenAt(
            draft,
            target.selectionStart ?? draft.length,
        );
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
            composerNotice =
                entries.length > 1
                    ? `Completed ${token.sigil}${entries[0].value}; ${entries.length - 1} more candidate(s)`
                    : null;
        } catch (error) {
            completionError =
                error instanceof Error ? error.message : String(error);
        } finally {
            completionBusy = false;
        }
    }

    async function resolveCompletionEntries(
        token: ComposerCompletionToken,
    ): Promise<ComposerCompletionEntry[]> {
        if (token.kind === "command") {
            return localCommandCompletions(token.prefix);
        }
        const completionKind = token.kind;
        const completionPrefix = token.prefix;
        return new Promise((resolve, reject) => {
            if (pendingCompletionRequest) {
                rejectPendingCompletion(
                    new Error("Superseded by a newer completion request."),
                );
            }
            const timeout = window.setTimeout(() => {
                rejectPendingCompletion(
                    new Error("Worker completion request timed out."),
                );
            }, 30_000);
            pendingCompletionRequest = { resolve, reject, timeout };
            try {
                sendProtocolMethod({
                    method: "list_completions",
                    params: { kind: completionKind, prefix: completionPrefix },
                });
            } catch (error) {
                rejectPendingCompletion(
                    error instanceof Error ? error : new Error(String(error)),
                );
            }
        });
    }

    function handleComposerKeydown(event: KeyboardEvent) {
        if (event.key === "PageUp" || event.key === "PageDown") {
            event.preventDefault();
            scrollConsoleByPage(event.key === "PageDown" ? 1 : -1);
            return;
        }
        if (event.key !== "Tab") {
            return;
        }
        event.preventDefault();
        void applyComposerCompletion(event);
    }

    function scrollConsoleByPage(direction: 1 | -1) {
        if (!consoleBodyElement) {
            return;
        }
        consoleBodyElement.scrollBy({
            top: direction * Math.max(consoleBodyElement.clientHeight * 0.86, 1),
            behavior: "auto",
        });
        window.requestAnimationFrame(updateConsoleScrollMetrics);
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
            sendError = message;
            pushWorkspaceAlert("error", message, {
                id: controlAlertId,
                title: "Worker control failed",
            });
        }
    }

    function sendWorkerControl(command: "pause" | "cancel" | "resume") {
        const label = command[0].toUpperCase() + command.slice(1);
        sendControl({ method: command }, label);
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
        const composerFocused = event.target === composerTextareaElement;
        const command = resolveWorkerControlShortcut(event, {
            protocolOpen: protocolState === "open",
            running: workerRunning,
            paused: workerPaused,
            composerFocused,
            draftBlank: draft.trim().length === 0,
            editableTarget: isEditableTarget(event.target) && !composerFocused,
            hasSelection: targetHasSelection(event.target),
        });
        if (!command) return;

        event.preventDefault();
        event.stopPropagation();
        sendWorkerControl(command);
    }

    function requestRewindTargets() {
        sendControl({ method: "list_rewind_targets" }, "Rewind target request");
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
                    method: "run",
                    params: {
                        input: request.segments ?? [
                            { kind: "text", content: request.content },
                        ],
                    },
                };
            case "notify":
                return {
                    method: "notify",
                    params: { message: request.content, auto_run: true },
                };
            case "compact":
                return { method: "compact" };
            case "list_rewind_targets":
                return { method: "list_rewind_targets" };
            case "register_peer":
                return {
                    method: "register_peer",
                    params: { name: request.content },
                };
        }
    }

    function handleComposerSubmit(value = draft) {
        if (workerRunning) {
            sendControl({ method: "cancel" }, "Stop");
            return;
        }
        void submitDraft(value);
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
            draft = "";
            return;
        }
        if (sending || !inputReady) {
            return;
        }

        sending = true;
        sendError = null;
        try {
            const method = composerRequestToProtocolMethod(command.request);
            sendProtocolMethod(method);
            draft = "";
            if (method.method === "run" || method.method === "notify") {
                liveWorkerState = "running";
            }
            composerNotice = "Sent through Worker protocol.";
        } catch (error) {
            sendError = error instanceof Error ? error.message : String(error);
        } finally {
            sending = false;
        }
    }

    function sendMessage(event: SubmitEvent) {
        event.preventDefault();
        handleComposerSubmit();
    }

    function workerStateFromProtocolEvent(
        event: PodProtocolEvent,
    ): string | null {
        switch (event.event) {
            case "snapshot":
            case "status":
                return event.data.status;
            case "shutdown":
                return "shutdown";
            default:
                return null;
        }
    }

    function connectProtocolTransport(
        targetWorker: Worker | null,
        token: number,
        target: ConsoleTarget,
    ) {
        if (!targetWorker) {
            protocolState = "closed";
            return;
        }
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
                            for (const event of frame.message.payload.snapshot.data.events) {
                                handleIncomingProtocolEvent(event);
                            }
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
                            rejectPendingCompletion(new Error(frame.message.data.message));
                        } else if (
                            frame.frame === "response" &&
                            frame.message.result === "subscription_rejected"
                        ) {
                            protocolState = "error";
                            throw new Error(frame.message.payload.message);
                        }
                    } catch (error) {
                        streamDiagnostics = [
                            ...streamDiagnostics,
                            {
                                code: "worker_protocol_frame_invalid",
                                severity: "warning",
                                message: error instanceof Error ? error.message : String(error),
                            },
                        ];
                    }
                },
                onStatus: (status) => {
                    if (token !== reloadToken) return;
                    protocolState = status === "open" ? "connecting" : status;
                    if (status === "closed") {
                        rejectPendingCompletion(new Error("Worker protocol WebSocket closed."));
                    }
                },
            },
        );
        protocolSubscription = subscription;
        return () => {
            if (protocolSubscription === subscription) protocolSubscription = null;
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
            const pending = pendingCompletionRequest;
            if (!pending) {
                return;
            }
            pendingCompletionRequest = null;
            window.clearTimeout(pending.timeout);
            pending.resolve(event.data.entries);
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
            if (pendingCompletionRequest) {
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
        const pending = pendingCompletionRequest;
        if (!pending) {
            return;
        }
        pendingCompletionRequest = null;
        window.clearTimeout(pending.timeout);
        pending.reject(error);
    }

    function mergeDiagnostics(...groups: Diagnostic[][]): Diagnostic[] {
        return groups.flat();
    }

    function diagnosticsToText(items: Diagnostic[]): string {
        return items
            .map((item) => `${item.severity}: ${item.message}`)
            .join("\n");
    }

    function buildTimelineLayout(
        items: ConsoleLine[],
        _observedAtVersion: number,
        metrics: ScrollMetrics,
    ): TimelineLayout {
        const denominator = Math.max(items.length - 1, 1);
        const rawMarks = selectConsoleTimelineLines(items)
            .map(({ item, index }) =>
                timelineMarkForLine(item, index, denominator)
            )
            .filter((mark): mark is TimelineMark => mark !== null);
        const trackHeight = timelineTrackHeight(metrics);
        const positioned = positionTimelineMarks(rawMarks, trackHeight);
        const axisSize = timelineAxisSize(positioned, trackHeight);
        const scale = buildTimelineScale(
            positioned.map((mark) => mark.sourcePosition),
            positioned.map((mark) => mark.position),
            axisSize,
        );
        return {
            scale,
            axisSize,
            marks: projectTimelineMarks(positioned, axisSize, trackHeight),
        };
    }

    function positionTimelineMarks(
        marks: TimelineMark[],
        trackHeight: number,
    ): TimelineMark[] {
        if (marks.length === 0) {
            return marks;
        }
        const ordered = marks
            .map((mark, index) => ({ mark, index }))
            .sort(
                (a, b) =>
                    a.mark.sourcePosition - b.mark.sourcePosition ||
                    a.index - b.index,
            );
        const usableHeight = Math.max(trackHeight - TIMELINE_EDGE_PX * 2, 1);
        const targetPositions = ordered.map(
            ({ mark }) =>
                TIMELINE_EDGE_PX + (mark.sourcePosition / 100) * usableHeight,
        );
        const minimumGap = TIMELINE_MIN_MARK_GAP_PX;

        for (let index = 1; index < targetPositions.length; index += 1) {
            targetPositions[index] = Math.max(
                targetPositions[index],
                targetPositions[index - 1] + minimumGap,
            );
        }

        const positioned = [...marks];
        ordered.forEach(({ mark, index }, orderIndex) => {
            positioned[index] = {
                ...mark,
                position: targetPositions[orderIndex],
            };
        });
        return positioned;
    }

    function projectTimelineMarks(
        marks: TimelineMark[],
        axisSize: number,
        trackHeight: number,
    ): TimelineMark[] {
        return marks.map((mark) => ({
            ...mark,
            position: projectTimelineAxisPosition(mark.position, axisSize, trackHeight),
        }));
    }

    function projectTimelineAxisPosition(
        axisPosition: number,
        axisSize: number,
        trackHeight: number,
    ): number {
        if (axisSize <= 0) {
            return 0;
        }
        return (axisPosition / axisSize) * trackHeight;
    }

    function timelineAxisSize(
        marks: TimelineMark[],
        trackHeight: number,
    ): number {
        const last = marks.reduce(
            (max, mark) => Math.max(max, mark.position),
            0,
        );
        return Math.max(trackHeight, last + TIMELINE_EDGE_PX);
    }

    function timelineMarkForLine(
        item: ConsoleLine,
        index: number,
        denominator: number,
    ): TimelineMark | null {
        const kind = timelineKindForLine(item);
        if (!kind) {
            return null;
        }
        const observedAt = item.eventId
            ? observedAtForEventId(item.eventId)
            : null;
        const sourcePosition = (index / denominator) * 100;
        return {
            id: `timeline-${item.id}`,
            lineId: item.id,
            label: timelineLabelForLine(item, kind),
            detail: item.body || item.title,
            timeLabel: observedAt
                ? formatTimelineTime(observedAt)
                : "time unknown",
            position: sourcePosition,
            sourcePosition,
            kind,
        };
    }

    function buildTimelineScale(
        sourcePositions: number[],
        targetPositions: number[],
        axisSize: number,
    ): TimelineScale {
        if (sourcePositions.length === 0 || targetPositions.length === 0) {
            return {
                segments: [
                    {
                        sourceStart: 0,
                        sourceEnd: 100,
                        targetStart: 0,
                        targetEnd: axisSize,
                    },
                ],
            };
        }

        const pairs = sourcePositions
            .map((source, index) => ({
                source,
                target: targetPositions[index] ?? source,
                index,
            }))
            .filter((pair) => pair.source > 0 && pair.source < 100)
            .sort((a, b) => a.source - b.source || a.index - b.index);
        const sourceAnchors = [0, ...pairs.map((pair) => pair.source), 100];
        const targetAnchors = [
            0,
            ...pairs.map((pair) => pair.target),
            axisSize,
        ];
        const segments: TimelineScaleSegment[] = [];
        for (let index = 1; index < sourceAnchors.length; index += 1) {
            segments.push({
                sourceStart: sourceAnchors[index - 1],
                sourceEnd: sourceAnchors[index],
                targetStart: targetAnchors[index - 1],
                targetEnd: targetAnchors[index],
            });
        }
        return { segments };
    }

    function mapTimelinePosition(
        scale: TimelineScale,
        sourcePosition: number,
    ): number {
        const source = Math.max(0, Math.min(100, sourcePosition));
        const segment =
            scale.segments.find((item) => source <= item.sourceEnd) ??
            scale.segments.at(-1)!;
        const sourceRange = segment.sourceEnd - segment.sourceStart;
        if (sourceRange <= 0) {
            return segment.targetEnd;
        }
        const ratio = (source - segment.sourceStart) / sourceRange;
        return (
            segment.targetStart +
            (segment.targetEnd - segment.targetStart) * ratio
        );
    }

    function unmapTimelinePosition(
        scale: TimelineScale,
        targetPosition: number,
    ): number {
        const maxTarget = scale.segments.at(-1)?.targetEnd ?? 100;
        const target = Math.max(0, Math.min(maxTarget, targetPosition));
        const segment =
            scale.segments.find((item) => target <= item.targetEnd) ??
            scale.segments.at(-1)!;
        const targetRange = segment.targetEnd - segment.targetStart;
        if (targetRange <= 0) {
            return segment.sourceEnd;
        }
        const ratio = (target - segment.targetStart) / targetRange;
        return (
            segment.sourceStart +
            (segment.sourceEnd - segment.sourceStart) * ratio
        );
    }

    function timelineKindForLine(item: ConsoleLine): TimelineKind | null {
        if (item.kind === "user") {
            return "turn";
        }
        if (item.kind === "assistant") {
            return "assistant";
        }
        return null;
    }

    function timelineLabelForLine(
        item: ConsoleLine,
        _kind: TimelineKind,
    ): string {
        return firstTimelineText(item.body || item.title);
    }

    function firstTimelineText(value: string): string {
        const firstLine = value.trim().split(/\r?\n/, 1)[0] ?? "";
        const compact = firstLine.replace(/\s+/g, " ").trim();
        if (!compact) {
            return "—";
        }
        return compact.length > 10 ? `${compact.slice(0, 10)}…` : compact;
    }

    function observedAtForEventId(eventId: string): number | null {
        if (eventObservedAtById.has(eventId)) {
            return eventObservedAtById.get(eventId) ?? null;
        }
        const snapshotIndex = eventId.indexOf("-snapshot-");
        if (snapshotIndex > 0) {
            return (
                eventObservedAtById.get(eventId.slice(0, snapshotIndex)) ?? null
            );
        }
        return null;
    }

    function formatTimelineTime(timestampMs: number): string {
        return new Intl.DateTimeFormat(undefined, {
            hour: "2-digit",
            minute: "2-digit",
            second: "2-digit",
        }).format(new Date(timestampMs));
    }

    function timelineThumbStyle(
        metrics: ScrollMetrics,
        layout: TimelineLayout,
    ): string {
        const trackHeight = timelineTrackHeight(metrics);
        const contentHeight = Math.max(metrics.height, 1);
        const viewportHeight = Math.max(metrics.client, 1);
        const scrollable = Math.max(contentHeight - viewportHeight, 1);
        const viewportRatio = Math.min(viewportHeight / contentHeight, 1);
        const sourceTop =
            (metrics.top / scrollable) * (100 * (1 - viewportRatio));
        const sourceBottom = Math.min(100, sourceTop + viewportRatio * 100);
        const targetTop = mapTimelinePosition(layout.scale, sourceTop);
        const targetBottom = mapTimelinePosition(layout.scale, sourceBottom);
        const projectedTop = projectTimelineAxisPosition(
            targetTop,
            layout.axisSize,
            trackHeight,
        );
        const projectedBottom = projectTimelineAxisPosition(
            targetBottom,
            layout.axisSize,
            trackHeight,
        );
        const height = Math.max(projectedBottom - projectedTop, 18);
        const top = Math.min(projectedTop, Math.max(0, trackHeight - height));
        return `top: ${Math.max(0, top)}px; height: ${Math.min(height, trackHeight)}px;`;
    }

    function timelineTrackHeight(metrics: ScrollMetrics): number {
        return Math.max(metrics.client - TIMELINE_AXIS_PADDING_PX * 2, 1);
    }

    function timelineAxisStyleFor(
        _layout: TimelineLayout,
        metrics: ScrollMetrics,
    ): string {
        return [
            `height: ${timelineTrackHeight(metrics)}px`,
            `top: ${TIMELINE_AXIS_PADDING_PX}px`,
        ].join("; ");
    }

    function jumpToTimelineMark(mark: TimelineMark) {
        const target = consoleBodyElement?.querySelector(
            `[data-console-line-id="${cssEscape(mark.lineId)}"]`,
        );
        if (target instanceof HTMLElement) {
            target.scrollIntoView({ block: "start", behavior: "smooth" });
        }
    }

    function scrollConsoleToTimelineRailPosition(
        rail: HTMLElement,
        clientY: number,
        behavior: ScrollBehavior,
    ) {
        if (!consoleBodyElement) {
            return;
        }
        const rect = rail.getBoundingClientRect();
        const trackHeight = Math.max(rect.height, 1);
        const targetPx = Math.max(0, Math.min(trackHeight, clientY - rect.top));
        const axisPosition = (targetPx / trackHeight) * timelineLayout.axisSize;
        const sourcePercent = unmapTimelinePosition(
            timelineLayout.scale,
            axisPosition,
        );
        consoleBodyElement.scrollTo({
            top:
                (sourcePercent / 100) *
                Math.max(
                    consoleBodyElement.scrollHeight -
                        consoleBodyElement.clientHeight,
                    0,
                ),
            behavior,
        });
    }

    function handleTimelineRailPointerDown(event: PointerEvent) {
        if (!(event.currentTarget instanceof HTMLElement)) {
            return;
        }
        event.preventDefault();
        const rail = event.currentTarget;
        const pointerId = event.pointerId;
        scrollConsoleToTimelineRailPosition(rail, event.clientY, "auto");

        const handleMove = (moveEvent: PointerEvent) => {
            if (moveEvent.pointerId !== pointerId) {
                return;
            }
            moveEvent.preventDefault();
            scrollConsoleToTimelineRailPosition(rail, moveEvent.clientY, "auto");
        };
        const stopDrag = (finishEvent: PointerEvent) => {
            if (finishEvent.pointerId !== pointerId) {
                return;
            }
            timelineRailDragCleanup?.();
        };

        timelineRailDragCleanup?.();
        timelineRailDragCleanup = () => {
            window.removeEventListener("pointermove", handleMove);
            window.removeEventListener("pointerup", stopDrag);
            window.removeEventListener("pointercancel", stopDrag);
            timelineRailDragCleanup = null;
        };
        window.addEventListener("pointermove", handleMove);
        window.addEventListener("pointerup", stopDrag);
        window.addEventListener("pointercancel", stopDrag);
        try {
            rail.setPointerCapture(pointerId);
        } catch {
            // Pointer capture can fail if the pointer is already released.
        }
    }

    function cssEscape(value: string): string {
        return typeof CSS !== "undefined" && typeof CSS.escape === "function"
            ? CSS.escape(value)
            : value.replaceAll('"', '\\"');
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
        updateConsoleScrollMetrics();
    }

    function updateConsoleScrollMetrics() {
        if (!consoleBodyElement) {
            return;
        }
        consoleScroll = {
            top: consoleBodyElement.scrollTop,
            height: consoleBodyElement.scrollHeight,
            client: consoleBodyElement.clientHeight,
        };
    }

    function isNearConsoleBottom(element: HTMLElement): boolean {
        return (
            element.scrollHeight - element.scrollTop - element.clientHeight <=
            CONSOLE_BOTTOM_THRESHOLD_PX
        );
    }

    function handleConsoleScroll() {
        if (!consoleWorkerViewSelectionIsResolved() || !consoleBodyElement) {
            return;
        }
        autoFollowConsole = isNearConsoleBottom(consoleBodyElement);
        updateConsoleScrollMetrics();
        rememberConsoleWorkerViewScroll();
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
        updateConsoleScrollMetrics();
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
        } else {
            const sessionId = selectedWorkerView.sessionId;
            tick().then(() => {
                if (
                    consoleWorkerViewSelectionIsResolved() &&
                    selectedWorkerView.sessionId === sessionId
                ) {
                    updateConsoleScrollMetrics();
                }
            });
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
        return () => timelineRailDragCleanup?.();
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
        protocolState = "connecting";
        const token = advanceReloadToken();
        if (!targetWorker) void loadWorker(target, token);
    });

    $effect(() => connectProtocolTransport(worker, reloadToken, consoleTarget));
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
            <button
                type="button"
                class="secondary-button"
                disabled={protocolState !== "open"}
                onclick={() => sendControl({ method: "compact" }, "Compact")}
            >
                Compact
            </button>
            <button
                type="button"
                class="secondary-button"
                disabled={protocolState !== "open"}
                onclick={requestRewindTargets}
            >
                Rewind
            </button>
            <button
                type="button"
                class="secondary-button"
                aria-expanded={taskPaneOpen}
                onclick={() => {
                    taskPaneOpen = !taskPaneOpen;
                    if (taskPaneOpen) workerDetailsOpen = false;
                }}
            >
                Tasks{tasks.length > 0 ? ` ${tasks.length}` : ""}
            </button>
            <button
                type="button"
                class="secondary-button"
                aria-expanded={workerDetailsOpen}
                onclick={() => {
                    workerDetailsOpen = !workerDetailsOpen;
                    if (workerDetailsOpen) taskPaneOpen = false;
                }}
            >
                Details
            </button>
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
        <section class:timeline-open={timelineOpen} class="console-body">
        <div class="console-timeline-spacer" aria-hidden="true"></div>
        <div class="timeline-fold-cell">
            <button
                type="button"
                class="timeline-fold"
                aria-expanded={timelineOpen}
                aria-label={timelineOpen ? "Hide timeline" : "Show timeline"}
                onclick={() => (timelineOpen = !timelineOpen)}
            >
                {timelineOpen ? "Timeline ◂" : "Timeline ▸"}
            </button>
        </div>
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
        </div>

            <ConsoleTimeline
                marks={timelineMarks}
                thumbStyle={timelineThumb}
                axisStyle={timelineAxisStyle}
                expanded={timelineOpen}
                onRailPointerDown={handleTimelineRailPointerDown}
                onMarkClick={jumpToTimelineMark}
            />
        </section>

        {#if taskPaneOpen}
            <ConsoleTasks {tasks} mode="pane" />
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
                <details class="metadata-details">
                    <summary>Capabilities</summary>
                    <ul>
                        <li>
                            stop: {worker.capabilities.can_stop
                                ? "available"
                                : "unsupported"}
                        </li>
                        <li>
                            follow-up spawn: {worker.capabilities
                                .can_spawn_followup
                                ? "available"
                                : "unsupported"}
                        </li>
                    </ul>
                </details>
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
        />
    {/if}

    <ConsoleTasks
        {tasks}
        mode="mini"
        workerViews={workerViews.map(({ sessionId, label }) => ({
            sessionId,
            label,
        }))}
        selectedWorkerViewSessionId={selectedWorkerView.sessionId}
        onSelectWorkerView={(sessionId) => {
            void selectConsoleWorkerView(sessionId);
        }}
    />

    <form class="console-composer" onsubmit={sendMessage}>
        <div class="composer-input-shell">
            <textarea
                id="worker-console-message"
                aria-label="Console input"
                aria-keyshortcuts="Meta+Enter Control+Enter"
                bind:this={composerTextareaElement}
                bind:value={draft}
                use:chatSubmit={{
                    enabled: canSubmitDraft,
                    onSubmit: (value) => handleComposerSubmit(value),
                }}
                use:fitTextarea={{ value: draft, maxRows: 10 }}
                onkeydown={handleComposerKeydown}
                disabled={!composerEditable}></textarea>
            <div class="composer-input-footer">
                <div class="composer-footer-slot">
                    {#if completionBusy || completionError || completionEntries.length > 0}
                        <div class="composer-completions" aria-live="polite">
                            {#if completionBusy}
                                <span>completing…</span>
                            {:else if completionError}
                                <span class="error">{completionError}</span>
                            {:else}
                                <span
                                    >Tab: {completionToken?.sigil}{completionEntries[0]
                                        ?.value}</span
                                >
                                {#if completionEntries.length > 1}
                                    <span>{completionEntries.length - 1} more</span>
                                {/if}
                            {/if}
                        </div>
                    {/if}
                </div>
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
        <div class="composer-actions">
            {#if composerNotice}
                <span class="composer-notice">{composerNotice}</span>
            {/if}
            {#if sendError}<p class="error">{sendError}</p>{/if}
        </div>
    </form>
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
        gap: 1rem;
        min-height: 0;
        height: calc(100dvh - (var(--space-6) * 2));
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
        align-items: flex-start;
        justify-content: flex-end;
        gap: var(--space-4);
    }

    .console-card {
        min-height: 100%;
    }

    .console-header-actions {
        display: flex;
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
        padding: 0.42rem 0.65rem;
        font: inherit;
        font-size: 0.7rem;
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
        font-size: 0.9rem;
    }

    .rewind-target-list {
        display: flex;
        flex-wrap: wrap;
        gap: var(--space-2);
    }

    .rewind-target-list span {
        margin-left: 0.5rem;
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
        --console-timeline-width: 12rem;
        --console-timeline-fold-width: 2.25rem;

        display: grid;
        grid-template-columns: minmax(0, 1fr) var(--console-timeline-fold-width);
        grid-template-rows: auto minmax(0, 1fr);
        gap: 0 var(--space-3);
        min-height: 0;
        overflow: hidden;
        transition: grid-template-columns 160ms ease;
    }

    .console-body.timeline-open {
        grid-template-columns: minmax(0, 1fr) var(--console-timeline-width);
    }

    .console-timeline-spacer {
        grid-column: 1;
        grid-row: 1;
    }

    .timeline-fold-cell {
        grid-column: 2;
        grid-row: 1;
        display: flex;
        justify-content: flex-start;
        padding-bottom: var(--space-2);
    }

    .timeline-fold {
        width: 100%;
        border: 1px solid var(--line);
        border-radius: 999px;
        background: var(--bg-raised);
        color: var(--text-muted);
        cursor: pointer;
        font-size: 0.76rem;
        font-weight: 800;
        padding: 0.35rem 0.5rem;
        text-align: left;
        white-space: nowrap;
    }

    .timeline-fold:focus-visible {
        border-color: var(--tui-cyan);
        color: var(--text-strong);
    }

    .console-scroll {
        grid-column: 1;
        grid-row: 2;
        height: 100%;
        min-width: 0;
        min-height: 0;
        overflow-y: auto;
        padding-right: 0;
        scrollbar-gutter: auto;
        scrollbar-width: none;
    }

    .console-scroll::-webkit-scrollbar {
        display: none;
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
        font-size: 0.72rem;
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
        font-size: 0.84rem;
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
        padding: 0.35rem;
    }

    .composer-input-shell:focus-within {
        border-color: color-mix(in srgb, var(--tui-cyan) 60%, var(--line));
        box-shadow: 0 0 0 1px color-mix(in srgb, var(--tui-cyan) 18%, transparent);
    }

    .composer-input-footer {
        position: absolute;
        right: 0.7rem;
        bottom: 0.7rem;
        left: 1rem;
        display: grid;
        grid-template-columns: minmax(0, 1fr) auto;
        gap: var(--space-2);
        align-items: end;
        min-height: 2.35rem;
        pointer-events: none;
    }

    .composer-footer-slot {
        min-width: 0;
    }

    .console-composer textarea {
        box-sizing: border-box;
        width: 100%;
        min-height: 5.35rem;
        resize: none;
        overflow-y: hidden;
        border: 0;
        border-radius: 14px;
        background: transparent;
        padding: 0.55rem 3.4rem 3rem 0.65rem;
        font: inherit;
        line-height: 1.45;
        color: var(--text-strong);
        outline: none;
        cursor: text;
    }

    .console-composer textarea:disabled {
        color: var(--text-muted);
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

    .composer-send-icon {
        width: 1.2rem;
        height: 1.2rem;
        fill: none;
        stroke: currentColor;
        stroke-linecap: round;
        stroke-linejoin: round;
        stroke-width: 2;
    }

    .composer-completions {
        display: flex;
        flex-wrap: wrap;
        gap: var(--space-2);
        align-items: center;
        color: var(--text-muted);
        font-size: 0.86rem;
    }

    .composer-notice {
        color: var(--text-muted);
        font-size: 0.86rem;
    }

    .composer-actions {
        display: flex;
        flex-wrap: wrap;
        align-items: center;
        justify-content: flex-end;
        gap: 0.7rem;
    }

    .composer-actions .composer-notice {
        margin-right: auto;
    }

    @media (max-width: 960px) {
        .console-history.with-task-pane {
            grid-template-columns: minmax(0, 1fr);
        }

        .console-header {
            flex-direction: column;
        }
    }
</style>

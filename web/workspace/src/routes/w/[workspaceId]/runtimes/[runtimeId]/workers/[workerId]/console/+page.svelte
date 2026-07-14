<script lang="ts">
    import { tick } from "svelte";
    import ConsoleLineItem from "$lib/workspace-console/ConsoleLineItem.svelte";
    import ConsoleTimeline from "$lib/workspace-console/ConsoleTimeline.svelte";
    import { chatSubmit } from "$lib/workspace-console/chat-submit";
    import { buildComposerRequest } from "$lib/workspace-console/composer-command";
    import {
        applyCompletion,
        completionTokenAt,
        localCommandCompletions,
        type ComposerCompletionEntry,
        type ComposerCompletionToken,
    } from "$lib/workspace-console/composer-completion";
    import { fitTextarea } from "$lib/workspace-console/textarea-fit";
    import {
        createConsoleProjector,
        type ConsoleEventInput,
        type ConsoleLine,
        type ConsoleProjection,
    } from "$lib/workspace-console/model";
    import { workspaceApiPath } from "$lib/workspace-api/http";
    import type {
        ClientWorkerEventWsFrame,
        Diagnostic,
        Worker,
        WorkerInputResult,
        PodProtocolEvent,
    } from "$lib/workspace-sidebar/types";

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
        kind: "file" | "knowledge" | "workflow";
        prefix: string;
        entries: ComposerCompletionEntry[];
        diagnostics: Diagnostic[];
    };

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

    let worker = $state<Worker | null>(null);
    let liveWorkerState = $state<string | null>(null);
    let workerError = $state<string | null>(null);
    let draft = $state("");
    let completionEntries = $state<ComposerCompletionEntry[]>([]);
    let completionToken = $state<ComposerCompletionToken | null>(null);
    let completionBusy = $state(false);
    let completionError = $state<string | null>(null);
    let sending = $state(false);
    let sendError = $state<string | null>(null);
    let composerNotice = $state<string | null>(null);
    let streamState = $state<"connecting" | "open" | "closed" | "error">(
        "connecting",
    );
    let streamDiagnostics = $state<Diagnostic[]>([]);
    let workerDetailsOpen = $state(false);
    let timelineOpen = $state(false);
    let consoleBodyElement: HTMLElement | null = null;
    let autoFollowConsole = $state(true);
    let consoleScroll = $state<ScrollMetrics>({ top: 0, height: 1, client: 1 });
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
    const inputReady = $derived(workerState === "idle");
    const canSend = $derived(inputReady && draft.trim().length > 0 && !sending);

    async function getJson<T>(path: string): Promise<T> {
        const response = await fetch(path);
        if (!response.ok) {
            throw new Error(`GET ${path} failed: ${response.status}`);
        }
        return response.json() as Promise<T>;
    }

    async function postJson<T>(
        path: string,
        body: unknown,
        timeoutMs = 30_000,
    ): Promise<T> {
        const controller = new AbortController();
        const timeout = window.setTimeout(() => controller.abort(), timeoutMs);
        try {
            const response = await fetch(path, {
                method: "POST",
                headers: { "content-type": "application/json" },
                body: JSON.stringify(body),
                signal: controller.signal,
            });
            if (!response.ok) {
                let detail = "";
                try {
                    detail = await response.text();
                } catch {
                    detail = "";
                }
                throw new Error(
                    `POST ${path} failed: ${response.status}${detail ? ` ${detail}` : ""}`,
                );
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
                workerApiPath(
                    `/runtimes/${encodeURIComponent(target.runtimeId)}/workers/${encodeURIComponent(target.workerId)}`,
                ),
            );
            worker = payload;
            liveWorkerState = payload.state;
        } catch (error) {
            workerError =
                error instanceof Error ? error.message : String(error);
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

    function advanceEventObservedAtVersion() {
        nextEventObservedAtVersion += 1;
        eventObservedAtVersion = nextEventObservedAtVersion;
    }

    function resetObservedEvents() {
        cancelObservationFlush();
        consoleProjection = consoleProjector.reset();
        eventObservedAtById.clear();
        advanceEventObservedAtVersion();
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
            advanceEventObservedAtVersion();
        }

        if (diagnosticBatch.length > 0) {
            streamDiagnostics = [...streamDiagnostics, ...diagnosticBatch];
        }
    }

    function queueObservationEvent(
        frame: ClientWorkerEventWsFrame & { kind: "event" },
    ) {
        if (!rememberObservationEvent(frame.envelope.event_id)) {
            return;
        }
        const observedAtMs = Date.now();
        eventObservedAtById.set(frame.envelope.event_id, observedAtMs);
        pendingObservationEvents.push({
            eventId: frame.envelope.event_id,
            event: frame.envelope.payload,
            observedAtMs,
        });
        pendingObservedStates.push(
            workerStateFromProtocolEvent(frame.envelope.payload),
        );
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
        const result = await postJson<WorkerCompletionsResult>(
            workerApiPath(
                `/runtimes/${encodeURIComponent(runtimeId)}/workers/${encodeURIComponent(workerId)}/completions`,
            ),
            { kind: token.kind, prefix: token.prefix },
        );
        if (result.diagnostics.length > 0 && result.entries.length === 0) {
            throw new Error(diagnosticsToText(result.diagnostics));
        }
        return result.entries;
    }

    function handleComposerKeydown(event: KeyboardEvent) {
        if (event.key !== "Tab") {
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
            draft = "";
            return;
        }
        if (sending || !inputReady) {
            return;
        }

        sending = true;
        sendError = null;
        try {
            const result = await postJson<WorkerInputResult>(
                workerApiPath(
                    `/runtimes/${encodeURIComponent(runtimeId)}/workers/${encodeURIComponent(workerId)}/input`,
                ),
                command.request,
            );
            if (result.state === "accepted") {
                draft = "";
                liveWorkerState = "running";
            } else {
                sendError =
                    diagnosticsToText(result.diagnostics) ||
                    `Input was ${result.state}.`;
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

    function connectObservation(
        targetWorker: Worker | null,
        token: number,
        target: ConsoleTarget,
    ) {
        if (!targetWorker) {
            streamState = "closed";
            return;
        }
        streamState = "connecting";
        const protocol = window.location.protocol === "https:" ? "wss:" : "ws:";
        const wsPath = workerApiPath(
            `/runtimes/${encodeURIComponent(target.runtimeId)}/workers/${encodeURIComponent(
                target.workerId,
            )}/events/ws`,
        );
        const ws = new WebSocket(
            `${protocol}//${window.location.host}${wsPath}`,
        );

        ws.onopen = () => {
            if (token === reloadToken) {
                streamState = "open";
            }
        };
        ws.onmessage = (message) => {
            if (token !== reloadToken) {
                return;
            }
            try {
                const frame = JSON.parse(
                    String(message.data),
                ) as ClientWorkerEventWsFrame;
                if (frame.kind === "event") {
                    queueObservationEvent(frame);
                } else {
                    queueObservationDiagnostic({
                        code: frame.diagnostic.code,
                        severity: "warning",
                        message: frame.diagnostic.message,
                    });
                }
            } catch (error) {
                queueObservationDiagnostic({
                    code: "worker_observation_frame_invalid",
                    severity: "warning",
                    message:
                        error instanceof Error ? error.message : String(error),
                });
            }
        };
        ws.onerror = () => {
            if (token === reloadToken) {
                streamState = "error";
                streamDiagnostics = [
                    ...streamDiagnostics,
                    {
                        code: "worker_observation_ws_error",
                        severity: "error",
                        message: "Worker observation WebSocket failed.",
                    },
                ];
            }
        };
        ws.onclose = () => {
            if (token === reloadToken && streamState !== "error") {
                streamState = "closed";
            }
        };

        return () => ws.close();
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
        const rawMarks = items
            .map((item, index) => timelineMarkForLine(item, index, denominator))
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
            marks: positioned,
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
        const contentHeight = Math.max(metrics.height, 1);
        const viewportHeight = Math.max(metrics.client, 1);
        const scrollable = Math.max(contentHeight - viewportHeight, 1);
        const viewportRatio = Math.min(viewportHeight / contentHeight, 1);
        const sourceTop =
            (metrics.top / scrollable) * (100 * (1 - viewportRatio));
        const sourceBottom = Math.min(100, sourceTop + viewportRatio * 100);
        const targetTop = mapTimelinePosition(layout.scale, sourceTop);
        const targetBottom = mapTimelinePosition(layout.scale, sourceBottom);
        const height = Math.max(targetBottom - targetTop, 18);
        const top = Math.min(targetTop, Math.max(0, layout.axisSize - height));
        return `top: ${Math.max(0, top)}px; height: ${Math.min(height, layout.axisSize)}px;`;
    }

    function timelineTrackHeight(metrics: ScrollMetrics): number {
        return Math.max(metrics.client - TIMELINE_AXIS_PADDING_PX * 2, 1);
    }

    function timelineAxisStyleFor(
        layout: TimelineLayout,
        metrics: ScrollMetrics,
    ): string {
        const trackHeight = timelineTrackHeight(metrics);
        const scrollable = Math.max(metrics.height - metrics.client, 1);
        const scrollRatio = Math.max(0, Math.min(1, metrics.top / scrollable));
        const offset = Math.max(0, layout.axisSize - trackHeight) * scrollRatio;
        return [
            `height: ${layout.axisSize}px`,
            `top: ${TIMELINE_AXIS_PADDING_PX - offset}px`,
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

    function jumpTimelineRatio(event: MouseEvent) {
        if (
            !consoleBodyElement ||
            !(event.currentTarget instanceof HTMLElement)
        ) {
            return;
        }
        const rect = event.currentTarget.getBoundingClientRect();
        const targetPx = Math.max(
            0,
            Math.min(rect.height, event.clientY - rect.top),
        );
        const sourcePercent = unmapTimelinePosition(
            timelineLayout.scale,
            targetPx,
        );
        consoleBodyElement.scrollTo({
            top:
                (sourcePercent / 100) *
                Math.max(
                    consoleBodyElement.scrollHeight -
                        consoleBodyElement.clientHeight,
                    0,
                ),
            behavior: "smooth",
        });
    }

    function cssEscape(value: string): string {
        return typeof CSS !== "undefined" && typeof CSS.escape === "function"
            ? CSS.escape(value)
            : value.replaceAll('"', '\\"');
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
        if (!consoleBodyElement) {
            return;
        }
        autoFollowConsole = isNearConsoleBottom(consoleBodyElement);
        updateConsoleScrollMetrics();
    }

    async function scrollConsoleToBottom() {
        await tick();
        if (!consoleBodyElement) {
            return;
        }
        consoleBodyElement.scrollTop = consoleBodyElement.scrollHeight;
        updateConsoleScrollMetrics();
        autoFollowConsole = true;
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
        if (autoFollowConsole) {
            void scrollConsoleToBottom();
        } else {
            tick().then(updateConsoleScrollMetrics);
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
    <meta
        name="description"
        content="Worker attach console through Workspace Backend APIs"
    />
</svelte:head>

<div class="console-shell worker-console-shell">
    <section class="console-header card">
        <div>
            <h2>{worker?.label ?? workerId}</h2>
        </div>
        <div class="console-header-actions">
            <div
                class="console-status-pill"
                class:warn={streamState !== "open"}
            >
                {workerState} · stream {streamState}
            </div>
            <button
                type="button"
                class="secondary-button"
                aria-expanded={workerDetailsOpen}
                onclick={() => (workerDetailsOpen = !workerDetailsOpen)}
            >
                Details
            </button>
        </div>
    </section>

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
            <article class="card console-card worker-console-card">
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

        {#if timelineOpen}
            <ConsoleTimeline
                marks={timelineMarks}
                thumbStyle={timelineThumb}
                axisStyle={timelineAxisStyle}
                onRailClick={jumpTimelineRatio}
                onMarkClick={jumpToTimelineMark}
            />
        {/if}
    </section>

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
                        <dt>Role / profile</dt>
                        <dd>
                            {worker.role ?? "unknown"} / {worker.profile ??
                                "unknown"}
                        </dd>
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
                    open={streamState === "error"}
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

    <form class="console-composer card" onsubmit={sendMessage}>
        <textarea
            id="worker-console-message"
            aria-label="Console input"
            aria-keyshortcuts="Meta+Enter Control+Enter"
            bind:value={draft}
            use:chatSubmit={{
                enabled: inputReady && !sending,
                onSubmit: (value) => void submitDraft(value),
            }}
            use:fitTextarea={{ value: draft, maxRows: 10 }}
            onkeydown={handleComposerKeydown}
            disabled={!inputReady || sending}></textarea>
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
        <div class="composer-actions">
            <button type="submit" disabled={!canSend}>
                {sending ? "Sending…" : "Send"}
            </button>
            {#if composerNotice}
                <span class="composer-notice">{composerNotice}</span>
            {/if}
            {#if sendError}<p class="error">{sendError}</p>{/if}
        </div>
    </form>
</div>

<style>
    .worker-console-shell {
        display: flex;
        flex-direction: column;
        gap: 1rem;
        min-height: 0;
        height: calc(100dvh - (var(--space-6) * 2));
        overflow: hidden;
    }

    .worker-console-shell > .console-body {
        flex: 1 1 auto;
        min-height: 0;
        overflow: hidden;
        overscroll-behavior: contain;
    }

    .console-header {
        display: flex;
        align-items: flex-start;
        justify-content: space-between;
        gap: var(--space-4);
    }

    .console-card {
        min-height: 100%;
    }

    .console-header-actions {
        display: flex;
        align-items: center;
        justify-content: flex-end;
        gap: var(--space-2);
    }

    .console-status-pill {
        min-width: 14rem;
        padding: 0.75rem 0.9rem;
        border: 1px solid var(--line);
        border-radius: 16px;
        background: var(--bg-raised);
        color: var(--text-muted);
        font-weight: 800;
        text-transform: uppercase;
        letter-spacing: 0.04em;
        font-size: 0.76rem;
        text-align: right;
    }

    .console-status-pill.warn {
        color: var(--warning);
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
        padding-right: var(--space-2);
        scrollbar-color: color-mix(in srgb, var(--tui-cyan) 60%, var(--line))
            color-mix(in srgb, var(--bg-raised) 70%, transparent);
        scrollbar-gutter: stable;
        scrollbar-width: thin;
    }

    .console-scroll::-webkit-scrollbar {
        width: 0.65rem;
    }

    .console-scroll::-webkit-scrollbar-track {
        border-radius: 999px;
        background: color-mix(in srgb, var(--bg-raised) 70%, transparent);
    }

    .console-scroll::-webkit-scrollbar-thumb {
        border: 2px solid transparent;
        border-radius: 999px;
        background: color-mix(in srgb, var(--tui-cyan) 60%, var(--line));
        background-clip: padding-box;
    }

    .console-scroll::-webkit-scrollbar-thumb:hover {
        background: var(--tui-cyan);
        background-clip: padding-box;
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
        margin-inline: calc(-1 * var(--space-6));
        padding: var(--space-3) var(--space-6) var(--space-4);
        background: var(--bg);
    }

    .console-composer textarea {
        box-sizing: border-box;
        width: 100%;
        min-height: 0;
        resize: none;
        overflow-y: hidden;
        border: 1px solid var(--line);
        border-radius: 14px;
        padding: 0.85rem 1rem;
        font: inherit;
        line-height: 1.45;
        color: var(--text-strong);
    }

    .console-composer textarea:disabled {
        background: var(--bg-raised);
        color: var(--text-muted);
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

    .composer-actions button {
        border: 0;
        border-radius: 999px;
        padding: 0.65rem 1rem;
        background: var(--accent);
        color: var(--bg);
        font-weight: 800;
        cursor: pointer;
    }

    .composer-actions button:disabled {
        cursor: not-allowed;
        opacity: 0.55;
    }

    @media (max-width: 960px) {
        .console-header {
            flex-direction: column;
        }

        .console-status-pill {
            width: 100%;
            text-align: left;
        }
    }
</style>

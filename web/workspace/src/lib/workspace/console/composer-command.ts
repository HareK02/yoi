import type { Segment } from "$lib/generated/protocol";

export type WorkerConsoleInputKind =
  | "user"
  | "notify"
  | "compact"
  | "list_rewind_targets"
  | "register_peer";

export type WorkerConsoleInputRequest = {
  kind: WorkerConsoleInputKind;
  content: string;
  segments?: Segment[];
};

export type ComposerCommandResult =
  | { ok: true; request?: WorkerConsoleInputRequest; notice?: string }
  | { ok: false; message: string };

type CommandSpec = {
  usage: string;
  description: string;
};

const COMMANDS: Record<string, CommandSpec> = {
  help: {
    usage: ":help [command]",
    description:
      "Show available Web Console commands or details for one command.",
  },
  "?": {
    usage: ":? [command]",
    description: "Alias for :help.",
  },
  noop: {
    usage: ":noop",
    description: "Validate command dispatch without side effects.",
  },
  compact: {
    usage: ":compact",
    description: "Request immediate Worker context compaction.",
  },
  rewind: {
    usage: ":rewind",
    description: "Ask the Worker for rewind targets.",
  },
  rollback: {
    usage: ":rollback",
    description: "Alias for :rewind.",
  },
  peer: {
    usage: ":peer <worker-name>",
    description:
      "Register another existing Worker as a reciprocal metadata peer.",
  },
  notify: {
    usage: ":notify <message>",
    description: "Send an agent-visible notification to the Worker.",
  },
};

export function buildComposerRequest(value: string): ComposerCommandResult {
  const content = value.trim();
  if (!content) {
    return { ok: false, message: "Input is empty." };
  }
  if (content.startsWith(":")) {
    return buildColonCommand(content.slice(1));
  }
  const segments = parseSigilSegments(content);
  return {
    ok: true,
    request: {
      kind: "user",
      content,
      segments: segments.some((segment) => segment.kind !== "text")
        ? segments
        : undefined,
    },
  };
}

export interface ComposerSegmentsRequestOptions {
  preserveExactText?: boolean;
}

export function buildComposerSegmentsRequest(
  sourceSegments: readonly Segment[],
  options: ComposerSegmentsRequestOptions = {},
): ComposerCommandResult {
  const hasRichSegment = sourceSegments.some((segment) =>
    segment.kind === "paste" || segment.kind === "uploaded_file"
  );
  if (!hasRichSegment) {
    const content = sourceSegments.map(segmentContent).join("");
    if (!options.preserveExactText || content.trimStart().startsWith(":")) {
      return buildComposerRequest(content);
    }
    if (!content.trim()) {
      return { ok: false, message: "Input is empty." };
    }
    const segments = coalesceTextSegments(
      sourceSegments.flatMap((segment) =>
        segment.kind === "text"
          ? parseSigilSegments(segment.content)
          : [segment]
      ),
    );
    return {
      ok: true,
      request: { kind: "user", content, segments },
    };
  }

  const content = sourceSegments.map(segmentContent).join("");
  if (!content.trim()) {
    return { ok: false, message: "Input is empty." };
  }
  const leadingText = sourceSegments[0]?.kind === "text"
    ? sourceSegments[0].content
    : "";
  if (leadingText.trimStart().startsWith(":")) {
    return {
      ok: false,
      message:
        "Commands cannot include paste or attachment chips. Remove the chip or send it as a message.",
    };
  }

  const segments: Segment[] = [];
  for (const segment of sourceSegments) {
    if (segment.kind === "text") {
      segments.push(...parseSigilSegments(segment.content));
    } else {
      segments.push(segment);
    }
  }
  return {
    ok: true,
    request: {
      kind: "user",
      content,
      segments: coalesceTextSegments(segments),
    },
  };
}

function segmentContent(segment: Segment): string {
  switch (segment.kind) {
    case "text":
    case "paste":
      return segment.content;
    case "file_ref":
      return `@${segment.path}`;
    case "flow":
      return segment.selector;
    case "paste_artifact":
      return "";
    case "uploaded_file":
      return `[Attached file: ${segment.file.file_name}]`;
    default:
      return "";
  }
}

function buildColonCommand(commandLine: string): ComposerCommandResult {
  const [name = "", ...argv] = commandLine.trim().split(/\s+/).filter(Boolean);
  if (!name) {
    return {
      ok: false,
      message: "Empty command. Type :help for available commands.",
    };
  }
  switch (name) {
    case "help":
    case "?":
      return helpCommand(argv);
    case "noop":
      if (argv.length > 0) {
        return invalidUsage("noop");
      }
      return { ok: true, notice: "noop: no action" };
    case "compact":
      if (argv.length > 0) {
        return invalidUsage("compact");
      }
      return {
        ok: true,
        request: { kind: "compact", content: "" },
        notice: "compact requested",
      };
    case "rewind":
    case "rollback":
      if (argv.length > 0) {
        return invalidUsage("rewind");
      }
      return {
        ok: true,
        request: { kind: "list_rewind_targets", content: "" },
        notice: "rewind targets requested",
      };
    case "peer":
      if (argv.length !== 1) {
        return invalidUsage("peer");
      }
      return {
        ok: true,
        request: { kind: "register_peer", content: argv[0] },
        notice: `peer metadata registration requested with \`${argv[0]}\``,
      };
    case "notify": {
      const message = commandLine.trim().slice(name.length).trimStart();
      if (!message) {
        return invalidUsage("notify");
      }
      return { ok: true, request: { kind: "notify", content: message } };
    }
    default:
      return {
        ok: false,
        message: `Unknown command: ${name}. Type :help for available commands.`,
      };
  }
}

function helpCommand(argv: string[]): ComposerCommandResult {
  if (argv.length > 1) {
    return invalidUsage("help");
  }
  const name = argv[0];
  if (name) {
    const spec = COMMANDS[name];
    if (!spec) {
      return {
        ok: false,
        message: `Unknown command: ${name}. Type :help for available commands.`,
      };
    }
    return {
      ok: true,
      notice: `command: ${name} — usage: ${spec.usage}. ${spec.description}`,
    };
  }
  const list = ["help", "noop", "compact", "rewind", "peer", "notify"]
    .map((command) => `${command} (${COMMANDS[command].usage})`)
    .join(", ");
  return {
    ok: true,
    notice: `available commands: ${list}`,
  };
}

function invalidUsage(name: string): ComposerCommandResult {
  return {
    ok: false,
    message: `Invalid arguments. Usage: ${COMMANDS[name].usage}`,
  };
}

export function parseSigilSegments(input: string): Segment[] {
  const segments: Segment[] = [];
  const pattern = /(^|\s)([@])([^\s]+)/g;
  let cursor = 0;
  let match: RegExpExecArray | null;
  while ((match = pattern.exec(input)) !== null) {
    const leading = match[1] ?? "";
    const sigil = match[2];
    const value = match[3];
    const atomStart = match.index + leading.length;
    if (atomStart > cursor) {
      segments.push({ kind: "text", content: input.slice(cursor, atomStart) });
    }
    segments.push(sigilSegment(sigil, value));
    cursor = atomStart + sigil.length + value.length;
  }
  if (cursor < input.length) {
    segments.push({ kind: "text", content: input.slice(cursor) });
  }
  return coalesceTextSegments(
    segments.length > 0 ? segments : [{ kind: "text", content: input }],
  );
}

function sigilSegment(sigil: string, value: string): Segment {
  switch (sigil) {
    case "@":
      return { kind: "file_ref", path: value };
    default:
      return { kind: "text", content: `${sigil}${value}` };
  }
}

function coalesceTextSegments(segments: Segment[]): Segment[] {
  const coalesced: Segment[] = [];
  for (const segment of segments) {
    const last = coalesced.at(-1);
    if (segment.kind === "text" && last?.kind === "text") {
      last.content += segment.content;
    } else {
      coalesced.push(segment);
    }
  }
  return coalesced;
}

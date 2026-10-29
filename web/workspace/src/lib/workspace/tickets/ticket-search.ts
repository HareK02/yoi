import type {
  TicketAssignmentPrincipalSnapshot,
  TicketBrowseQueryRequest,
  TicketClosureReason,
  WorkspaceTag,
} from "../../generated/ticket-api.ts";

export const DEFAULT_TICKET_SEARCH = "state:open";

function tokens(input: string): string[] {
  const result: string[] = [];
  let start = 0;
  let quoteStart: number | null = null;
  for (let index = 0; index <= input.length; index++) {
    if (input[index] === '"') {
      quoteStart = quoteStart === null ? index : null;
    }
    if (
      index === input.length || (quoteStart === null && /\s/.test(input[index]))
    ) {
      if (index > start) result.push(input.slice(start, index));
      start = index + 1;
    }
  }
  if (quoteStart !== null) {
    throw new Error(
      `Unterminated quote at column ${
        quoteStart + 1
      }; add a closing double quote.`,
    );
  }
  return result;
}

export function ticketSearchNeedsTags(input: string): boolean {
  return tokens(input).some((token) => token.startsWith("tag:"));
}

function qualifierValue(
  raw: string,
  qualifier: string,
): { value: string; quoted: boolean } {
  const quoted = raw.startsWith('"');
  const value = quoted && raw.endsWith('"') ? raw.slice(1, -1) : raw;
  if (value.includes('"')) {
    throw new Error(
      `Invalid ${qualifier}: value; put the entire value inside one pair of double quotes.`,
    );
  }
  if (!value.trim()) {
    throw new Error(
      `Empty ${qualifier}: value; provide a value immediately after the colon.`,
    );
  }
  return { value, quoted };
}

function tagId(value: string, tags: readonly WorkspaceTag[]): string {
  const byId = tags.find((tag) => tag.id === value);
  if (byId) {
    if (byId.deleted) {
      throw new Error(`Tag "${value}" is deleted; choose an active tag.`);
    }
    return byId.id;
  }
  const matches = tags.filter((tag) => !tag.deleted && tag.name === value);
  const ids = [...new Set(matches.map((tag) => tag.id))];
  if (ids.length > 1) {
    throw new Error(`Tag name "${value}" is ambiguous; use its stable tag ID.`);
  }
  if (ids.length === 0) {
    throw new Error(
      `Unknown or deleted tag "${value}"; use an active tag name or stable ID.`,
    );
  }
  return ids[0];
}

function principal(value: string): TicketAssignmentPrincipalSnapshot {
  if (value.startsWith("subject:")) {
    const subject_id = value.slice("subject:".length);
    if (!subject_id.trim()) {
      throw new Error("Enter a Subject identity after subject:");
    }
    return { kind: "subject", subject_id };
  }
  if (value === "orchestrator") {
    return { kind: "workspace_agent", agent_key: "workspace-orchestrator" };
  }
  const parts = value.split("/");
  if (parts.every((part) => part.length > 0 && !/\s/.test(part))) {
    if (parts[0] === "user" && parts.length === 2) {
      return { kind: "user", account_id: parts[1] };
    }
    if (parts[0] === "worker" && parts.length === 3) {
      return { kind: "worker", runtime_id: parts[1], worker_id: parts[2] };
    }
  }
  throw new Error(
    "Invalid assignee: value; use subject:<subject-id>. Audit-only legacy filters: orchestrator, user/<account-id>, or worker/<runtime-id>/<worker-id>.",
  );
}

function reason(value: string, quoted: boolean): TicketClosureReason {
  // 引用符は組込み理由をcustomへ変換しない。非組込みの完全一致テキストだけをotherへ送る。
  switch (value) {
    case "completed":
    case "not_planned":
    case "duplicate":
    case "unknown":
      return value;
    default:
      if (quoted) return { other: value };
      throw new Error(
        "Invalid reason: value; use completed, not_planned, duplicate, unknown, or double-quote custom exact text.",
      );
  }
}

/** 既定検索の適用はloaderが担い、空入力やstate省略では全stateを検索する。 */
export function compileTicketSearch(
  input: string,
  tags: readonly WorkspaceTag[],
): TicketBrowseQueryRequest {
  const request: TicketBrowseQueryRequest = {
    query: null,
    states: [],
    tag_ids: [],
    assigned_principal: null,
    handling_status: null,
    sort: "updated_desc",
    limit: 30 as TicketBrowseQueryRequest["limit"],
    cursor: null,
  };
  const text: string[] = [];
  const tagIds = new Set<string>();
  const singletons = new Map<string, string>();
  for (const token of tokens(input)) {
    const match = /^(state|tag|assignee|reason):(.*)$/s.exec(token);
    if (!match) {
      // 未知のqualifierやURLはそのまま残す。引用された通常文もfilterとして解釈しない。
      text.push(
        token.startsWith('"') && token.endsWith('"')
          ? token.slice(1, -1)
          : token,
      );
      continue;
    }
    const [, qualifier, raw] = match;
    const { value, quoted } = qualifierValue(raw, qualifier);
    if (qualifier === "tag") {
      tagIds.add(tagId(value, tags));
      continue;
    }
    let canonical: string;
    switch (qualifier) {
      case "state":
        if (value !== "open" && value !== "closed" && value !== "all") {
          throw new Error(
            "Invalid state: value; use open, closed, or all (not close).",
          );
        }
        canonical = value;
        request.states = value === "all" ? [] : [value];
        break;
      case "assignee":
        request.assigned_principal = principal(value);
        canonical = JSON.stringify(request.assigned_principal);
        break;
      default:
        request.closure_reason = reason(value, quoted);
        canonical = JSON.stringify(request.closure_reason);
        break;
    }
    if (singletons.has(qualifier) && singletons.get(qualifier) !== canonical) {
      throw new Error(
        `Conflicting ${qualifier}: values; keep only one ${qualifier}: filter.`,
      );
    }
    singletons.set(qualifier, canonical);
  }
  request.query = text.join(" ").trim() || null;
  request.tag_ids = [...tagIds];
  return request;
}

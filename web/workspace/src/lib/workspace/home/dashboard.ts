import type { TicketListItemSummary } from "#lib/generated/ticket-api.ts";
import { loadJson, workspaceApiPath } from "#lib/workspace/api/http.ts";
import {
  parseMergeRequestListResponse,
  parseObjectiveListResponse,
  parseTicketDetail,
  parseTicketListResponse,
  TICKET_BROWSER_API_LOAD_POLICY,
} from "#lib/workspace/api/ticket-browser.ts";
import { mergeRequestPagePath } from "#lib/workspace/api/merge-requests.ts";
import { objectiveHref, ticketHref } from "#lib/workspace/resource-links.ts";

export const HOME_LIMIT = 5;
export type DashboardRow = {
  key: string;
  kind: "Ticket" | "Objective" | "Merge Request";
  reference: string;
  title: string;
  href: string;
  status: string;
  detail?: string;
  workerKey?: string;
  attention?: boolean;
  updatedAt?: string | null;
};
export type DashboardFeed = {
  rows: DashboardRow[];
  errors: string[];
  notice?: string;
};
export type Dashboard = {
  reviews: Promise<DashboardFeed>;
  active: Promise<DashboardFeed>;
  recent: Promise<DashboardFeed>;
};

const states: Record<string, string> = {
  inprogress: "In progress",
  queued: "Queued",
  done: "Done",
  closed: "Closed",
  active: "Active",
  completed: "Completed",
  paused: "Paused",
};
const reviewStates: Record<string, string> = {
  none: "Not reviewed",
  pending: "Review pending",
  approved: "Approved",
  request_changes: "Changes requested",
  unresolved_changes: "Unresolved changes",
};
function ticketRow(
  workspaceId: string,
  ticket: TicketListItemSummary,
): DashboardRow {
  return {
    key: `ticket:${ticket.resource_key}`,
    kind: "Ticket",
    reference: ticket.resource_key,
    title: ticket.title,
    href: ticketHref(workspaceId, ticket),
    status: states[ticket.state] ?? ticket.state,
    updatedAt: ticket.updated_at,
  };
}
function failure(label: string, error: string | null): string {
  const status = error?.match(/HTTP \d{3}/)?.[0];
  return `${label} unavailable.${
    status ? ` ${status}.` : ""
  } Refresh to retry.`;
}
function dateValue(value?: string | null): number {
  const timestamp = value ? Date.parse(value) : NaN;
  return Number.isFinite(timestamp) ? timestamp : -Infinity;
}
export function recentRows(rows: DashboardRow[]): DashboardRow[] {
  const latest = new Map<string, DashboardRow>();
  for (const row of rows) {
    const previous = latest.get(row.key);
    if (
      !previous || dateValue(row.updatedAt) >= dateValue(previous.updatedAt)
    ) latest.set(row.key, row);
  }
  return [...latest.values()]
    .sort((a, b) =>
      dateValue(b.updatedAt) - dateValue(a.updatedAt) ||
      a.key.localeCompare(b.key)
    )
    .slice(0, HOME_LIMIT);
}

// These are bounded, read-only browser endpoints. QueryTicket/QueryObjective are
// tool-only capabilities and must not be approximated by guessed HTTP routes.
export function loadDashboard(
  fetchFn: typeof fetch,
  workspaceId: string,
): Dashboard {
  function get<T>(path: string, parse: (value: unknown) => T) {
    return loadJson(
      fetchFn,
      workspaceApiPath(workspaceId, path),
      { signal: AbortSignal.timeout(10_000) },
      parse,
      TICKET_BROWSER_API_LOAD_POLICY,
    );
  }
  const tickets = (state: string) =>
    get(
      `/tickets?${new URLSearchParams({
        states: state,
        limit: String(HOME_LIMIT),
      })}`,
      parseTicketListResponse,
    );
  const activeResult = tickets("inprogress,queued");
  const doneResult = tickets("done");
  const closedResult = tickets("closed");
  const objectivesResult = get(
    `/objectives?limit=${HOME_LIMIT}`,
    parseObjectiveListResponse,
  );
  const reviewsResult = get(
    `/merge-requests?state=open&limit=${HOME_LIMIT}`,
    parseMergeRequestListResponse,
  );

  return {
    reviews: reviewsResult.then((result): DashboardFeed => {
      if (!result.data) {
        return { rows: [], errors: [failure("Merge Requests", result.error)] };
      }
      return {
        rows: result.data.items.slice(0, HOME_LIMIT).map((
          { summary, ref_diagnostics },
        ) => ({
          key: `merge:${summary.merge_request_id}`,
          kind: "Merge Request",
          reference: summary.repository_key,
          title: `${
            summary.selector_from ?? "Source unavailable"
          } → ${summary.selector_to}`,
          href: mergeRequestPagePath(workspaceId, summary.merge_request_id),
          status: reviewStates[summary.review_status] ?? summary.review_status,
          attention: ["request_changes", "unresolved_changes"].includes(
            summary.review_status,
          ) || Boolean(ref_diagnostics?.length),
          detail: ref_diagnostics?.length
            ? "Source or target needs attention"
            : undefined,
          updatedAt: summary.updated_at,
        })),
        errors: [],
        notice: result.data.next_cursor
          ? `Showing ${
            Math.min(result.data.items.length, HOME_LIMIT)
          } most recently updated open Merge Requests.`
          : undefined,
      };
    }),
    active: activeResult.then(async (result): Promise<DashboardFeed> => {
      if (!result.data) {
        return { rows: [], errors: [failure("Active tickets", result.error)] };
      }
      const details = await Promise.all(
        result.data.items.slice(0, HOME_LIMIT).map(async (ticket) => {
          const detail = await get(
            `/tickets/${encodeURIComponent(ticket.resource_key)}`,
            parseTicketDetail,
          );
          if (!detail.data) {
            return {
              row: {
                ...ticketRow(workspaceId, ticket),
                detail: "Assignment and blockers unavailable",
              },
              error: failure(`${ticket.resource_key} details`, detail.error),
            };
          }
          const current = detail.data;
          // A concurrent transition can remove a Ticket from this snapshot's scope.
          if (!["inprogress", "queued"].includes(current.state)) {
            return { changed: true };
          }
          const blockers = current.relations.blockers;
          const row = ticketRow(workspaceId, { ...ticket, ...current });
          const blockedKeys = blockers.map((blocker) =>
            blocker.blocking_resource_key
          ).filter(Boolean);
          return {
            row: {
              ...row,
              status: blockers.length ? `${row.status} · Blocked` : row.status,
              attention: blockers.length > 0,
              detail: blockers.length
                ? (blockedKeys.length === blockers.length
                  ? `Blocked by ${blockedKeys.join(", ")}`
                  : `${blockers.length} unresolved dependencies`)
                : current.current_coder
                ? undefined
                : "No coder assigned",
              workerKey: current.current_coder?.worker_resource_key ??
                undefined,
            },
          };
        }),
      );
      return {
        rows: details.flatMap((item) => item.row ? [item.row] : []),
        errors: [
          ...details.flatMap((item) => item.error ? [item.error] : []),
          ...(result.data.invalid_records.length
            ? ["Some Ticket records could not be read."]
            : []),
        ],
        notice: details.some((item) => item.changed)
          ? "Work changed while loading. Refresh for the latest list."
          : result.data.page.has_more || result.data.page.source_truncated
          ? `Showing ${
            Math.min(result.data.items.length, HOME_LIMIT)
          } active tickets, with in-progress work first.`
          : undefined,
      };
    }),
    recent: Promise.all([doneResult, closedResult, objectivesResult]).then((
      [done, closed, objectives],
    ): DashboardFeed => ({
      rows: recentRows([
        ...(done.data?.items ?? []).map((ticket) =>
          ticketRow(workspaceId, ticket)
        ),
        ...(closed.data?.items ?? []).map((ticket) =>
          ticketRow(workspaceId, ticket)
        ),
        ...(objectives.data?.items ?? []).map((objective): DashboardRow => ({
          key: `objective:${objective.resource_key}`,
          kind: "Objective",
          reference: objective.resource_key,
          title: objective.title,
          href: objectiveHref(workspaceId, objective),
          status: states[objective.state] ?? objective.state,
          updatedAt: objective.updated_at,
        })),
      ]),
      errors: [
        ...(!done.data ? [failure("Done tickets", done.error)] : []),
        ...(!closed.data ? [failure("Closed tickets", closed.error)] : []),
        ...(!objectives.data ? [failure("Objectives", objectives.error)] : []),
        ...([done.data, closed.data, objectives.data].some((data) =>
            data?.invalid_records.length
          )
          ? ["Some records could not be read."]
          : []),
      ],
      notice:
        [done.data, closed.data, objectives.data].some((data) =>
            data?.items.length
          )
          ? "Up to 5 latest updates to done / closed tickets and Objectives."
          : undefined,
    })),
  };
}

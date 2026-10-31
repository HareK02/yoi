import type {
  TicketBrowseQueryRequest,
  TicketChildrenPage,
  TicketLinkSummary,
  TicketParentLink,
  TicketParentSetRequest,
} from "#lib/generated/ticket-api.ts";
import { workspaceApiJson, workspaceApiJsonWithBody, workspaceApiPath } from "./http.ts";
import {
  parseTicketBrowseQueryResponse,
  parseTicketChildrenPage,
  parseTicketLinkSummary,
  parseTicketParentLink,
  TICKET_BROWSER_API_MAX_RESPONSE_BYTES,
} from "./ticket-browser.ts";
import { observeOperation, type ObservedOperation } from "./ticket-operations.ts";

export type ParentCandidates = {
  items: TicketLinkSummary[];
  hasMore: boolean;
};
export type TicketParentClient = {
  parent(workspaceId: string, ticketId: string): Promise<TicketParentLink>;
  children(workspaceId: string, ticketId: string, cursor: string | null): Promise<TicketChildrenPage>;
  search(workspaceId: string, query: string): Promise<ParentCandidates>;
  resolve(workspaceId: string, reference: string): Promise<TicketLinkSummary>;
  set(operation: ObservedOperation): Promise<TicketParentLink>;
};

function ticketPath(workspaceId: string, ticketId: string): string {
  return workspaceApiPath(workspaceId, `/tickets/${encodeURIComponent(ticketId)}`);
}

export function parentOperation(
  workspaceId: string,
  link: TicketParentLink,
  parent: string | null,
  operationKey: string,
): ObservedOperation {
  const request: TicketParentSetRequest = {
    expected: { ...link.observation },
    parent,
    operation_key: operationKey,
  };
  return observeOperation(`${ticketPath(workspaceId, link.ticket_id)}/parent`, "PUT", request);
}

export function createTicketParentClient(): TicketParentClient {
  return {
    parent: (workspaceId, ticketId) => workspaceApiJson(
      `${ticketPath(workspaceId, ticketId)}/parent`, parseTicketParentLink,
      TICKET_BROWSER_API_MAX_RESPONSE_BYTES,
    ),
    children: (workspaceId, ticketId, cursor) => {
      const query = new URLSearchParams({ limit: "50" });
      if (cursor !== null) query.set("cursor", cursor);
      return workspaceApiJson(`${ticketPath(workspaceId, ticketId)}/children?${query}`,
        parseTicketChildrenPage, TICKET_BROWSER_API_MAX_RESPONSE_BYTES);
    },
    search: async (workspaceId, query) => {
      const request: TicketBrowseQueryRequest = {
        query, states: [], limit: 20 as TicketBrowseQueryRequest["limit"],
        sort: "updated_desc", cursor: null,
      };
      const page = await workspaceApiJsonWithBody(workspaceApiPath(workspaceId, "/tickets/browse"),
        { method: "POST", body: JSON.stringify(request) },
        parseTicketBrowseQueryResponse, TICKET_BROWSER_API_MAX_RESPONSE_BYTES);
      return {
        items: page.items.map((ticket) => ({
          id: ticket.id, resource_key: ticket.resource_key, title: ticket.title,
          state: ticket.state, closure_reason: ticket.current_closure?.reason ?? null,
        })),
        hasMore: page.page.has_more || page.page.source_truncated,
      };
    },
    resolve: (workspaceId, reference) => workspaceApiJson(
      `${ticketPath(workspaceId, reference)}/link-summary`, parseTicketLinkSummary,
      TICKET_BROWSER_API_MAX_RESPONSE_BYTES,
    ),
    set: (operation) => workspaceApiJsonWithBody(operation.path,
      { method: operation.method, body: operation.body }, parseTicketParentLink,
      TICKET_BROWSER_API_MAX_RESPONSE_BYTES),
  };
}

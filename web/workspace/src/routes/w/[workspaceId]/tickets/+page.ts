import { loadJson, workspaceApiPath } from "#lib/workspace/api/http.ts";
import {
  parseTicketBrowseQueryResponse,
  TICKET_BROWSER_API_LOAD_POLICY,
} from "#lib/workspace/api/ticket-browser.ts";
import { parseTags } from "#lib/workspace/api/ticket-operations.ts";
import {
  compileTicketSearch,
  DEFAULT_TICKET_SEARCH,
  ticketSearchNeedsTags,
} from "#lib/workspace/tickets/ticket-search.ts";
import type {
  TicketBrowseQueryRequest,
  WorkspaceTag,
} from "#lib/generated/ticket-api.ts";
import type { PageLoad } from "./$types";
import { deferPage } from "#lib/workspace/navigation/deferred.ts";

export const load = deferPage(async ({ fetch, params, url }: Parameters<PageLoad>[0]) => {
  const workspaceId = params.workspaceId;
  const search = url.searchParams.get("q") ?? DEFAULT_TICKET_SEARCH;
  const loadTickets = async () => {
    let queryRequest: TicketBrowseQueryRequest | null = null;
    try {
      const tags: WorkspaceTag[] = [];
      // タグ名をstable IDへ解決する場合だけ一覧を取得する。状態検索をタグAPIに依存させない。
      if (ticketSearchNeedsTags(search)) {
        let after = "";
        while (true) {
          const result = await loadJson(
            fetch,
            workspaceApiPath(
              workspaceId,
              `/ticket-tags?limit=200${
                after ? `&after=${encodeURIComponent(after)}` : ""
              }`,
            ),
            undefined,
            parseTags,
            TICKET_BROWSER_API_LOAD_POLICY,
          );
          if (!result.data) {
            throw new Error(result.error ?? "Tags could not be loaded");
          }
          tags.push(...result.data);
          if (result.data.length < 200) break;
          const next = result.data.at(-1)!.id;
          if (next <= after) throw new Error("Tag pagination did not advance");
          after = next;
        }
      }
      queryRequest = compileTicketSearch(search, tags);
      const tickets = await loadJson(
        fetch,
        workspaceApiPath(workspaceId, "/tickets/browse"),
        {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify(queryRequest),
        },
        parseTicketBrowseQueryResponse,
        TICKET_BROWSER_API_LOAD_POLICY,
      );
      return { queryRequest, tickets };
    } catch (error) {
      return {
        queryRequest,
        tickets: {
          data: null,
          error: error instanceof Error ? error.message : String(error),
        },
      };
    }
  };
  const result = await loadTickets();
  return { workspaceId, search, ...result };
});

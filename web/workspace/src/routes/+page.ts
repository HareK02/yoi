import { loadJson } from "$lib/workspace-api/http";
import type { Host, ListResponse, Worker } from "$lib/workspace-sidebar/types";
import type { PageLoad } from "./$types";

export const load: PageLoad = async ({ fetch }) => {
  const [hosts, workers] = await Promise.all([
    loadJson<ListResponse<Host>>(fetch, "/api/hosts"),
    loadJson<ListResponse<Worker>>(fetch, "/api/workers"),
  ]);

  return {
    hosts: hosts.data,
    hostsError: hosts.error,
    workers: workers.data,
    workersError: workers.error,
  };
};

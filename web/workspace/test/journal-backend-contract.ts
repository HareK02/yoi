// 実router/store試験がserializeした7 actionの応答を、本番Web parserと呼出契約へ通す。
// HTTP・認証・保存はRust側、ここではその実応答のWeb受理を検証する。画面のE2Eではない。
import {
  type JournalRequest,
  loadJournal,
} from "../src/lib/workspace/memory/journal-api.ts";
const path = Deno.args[0];
if (!path) {
  throw new Error(
    "Expected the captured journal_browser_web_contract test output path",
  );
}
const log = await Deno.readTextFile(path);
const marker = "JOURNAL_BROWSER_CONTRACT ";
const records = log.split("\n").filter((line) => line.includes(marker)).map((
  line,
) => JSON.parse(line.slice(line.indexOf(marker) + marker.length)));
const actions = new Set<string>();
for (const record of records) {
  const request = record.request as JournalRequest;
  let seen = false;
  const fetcher = ((url: string, init: RequestInit) => {
    if (
      url !==
        `/api/w/${record.workspace_id}/subjektiv/subjects/${record.subject_id}/journal-browser`
    ) throw new Error("Web endpoint differs from actual API");
    if (
      init.method !== "POST" ||
      JSON.stringify(JSON.parse(String(init.body))) !== JSON.stringify(request)
    ) throw new Error("Web request differs from actual API");
    seen = true;
    return Promise.resolve(Response.json(record.response));
  }) as typeof fetch;
  const parsed = await loadJournal(
    fetcher,
    record.workspace_id,
    record.subject_id,
    request,
  );
  if (!seen || parsed.error || !parsed.data) {
    throw new Error(
      `${request.action}: ${parsed.error ?? "No parsed response"}`,
    );
  }
  actions.add(request.action);
}
for (
  const action of [
    "sessions",
    "entries",
    "entry",
    "text",
    "corrections",
    "memory_sources",
    "related_memory",
  ]
) {
  if (!actions.has(action)) {
    throw new Error(`No actual serialized response for ${action}`);
  }
}
console.log(
  `Actual Backend responses accepted by production Web contract: ${records.length} records, ${actions.size} actions`,
);
